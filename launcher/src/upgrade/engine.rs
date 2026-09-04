//! LCH-010/012：升级状态机编排（计划 §6.6 全链路）与候选启动/提交/回滚。
//!
//! 规则：
//! - 每个持久边先原子写 journal 意图（state/last_step），再执行动作，完成后推进；
//!   journal 每次变更都「读盘→改→原子写回」，不留跨步内存态（QA-004 断电矩阵友好）。
//! - committed 之前任何失败走 [`Engine::rollback_procedure`]：停候选 → 隔离失败
//!   数据 → 恢复快照 → current.json 切回 previous → （可选）重启旧版本并验证；
//!   回滚也失败 → manual_recovery_required 并停止自动重试（后续 upgrade 拒绝执行）。
//! - 候选启动注入 GAMER_ACTIVATION_GATE=1 + GAMER_LAUNCHER_PIPE/GAMER_LAUNCHER_IPC_TOKEN；
//!   candidate_ready = /health/ready 200 且 boot_id 与旧实例不同、app.version==目标
//!   （GET /api/system/info 优先，非 200 回退 /health/ready body 字段）；
//!   activating = POST /api/system/activate（X-Launcher-Token header）。

use std::fs;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::archive::{extract_app_zip, ExtractOptions};
use crate::fetch::{obtain_artifact, FetchOptions};
use crate::inventory::{check_component, CheckOptions, ComponentSpec, ComponentStatus};
use crate::layout::InstallLayout;
use crate::manifest::model::Manifest;
use crate::manifest::{validate_manifest_file, ValidateOptions};
use crate::repair::{verify_app_dir, AppInstallSpec};
use crate::state::atomic::{now_unix_millis, rename_with_retry, LoadOutcome};
use crate::state::{ChildInfo, CurrentState, SnapshotInfo, StateStore, UpdateJournal, UpdateState};
use crate::supervisor;
use crate::supervisor::{
    read_configured_port, resolve_entrypoint, spawn_child_with_extras, LaunchExtras, LaunchPlan,
    ReadyProbe, HEALTH_PATH,
};
use crate::winutil;

use super::codes;
use super::httpc::http_request;
use super::snapshot;
use super::{new_update_id, BusinessError};

/// 下载进度（内存态；status 仅 downloading 态读取，重启后自然归零）。
#[derive(Debug, Default)]
pub struct Progress {
    pub bytes_done: AtomicU64,
    pub bytes_total: AtomicU64,
}

impl Progress {
    pub fn set_total(&self, total: u64) {
        self.bytes_total.store(total, Ordering::Relaxed);
    }
    pub fn add_done(&self, delta: u64) {
        self.bytes_done.fetch_add(delta, Ordering::Relaxed);
    }
    pub fn snapshot(&self) -> (u64, u64) {
        (
            self.bytes_done.load(Ordering::Relaxed),
            self.bytes_total.load(Ordering::Relaxed),
        )
    }
}

/// 升级引擎参数。
#[derive(Debug, Clone)]
pub struct UpgradeOptions {
    /// 可信公钥目录（manifest 验签）。
    pub keys_dir: PathBuf,
    pub fetch: FetchOptions,
    pub probe: ReadyProbe,
    /// 优雅停机等待上限；超时按契约默认取消升级（准确 PID 未退出不硬杀）。
    pub shutdown_timeout: Duration,
    /// 候选启动注入的 IPC 寻址（pipe 名, 会话令牌）；None = 只注入 gate（演练极简模式，
    /// activate 以跳过代替拒绝——由调用方选择）。
    pub ipc: Option<(String, String)>,
    /// 演练模式：无 IPC 令牌时不跳过 activate（夹具不校验 token 时可开）。
    pub activate_without_token: bool,
    /// 回环管理通道令牌（与子进程 GAMER_ADMIN_TOKEN 同源）：drain 旧版本时以
    /// X-Admin-Token 通过 /api/shutdown 鉴权；None = 匿名 drain（生产 server 会 401）。
    pub admin_token: Option<String>,
}

impl Default for UpgradeOptions {
    fn default() -> Self {
        Self {
            keys_dir: PathBuf::from("keys"),
            fetch: FetchOptions::default(),
            probe: ReadyProbe::default(),
            shutdown_timeout: Duration::from_secs(90),
            ipc: None,
            activate_without_token: false,
            admin_token: None,
        }
    }
}

/// manifest 来源：本地路径（M2 演练主路径）/ 远端 URL / 无源（check 必然
/// update_not_available）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestSource {
    None,
    Path(PathBuf),
    Url(String),
}

/// check 阶段产物（已验签候选）。
#[derive(Debug)]
pub struct Checked {
    pub version: String,
    pub manifest: Manifest,
}

/// 升级事务终态。
#[derive(Debug, Clone)]
pub enum UpgradeOutcome {
    Committed {
        from: String,
        to: String,
    },
    /// draining 超时等：事务取消回 staged，旧版未动。
    Cancelled {
        reason: String,
    },
    /// committed 之前失败且旧版恢复健康（含未触及数据的失败）。
    FailedOldHealthy {
        error: BusinessError,
    },
    /// 回滚也失败：manual_recovery_required，停止自动重试。
    ManualRecovery {
        error: BusinessError,
    },
}

/// 升级引擎（持有安装根副本；可整体 move 进阻塞线程执行长操作）。
pub struct Engine {
    pub layout: InstallLayout,
    pub opts: UpgradeOptions,
    pub progress: Arc<Progress>,
    pid_ops: Arc<dyn PidOps>,
    available_space: Arc<dyn AvailableSpaceProvider>,
}

trait PidOps: Send + Sync {
    fn terminate_if_image(&self, pid: u32, expected_exe: &Path) -> bool;
}

struct NativePidOps;

impl PidOps for NativePidOps {
    fn terminate_if_image(&self, pid: u32, expected_exe: &Path) -> bool {
        winutil::terminate_pid_if_image(pid, expected_exe)
    }
}

trait AvailableSpaceProvider: Send + Sync {
    fn available_bytes(&self, path: &Path) -> io::Result<u64>;
}

struct NativeAvailableSpaceProvider;

impl AvailableSpaceProvider for NativeAvailableSpaceProvider {
    fn available_bytes(&self, path: &Path) -> io::Result<u64> {
        winutil::free_disk_bytes(path)
    }
}

trait CandidateProcessProbe {
    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>>;
}

impl CandidateProcessProbe for Child {
    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        Child::try_wait(self)
    }
}

impl Engine {
    pub fn new(layout: InstallLayout, opts: UpgradeOptions) -> Self {
        Self {
            layout,
            opts,
            progress: Arc::new(Progress::default()),
            pid_ops: Arc::new(NativePidOps),
            available_space: Arc::new(NativeAvailableSpaceProvider),
        }
    }

    #[cfg(test)]
    fn with_pid_ops(layout: InstallLayout, opts: UpgradeOptions, pid_ops: Arc<dyn PidOps>) -> Self {
        Self {
            layout,
            opts,
            progress: Arc::new(Progress::default()),
            pid_ops,
            available_space: Arc::new(NativeAvailableSpaceProvider),
        }
    }

    #[cfg(test)]
    fn with_available_space_provider(
        layout: InstallLayout,
        opts: UpgradeOptions,
        available_space: Arc<dyn AvailableSpaceProvider>,
    ) -> Self {
        Self {
            layout,
            opts,
            progress: Arc::new(Progress::default()),
            pid_ops: Arc::new(NativePidOps),
            available_space,
        }
    }

    fn store(&self) -> StateStore {
        StateStore::new(&self.layout.root)
    }

    fn load_journal(&self) -> Result<UpdateJournal, BusinessError> {
        self.store()
            .load_journal()
            .map(|jl| jl.journal)
            .map_err(|e| {
                BusinessError::new(
                    codes::LAUNCHER_UNREACHABLE,
                    format!("读取 journal 失败: {e}"),
                )
            })
    }

    /// 读盘→改→原子写回（无跨步内存态）。
    fn mutate_journal(
        &self,
        f: impl FnOnce(&mut UpdateJournal),
    ) -> Result<UpdateJournal, BusinessError> {
        let mut journal = self.load_journal()?;
        f(&mut journal);
        journal.updated_at_unix_ms = Some(now_unix_millis());
        self.store().write_journal(&journal).map_err(|e| {
            BusinessError::new(codes::LAUNCHER_UNREACHABLE, format!("写 journal 失败: {e}"))
        })?;
        Ok(journal)
    }

    fn set_failed_idle(journal: &mut UpdateJournal, err: &BusinessError) {
        journal.state = UpdateState::Idle;
        journal.last_step = Some("failed".to_string());
        journal.error = Some(crate::state::JournalError {
            code: err.code.clone(),
            message: err.message.clone(),
        });
    }

    fn enter_manual_recovery(&self, err: &BusinessError) -> BusinessError {
        let _ = self.mutate_journal(|j| {
            j.state = UpdateState::ManualRecoveryRequired;
            j.last_step = Some("manual_recovery_required".to_string());
            j.error = Some(crate::state::JournalError {
                code: err.code.clone(),
                message: err.message.clone(),
            });
        });
        tracing::error!(code = %err.code, %err.message, "回滚失败，进入 manual_recovery_required（停止自动重试）");
        BusinessError::new(
            codes::MANUAL_RECOVERY_REQUIRED,
            format!("升级与自动回滚均失败：{err}；已保留 journal/快照/新旧版本/quarantine 证据"),
        )
    }

    fn current_version(&self) -> Result<String, BusinessError> {
        match self.store().load_current() {
            Ok(LoadOutcome::Present(c)) => Ok(c.current),
            Ok(LoadOutcome::Missing) => Err(BusinessError::new(
                codes::UPDATE_NOT_AVAILABLE,
                "尚未安装（state/current.json 不存在），无升级基线",
            )),
            Ok(LoadOutcome::Corrupted { .. }) => Err(BusinessError::new(
                codes::UPDATE_NOT_AVAILABLE,
                "state/current.json 损坏，升级基线不可信",
            )),
            Err(e) => Err(BusinessError::new(
                codes::UPDATE_NOT_AVAILABLE,
                format!("读取 state/current.json 失败: {e}"),
            )),
        }
    }

    fn guard_not_manual(&self, journal: &UpdateJournal) -> Result<(), BusinessError> {
        if journal.state == UpdateState::ManualRecoveryRequired {
            return Err(BusinessError::new(
                codes::MANUAL_RECOVERY_REQUIRED,
                "上次升级与自动回滚均失败，已停止自动重试；请人工恢复后复位 journal",
            ));
        }
        Ok(())
    }

    // -- check ------------------------------------------------------------------

    /// idle → checking → checked（有候选）或失败（回 idle/failed 展示）。
    pub fn phase_check(&self, source: &ManifestSource) -> Result<Checked, BusinessError> {
        let journal = self.load_journal()?;
        self.guard_not_manual(&journal)?;
        let current = self.current_version()?;
        // checked 驻留态内重新 check 复用同一 update_id（幂等语义 §5.3）
        let reuse_tx = journal.state == UpdateState::Checking
            && journal.last_step.as_deref() == Some("checked")
            && journal.update_id.is_some();

        // 意图先行
        self.mutate_journal(|j| {
            j.state = UpdateState::Checking;
            j.last_step = Some("checking".to_string());
            if !reuse_tx {
                j.update_id = Some(new_update_id());
            }
            j.from_version = Some(current.clone());
            j.to_version = None;
            j.error = None;
            j.snapshot = None;
            j.child = None;
        })?;

        match self.load_candidate(source, &current) {
            Ok(checked) => {
                self.mutate_journal(|j| {
                    j.to_version = Some(checked.version.clone());
                    j.last_step = Some("checked".to_string());
                })?;
                Ok(checked)
            }
            Err(err) => {
                self.mutate_journal(|j| Self::set_failed_idle(j, &err))?;
                Err(err)
            }
        }
    }

    /// manifest 获取 + 验签 + 语义门禁 + 空间预估 + 缓存。
    fn load_candidate(
        &self,
        source: &ManifestSource,
        current: &str,
    ) -> Result<Checked, BusinessError> {
        let (path, temporary): (PathBuf, bool) = match source {
            ManifestSource::None => {
                return Err(BusinessError::new(
                    codes::UPDATE_NOT_AVAILABLE,
                    "未配置远端发布源，无可检查的候选版本",
                ));
            }
            ManifestSource::Path(p) => (p.clone(), false),
            ManifestSource::Url(url) => (self.fetch_remote_manifest(url)?, true),
        };
        let opts = ValidateOptions {
            keys_dir: Some(self.opts.keys_dir.clone()),
            expect_current_version: Some(current.to_string()),
            launcher_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            ..ValidateOptions::default()
        };
        let outcome = validate_manifest_file(&path, &opts);
        if !outcome.ok {
            let summary = outcome
                .errors
                .iter()
                .map(|e| format!("[{}] {}", e.code, e.detail))
                .collect::<Vec<_>>()
                .join("; ");
            // 版本不高于当前 = 没有可用更新；其余 manifest 故障按验签/清单无效 fail closed
            let downgrade = outcome
                .errors
                .iter()
                .any(|e| e.code == crate::manifest::codes::VERSION_DOWNGRADE);
            let launcher_too_old = outcome
                .errors
                .iter()
                .any(|e| e.code == crate::manifest::codes::LAUNCHER_TOO_OLD);
            let code = if downgrade {
                codes::UPDATE_NOT_AVAILABLE
            } else if launcher_too_old {
                codes::LAUNCHER_TOO_OLD
            } else {
                codes::SIGNATURE_INVALID
            };
            return Err(BusinessError::new(
                code,
                format!("候选 manifest 校验失败: {summary}"),
            ));
        }
        let raw = fs::read(&path).map_err(|e| {
            BusinessError::new(codes::SIGNATURE_INVALID, format!("读取 manifest 失败: {e}"))
        })?;
        let value: Value = serde_json::from_slice(&raw).map_err(|e| {
            BusinessError::new(
                codes::SIGNATURE_INVALID,
                format!("manifest 不是合法 JSON: {e}"),
            )
        })?;
        let manifest = Manifest::parse(&value).map_err(|e| {
            BusinessError::new(
                codes::SIGNATURE_INVALID,
                format!("manifest 模型解析失败: {e}"),
            )
        })?;
        // validate_manifest_file 已完成签名/结构门禁；这里再显式调用同一最低
        // launcher 版本规则，避免未来新增 manifest 消费入口时绕过门禁。
        super::check_minimum_launcher_version(&manifest.release.minimum_launcher_version)?;
        let version = manifest.release.version.clone();

        // 必须严格更新（等于当前也无更新可装）
        if let (Some(cur), Some(cand)) = (
            crate::manifest::semver::parse(current),
            crate::manifest::semver::parse(&version),
        ) {
            if !crate::manifest::semver::is_lt(&cur, &cand) {
                return Err(BusinessError::new(
                    codes::UPDATE_NOT_AVAILABLE,
                    format!("候选 {version} 不高于当前版本 {current}"),
                ));
            }
        }

        // schema 门禁：rollback_floor 约束 + 不允许把数据 schema 往下迁
        if manifest.release.data_schema < manifest.release.rollback_floor {
            return Err(BusinessError::new(
                codes::SCHEMA_INCOMPATIBLE,
                format!(
                    "candidate schema {} 低于 rollback_floor {}",
                    manifest.release.data_schema, manifest.release.rollback_floor
                ),
            ));
        }
        if let Some(before) = self.probe_live_schema() {
            if manifest.release.data_schema < i64::from(before) {
                return Err(BusinessError::new(
                    codes::SCHEMA_INCOMPATIBLE,
                    format!(
                        "candidate schema {} 低于现网数据 schema {before}（禁止 schema 降级）",
                        manifest.release.data_schema
                    ),
                ));
            }
        }

        // 空间预估：产物声明总量 + 现网数据体积（快照副本）+ 64 MiB 余量
        let platform = manifest.platforms.get("windows-x86_64").ok_or_else(|| {
            BusinessError::new(
                codes::SIGNATURE_INVALID,
                "manifest 缺少 windows-x86_64 平台",
            )
        })?;
        let mut required: u64 = u64::try_from(platform.app.artifact.size).unwrap_or(0);
        for comp in &platform.components {
            required += u64::try_from(comp.artifact.size).unwrap_or(0);
        }
        required += dir_size(&self.layout.data_dir());
        required += 64 * 1024 * 1024;
        let available = self
            .available_space
            .available_bytes(&self.layout.root)
            .map_err(|e| {
                BusinessError::new(codes::INSUFFICIENT_SPACE, format!("磁盘空间查询失败: {e}"))
            })?;
        if required > available {
            return Err(BusinessError::new(
                codes::INSUFFICIENT_SPACE,
                format!("空间不足：预估需要 {required} 字节，可用 {available} 字节"),
            ));
        }

        // 缓存已验签 manifest（download/prepare/入口解析复用）
        let cached = self.layout.manifests_dir().join(format!("{version}.json"));
        fs::create_dir_all(self.layout.manifests_dir()).map_err(|e| {
            BusinessError::new(
                codes::SIGNATURE_INVALID,
                format!("创建 manifests/ 失败: {e}"),
            )
        })?;
        crate::state::atomic::write_json_atomic(&cached, &value).map_err(|e| {
            BusinessError::new(codes::SIGNATURE_INVALID, format!("缓存 manifest 失败: {e}"))
        })?;
        if temporary {
            let _ = fs::remove_file(&path);
        }
        Ok(Checked { version, manifest })
    }

    fn fetch_remote_manifest(&self, url: &str) -> Result<PathBuf, BusinessError> {
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err(BusinessError::new(
                codes::UPDATE_NOT_AVAILABLE,
                format!("manifest URL 非法: {url:?}"),
            ));
        }
        let staging = self.layout.staging_dir().join("remote-manifest");
        fs::create_dir_all(&staging).map_err(|e| {
            BusinessError::new(codes::ARTIFACT_INVALID, format!("创建 staging 失败: {e}"))
        })?;
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(self.opts.fetch.connect_timeout)
            .timeout_read(self.opts.fetch.read_timeout)
            .user_agent(concat!("gamer-launcher/", env!("CARGO_PKG_VERSION")))
            .build();
        let dest = staging.join("manifest.json");
        let response = agent.get(url).call().map_err(|e| match e {
            ureq::Error::Status(code, _) => BusinessError::new(
                codes::UPDATE_NOT_AVAILABLE,
                format!("远端 manifest 获取失败（HTTP {code}）"),
            ),
            other => BusinessError::new(
                codes::UPDATE_NOT_AVAILABLE,
                format!("远端 manifest 获取失败: {other}"),
            ),
        })?;
        let mut file = fs::File::create(&dest).map_err(|e| {
            BusinessError::new(
                codes::ARTIFACT_INVALID,
                format!("写远端 manifest 失败: {e}"),
            )
        })?;
        std::io::copy(&mut response.into_reader(), &mut file).map_err(|e| {
            BusinessError::new(
                codes::ARTIFACT_INVALID,
                format!("下载远端 manifest 失败: {e}"),
            )
        })?;
        drop(file);
        // 分离签名：约定 URL + ".sig"
        let sig_dest = staging.join("manifest.sig");
        let sig_url = format!("{url}.sig");
        if let Ok(sig_resp) = agent.get(&sig_url).call() {
            if let Ok(mut f) = fs::File::create(&sig_dest) {
                let _ = std::io::copy(&mut sig_resp.into_reader(), &mut f);
            }
        }
        Ok(dest)
    }

    /// 旧 exe 对现网数据 inspect（尽力而为，不可用 = None）。
    fn probe_live_schema(&self) -> Option<u32> {
        let current = self.store().load_current().ok().and_then(|o| match o {
            LoadOutcome::Present(c) => Some(c.current),
            _ => None,
        })?;
        let exe = resolve_entrypoint(&self.layout, &current).ok()?;
        snapshot::inspect_schema(&exe, &self.layout.data_dir())
    }

    // -- download ---------------------------------------------------------------

    /// checked/downloading → staged：产物获取（seed/cache/remote）+ 组件换装 +
    /// app staging + 完整性验证。
    pub fn phase_download(&self) -> Result<(), BusinessError> {
        let journal = self.load_journal()?;
        self.guard_not_manual(&journal)?;
        let Some(update_id) = journal.update_id.clone() else {
            return Err(BusinessError::new(
                codes::UPDATE_BUSY,
                "无进行中的升级事务（先 check）",
            ));
        };
        let Some(to_version) = journal.to_version.clone() else {
            return Err(BusinessError::new(
                codes::UPDATE_NOT_AVAILABLE,
                "无候选版本（先 check）",
            ));
        };
        let checked_ready = journal.state == UpdateState::Checking
            && journal.last_step.as_deref() == Some("checked");
        let resuming = journal.state == UpdateState::Downloading;
        if !checked_ready && !resuming {
            return Err(BusinessError::new(
                codes::UPDATE_BUSY,
                format!(
                    "当前状态 {} 不接受 download",
                    super::display_state(&journal)
                ),
            ));
        }

        // 意图先行
        self.mutate_journal(|j| {
            j.state = UpdateState::Downloading;
            j.last_step = Some("downloading".to_string());
        })?;
        let manifest = self.load_cached_manifest(&to_version)?;
        let platform = manifest.platforms.get("windows-x86_64").ok_or_else(|| {
            BusinessError::new(codes::ARTIFACT_INVALID, "manifest 缺少 windows-x86_64 平台")
        })?;
        let staging = self.layout.staging_dir().join(&update_id);
        let _ = fs::remove_dir_all(&staging);
        fs::create_dir_all(&staging).map_err(|e| {
            BusinessError::new(codes::ARTIFACT_INVALID, format!("创建 staging 失败: {e}"))
        })?;

        let run = || -> Result<(), BusinessError> {
            // 组件：复用 repair（深检 → seed/cache/remote → staging → 原子换装 → 复验）
            let mut totals: u64 = 0;
            for comp in &platform.components {
                totals += u64::try_from(comp.artifact.size).unwrap_or(0);
                let spec = ComponentSpec::from_model(comp).map_err(|e| {
                    BusinessError::new(codes::ARTIFACT_INVALID, format!("组件规格非法: {e}"))
                })?;
                let outcome = crate::repair::repair_component(
                    &self.layout,
                    &spec,
                    &crate::repair::RepairOptions {
                        fetch: self.opts.fetch.clone(),
                        probe: false,
                    },
                );
                if let crate::repair::ComponentOutcome::Failed { reason } = outcome.outcome {
                    return Err(BusinessError::new(
                        codes::ARTIFACT_INVALID,
                        format!("组件 {} 安装失败: {reason}", comp.id),
                    ));
                }
                self.progress
                    .add_done(u64::try_from(comp.artifact.size).unwrap_or(0));
            }
            // app：下载 + 解压到 staging + 校验（versions/<to>/ 在 switched 才换入）
            let app = AppInstallSpec::from_model(platform, &to_version).map_err(|e| {
                BusinessError::new(codes::ARTIFACT_INVALID, format!("app 规格非法: {e}"))
            })?;
            totals += app.artifact_size;
            self.progress.set_total(totals);
            let artifact = obtain_artifact(
                &self.layout,
                &app.artifact_name,
                &app.artifact_sha256,
                app.artifact_size,
                Some(&app.artifact_url),
                &self.opts.fetch,
            )
            .map_err(|e| {
                BusinessError::new(codes::ARTIFACT_INVALID, format!("获取应用产物失败: {e}"))
            })?;
            self.progress.add_done(app.artifact_size);
            let app_staging = staging.join("app");
            extract_app_zip(artifact.path(), &app_staging, &ExtractOptions::default()).map_err(
                |e| BusinessError::new(codes::ARTIFACT_INVALID, format!("app 解压/校验失败: {e}")),
            )?;
            verify_app_dir(&app_staging, &app).map_err(|e| {
                BusinessError::new(
                    codes::ARTIFACT_INVALID,
                    format!("app staging 复验失败: {e}"),
                )
            })?;
            Ok(())
        };
        if let Err(err) = run() {
            let _ = fs::remove_dir_all(&staging);
            self.mutate_journal(|j| Self::set_failed_idle(j, &err))?;
            return Err(err);
        }
        self.mutate_journal(|j| {
            j.state = UpdateState::Staged;
            j.last_step = Some("staged".to_string());
        })?;
        Ok(())
    }

    fn load_cached_manifest(&self, version: &str) -> Result<Manifest, BusinessError> {
        let path = self.layout.manifests_dir().join(format!("{version}.json"));
        let raw = fs::read(&path).map_err(|e| {
            BusinessError::new(
                codes::UPDATE_NOT_AVAILABLE,
                format!("候选 manifest 未缓存: {e}"),
            )
        })?;
        let value: Value = serde_json::from_slice(&raw).map_err(|e| {
            BusinessError::new(
                codes::ARTIFACT_INVALID,
                format!("缓存 manifest 解析失败: {e}"),
            )
        })?;
        Manifest::parse(&value).map_err(|e| {
            BusinessError::new(codes::ARTIFACT_INVALID, format!("缓存 manifest 非法: {e}"))
        })
    }

    // -- prepare_install ----------------------------------------------------------

    /// staged → staged：复验 staging 完整性并标记可切换（幂等）。
    pub fn phase_prepare_install(&self) -> Result<(), BusinessError> {
        let journal = self.load_journal()?;
        self.guard_not_manual(&journal)?;
        let to_version = if journal.state == UpdateState::Staged {
            journal.to_version.clone()
        } else if journal.state == UpdateState::Checking
            && journal.last_step.as_deref() == Some("checked")
        {
            // 候选已检查但未就位
            return Err(BusinessError::new(
                codes::UPDATE_NOT_READY,
                "候选尚未下载/就位（先 download）",
            ));
        } else {
            return Err(BusinessError::new(
                codes::UPDATE_BUSY,
                format!("当前状态 {} 无已就位候选", super::display_state(&journal)),
            ));
        };
        let Some(to_version) = to_version else {
            return Err(BusinessError::new(codes::UPDATE_NOT_READY, "无候选版本"));
        };
        // 意图先行（复验是一等持久边：staged → verifying → staged）
        self.mutate_journal(|j| {
            j.state = UpdateState::Verifying;
            j.last_step = Some("verifying".to_string());
        })?;
        match self.reverify_staging(&to_version) {
            Ok(()) => {
                self.mutate_journal(|j| {
                    j.state = UpdateState::Staged;
                    j.last_step = Some("staged".to_string());
                })?;
                Ok(())
            }
            Err(err) => {
                // 复验失败：回 staged 驻留并记录错误（候选可重下载）
                self.mutate_journal(|j| {
                    j.state = UpdateState::Staged;
                    j.last_step = Some("staged".to_string());
                    j.error = Some(crate::state::JournalError {
                        code: err.code.clone(),
                        message: err.message.clone(),
                    });
                })?;
                Err(err)
            }
        }
    }

    fn reverify_staging(&self, to_version: &str) -> Result<(), BusinessError> {
        let manifest = self.load_cached_manifest(to_version)?;
        let platform = manifest.platforms.get("windows-x86_64").ok_or_else(|| {
            BusinessError::new(codes::ARTIFACT_INVALID, "manifest 缺少 windows-x86_64 平台")
        })?;
        // 组件完好性（runtime 已换装位深检）
        for comp in &platform.components {
            let spec = ComponentSpec::from_model(comp).map_err(|e| {
                BusinessError::new(codes::ARTIFACT_INVALID, format!("组件规格非法: {e}"))
            })?;
            let finding = check_component(
                &spec.install_dir(&self.layout),
                &spec,
                CheckOptions {
                    deep: true,
                    probe: false,
                },
            );
            if finding.status != ComponentStatus::Ok {
                return Err(BusinessError::new(
                    codes::ARTIFACT_INVALID,
                    format!("组件 {} 复验未通过", comp.id),
                ));
            }
        }
        // app staging 完整性
        let update_id = self
            .load_journal()?
            .update_id
            .ok_or_else(|| BusinessError::new(codes::UPDATE_NOT_READY, "无升级事务 id"))?;
        let app = AppInstallSpec::from_model(platform, to_version).map_err(|e| {
            BusinessError::new(codes::ARTIFACT_INVALID, format!("app 规格非法: {e}"))
        })?;
        let app_staging = self.layout.staging_dir().join(update_id).join("app");
        verify_app_dir(&app_staging, &app).map_err(|e| {
            BusinessError::new(codes::UPDATE_NOT_READY, format!("staging 不完整: {e}"))
        })
    }

    // -- rollback（对外操作） -------------------------------------------------------

    /// IPC `rollback` / 手动取消：staged/waiting → 取消；快照在 → 完整恢复；
    /// 无有效回滚点 → rollback_unavailable。
    pub fn phase_rollback(&self) -> Result<&'static str, BusinessError> {
        let journal = self.load_journal()?;
        self.guard_not_manual(&journal)?;
        let cancellable = matches!(
            journal.state,
            UpdateState::Checking
                | UpdateState::Downloading
                | UpdateState::Verifying
                | UpdateState::Staged
                | UpdateState::WaitingIdle
        );
        if cancellable {
            self.mutate_journal(|j| {
                j.last_step = Some("rolling_back".to_string());
            })?;
            if let Some(update_id) = journal.update_id.clone() {
                let _ = fs::remove_dir_all(self.layout.staging_dir().join(&update_id));
            }
            self.mutate_journal(|j| {
                j.state = UpdateState::Idle;
                j.last_step = Some("idle".to_string());
                j.error = None;
            })?;
            return Ok("idle");
        }
        let rollbackable = matches!(
            journal.state,
            UpdateState::Draining
                | UpdateState::Stopped
                | UpdateState::Snapshotting
                | UpdateState::SnapshotVerified
                | UpdateState::Migrating
                | UpdateState::Switched
                | UpdateState::CandidateStarting
                | UpdateState::CandidateReady
                | UpdateState::Activating
        ) && journal.snapshot.is_some();
        if !rollbackable {
            // idle/committed：无事务或已提交，超出自动回滚承诺
            return Err(BusinessError::new(
                codes::ROLLBACK_UNAVAILABLE,
                "无有效回滚点（无进行中事务或无已验证快照）",
            ));
        }
        match self.rollback_procedure(&journal, None, false) {
            Ok(()) => Ok("idle"),
            Err(err) => Err(self.enter_manual_recovery(&err)),
        }
    }

    // -- 全链路（CLI 驱动） ----------------------------------------------------------

    /// §6.6 全链路：check → … → cleaning → idle。任一步失败按契约分支恢复。
    pub fn run_full(&self, source: &ManifestSource) -> UpgradeOutcome {
        let port = read_configured_port(&self.layout.config_file());
        // 1) check
        let checked = match self.phase_check(source) {
            Ok(c) => c,
            Err(err) => return UpgradeOutcome::FailedOldHealthy { error: err },
        };
        let to = checked.version.clone();
        tracing::info!(version = %to, "check 完成，候选可用");
        // 2) download → staged
        if let Err(err) = self.phase_download() {
            return UpgradeOutcome::FailedOldHealthy { error: err };
        }
        // 3) waiting_idle（手动 CLI 语义 = 立即安装；批次 3 无策略引擎）
        if let Err(err) = self.mutate_journal(|j| {
            j.state = UpdateState::WaitingIdle;
            j.last_step = Some("waiting_idle".to_string());
        }) {
            return self.fail_before_snapshot(err);
        }
        let journal = match self.load_journal() {
            Ok(j) => j,
            Err(err) => return self.fail_before_snapshot(err),
        };
        let Some(update_id) = journal.update_id.clone() else {
            return self.fail_before_snapshot(BusinessError::new(
                codes::ARTIFACT_INVALID,
                "journal 缺少 update_id",
            ));
        };
        let from = journal
            .from_version
            .clone()
            .or(journal.current_version.clone())
            .unwrap_or_default();
        // 4) draining：优雅停旧版本
        let old_boot_id = self.capture_boot_id(port);
        let was_running = self.server_listening(port);
        if let Err(reason) = self.drain_old_server(port, was_running) {
            // 准确 PID 未退出 → 默认取消（不硬杀，不 dirty_shutdown）
            let _ = self.mutate_journal(|j| {
                j.state = UpdateState::Staged;
                j.last_step = Some("staged".to_string());
                j.error = Some(crate::state::JournalError {
                    code: codes::UPDATE_BUSY.to_string(),
                    message: format!("旧版本未在时限内退出，升级已取消: {reason}"),
                });
            });
            return UpgradeOutcome::Cancelled { reason };
        }
        if let Err(err) = self.mutate_journal(|j| {
            j.state = UpdateState::Stopped;
            j.last_step = Some("stopped".to_string());
        }) {
            return self.fail_before_snapshot(err);
        }
        // 5) snapshotting → snapshot_verified
        if let Err(err) = self.mutate_journal(|j| {
            j.state = UpdateState::Snapshotting;
            j.last_step = Some("snapshotting".to_string());
        }) {
            return self.fail_before_snapshot(err);
        }
        let candidate_exe = self.resolve_candidate_exe(&update_id, &to);
        let snap = match snapshot::create(&self.layout, &update_id, candidate_exe.as_deref()) {
            Ok(s) => s,
            Err(reason) => {
                tracing::error!(%reason, "快照失败（数据未触及）");
                let err =
                    BusinessError::new(codes::ARTIFACT_INVALID, format!("快照失败: {reason}"));
                return self.fail_after_stop(err, was_running, port);
            }
        };
        let schema_before = self.probe_live_schema();
        if let Err(err) = self.mutate_journal(|j| {
            j.state = UpdateState::SnapshotVerified;
            j.last_step = Some("snapshot_verified".to_string());
            j.snapshot = Some(SnapshotInfo {
                id: snap.id.clone(),
                path: snap.path.clone(),
                file_count: snap.file_count,
                total_bytes: snap.total_bytes,
            });
            j.data_schema_before = schema_before;
            j.data_schema_after = snap.schema_after;
        }) {
            return self.fail_after_stop(err, was_running, port);
        }
        // 6) migrating（passthrough；数据迁移由候选 gate 启动时自行执行）
        if let Err(err) = self.mutate_journal(|j| {
            j.state = UpdateState::Migrating;
            j.last_step = Some("migrating".to_string());
        }) {
            return self.fail_after_stop(err, was_running, port);
        }
        // 7) switched：versions/<to>/ 换入 + current.json（previous 保留）
        if let Err(err) = self.install_app_dir(&update_id, &to) {
            return self.fail_after_switch(err, was_running, port);
        }
        if let Err(err) = self.mutate_journal(|j| {
            j.state = UpdateState::Switched;
            j.last_step = Some("switched".to_string());
        }) {
            return self.fail_after_switch(err, was_running, port);
        }
        if let Err(err) = self.write_switched_pointer(&from, &to) {
            return self.fail_after_switch(err, was_running, port);
        }
        // 8) candidate_starting → candidate_ready
        let mut child = match self.start_candidate(&to) {
            Ok(c) => c,
            Err(err) => return self.fail_after_switch(err, was_running, port),
        };
        let exe_for_journal = resolve_entrypoint(&self.layout, &to)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        if let Err(err) = self.mutate_journal(|j| {
            j.state = UpdateState::CandidateStarting;
            j.last_step = Some("candidate_starting".to_string());
            j.child = Some(ChildInfo {
                pid: child.id(),
                created_at_unix_ms: Some(now_unix_millis()),
                exe: exe_for_journal,
            });
        }) {
            return self.fail_candidate(err, &mut child, was_running, port);
        }
        let expected_schema = u32::try_from(checked.manifest.release.data_schema).ok();
        if let Err(err) = self.wait_candidate_ready(
            port,
            old_boot_id.as_deref(),
            &to,
            expected_schema,
            &mut child,
        ) {
            return self.fail_candidate(err, &mut child, was_running, port);
        }
        if let Err(err) = self.mutate_journal(|j| {
            j.state = UpdateState::CandidateReady;
            j.last_step = Some("candidate_ready".to_string());
        }) {
            return self.fail_candidate(err, &mut child, was_running, port);
        }
        // 9) activating → committed
        if let Err(err) = self.mutate_journal(|j| {
            j.state = UpdateState::Activating;
            j.last_step = Some("activating".to_string());
        }) {
            return self.fail_candidate(err, &mut child, was_running, port);
        }
        if let Err(err) = self.activate(port) {
            return self.fail_candidate(err, &mut child, was_running, port);
        }
        if let Err(err) = self.mutate_journal(|j| {
            j.state = UpdateState::Committed;
            j.last_step = Some("committed".to_string());
            j.error = None;
        }) {
            // journal 写不进但候选已激活：不再回滚（激活后不属于自动快照回滚承诺），如实上报
            return UpgradeOutcome::ManualRecovery { error: err };
        }
        // 10) cleaning → idle
        let _ = fs::remove_dir_all(self.layout.staging_dir().join(&update_id));
        let _ = self.mutate_journal(|j| {
            j.state = UpdateState::Cleaning;
            j.last_step = Some("cleaning".to_string());
        });
        if let Err(err) = self.mutate_journal(|j| {
            j.state = UpdateState::Idle;
            j.last_step = Some("idle".to_string());
            j.error = None;
        }) {
            return UpgradeOutcome::ManualRecovery { error: err };
        }
        tracing::info!(%from, %to, "升级 committed 并清理完成");
        UpgradeOutcome::Committed { from, to }
    }

    fn fail_before_snapshot(&self, err: BusinessError) -> UpgradeOutcome {
        // 数据未触及：直接回 idle/failed（旧版健康）
        let _ = self.mutate_journal(|j| Self::set_failed_idle(j, &err));
        UpgradeOutcome::FailedOldHealthy { error: err }
    }

    /// stopped 后失败：数据未切换，重启旧版本（无需恢复快照）。
    fn fail_after_stop(&self, err: BusinessError, was_running: bool, port: u16) -> UpgradeOutcome {
        let _ = self.mutate_journal(|j| {
            j.last_step = Some("rolling_back".to_string());
        });
        if was_running {
            self.restart_old_and_verify(port);
        }
        let _ = self.mutate_journal(|j| Self::set_failed_idle(j, &err));
        UpgradeOutcome::FailedOldHealthy { error: err }
    }

    /// switched 后失败：候选可能已迁移数据 → 完整回滚。
    fn fail_after_switch(
        &self,
        err: BusinessError,
        was_running: bool,
        _port: u16,
    ) -> UpgradeOutcome {
        let journal = match self.load_journal() {
            Ok(j) => j,
            Err(e) => return UpgradeOutcome::ManualRecovery { error: e },
        };
        // 失败原因先落 journal（计划 §6.6「错误摘要」）：rollback_procedure 的
        // 终态只改 state/child/last_step，不覆盖 error——回滚成功后 journal 保持
        // idle/failed + 原始错误，人工排查有据可查。
        let _ = self.mutate_journal(|j| {
            j.error = Some(crate::state::JournalError {
                code: err.code.clone(),
                message: err.message.clone(),
            });
        });
        match self.rollback_procedure(&journal, None, was_running) {
            Ok(()) => UpgradeOutcome::FailedOldHealthy { error: err },
            Err(rollback_err) => UpgradeOutcome::ManualRecovery {
                error: self.enter_manual_recovery(&rollback_err),
            },
        }
    }

    /// candidate 在跑时的失败：先精确停候选再走完整回滚。
    fn fail_candidate(
        &self,
        err: BusinessError,
        child: &mut Child,
        was_running: bool,
        port: u16,
    ) -> UpgradeOutcome {
        tracing::warn!(code = %err.code, %err.message, "候选阶段失败，开始回滚");
        let _ = child.kill();
        let _ = child.wait();
        self.fail_after_switch(err, was_running, port)
    }

    // -- 恢复/回滚共用 ---------------------------------------------------------------

    /// 完整回滚（计划 §6.6）：rolling_back 标记 → 停候选（句柄优先，孤儿按
    /// journal PID+exe 精确终止）→ 隔离失败数据 + 恢复快照 → current.json 切回
    /// previous → （可选）重启旧版本。成功 journal → Idle(failed)；失败 Err。
    pub fn rollback_procedure(
        &self,
        journal: &UpdateJournal,
        live_child: Option<&mut Child>,
        restart_old: bool,
    ) -> Result<(), BusinessError> {
        let _ = self.mutate_journal(|j| {
            j.last_step = Some("rolling_back".to_string());
        });
        // 1) 停候选
        if let Some(child) = live_child {
            let _ = child.kill();
            let _ = child.wait();
        } else if let Some(info) = &journal.child {
            if info.pid > 0 && !info.exe.is_empty() {
                self.pid_ops
                    .terminate_if_image(info.pid, Path::new(&info.exe));
            }
        }
        // 2) 恢复快照（switched 之后才可能有数据变更；此前恢复是等价冗余，跳过）
        let switched = matches!(
            journal.state,
            UpdateState::SnapshotVerified
                | UpdateState::Migrating
                | UpdateState::Switched
                | UpdateState::CandidateStarting
                | UpdateState::CandidateReady
                | UpdateState::Activating
                | UpdateState::Committed
        );
        if switched {
            let Some(snap) = &journal.snapshot else {
                return Err(BusinessError::new(
                    codes::ROLLBACK_UNAVAILABLE,
                    "已切换但 journal 无快照信息，无法自动恢复",
                ));
            };
            snapshot::restore(&self.layout, &snap.id).map_err(|e| {
                BusinessError::new(codes::ROLLBACK_UNAVAILABLE, format!("恢复快照失败: {e}"))
            })?;
        }
        // 3) current.json 切回 previous
        let from = journal
            .from_version
            .clone()
            .or_else(|| journal.current_version.clone())
            .ok_or_else(|| {
                BusinessError::new(codes::ROLLBACK_UNAVAILABLE, "journal 缺少 from_version")
            })?;
        self.store()
            .write_current(&CurrentState::new(
                from.clone(),
                journal.previous_version.clone(),
            ))
            .map_err(|e| {
                BusinessError::new(
                    codes::ROLLBACK_UNAVAILABLE,
                    format!("写回 current.json 失败: {e}"),
                )
            })?;
        // 4) 重启旧版本并验证 ready
        if restart_old {
            let port = read_configured_port(&self.layout.config_file());
            if !self.restart_old_and_verify(port) {
                return Err(BusinessError::new(
                    codes::ROLLBACK_UNAVAILABLE,
                    format!("旧版本重启或就绪校验失败（端口 {port}）"),
                ));
            }
        }
        // 5) journal → idle/failed（旧版健康）
        self.mutate_journal(|j| {
            j.state = UpdateState::Idle;
            j.child = None;
            j.last_step = Some("failed".to_string());
        })?;
        tracing::info!(from = %from, "回滚完成，旧版本已恢复");
        Ok(())
    }

    /// 重启旧版本并轮询 ready；失败返回 false，由调用方转入人工恢复。
    fn restart_old_and_verify(&self, port: u16) -> bool {
        let Ok(LoadOutcome::Present(current)) = self.store().load_current() else {
            tracing::error!("重启旧版本失败：current.json 不可读");
            return false;
        };
        let Ok(exe) = resolve_entrypoint(&self.layout, &current.current) else {
            tracing::error!("重启旧版本失败：入口解析失败");
            return false;
        };
        let app_dir = self.layout.versions_dir().join(&current.current);
        let plan = self.launch_plan_for(&current.current, exe, app_dir);
        // 重启的旧版本同样注入回环管理令牌，保证后续 drain/再次升级可用
        let extras = LaunchExtras::default()
            .with_admin_token(self.opts.admin_token.clone())
            .pipe_with(self.opts.ipc.clone());
        // stdio 全脱离（null）：升级器进程是一次性的（commit/回滚后即退出），
        // 子进程必须不继承其控制台/管道——否则 CLI 退出、读取端关闭后，继承的
        // stdio 句柄失效，子进程再派生的外部进程探针（adb/ffmpeg）会持续失败。
        // server 自身日志走 GB_LOG 文件，不依赖继承的 stdout/stderr。
        match spawn_child_with_extras(&plan, &[], &extras, Stdio::null(), Stdio::null()) {
            Ok(child) => {
                let probe = self.opts.probe.clone();
                match supervisor::wait_for_ready(port, &probe) {
                    Ok(()) => {
                        tracing::info!(pid = child.id(), "旧版本已重启且就绪");
                        std::thread::spawn(move || {
                            let mut child = child;
                            let _ = child.wait();
                        });
                        true
                    }
                    Err(reason) => {
                        tracing::error!(%reason, "旧版本重启后就绪探测未通过（保留进程，状态以 journal 为准）");
                        std::thread::spawn(move || {
                            let mut child = child;
                            let _ = child.wait();
                        });
                        false
                    }
                }
            }
            Err(e) => {
                tracing::error!("旧版本重启失败: {e}");
                false
            }
        }
    }

    fn launch_plan_for(&self, version: &str, exe: PathBuf, app_dir: PathBuf) -> LaunchPlan {
        LaunchPlan {
            exe,
            cwd: app_dir.clone(),
            app_dir,
            data_dir: self.layout.data_dir(),
            adb_path: supervisor::latest_component_exe(&self.layout, "adb", "adb.exe"),
            ffmpeg_path: supervisor::latest_component_exe(&self.layout, "ffmpeg", "ffmpeg.exe"),
            scrcpy_server: self
                .layout
                .versions_dir()
                .join(version)
                .join("assets")
                .join("scrcpy-server.jar"),
            config_path: self.layout.config_file(),
            log_path: self.layout.logs_dir().join("gamer-server.log"),
        }
    }

    // -- 候选启动 / 探测 / activate -----------------------------------------------------

    fn install_app_dir(&self, update_id: &str, version: &str) -> Result<(), BusinessError> {
        let staged = self.layout.staging_dir().join(update_id).join("app");
        let target = self.layout.versions_dir().join(version);
        if target.exists() {
            // 版本目录不可变：已存在且复验通过则复用；否则失败（不覆盖）
            let manifest = self.load_cached_manifest(version)?;
            let platform = manifest
                .platforms
                .get("windows-x86_64")
                .ok_or_else(|| BusinessError::new(codes::ARTIFACT_INVALID, "manifest 缺少平台"))?;
            let app = AppInstallSpec::from_model(platform, version)
                .map_err(|e| BusinessError::new(codes::ARTIFACT_INVALID, e))?;
            return verify_app_dir(&target, &app).map_err(|e| {
                BusinessError::new(
                    codes::ARTIFACT_INVALID,
                    format!("目标版本目录已存在但复验失败（不覆盖）: {e}"),
                )
            });
        }
        fs::create_dir_all(self.layout.versions_dir()).map_err(|e| {
            BusinessError::new(codes::ARTIFACT_INVALID, format!("创建 versions/ 失败: {e}"))
        })?;
        rename_with_retry(&staged, &target).map_err(|e| {
            BusinessError::new(
                codes::ARTIFACT_INVALID,
                format!("staging 换入 versions/ 失败: {e}"),
            )
        })
    }

    fn write_switched_pointer(&self, from: &str, to: &str) -> Result<(), BusinessError> {
        self.store()
            .write_current(&CurrentState::new(to.to_string(), Some(from.to_string())))
            .map_err(|e| {
                BusinessError::new(
                    codes::ROLLBACK_UNAVAILABLE,
                    format!("写 current.json 失败: {e}"),
                )
            })
    }

    fn start_candidate(&self, version: &str) -> Result<Child, BusinessError> {
        let exe = resolve_entrypoint(&self.layout, version)
            .map_err(|e| BusinessError::new(codes::ARTIFACT_INVALID, e))?;
        let app_dir = self.layout.versions_dir().join(version);
        let plan = self.launch_plan_for(version, exe, app_dir);
        let extras = match &self.opts.ipc {
            Some((pipe, token)) => LaunchExtras::candidate(pipe.clone(), token.clone()),
            None => LaunchExtras {
                activation_gate: true,
                ..LaunchExtras::default()
            },
        }
        // 候选就是提交后的「现网版本」：同样注入回环管理令牌，否则下一次升级
        // 对它 drain 时 /api/shutdown 会 401（实测缺陷，2026-08-31）。
        .with_admin_token(self.opts.admin_token.clone());
        // stdio 全脱离（null）：升级器进程是一次性的（commit 后即退出，候选成为
        // 孤儿继续服务）。继承 CLI 的 stdout/stderr 会在 CLI 退出、管道读取端关闭
        // 后失效，导致候选再派生的外部进程探针（adb/ffmpeg readiness）持续失败
        // （实测缺陷，2026-08-31）；server 日志走 GB_LOG 文件，无需继承 stdio。
        spawn_child_with_extras(&plan, &[], &extras, Stdio::null(), Stdio::null()).map_err(|e| {
            BusinessError::new(codes::LAUNCHER_UNREACHABLE, format!("候选启动失败: {e}"))
        })
    }

    fn resolve_candidate_exe(&self, update_id: &str, version: &str) -> Option<PathBuf> {
        let staged = self.layout.staging_dir().join(update_id).join("app");
        let manifest = self.load_cached_manifest(version).ok()?;
        let platform = manifest.platforms.get("windows-x86_64")?;
        let entry = platform.app.entrypoint.replace('/', "\\");
        let exe = staged.join(&entry);
        if exe.is_file() {
            return Some(exe);
        }
        resolve_entrypoint(&self.layout, version).ok()
    }

    /// GET /health/ready，尽力解析旧实例 boot_id。
    fn capture_boot_id(&self, port: u16) -> Option<String> {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let resp = http_request(
            addr,
            "GET",
            HEALTH_PATH,
            &[],
            self.opts.probe.per_attempt_timeout,
        )
        .ok()?;
        let body = resp.body_json()?;
        find_string_field(&body, &["boot_id", "bootId"])
    }

    fn server_listening(&self, port: u16) -> bool {
        std::net::TcpStream::connect_timeout(
            &SocketAddr::from(([127, 0, 0, 1], port)),
            Duration::from_millis(300),
        )
        .is_ok()
    }

    /// draining：POST /api/shutdown → 等待端口关闭（有句柄时调用方另以 wait 兜底）。
    /// 超时 = 取消升级（契约默认，不硬杀）。带 X-Admin-Token 时走服务端回环
    /// 管理通道鉴权（本机管理凭据与子进程注入值同源）。
    ///
    /// 读超时语义（缺陷 #1，真机验收 2026-08-31，见
    /// docs/evidence/UPDATE_REALDEVICE_EVIDENCE.md §R-7）：server `/api/shutdown` handler
    /// **同步 await 完整 drain**（活动 run 10s 宽限 + 拆全部 scrcpy 会话，实测
    /// 11.6s）才回 200。读超时若小于完整 drain 时长，launcher 提前断开 →
    /// hyper 取消 handler future → `ShutdownCoordinator::request()` 在
    /// `(self.drain)().await` 处被 drop，drain 半途停滞且无自恢复 → 引擎只能
    /// 在 shutdown_timeout 后取消升级（无会话场景 drain 秒级故 E2E 未暴露）。
    /// 因此读超时必须覆盖 server 侧完整 drain：取 `shutdown_timeout + 5s`
    /// （余量只用于把响应读回来）；总等待仍由下方以 `started` 起算的
    /// shutdown_timeout deadline 收口，取消上界不变。
    fn drain_old_server(&self, port: u16, running: bool) -> Result<(), String> {
        if !running {
            return Ok(());
        }
        let started = Instant::now();
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let mut headers: Vec<(&str, &str)> = Vec::new();
        let admin_token = self
            .opts
            .admin_token
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty());
        if let Some(token) = admin_token {
            headers.push(("X-Admin-Token", token));
        }
        let shutdown = http_request(
            addr,
            "POST",
            "/api/shutdown",
            &headers,
            self.opts.shutdown_timeout + Duration::from_secs(5),
        );
        match shutdown {
            Ok(resp) if (200..300).contains(&resp.status) => {}
            Ok(resp) => {
                tracing::warn!(
                    status = resp.status,
                    "/api/shutdown 返回非 2xx，仍继续等待退出"
                );
            }
            Err(reason) => {
                tracing::warn!(%reason, "/api/shutdown 请求失败（可能未在监听），继续等待端口关闭");
            }
        }
        let deadline = started + self.opts.shutdown_timeout;
        while Instant::now() < deadline {
            if !self.server_listening(port) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Err(format!(
            "端口 {port} 在 {:?} 内仍未关闭",
            self.opts.shutdown_timeout
        ))
    }

    /// candidate_ready：/health/ready 200 + boot_id 差异 + 版本一致（info 优先，
    /// 非 200 回退 ready body 字段）。候选进程提前退出 → 立即失败。
    /// 候选带激活闸时（ipc 已配置），/health/ready 在 activate 前恒为
    /// 503 ready:false——引擎在此先完成 activate（幂等；后续 activating 边的
    /// 重复调用命中幂等回执），否则门内候选永远等不到 200。
    fn wait_candidate_ready(
        &self,
        port: u16,
        old_boot_id: Option<&str>,
        expected_version: &str,
        expected_schema: Option<u32>,
        child: &mut impl CandidateProcessProbe,
    ) -> Result<Option<String>, BusinessError> {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let deadline = Instant::now() + self.opts.probe.overall_timeout;
        let mut last: Option<String> = None;
        let gate_expected = self.opts.ipc.is_some();
        let mut gate_activate_attempted = false;
        loop {
            if let Ok(Some(_)) = child.try_wait() {
                return Err(BusinessError::new(
                    codes::ARTIFACT_INVALID,
                    "候选进程在就绪探测期间退出",
                ));
            }
            if Instant::now() >= deadline {
                return Err(BusinessError::new(
                    codes::LAUNCHER_UNREACHABLE,
                    format!(
                        "候选未在时限内就绪: {}",
                        last.unwrap_or_else(|| "无探测记录".to_string())
                    ),
                ));
            }
            match http_request(
                addr,
                "GET",
                HEALTH_PATH,
                &[],
                self.opts.probe.per_attempt_timeout,
            ) {
                Ok(resp) if resp.status == 200 => {
                    return self.verify_candidate_identity(
                        addr,
                        old_boot_id,
                        expected_version,
                        expected_schema,
                    );
                }
                Ok(resp) => {
                    last = Some(format!("HTTP {}", resp.status));
                    if gate_expected
                        && !gate_activate_attempted
                        && resp.status == 503
                        && resp
                            .body_json()
                            .and_then(|body| body.get("ready").and_then(serde_json::Value::as_bool))
                            == Some(false)
                    {
                        gate_activate_attempted = true;
                        match self.activate(port) {
                            Ok(()) => {
                                tracing::info!("候选处于激活闸内，已先行 activate（幂等）");
                                last = Some("激活闸内，activate 已受理".to_string());
                            }
                            // 令牌被拒 = 无法激活的确定性失败，立即中止等待
                            Err(err) if err.message.contains("(HTTP 403)") => return Err(err),
                            Err(err) => {
                                tracing::debug!(error = %err, "闸内 activate 未受理（继续等待就绪）")
                            }
                        }
                    }
                }
                Err(reason) => last = Some(reason),
            }
            std::thread::sleep(self.opts.probe.interval);
        }
    }

    /// boot_id / app.version 校验；info 非 200 时回退 /health/ready body 字段。
    fn verify_candidate_identity(
        &self,
        addr: SocketAddr,
        old_boot_id: Option<&str>,
        expected_version: &str,
        expected_schema: Option<u32>,
    ) -> Result<Option<String>, BusinessError> {
        // 1) GET /api/system/info（登录保护 → 可能 401/重定向拒绝）
        let info = http_request(
            addr,
            "GET",
            "/api/system/info",
            &[],
            self.opts.probe.per_attempt_timeout,
        )
        .ok();
        let (version, boot, schema) = match info {
            Some(resp) if resp.status == 200 => match resp.body_json() {
                Some(body) => (
                    // 结构化路径优先，防依赖字段里的同名键
                    body.get("app")
                        .and_then(|app| app.get("version"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .or_else(|| find_string_field(&body, &["app_version"])),
                    body.get("startup")
                        .and_then(|s| s.get("boot_id"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .or_else(|| find_string_field(&body, &["boot_id", "bootId"])),
                    find_schema(&body),
                ),
                None => (None, None, None),
            },
            _ => {
                // 2) 回退 /health/ready body 字段（匿名；字段形态实测前尽力解析）
                let resp = http_request(
                    addr,
                    "GET",
                    HEALTH_PATH,
                    &[],
                    self.opts.probe.per_attempt_timeout,
                )
                .map_err(|e| {
                    BusinessError::new(codes::LAUNCHER_UNREACHABLE, format!("就绪探测失败: {e}"))
                })?;
                if resp.status != 200 {
                    return Err(BusinessError::new(
                        codes::LAUNCHER_UNREACHABLE,
                        format!("ready 探测非 200（{}）", resp.status),
                    ));
                }
                match resp.body_json() {
                    Some(body) => (
                        find_string_field(&body, &["app_version"]),
                        find_string_field(&body, &["boot_id", "bootId"]),
                        find_schema(&body),
                    ),
                    None => (None, None, None),
                }
            }
        };
        // 版本校验：观测到才校验（观测不到 = exe 路径由本次 spawn 锚定）
        if let Some(v) = &version {
            if v != expected_version {
                return Err(BusinessError::new(
                    codes::ARTIFACT_INVALID,
                    format!("候选版本不符（观测 {v}，期望 {expected_version}）"),
                ));
            }
        }
        if let (Some(expected), Some(actual)) = (expected_schema, schema) {
            if actual != expected {
                return Err(BusinessError::new(
                    codes::SCHEMA_INCOMPATIBLE,
                    format!("候选数据 schema 不符（观测 {actual}，期望 {expected}）"),
                ));
            }
        }
        // boot_id 校验：双方都观测到才比对（新实例 boot_id 必须变化）
        if let (Some(old), Some(new)) = (old_boot_id, boot.as_deref()) {
            if old == new {
                return Err(BusinessError::new(
                    codes::ARTIFACT_INVALID,
                    "boot_id 与旧实例相同（疑似旧进程仍在服务）",
                ));
            }
        }
        Ok(boot)
    }

    /// activating：POST /api/system/activate（X-Launcher-Token）。
    fn activate(&self, port: u16) -> Result<(), BusinessError> {
        let token = self.opts.ipc.as_ref().map(|(_, t)| t.clone());
        let Some(token) = token else {
            if self.opts.activate_without_token {
                tracing::warn!("未配置 IPC 令牌，按演练模式跳过 activate");
                return Ok(());
            }
            return Err(BusinessError::new(
                codes::UPDATE_NOT_READY,
                "未配置 IPC 会话令牌，无法执行 activate",
            ));
        };
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let resp = http_request(
            addr,
            "POST",
            "/api/system/activate",
            &[("X-Launcher-Token", token.as_str())],
            self.opts
                .probe
                .per_attempt_timeout
                .max(Duration::from_secs(10)),
        )
        .map_err(|e| {
            BusinessError::new(
                codes::LAUNCHER_UNREACHABLE,
                format!("activate 请求失败: {e}"),
            )
        })?;
        if !(200..300).contains(&resp.status) {
            return Err(BusinessError::new(
                codes::UPDATE_NOT_READY,
                format!("activate 被拒绝（HTTP {}）", resp.status),
            ));
        }
        Ok(())
    }
}

/// 在 JSON 树里递归找指定字符串字段（info/ready body 形态实测前尽力解析）。
fn find_string_field(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(v) = map.get(*key).and_then(Value::as_str) {
                    return Some(v.to_string());
                }
            }
            map.values().find_map(|v| find_string_field(v, keys))
        }
        Value::Array(items) => items.iter().find_map(|v| find_string_field(v, keys)),
        _ => None,
    }
}

fn find_schema(value: &Value) -> Option<u32> {
    value
        .get("schema")
        .and_then(|schema| schema.get("db"))
        .and_then(Value::as_u64)
        .and_then(|schema| u32::try_from(schema).ok())
        .or_else(|| find_u32_field(value, &["data_schema", "db_schema", "user_version"]))
}

fn find_u32_field(value: &Value, keys: &[&str]) -> Option<u32> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(schema) = map
                    .get(*key)
                    .and_then(Value::as_u64)
                    .and_then(|schema| u32::try_from(schema).ok())
                {
                    return Some(schema);
                }
            }
            map.values().find_map(|child| find_u32_field(child, keys))
        }
        Value::Array(items) => items.iter().find_map(|child| find_u32_field(child, keys)),
        _ => None,
    }
}

fn dir_size(path: &Path) -> u64 {
    if !path.is_dir() {
        return 0;
    }
    let mut total = 0u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).into_iter().flatten() {
            let Ok(entry) = entry else { continue };
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                total += entry.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::AtomicBool;
    use std::sync::Mutex;
    use std::thread::JoinHandle;

    fn temp_root(tag: &str) -> PathBuf {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "gamer-upgrade-engine-{tag}-{}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst),
            now_unix_millis()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    type TestResponder = Arc<dyn Fn(&str) -> (u16, String) + Send + Sync>;

    fn serve_for(millis: u64, responder: TestResponder) -> (SocketAddr, JoinHandle<()>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_millis(millis);
            while Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let mut request = Vec::new();
                        let mut buf = [0u8; 1024];
                        loop {
                            match stream.read(&mut buf) {
                                Ok(0) => break,
                                Ok(n) => {
                                    request.extend_from_slice(&buf[..n]);
                                    if request.windows(4).any(|w| w == b"\r\n\r\n") {
                                        break;
                                    }
                                }
                                Err(_) => break,
                            }
                        }
                        let request = String::from_utf8_lossy(&request);
                        let path = request.split_whitespace().nth(1).unwrap_or("/");
                        let (status, body) = responder(path);
                        let reason = if status == 200 {
                            "OK"
                        } else if status == 503 {
                            "Service Unavailable"
                        } else {
                            "Unauthorized"
                        };
                        let response = format!(
                            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
        });
        (addr, handle)
    }

    fn engine() -> (Engine, PathBuf) {
        let root = temp_root("case");
        (
            Engine::new(
                InstallLayout { root: root.clone() },
                UpgradeOptions::default(),
            ),
            root,
        )
    }

    /// 缺陷 #1 回归台架：模拟真实 server `/api/shutdown` 的耦合语义——
    /// handler **同步 await 完整 drain**（本 mock 延迟 `delay_ms` 才回 200）；
    /// 客户端若在读到响应前断开（旧缺陷：读超时 5s < drain 时长），等价于
    /// hyper 取消 handler future → drain 永不完成 → 端口不关闭。
    /// 客户端是否坚持到响应：mock 在延迟窗口内对连接做带超时的探测读——
    /// 返回 `Ok(0)`/连接错误 = 客户端已断开（记入 abandoned 并保持监听，
    /// 模拟 drain 停滞）；超时（连接仍在）= 客户端在等待 → 回 200 并关监听。
    fn serve_slow_shutdown(delay_ms: u64) -> (SocketAddr, Arc<AtomicBool>, JoinHandle<()>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let abandoned = Arc::new(AtomicBool::new(false));
        let handle = {
            let abandoned = abandoned.clone();
            std::thread::spawn(move || {
                let listener = listener;
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut buf = [0u8; 1024];
                loop {
                    match stream.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            request.extend_from_slice(&buf[..n]);
                            if request.windows(4).any(|w| w == b"\r\n\r\n") {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                assert!(
                    String::from_utf8_lossy(&request).starts_with("POST /api/shutdown "),
                    "台架只应收到 /api/shutdown"
                );
                stream
                    .set_read_timeout(Some(Duration::from_millis(delay_ms + 500)))
                    .unwrap();
                let mut probe = [0u8; 1];
                match stream.read(&mut probe) {
                    Ok(0) => abandoned.store(true, Ordering::SeqCst),
                    Err(e)
                        if !matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) =>
                    {
                        abandoned.store(true, Ordering::SeqCst)
                    }
                    _ => {}
                }
                if abandoned.load(Ordering::SeqCst) {
                    // 客户端已断开（回归态）：保持端口监听，模拟 drain 停滞；
                    // 线程只在回归路径存活，测试进程退出时随之消亡。
                    loop {
                        std::thread::sleep(Duration::from_secs(1));
                    }
                }
                let body = br#"{"ok":true}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                drop(stream);
                drop(listener); // 回 200 后关监听 → 端口关闭，drain 可完成
            })
        };
        (addr, abandoned, handle)
    }

    fn prepare_switched_fixture(engine: &Engine, root: &Path, update_id: &str) {
        let store = StateStore::new(root);
        store
            .write_current(&CurrentState::new("1.0.0", None))
            .unwrap();
        fs::create_dir_all(root.join("data")).unwrap();
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(root.join("data/state.bin"), b"old").unwrap();
        fs::write(root.join("config/config.toml"), b"port = 8443\n").unwrap();

        let report = snapshot::create(&engine.layout, update_id, None).unwrap();
        fs::write(root.join("data/state.bin"), b"migrated").unwrap();
        fs::write(root.join("data/candidate-only.bin"), b"candidate").unwrap();
        store
            .write_current(&CurrentState::new("2.0.0", Some("1.0.0".to_string())))
            .unwrap();
        store
            .write_journal(&UpdateJournal {
                update_id: Some(update_id.to_string()),
                state: UpdateState::CandidateReady,
                from_version: Some("1.0.0".to_string()),
                to_version: Some("2.0.0".to_string()),
                snapshot: Some(SnapshotInfo {
                    id: report.id,
                    path: report.path,
                    file_count: report.file_count,
                    total_bytes: report.total_bytes,
                }),
                ..UpdateJournal::default()
            })
            .unwrap();
    }

    fn assert_automatic_rollback_completed(engine: &Engine, root: &Path) {
        assert!(matches!(
            StateStore::new(root).load_current(),
            Ok(LoadOutcome::Present(CurrentState { current, .. })) if current == "1.0.0"
        ));
        assert_eq!(fs::read(root.join("data/state.bin")).unwrap(), b"old");
        assert!(!root.join("data/candidate-only.bin").exists());
        let journal = StateStore::new(root).load_journal().unwrap().journal;
        assert_eq!(journal.state, UpdateState::Idle);
        assert_eq!(journal.last_step.as_deref(), Some("failed"));
        let quarantined = fs::read_dir(engine.layout.quarantine_dir())
            .map(|entries| {
                entries
                    .flatten()
                    .any(|entry| entry.path().join("data/state.bin").is_file())
            })
            .unwrap_or(false);
        assert!(quarantined, "失败数据必须保留在 quarantine");
    }

    struct AlwaysRunning;

    impl CandidateProcessProbe for AlwaysRunning {
        fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
            Ok(None)
        }
    }

    struct ExitedImmediately(Option<std::process::ExitStatus>);

    impl CandidateProcessProbe for ExitedImmediately {
        fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
            Ok(self.0.take())
        }
    }

    #[test]
    fn candidate_identity_rejects_wrong_version() {
        let (engine, root) = engine();
        let (addr, server) = serve_for(
            250,
            Arc::new(|_| {
                (
                    200,
                    r#"{"app":{"version":"9.9.9"},"schema":{"db":2},"startup":{"boot_id":"new"}}"#
                        .to_string(),
                )
            }),
        );
        let err = engine
            .verify_candidate_identity(addr, Some("old"), "2.0.0", Some(2))
            .unwrap_err();
        assert_eq!(err.code, codes::ARTIFACT_INVALID);
        assert!(err.message.contains("版本不符"));
        server.join().unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn candidate_identity_rejects_wrong_schema() {
        let (engine, root) = engine();
        let (addr, server) = serve_for(
            250,
            Arc::new(|_| {
                (
                    200,
                    r#"{"app":{"version":"2.0.0"},"schema":{"db":1},"startup":{"boot_id":"new"}}"#
                        .to_string(),
                )
            }),
        );
        let err = engine
            .verify_candidate_identity(addr, Some("old"), "2.0.0", Some(2))
            .unwrap_err();
        assert_eq!(err.code, codes::SCHEMA_INCOMPATIBLE);
        assert!(err.message.contains("schema"));
        server.join().unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn candidate_identity_rejects_reused_boot_id() {
        let (engine, root) = engine();
        let (addr, server) = serve_for(
            250,
            Arc::new(|_| {
                (
                    200,
                    r#"{"app":{"version":"2.0.0"},"schema":{"db":2},"startup":{"boot_id":"old"}}"#
                        .to_string(),
                )
            }),
        );
        let err = engine
            .verify_candidate_identity(addr, Some("old"), "2.0.0", Some(2))
            .unwrap_err();
        assert_eq!(err.code, codes::ARTIFACT_INVALID);
        assert!(err.message.contains("boot_id"));
        server.join().unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn candidate_identity_fallback_checks_version_schema_and_boot_from_ready() {
        let (engine, root) = engine();
        let (addr, server) = serve_for(
            1_000,
            Arc::new(|path| {
                if path == "/api/system/info" {
                    (401, r#"{"error":"unauthorized"}"#.to_string())
                } else {
                    (
                        200,
                        r#"{"app_version":"2.0.0","schema":{"db":2},"boot_id":"new"}"#.to_string(),
                    )
                }
            }),
        );
        let boot = engine
            .verify_candidate_identity(addr, Some("old"), "2.0.0", Some(2))
            .expect("ready body 应可作为回退身份");
        assert_eq!(boot.as_deref(), Some("new"));
        server.join().unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn candidate_readiness_failure_is_bounded_without_a_real_child_process() {
        let (engine, root) = engine();
        let (addr, server) = serve_for(300, Arc::new(|_| (503, r#"{"ready":false}"#.to_string())));
        let mut child = AlwaysRunning;
        let mut opts = engine.opts.clone();
        opts.probe = ReadyProbe {
            overall_timeout: Duration::from_millis(80),
            per_attempt_timeout: Duration::from_millis(20),
            interval: Duration::from_millis(10),
        };
        let engine = Engine::new(engine.layout.clone(), opts);
        let err = engine
            .wait_candidate_ready(addr.port(), None, "2.0.0", Some(2), &mut child)
            .unwrap_err();
        assert_eq!(err.code, codes::LAUNCHER_UNREACHABLE);
        assert!(err.message.contains("时限") || err.message.contains("就绪"));
        server.join().unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn qa005_candidate_exit_is_immediate_failure_and_auto_rollback() {
        let (engine, root) = engine();
        let status = std::process::Command::new("cmd.exe")
            .args(["/C", "exit", "1"])
            .status()
            .expect("Windows cmd.exe 应可生成退出状态");
        let mut child = ExitedImmediately(Some(status));
        let err = engine
            .wait_candidate_ready(0, None, "2.0.0", Some(2), &mut child)
            .expect_err("候选立即退出必须立即失败，不应等待 ready 超时");
        assert_eq!(err.code, codes::ARTIFACT_INVALID);
        assert!(err.message.contains("退出"));

        prepare_switched_fixture(&engine, &root, "qa005-candidate-exit");
        let outcome = engine.fail_after_switch(err, false, 0);
        assert!(matches!(
            outcome,
            UpgradeOutcome::FailedOldHealthy { error } if error.code == codes::ARTIFACT_INVALID
        ));
        assert_automatic_rollback_completed(&engine, &root);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn qa005_port_occupancy_cancels_before_snapshot_or_switch() {
        let (base, root) = engine();
        let mut opts = base.opts.clone();
        opts.shutdown_timeout = Duration::from_millis(80);
        let engine = Engine::new(base.layout.clone(), opts);
        let (addr, server) = serve_for(400, Arc::new(|_| (503, r#"{"ready":false}"#.to_string())));

        assert!(engine.server_listening(addr.port()));
        let reason = engine
            .drain_old_server(addr.port(), true)
            .expect_err("被占用端口在超时后必须取消升级");
        assert!(reason.contains("仍未关闭"));
        server.join().unwrap();
        let _ = fs::remove_dir_all(root);
    }

    /// 缺陷 #1 回归：server /api/shutdown 同步 await 完整 drain（活动 run 宽限 +
    /// 拆 scrcpy 会话，真机实测 11.6s > 旧读超时 5s）。drain 的读超时必须覆盖
    /// 完整 drain 时长——慢响应（6s，超过旧 5s 读超时）必须等到 200 并完成
    /// 停机，而不是 5s 断开导致 drain 半途停滞、升级在 shutdown_timeout 后被取消。
    #[test]
    fn drain_survives_slow_graceful_shutdown_beyond_legacy_read_timeout() {
        let (base, root) = engine();
        let mut opts = base.opts.clone();
        // 总等待上界收口到 20s（回归态：5s 断开 + 20s 端口等待后取消）；
        // 修复态：~6.5s 收到 200 + 端口关闭即成功。
        opts.shutdown_timeout = Duration::from_secs(20);
        let engine = Engine::new(base.layout.clone(), opts);
        let (addr, abandoned, server) = serve_slow_shutdown(6_000);

        let began = Instant::now();
        engine
            .drain_old_server(addr.port(), true)
            .expect("慢响应 shutdown 必须等到完成，而非读超时断开");
        let waited = began.elapsed();
        assert!(
            waited >= Duration::from_millis(6_000),
            "drain 应等待完整 drain 时长，实际 {:?}",
            waited
        );
        assert!(
            !abandoned.load(Ordering::SeqCst),
            "等待期间 launcher 不得提前断开 /api/shutdown 连接"
        );
        assert!(
            !engine.server_listening(addr.port()),
            "shutdown 完成后端口应已关闭"
        );
        server.join().unwrap();
        let _ = fs::remove_dir_all(root);
    }

    /// 加速路径不退化：秒级完成的空载 drain（M2 E2E 形态）不受更长读超时影响，
    /// 响应即返回、端口关闭即成功，无新增等待。
    #[test]
    fn drain_fast_shutdown_path_stays_fast_under_longer_read_timeout() {
        let (base, root) = engine();
        let mut opts = base.opts.clone();
        opts.shutdown_timeout = Duration::from_secs(20);
        let engine = Engine::new(base.layout.clone(), opts);
        let (addr, server) = serve_for(1_500, Arc::new(|_| (200, r#"{"ok":true}"#.to_string())));

        let began = Instant::now();
        engine
            .drain_old_server(addr.port(), true)
            .expect("空载秒级 drain 必须照常成功");
        let waited = began.elapsed();
        assert!(
            waited < Duration::from_secs(5),
            "快路径不应被新读超时拖慢，实际 {:?}",
            waited
        );
        server.join().unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn qa005_identity_failures_auto_rollback_without_repeating_identity_coverage() {
        let cases = [
            (
                "wrong-version",
                r#"{"app":{"version":"9.9.9"},"schema":{"db":2},"startup":{"boot_id":"new"}}"#,
                codes::ARTIFACT_INVALID,
            ),
            (
                "wrong-schema",
                r#"{"app":{"version":"2.0.0"},"schema":{"db":1},"startup":{"boot_id":"new"}}"#,
                codes::SCHEMA_INCOMPATIBLE,
            ),
            (
                "reused-boot-id",
                r#"{"app":{"version":"2.0.0"},"schema":{"db":2},"startup":{"boot_id":"old"}}"#,
                codes::ARTIFACT_INVALID,
            ),
        ];

        for (tag, body, expected_code) in cases {
            let (engine, root) = engine();
            let body = body.to_string();
            let (addr, server) = serve_for(250, Arc::new(move |_| (200, body.clone())));
            let err = engine
                .verify_candidate_identity(addr, Some("old"), "2.0.0", Some(2))
                .expect_err("身份门禁失败应返回具体故障");
            assert_eq!(err.code, expected_code, "{tag}");

            prepare_switched_fixture(&engine, &root, &format!("qa005-{tag}"));
            let outcome = engine.fail_after_switch(err, false, addr.port());
            assert!(
                matches!(outcome, UpgradeOutcome::FailedOldHealthy { .. }),
                "{tag} 身份失败应自动回滚并恢复旧版"
            );
            assert_automatic_rollback_completed(&engine, &root);
            server.join().unwrap();
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn qa006_rollback_failure_enters_manual_and_preserves_recovery_evidence() {
        let (engine, root) = engine();
        prepare_switched_fixture(&engine, &root, "qa006-rollback-failure");

        let outcome = engine.fail_after_switch(
            BusinessError::new(codes::ARTIFACT_INVALID, "候选 ready 失败"),
            true,
            8443,
        );
        let error = match outcome {
            UpgradeOutcome::ManualRecovery { error } => error,
            other => panic!("回滚旧版本失败必须进入人工恢复，实际 {other:?}"),
        };
        assert_eq!(error.code, codes::MANUAL_RECOVERY_REQUIRED);
        assert!(error.message.contains("保留"));

        let store = StateStore::new(&root);
        let journal = store.load_journal().unwrap().journal;
        assert_eq!(journal.state, UpdateState::ManualRecoveryRequired);
        assert_eq!(
            journal.last_step.as_deref(),
            Some("manual_recovery_required")
        );
        assert_eq!(
            journal.error.as_ref().map(|error| error.code.as_str()),
            Some(codes::ROLLBACK_UNAVAILABLE)
        );
        assert!(snapshot::backup_dir(&engine.layout, "qa006-rollback-failure").is_dir());
        assert!(fs::read_dir(engine.layout.quarantine_dir())
            .unwrap()
            .flatten()
            .any(|entry| entry.path().join("data/state.bin").is_file()));
        assert!(matches!(
            store.load_current(),
            Ok(LoadOutcome::Present(CurrentState { current, .. })) if current == "1.0.0"
        ));
        let _ = fs::remove_dir_all(root);
    }

    struct FixedAvailableSpace(u64);

    impl AvailableSpaceProvider for FixedAvailableSpace {
        fn available_bytes(&self, _path: &Path) -> std::io::Result<u64> {
            Ok(self.0)
        }
    }

    #[test]
    fn qa007_insufficient_space_is_rejected_before_current_data_or_snapshot_changes() {
        let root = temp_root("qa007-insufficient-space");
        let layout = InstallLayout { root: root.clone() };
        let opts = UpgradeOptions {
            keys_dir: Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("release")
                .join("contracts")
                .join("fixtures")
                .join("keys"),
            ..UpgradeOptions::default()
        };
        let engine = Engine::with_available_space_provider(
            layout.clone(),
            opts,
            Arc::new(FixedAvailableSpace(0)),
        );
        let store = StateStore::new(&root);
        let current_before = CurrentState::new("0.1.0", Some("0.0.9".to_string()));
        store.write_current(&current_before).unwrap();
        fs::create_dir_all(root.join("data")).unwrap();
        fs::write(root.join("data/state.bin"), b"old-data").unwrap();
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(root.join("config/config.toml"), b"port = 8443\n").unwrap();
        let data_before = fs::read(root.join("data/state.bin")).unwrap();
        let config_before = fs::read(root.join("config/config.toml")).unwrap();
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("release")
            .join("contracts")
            .join("fixtures")
            .join("manifest")
            .join("valid")
            .join("manifest-valid-basic.json");

        let outcome = engine.run_full(&ManifestSource::Path(manifest));
        let error = match outcome {
            UpgradeOutcome::FailedOldHealthy { error } => error,
            other => panic!("空间不足必须在检查阶段拒绝，实际 {other:?}"),
        };
        assert_eq!(error.code, codes::INSUFFICIENT_SPACE);
        assert_eq!(
            fs::read(root.join("data/state.bin")).unwrap(),
            data_before,
            "空间预检失败不得修改 data"
        );
        assert_eq!(
            fs::read(root.join("config/config.toml")).unwrap(),
            config_before,
            "空间预检失败不得修改 config"
        );
        match store.load_current().unwrap() {
            LoadOutcome::Present(current) => {
                assert_eq!(current, current_before, "空间预检失败不得修改 current.json")
            }
            other => panic!("空间预检后 current.json 不可读: {other:?}"),
        }
        assert!(!layout.backups_dir().exists(), "空间预检失败不得创建快照");
        assert!(
            !layout.manifests_dir().join("0.2.0.json").exists(),
            "空间预检失败不得缓存候选 manifest"
        );

        let journal = store.load_journal().unwrap().journal;
        assert_eq!(journal.state, UpdateState::Idle);
        assert_eq!(journal.last_step.as_deref(), Some("failed"));
        assert_eq!(
            journal.error.as_ref().map(|error| error.code.as_str()),
            Some(codes::INSUFFICIENT_SPACE)
        );
        assert!(journal.snapshot.is_none(), "空间预检失败不得登记快照");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn qa007_preflight_counts_sparse_gib_db_without_materializing_payload() {
        // QA-007 本地替代：preflight 只需读取逻辑长度；不要把该文件送进
        // snapshot::create，因为 std::fs::copy 在本机可能物化稀疏空洞。
        let root = temp_root("qa007-preflight-sparse");
        let db = root.join("data/gamer.db");
        fs::create_dir_all(db.parent().unwrap()).unwrap();
        fs::File::create(&db).unwrap();
        let sparse = std::process::Command::new("fsutil")
            .args(["sparse", "setflag"])
            .arg(&db)
            .status()
            .expect("Windows fsutil 应可设置稀疏文件标志");
        assert!(sparse.success(), "DB 稀疏标志设置失败");

        const LOGICAL_DB_SIZE: u64 = 1u64 << 30;
        fs::OpenOptions::new()
            .write(true)
            .open(&db)
            .unwrap()
            .set_len(LOGICAL_DB_SIZE)
            .unwrap();
        const SMALL_FILE_COUNT: usize = 2048;
        for index in 0..SMALL_FILE_COUNT {
            let path = root
                .join("data/com.example.game/yaml")
                .join(format!("fixture-{index:04}.yaml"));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, format!("steps: []\n# {index}\n")).unwrap();
        }

        assert_eq!(fs::metadata(&db).unwrap().len(), LOGICAL_DB_SIZE);
        let small_files_bytes = (0..SMALL_FILE_COUNT)
            .map(|index| format!("steps: []\n# {index}\n").len() as u64)
            .sum::<u64>();
        assert_eq!(
            dir_size(&root.join("data")),
            LOGICAL_DB_SIZE + small_files_bytes
        );
        let _ = fs::remove_dir_all(root);
    }

    #[derive(Default)]
    struct RecordingPidOps {
        calls: Mutex<Vec<(u32, PathBuf)>>,
    }

    impl PidOps for RecordingPidOps {
        fn terminate_if_image(&self, pid: u32, expected_exe: &Path) -> bool {
            self.calls
                .lock()
                .unwrap()
                .push((pid, expected_exe.to_path_buf()));
            true
        }
    }

    #[test]
    fn rollback_uses_injected_pid_image_terminator() {
        let root = temp_root("pid-mock");
        let pid_ops = Arc::new(RecordingPidOps::default());
        let engine = Engine::with_pid_ops(
            InstallLayout { root: root.clone() },
            UpgradeOptions::default(),
            pid_ops.clone(),
        );
        let journal = UpdateJournal {
            state: UpdateState::Stopped,
            from_version: Some("1.0.0".to_string()),
            child: Some(ChildInfo {
                pid: 4242,
                created_at_unix_ms: Some(123),
                exe: "C:\\GameBot\\versions\\2.0.0\\gamer-server.exe".to_string(),
            }),
            ..UpdateJournal::default()
        };
        engine
            .rollback_procedure(&journal, None, false)
            .expect("mock PID 终止器不应阻塞回滚");
        let calls = pid_ops.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, 4242);
        assert!(calls[0].1.ends_with("gamer-server.exe"));
        let _ = fs::remove_dir_all(root);
    }
}
