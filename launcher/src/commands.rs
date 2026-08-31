//! 子命令实现：start / status / doctor / repair / upgrade。

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::cli::{Cli, Command};
use crate::installation;
use crate::inventory::{CheckOptions, ComponentSpec, ComponentStatus, FileCheck, ProbeCheck};
use crate::ipc::{self, Dispatcher, IpcServerConfig};
use crate::layout::InstallLayout;
use crate::manifest::model::Manifest;
use crate::manifest::{validate_manifest_file, ValidateOptions};
use crate::repair::{self, RepairGate, RepairOptions};
use crate::state::atomic::LoadOutcome;
use crate::state::lock::InstanceLock;
use crate::state::StateStore;
use crate::supervisor::{self, LaunchExtras, LaunchPlan, ReadyProbe};
use crate::upgrade::engine::{Engine, ManifestSource, UpgradeOptions, UpgradeOutcome};
use crate::upgrade::recovery::{self, RecoveryOutcome};
use crate::upgrade::trampoline;

/// doctor --manifest 的参数集合。
#[derive(Debug, Clone)]
pub struct DoctorInvocation {
    pub manifest: Option<PathBuf>,
    pub sig: Option<PathBuf>,
    pub key: Option<PathBuf>,
    pub expect_current_version: Option<String>,
    pub expect_channel: Option<String>,
}

/// CLI 提供的路径统一转成扩展长度（verbatim `\\?\`）形态：LongPathsEnabled=0
/// 的主机上 >260 字符路径只有该形态可用（与安装根的 verbatim 化同源）。
/// URL 形态的 manifest 来源保持原样。
pub fn normalize_cli_paths(cli: &mut Cli) {
    fn ext(p: &mut PathBuf) {
        *p = crate::winutil::extended_len_path(p);
    }
    let mut manifest_url: Option<String> = None;
    if let Command::Upgrade { manifest } = &mut cli.command {
        if !(manifest.starts_with("http://") || manifest.starts_with("https://")) {
            let p = PathBuf::from(manifest.as_str());
            manifest_url = Some(
                crate::winutil::extended_len_path(&p)
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }
    if let Some(p) = cli.install_root.as_mut() {
        ext(p);
    }
    if let Some(p) = cli.keys_dir.as_mut() {
        ext(p);
    }
    match &mut cli.command {
        Command::Doctor {
            manifest, sig, key, ..
        } => {
            if let Some(p) = manifest.as_mut() {
                ext(p);
            }
            if let Some(p) = sig.as_mut() {
                ext(p);
            }
            if let Some(p) = key.as_mut() {
                ext(p);
            }
        }
        Command::Repair { manifest, .. } => {
            if let Some(p) = manifest.as_mut() {
                ext(p);
            }
        }
        Command::Upgrade { manifest } => {
            if let Some(normalized) = manifest_url {
                *manifest = normalized;
            }
        }
        Command::Start | Command::Status => {}
    }
}

pub fn dispatch(cli: &Cli, layout: &InstallLayout) -> i32 {
    // trampoline helper 复用一个现有的无参数子命令形态以保持 CLI 兼容；必须在
    // 普通 dispatch 前拦截，避免 helper 获取安装锁或启动 server。
    if trampoline::is_requested() {
        return match trampoline::run_from_environment() {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("错误: launcher 自更新 trampoline 失败: {e}");
                1
            }
        };
    }
    match &cli.command {
        Command::Start => cmd_start(layout, cli),
        Command::Status => cmd_status(layout),
        Command::Doctor {
            manifest,
            sig,
            key,
            expect_current_version,
            expect_channel,
            deep,
            probe,
        } => {
            let invocation = DoctorInvocation {
                manifest: manifest.clone(),
                sig: sig.clone(),
                key: key.clone(),
                expect_current_version: expect_current_version.clone(),
                expect_channel: expect_channel.clone(),
            };
            match invocation.manifest.as_deref() {
                Some(path) if !deep && !probe => {
                    cmd_doctor_manifest(layout, cli, &invocation, path)
                }
                _ => {
                    cmd_doctor_inventory(layout, cli, *deep || *probe, *probe, manifest.as_deref())
                }
            }
        }
        Command::Repair { manifest, probe } => cmd_repair(layout, cli, manifest.as_deref(), *probe),
        Command::Upgrade { manifest } => cmd_upgrade(layout, cli, manifest),
    }
}

/// status：只读查询，不获取单实例锁；current.json 损坏时报错而非崩溃。
fn cmd_status(layout: &InstallLayout) -> i32 {
    println!("安装根: {}", layout.root.display());
    let store = StateStore::new(&layout.root);
    match store.load_current() {
        Ok(LoadOutcome::Present(current)) => {
            println!("当前版本: {}", current.current);
            println!(
                "上一版本: {}",
                current.previous.as_deref().unwrap_or("（无）")
            );
            if let Some(ts) = current.updated_at_unix_ms {
                println!("指针更新时间: {ts} (unix ms)");
            }
            println!("应用目录: versions/{}", current.current);
        }
        Ok(LoadOutcome::Missing) => {
            println!("当前状态: 未安装（state/current.json 不存在）");
        }
        Ok(LoadOutcome::Corrupted { backup_path }) => {
            tracing::error!(backup = %backup_path.display(), "current.json 损坏");
            eprintln!(
                "错误: state/current.json 损坏（半截/非法 JSON），已按空状态处理并备份到 {}；请运行 repair（后续批次）或重新安装。",
                backup_path.display()
            );
            return 1;
        }
        Err(e) => {
            tracing::error!(error = %e, "读取 current.json 失败");
            eprintln!("错误: 读取 state/current.json 失败: {e}");
            return 1;
        }
    }
    match store.load_journal() {
        Ok(jl) => {
            let mut line = format!("升级状态机: {}", jl.journal.state.as_str());
            if let Some(id) = &jl.journal.update_id {
                line.push_str(&format!("（update id: {id}）"));
            }
            if let Some(backup) = &jl.reset_from {
                line.push_str(&format!(
                    "（journal 损坏已重置，备份: {}）",
                    backup.display()
                ));
            }
            println!("{line}");
        }
        Err(e) => println!("升级状态机: 读取失败（{e}）"),
    }
    let held = InstanceLock::is_locked(&store.lock_path());
    println!(
        "实例锁: {}",
        if held {
            "被其他实例持有"
        } else {
            "空闲"
        }
    );
    0
}

/// doctor（库存检查）：目录级检查恒做；能解析到 release manifest（--manifest 或
/// manifests/ 缓存）时叠加组件级检查——quick = 存在 + size，deep = 逐文件 sha256，
/// probe = 附 adb/ffmpeg 版本探针。首装态（从未安装）输出 WARN 提示先运行 repair，
/// 不按失败计（已安装后组件缺失仍 FAIL）。
fn cmd_doctor_inventory(
    layout: &InstallLayout,
    cli: &Cli,
    deep: bool,
    probe: bool,
    explicit_manifest: Option<&Path>,
) -> i32 {
    let (lines, code) = doctor_inventory_report(layout, cli, deep, probe, explicit_manifest);
    for line in &lines {
        println!("{line}");
    }
    code
}

/// doctor 库存检查的报告形态（lines 逐行输出 + 退出码）；tests 直接断言内容。
pub fn doctor_inventory_report(
    layout: &InstallLayout,
    cli: &Cli,
    deep: bool,
    probe: bool,
    explicit_manifest: Option<&Path>,
) -> (Vec<String>, i32) {
    let mode = if deep {
        "深检（逐文件 sha256）"
    } else {
        "快速检查（存在 + size）"
    };
    let mut lines = Vec::new();
    let mut fail = 0usize;
    let mut warn = 0usize;
    lines.push(format!("doctor：安装库存{mode}（probe={probe}）"));
    lines.push(format!("安装根: {}", layout.root.display()));
    if !layout.root.is_dir() {
        lines.push("[FAIL] 安装根目录不存在".to_string());
        return (lines, 1);
    }
    lines.push("[PASS] 安装根目录存在".to_string());

    let store = StateStore::new(&layout.root);
    // 只读一次（Corrupted 分支会把坏文件备份改名，重复读会丢失该事实）
    let current = store.load_current();
    let never_installed = matches!(current, Ok(LoadOutcome::Missing));

    // state/ 目录：首装态缺失属正常（repair 生成），已安装态缺失才是故障
    if layout.state_dir().is_dir() {
        lines.push("[PASS] state/ 目录存在".to_string());
    } else if never_installed {
        lines.push(
            "[WARN] state/ 目录不存在（尚未安装：单实例锁与版本指针将在首次 repair 时创建）"
                .to_string(),
        );
        warn += 1;
    } else {
        lines.push("[FAIL] state/ 目录不存在（单实例锁与版本指针无落点）".to_string());
        fail += 1;
    }

    // 指针版本提前提取（current 在下方按值消费）
    let current_version = match &current {
        Ok(LoadOutcome::Present(c)) => Some(c.current.clone()),
        _ => None,
    };
    match current {
        Ok(LoadOutcome::Present(c)) => {
            lines.push(format!("[PASS] 当前版本指针: {}", c.current));
        }
        Ok(LoadOutcome::Missing) => {
            lines.push("[WARN] 尚未安装（state/current.json 不存在）".to_string());
            warn += 1;
        }
        Ok(LoadOutcome::Corrupted { backup_path }) => {
            lines.push(format!(
                "[FAIL] state/current.json 损坏（已备份: {}）",
                backup_path.display()
            ));
            fail += 1;
        }
        Err(e) => {
            lines.push(format!("[FAIL] state/current.json 读取失败: {e}"));
            fail += 1;
        }
    }

    let manifests = layout.manifests_dir();
    if manifests.is_dir() {
        let count = count_files_with_ext(&manifests, "json");
        lines.push(format!(
            "[PASS] manifests/ 目录存在（{count} 份缓存 manifest）"
        ));
    } else {
        lines.push("[WARN] manifests/ 目录不存在（尚未缓存任何 release manifest）".to_string());
        warn += 1;
    }

    let runtime = layout.runtime_dir();
    if runtime.is_dir() {
        let detail = subdir_names(&runtime).join(", ");
        lines.push(format!("[PASS] runtime/ 目录存在（依赖: {detail}）"));
    } else {
        lines.push("[WARN] runtime/ 目录不存在（managed 依赖尚未安装）".to_string());
        warn += 1;
    }

    let versions = layout.versions_dir();
    if versions.is_dir() {
        let detail = subdir_names(&versions).join(", ");
        lines.push(format!("[PASS] versions/ 目录存在（已装版本: {detail}）"));
    } else {
        lines.push("[WARN] versions/ 目录不存在（尚未安装任何应用版本）".to_string());
        warn += 1;
    }

    if never_installed {
        lines.push(
            "[WARN] 未安装——先运行 repair 完成首次安装（app + adb + ffmpeg 一步安装到位并写入版本指针）"
                .to_string(),
        );
        lines.push(format!("库存检查完成: {fail} 项失败 / {warn} 项警告"));
        return (lines, i32::from(fail > 0));
    }

    let bundle = match load_manifest_model(layout, cli, explicit_manifest) {
        Ok(bundle) => Some(bundle),
        Err(msg) => {
            if deep {
                lines.push(format!("错误: 深检需要 release manifest：{msg}"));
                return (lines, 1);
            }
            lines.push(format!(
                "[WARN] 未找到可用 release manifest，跳过组件级检查（{msg}）"
            ));
            warn += 1;
            None
        }
    };

    if let Some(bundle) = bundle {
        let Some(platform) = bundle.model.platforms.get("windows-x86_64") else {
            lines.push("[FAIL] manifest 缺少 windows-x86_64 平台".to_string());
            return (lines, 1);
        };
        lines.push(format!(
            "manifest: {}（release {}）",
            bundle.path.display(),
            bundle.model.release.version
        ));
        for comp in &platform.components {
            let spec = match ComponentSpec::from_model(comp) {
                Ok(s) => s,
                Err(msg) => {
                    lines.push(format!("[FAIL] {msg}"));
                    fail += 1;
                    continue;
                }
            };
            let finding =
                crate::inventory::check_installed(layout, &spec, CheckOptions { deep, probe });
            print_component_finding(&finding, &mut lines);
            if finding.status != ComponentStatus::Ok {
                fail += 1;
            }
        }
        // app 版本目录 quick 检查（entrypoint + scrcpy jar hash）；仅在已安装且
        // 指针版本与 manifest release 一致时做（跨版本的 jar hash 对比无意义）
        match crate::repair::AppInstallSpec::from_model(platform, &bundle.model.release.version) {
            Ok(app_spec) => match current_version {
                Some(v) if v == app_spec.version => {
                    match crate::repair::verify_app_dir(&app_spec.install_dir(layout), &app_spec) {
                        Ok(()) => lines.push(format!(
                            "[PASS] app {}: 版本目录完好（entrypoint + scrcpy-server hash）",
                            app_spec.version
                        )),
                        Err(reason) => {
                            lines.push(format!("[FAIL] app {}: {reason}", app_spec.version));
                            fail += 1;
                        }
                    }
                }
                Some(v) => {
                    lines.push(format!(
                        "[WARN] 当前版本 {v} 与 manifest release {} 不一致，跳过 app 版本目录检查",
                        app_spec.version
                    ));
                    warn += 1;
                }
                None => {}
            },
            Err(msg) => {
                lines.push(format!("[FAIL] {msg}"));
                fail += 1;
            }
        }
    }

    lines.push(format!("库存检查完成: {fail} 项失败 / {warn} 项警告"));
    (lines, i32::from(fail > 0))
}

fn print_component_finding(finding: &crate::inventory::ComponentFinding, out: &mut Vec<String>) {
    out.push(format!("组件目录 {}", finding.dir.display()));
    for f in &finding.files {
        match &f.check {
            FileCheck::Ok => out.push(format!("  [PASS] {}", f.path)),
            FileCheck::Missing => out.push(format!("  [FAIL] {}: 文件缺失", f.path)),
            FileCheck::SizeMismatch { actual, expected } => out.push(format!(
                "  [FAIL] {}: size 不符（实际 {actual}，声明 {expected}）",
                f.path
            )),
            FileCheck::HashMismatch { actual, expected } => out.push(format!(
                "  [FAIL] {}: sha256 不符（实际 {actual}，声明 {expected}）",
                f.path
            )),
            FileCheck::Io(e) => out.push(format!("  [FAIL] {}: 读取失败（{e}）", f.path)),
        }
    }
    match &finding.probe {
        Some(ProbeCheck::Match { reported }) => {
            out.push(format!("  [PASS] 探针: 版本匹配（{reported}）"))
        }
        Some(ProbeCheck::Mismatch { reported }) => {
            out.push(format!("  [FAIL] 探针: 版本不符（运行输出 {reported}）"))
        }
        Some(ProbeCheck::Failed { reason }) => out.push(format!("  [FAIL] 探针: {reason}")),
        Some(ProbeCheck::Unsupported) | None => {}
    }
    if finding.status == ComponentStatus::Ok {
        out.push("  => 组件完好".to_string());
    } else {
        out.push("  => 组件缺失/损坏（可运行 repair 修复）".to_string());
    }
}

/// doctor --manifest：对任意 manifest 文件跑完整校验（先验签、后解析，fail closed）。
fn cmd_doctor_manifest(
    layout: &InstallLayout,
    cli: &Cli,
    invocation: &DoctorInvocation,
    manifest: &Path,
) -> i32 {
    let keys_dir = match resolve_keys_dir(cli.keys_dir.as_ref(), layout) {
        Ok(dir) => Some(dir),
        Err(msg) => {
            eprintln!("错误: {msg}");
            return 2;
        }
    };
    let opts = ValidateOptions {
        sig_path: invocation.sig.clone(),
        keys_dir,
        key_path: invocation.key.clone(),
        expect_current_version: invocation.expect_current_version.clone(),
        expect_channel: invocation.expect_channel.clone(),
        launcher_version: Some(env!("CARGO_PKG_VERSION").to_string()),
    };
    let outcome = validate_manifest_file(manifest, &opts);
    println!("manifest: {}", manifest.display());
    if outcome.ok {
        println!(
            "signature: verified (key_id={})",
            outcome.info.key_id.as_deref().unwrap_or("?")
        );
        println!(
            "release: {} ({}); platforms: {}",
            outcome.info.version.as_deref().unwrap_or("?"),
            outcome.info.channel.as_deref().unwrap_or("?"),
            outcome.info.platforms.join(", ")
        );
        println!("OK — release manifest v1 valid（校验通过）");
        0
    } else {
        println!("FAIL — {} error(s)", outcome.errors.len());
        for e in &outcome.errors {
            println!("  [{}] {}", e.code, e.detail);
        }
        1
    }
}

/// repair（LCH-007）：深检 → seed/cache/remote 修复 → 复验探针；
/// 同时安装/修复 app 版本目录（manifest `app.artifact` + scrcpy jar）并写
/// `state/current.json` 版本指针——首装一步到位，`start` 随即可用。
fn cmd_repair(layout: &InstallLayout, cli: &Cli, manifest: Option<&Path>, probe: bool) -> i32 {
    println!("repair：依赖修复 + 应用安装（probe={probe}）");
    println!("安装根: {}", layout.root.display());
    let bundle = match load_manifest_model(layout, cli, manifest) {
        Ok(b) => b,
        Err(msg) => {
            eprintln!("错误: {msg}");
            return 2;
        }
    };
    println!(
        "manifest: {}（release {}）",
        bundle.path.display(),
        bundle.model.release.version
    );
    let Some(platform) = bundle.model.platforms.get("windows-x86_64") else {
        eprintln!("错误: manifest 缺少 windows-x86_64 平台");
        return 2;
    };
    let mut specs = Vec::new();
    let mut spec_errors = 0usize;
    for comp in &platform.components {
        match ComponentSpec::from_model(comp) {
            Ok(spec) => specs.push(spec),
            Err(msg) => {
                println!("[FAIL] {msg}");
                spec_errors += 1;
            }
        }
    }
    let app_spec = match repair::AppInstallSpec::from_model(platform, &bundle.model.release.version)
    {
        Ok(a) => Some(a),
        Err(msg) => {
            println!("[FAIL] {msg}");
            spec_errors += 1;
            None
        }
    };
    if spec_errors > 0 {
        return 1;
    }

    let opts = RepairOptions {
        fetch: Default::default(),
        probe,
    };
    match repair::repair_with_lock(layout, &specs, app_spec.as_ref(), &opts) {
        Err(RepairGate::Locked { path }) => {
            println!(
                "已有 launcher 实例持有安装根（{}），本次未执行任何动作。",
                path.display()
            );
            1
        }
        Err(RepairGate::Io(e)) => {
            eprintln!("错误: 单实例锁操作失败: {e}");
            1
        }
        Ok(report) => {
            for cr in &report.components {
                match &cr.outcome {
                    repair::ComponentOutcome::Healthy => {
                        println!("[PASS] {} {}: 组件完好，无需修复", cr.id, cr.version);
                    }
                    repair::ComponentOutcome::Repaired { source } => {
                        println!("[PASS] {} {}: 已修复（来源 {source}）", cr.id, cr.version);
                    }
                    repair::ComponentOutcome::Failed { reason } => {
                        println!("[FAIL] {} {}: {reason}", cr.id, cr.version);
                    }
                }
            }
            if let Some(app) = &report.app {
                match &app.outcome {
                    repair::AppOutcome::Healthy => {
                        println!(
                            "[PASS] app {}: 版本目录已安装且校验通过（不覆盖既有版本目录）",
                            app.version
                        );
                    }
                    repair::AppOutcome::Installed { source } => {
                        println!(
                            "[PASS] app {}: 应用安装完成（来源 {source}），版本指针已写入 state/current.json",
                            app.version
                        );
                    }
                    repair::AppOutcome::Failed { reason } => {
                        println!("[FAIL] app {}: {reason}", app.version);
                    }
                }
            }
            let failed = report.failed_count();
            let repaired = report.repaired_count();
            if failed == 0 {
                println!("修复完成：{repaired} 项恢复 / 全部组件可用。");
                0
            } else {
                println!(
                    "修复完成：{repaired} 项恢复 / {failed} 项失败（失败项的既有安装未被破坏）。"
                );
                1
            }
        }
    }
}

/// start（LCH-008 + OPS-003 + 批次 3）：env 注入（含 IPC 寻址）+ 句柄等待 +
/// 就绪探测 + 启动 journal 恢复 + named pipe IPC server。
fn cmd_start(layout: &InstallLayout, cli: &Cli) -> i32 {
    println!("start：启动并监管 gamer-server");
    println!("安装根: {}", layout.root.display());
    let lock = match InstanceLock::acquire(&layout.state_dir()) {
        Ok(l) => l,
        Err(crate::state::lock::LockError::Held { path }) => {
            eprintln!("错误: 已有 launcher 实例持有安装根（{}）", path.display());
            return 1;
        }
        Err(crate::state::lock::LockError::Io(e)) => {
            eprintln!("错误: 单实例锁操作失败: {e}");
            return 1;
        }
    };
    let _ = lock;

    let store = StateStore::new(&layout.root);
    // 批次 3：启动恢复（未完成 journal 按失败分支处理；manual recovery 拒绝启动）
    match recovery::recover_on_startup(layout, &store) {
        Ok(report) => match &report.outcome {
            RecoveryOutcome::NothingToDo => {}
            RecoveryOutcome::ManualRequired { reason } => {
                tracing::error!(%reason, "manual_recovery_required：拒绝启动（人工恢复前不再自动拉起）");
                eprintln!(
                    "错误: 上次升级与自动回滚均失败（manual_recovery_required）：{reason}\n\
                     证据保留在 backups/、quarantine/ 与 state/update-journal.json；人工恢复并复位 journal 后方可启动。"
                );
                return 2;
            }
            other => {
                println!("启动恢复: {other:?}");
            }
        },
        Err(e) => {
            eprintln!("错误: journal 恢复失败: {e}");
            return 1;
        }
    }
    let current = match store.load_current() {
        Ok(LoadOutcome::Present(c)) => c,
        Ok(LoadOutcome::Missing) => {
            eprintln!("错误: 尚未安装（state/current.json 不存在），无版本可启动。");
            return 1;
        }
        Ok(LoadOutcome::Corrupted { backup_path }) => {
            eprintln!(
                "错误: state/current.json 损坏（已备份到 {}），拒绝启动以避免版本指针不可信。",
                backup_path.display()
            );
            return 1;
        }
        Err(e) => {
            eprintln!("错误: 读取 state/current.json 失败: {e}");
            return 1;
        }
    };
    let version = current.current.clone();
    let app_dir = layout.versions_dir().join(&version);
    let exe = match supervisor::resolve_entrypoint(layout, &version) {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("错误: {msg}");
            return 1;
        }
    };

    // 批次 3：installation-id + 本次会话令牌（IPC 寻址注入）
    let installation_id = installation::load_or_create(&store).unwrap_or_else(|e| {
        tracing::warn!("installation-id 生成失败（{e}），IPC 不启用");
        String::new()
    });
    let ipc_enabled = !installation_id.is_empty();
    let ipc_token = if ipc_enabled {
        installation::new_session_token().unwrap_or_default()
    } else {
        String::new()
    };
    // 回环管理通道令牌：注入子进程（GAMER_ADMIN_TOKEN），使升级 drain 的
    // X-Admin-Token 快捷通道可用；生成失败仅降级为匿名 drain（与旧行为一致）。
    let admin_token = installation::load_or_create_admin_token(&store)
        .map_err(|e| tracing::warn!("admin-token 生成失败（{e}），drain 将为匿名请求"))
        .ok();
    let extras = if ipc_enabled && !ipc_token.is_empty() {
        LaunchExtras::managed(
            installation::pipe_name_for(&installation_id),
            ipc_token.clone(),
        )
        .with_admin_token(admin_token)
    } else {
        LaunchExtras::default().with_admin_token(admin_token)
    };

    let adb = supervisor::latest_component_exe(layout, "adb", "adb.exe");
    let ffmpeg = supervisor::latest_component_exe(layout, "ffmpeg", "ffmpeg.exe");
    if adb.is_none() {
        tracing::warn!(
            "runtime/adb 未安装，GAMER_ADB_PATH 不注入（server readiness 将报 adb not_ready）"
        );
    }
    if ffmpeg.is_none() {
        tracing::warn!("runtime/ffmpeg 未安装，GAMER_FFMPEG_PATH 不注入（server readiness 将报 ffmpeg not_ready）");
    }
    let _ = fs::create_dir_all(layout.logs_dir());
    let _ = fs::create_dir_all(layout.data_dir());

    let plan = LaunchPlan {
        exe: exe.clone(),
        cwd: app_dir.clone(),
        app_dir: app_dir.clone(),
        data_dir: layout.data_dir(),
        adb_path: adb,
        ffmpeg_path: ffmpeg,
        scrcpy_server: app_dir.join("assets").join("scrcpy-server.jar"),
        config_path: layout.config_file(),
        log_path: layout.logs_dir().join("gamer-server.log"),
    };
    let port = supervisor::read_configured_port(&plan.config_path);
    println!("当前版本: {version}");
    println!("入口程序: {}", plan.exe.display());
    println!(
        "就绪探测: http://127.0.0.1:{port}{}",
        supervisor::HEALTH_PATH
    );
    if ipc_enabled {
        println!("IPC: {}", installation::pipe_name_for(&installation_id));
    }

    let mut child = match supervisor::spawn_supervised_with_extras(&plan, &extras) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("错误: 启动 server 子进程失败: {e}");
            return 1;
        }
    };
    tracing::info!(pid = child.id(), exe = %plan.exe.display(), "server 子进程已启动");
    println!("server 子进程已启动 (pid={})，等待就绪…", child.id());

    // 批次 3：named pipe IPC server（后台线程，进程退出即结束）
    if ipc_enabled {
        let keys_dir = resolve_keys_dir(cli.keys_dir.as_ref(), layout).ok();
        spawn_ipc_server(
            layout.clone(),
            installation_id.clone(),
            ipc_token,
            keys_dir.unwrap_or_else(|| layout.root.join("keys")),
        );
    }

    match supervisor::wait_for_ready(port, &ReadyProbe::default()) {
        Ok(()) => println!(
            "[PASS] server 已就绪 (http://127.0.0.1:{port}{})",
            supervisor::HEALTH_PATH
        ),
        Err(reason) => {
            tracing::error!(%reason, "就绪探测未通过（继续持有子进程监管）");
            println!("[WARN] 就绪探测未通过: {reason}（继续持有子进程监管，不按端口判定进程死活）");
        }
    }

    // OPS-003：持有子进程句柄等待退出（不按端口/进程名判定）
    match child.wait() {
        Ok(status) => {
            let code = status.code();
            tracing::info!(?code, "server 子进程退出");
            println!(
                "server 子进程退出（退出码: {}）",
                code.map(|c| c.to_string())
                    .unwrap_or_else(|| "非正常终止".to_string())
            );
            code.unwrap_or(1)
        }
        Err(e) => {
            tracing::error!(error = %e, "等待子进程退出失败");
            eprintln!("错误: 等待子进程退出失败: {e}");
            1
        }
    }
}

/// 拉起 IPC named pipe 服务端（独立线程 + 独立 tokio runtime）。
fn spawn_ipc_server(
    layout: InstallLayout,
    installation_id: String,
    token: String,
    keys_dir: PathBuf,
) {
    let check_source = check_source_from_env();
    let store = StateStore::new(&layout.root);
    let admin_token = installation::load_or_create_admin_token(&store)
        .map_err(|e| tracing::warn!("admin-token 生成失败（{e}），IPC 回滚 drain 将为匿名请求"))
        .ok();
    let dispatcher = Dispatcher::new(
        layout,
        installation_id.clone(),
        check_source,
        keys_dir.clone(),
        UpgradeOptions {
            keys_dir,
            admin_token,
            ..UpgradeOptions::default()
        },
        false,
    );
    let cfg = IpcServerConfig {
        pipe_name: installation::pipe_name_for(&installation_id),
        token,
        ..IpcServerConfig::default()
    };
    let spawned = std::thread::Builder::new()
        .name("launcher-ipc".to_string())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    tracing::error!("IPC runtime 启动失败: {e}");
                    return;
                }
            };
            rt.block_on(async move {
                if let Err(e) = ipc::run_server(dispatcher, cfg).await {
                    tracing::error!(error = %e, "IPC server 退出");
                }
            });
        });
    match spawned {
        Ok(_) => tracing::info!("IPC server 线程已启动"),
        Err(e) => tracing::error!("IPC server 线程启动失败: {e}"),
    }
}

/// check 的候选来源（通道配置；IPC 请求不接受来源指定）。
/// `GAMER_LAUNCHER_RELEASE_MANIFEST`：URL 或本地路径；未设置 = 无远端源
/// （check 按 update_not_available 拒绝）。
fn check_source_from_env() -> ManifestSource {
    let raw = std::env::var("GAMER_LAUNCHER_RELEASE_MANIFEST")
        .unwrap_or_default()
        .trim()
        .to_string();
    if raw.is_empty() {
        return ManifestSource::None;
    }
    if raw.starts_with("http://") || raw.starts_with("https://") {
        ManifestSource::Url(raw)
    } else {
        ManifestSource::Path(PathBuf::from(raw))
    }
}

/// upgrade（LCH-010/011/012）：§6.6 全链路编排 + 启动恢复 + 自动回滚。
/// 退出码：0=committed；1=失败但旧版健康/取消；2=manual_recovery_required。
fn cmd_upgrade(layout: &InstallLayout, cli: &Cli, manifest: &str) -> i32 {
    println!("upgrade：检查并执行升级");
    println!("安装根: {}", layout.root.display());
    let lock = match InstanceLock::acquire(&layout.state_dir()) {
        Ok(l) => l,
        Err(crate::state::lock::LockError::Held { path }) => {
            eprintln!("错误: 已有 launcher 实例持有安装根（{}）", path.display());
            return 1;
        }
        Err(crate::state::lock::LockError::Io(e)) => {
            eprintln!("错误: 单实例锁操作失败: {e}");
            return 1;
        }
    };
    let _ = lock;
    let store = StateStore::new(&layout.root);
    match recovery::recover_on_startup(layout, &store) {
        Ok(report) if report.is_manual() => {
            eprintln!("错误: 上次升级与自动回滚均失败（manual_recovery_required），已停止自动重试；请人工恢复后重试。");
            return 2;
        }
        Ok(report) => {
            if !matches!(report.outcome, RecoveryOutcome::NothingToDo) {
                println!("启动恢复: {:?}", report.outcome);
            }
        }
        Err(e) => {
            eprintln!("错误: journal 恢复失败: {e}");
            return 1;
        }
    }
    let keys_dir = match resolve_keys_dir(cli.keys_dir.as_ref(), layout) {
        Ok(k) => k,
        Err(msg) => {
            eprintln!("错误: {msg}");
            return 2;
        }
    };
    let source = if manifest.starts_with("http://") || manifest.starts_with("https://") {
        ManifestSource::Url(manifest.to_string())
    } else {
        ManifestSource::Path(PathBuf::from(manifest))
    };
    let installation_id = installation::load_or_create(&store).unwrap_or_default();
    let ipc_token = installation::new_session_token().unwrap_or_default();
    // 回环管理通道令牌：与 start 注入子进程的值同源（state/admin-token），
    // drain 旧版本时以 X-Admin-Token 通过 /api/shutdown 鉴权。
    let admin_token = installation::load_or_create_admin_token(&store)
        .map_err(|e| tracing::warn!("admin-token 生成失败（{e}），drain 将为匿名请求"))
        .ok();
    let opts = UpgradeOptions {
        keys_dir,
        ipc: Some((installation::pipe_name_for(&installation_id), ipc_token)),
        admin_token,
        ..UpgradeOptions::default()
    };
    let engine = Engine::new(layout.clone(), opts);
    match engine.run_full(&source) {
        UpgradeOutcome::Committed { from, to } => {
            println!("升级完成: {from} → {to}（committed，旧版本保留可人工回退）");
            0
        }
        UpgradeOutcome::Cancelled { reason } => {
            println!("升级已取消（旧版本未动）: {reason}");
            1
        }
        UpgradeOutcome::FailedOldHealthy { error } => {
            eprintln!("升级失败（旧版本已恢复健康）: {error}");
            1
        }
        UpgradeOutcome::ManualRecovery { error } => {
            eprintln!("错误: 升级与自动回滚均失败（manual_recovery_required）: {error}");
            2
        }
    }
}

// -- manifest 装载（doctor 深检 / repair 共用） -------------------------------

struct ManifestBundle {
    path: PathBuf,
    model: Manifest,
}

/// 装载并完整校验（验签）release manifest：显式 --manifest 优先；否则扫描
/// manifests/ 缓存（匹配当前版本的优先，其余按 SemVer 降序），取第一份通过者。
fn load_manifest_model(
    layout: &InstallLayout,
    cli: &Cli,
    explicit: Option<&Path>,
) -> Result<ManifestBundle, String> {
    let keys_dir = Some(resolve_keys_dir(cli.keys_dir.as_ref(), layout)?);
    let opts = ValidateOptions {
        keys_dir,
        launcher_version: Some(env!("CARGO_PKG_VERSION").to_string()),
        ..ValidateOptions::default()
    };
    let candidates: Vec<PathBuf> = match explicit {
        Some(p) => vec![p.to_path_buf()],
        None => cached_manifest_candidates(layout),
    };
    if candidates.is_empty() {
        return Err(
            "未找到可用 release manifest：用 --manifest 指定，或先把已验签 manifest 放入 manifests/。"
                .to_string(),
        );
    }
    let mut failures = Vec::new();
    for path in candidates {
        let outcome = validate_manifest_file(&path, &opts);
        if outcome.ok {
            let raw = fs::read(&path)
                .map_err(|e| format!("读取 manifest 失败 {}: {e}", path.display()))?;
            let value: Value = serde_json::from_slice(&raw)
                .map_err(|e| format!("manifest 不是合法 JSON（{}）: {e}", path.display()))?;
            let model = Manifest::parse(&value)
                .map_err(|e| format!("manifest 模型解析失败（{}）: {e}", path.display()))?;
            return Ok(ManifestBundle { path, model });
        }
        let codes: Vec<&str> = outcome.errors.iter().map(|e| e.code.as_str()).collect();
        failures.push(format!("{}: {}", path.display(), codes.join(",")));
    }
    Err(format!(
        "候选 manifest 全部校验失败（信任库/签名不匹配或内容非法）:\n  {}",
        failures.join("\n  ")
    ))
}

/// manifests/ 缓存候选：匹配 state/current.json 当前版本的排前，各组内 SemVer 降序。
fn cached_manifest_candidates(layout: &InstallLayout) -> Vec<PathBuf> {
    let dir = layout.manifests_dir();
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    files.sort();
    let current_version = StateStore::new(&layout.root)
        .load_current()
        .ok()
        .and_then(|o| match o {
            LoadOutcome::Present(c) => Some(c.current),
            _ => None,
        });
    let score = |p: &Path| -> (u8, Option<crate::manifest::semver::Semver>) {
        let version = fs::read(p)
            .ok()
            .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
            .and_then(|v| {
                v.get("release")
                    .and_then(|r| r.get("version"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            });
        let matches_current = version
            .as_deref()
            .zip(current_version.as_deref())
            .is_some_and(|(v, c)| v == c);
        (
            u8::from(!matches_current),
            version.as_deref().and_then(crate::manifest::semver::parse),
        )
    };
    let mut scored: Vec<_> = files.into_iter().map(|p| (score(&p), p)).collect();
    scored.sort_by(|a, b| {
        a.0 .0.cmp(&b.0 .0).then_with(|| match (&a.0 .1, &b.0 .1) {
            (Some(sa), Some(sb)) => {
                if crate::manifest::semver::is_lt(sa, sb) {
                    std::cmp::Ordering::Greater
                } else if crate::manifest::semver::is_lt(sb, sa) {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Equal
                }
            }
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        })
    });
    scored.into_iter().map(|(_, p)| p).collect()
}

/// 公钥目录解析顺序：--keys-dir > GAMER_LAUNCHER_KEYS_DIR > <安装根>/keys > <exe 目录>/keys。
fn resolve_keys_dir(explicit: Option<&PathBuf>, layout: &InstallLayout) -> Result<PathBuf, String> {
    if let Some(dir) = explicit {
        return Ok(dir.clone());
    }
    if let Ok(env_dir) = std::env::var("GAMER_LAUNCHER_KEYS_DIR") {
        if !env_dir.trim().is_empty() {
            return Ok(PathBuf::from(env_dir));
        }
    }
    let mut candidates: Vec<PathBuf> = vec![layout.root.join("keys")];
    if let Some(exe_keys) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("keys")))
    {
        candidates.push(exe_keys);
    }
    for candidate in candidates {
        if candidate.is_dir() {
            return Ok(candidate);
        }
    }
    Err(
        "未找到可信公钥目录：请用 --keys-dir 指定（例如 release/contracts/fixtures/keys），\
         或设置 GAMER_LAUNCHER_KEYS_DIR，或在安装根下放置 keys/*.pem"
            .to_string(),
    )
}

fn count_files_with_ext(dir: &Path, ext: &str) -> usize {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|e| {
                    e.path()
                        .extension()
                        .and_then(|x| x.to_str())
                        .is_some_and(|x| x.eq_ignore_ascii_case(ext))
                })
                .count()
        })
        .unwrap_or(0)
}

fn subdir_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|e| e.path().is_dir())
                .filter_map(|e| e.file_name().to_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}
