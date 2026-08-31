//! IPC 操作分派：6 操作 → 内部枚举 → Engine 阶段方法 / repair；幂等去重、
//! 冲突（update_busy）与 status 只读快照（ipc-v1 §4/§5）。
//!
//! 长操作「受理即回」：admission 通过即回受理帧，动作在独立线程跑完
//! （`run_inline=true` 时在调用线程跑，供测试/CLI）；server 以 status 轮询。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::{json, Value};

use crate::layout::InstallLayout;
use crate::state::atomic::LoadOutcome;
use crate::state::{StateStore, UpdateState};
use crate::supervisor::latest_component_exe;
use crate::upgrade::engine::{Engine, ManifestSource, UpgradeOptions};
use crate::upgrade::{codes, display_detail, display_state, BusinessError};

use super::frames::{error_frame, parse_request, success_frame, Dependency, Operation};
use super::DEDUP_WINDOW;
use super::PROTOCOL_VERSION;

struct ActiveOp {
    op: Operation,
    accepted_result: Value,
}

struct CachedReply {
    frame: Value,
    expires: Instant,
}

#[derive(Default)]
struct Inner {
    active: Option<ActiveOp>,
    cache: HashMap<String, CachedReply>,
}

/// IPC 分派器（server 持一个；handle 由 tokio 客户端任务调用）。
pub struct Dispatcher {
    pub layout: InstallLayout,
    pub installation_id: String,
    pub launcher_version: String,
    pub engine: Arc<Engine>,
    /// check 操作的候选来源（通道配置；IPC 请求不接受来源指定）。
    pub check_source: ManifestSource,
    pub keys_dir: PathBuf,
    /// 测试/CLI 内联执行长操作（不另起线程）。
    pub run_inline: bool,
    inner: Mutex<Inner>,
}

/// 一帧的处理结果。
#[derive(Debug, Clone)]
pub struct Reply {
    pub frame: Value,
    pub disconnect: bool,
}

impl Dispatcher {
    pub fn new(
        layout: InstallLayout,
        installation_id: String,
        check_source: ManifestSource,
        keys_dir: PathBuf,
        engine_opts: UpgradeOptions,
        run_inline: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            engine: Arc::new(Engine::new(layout.clone(), engine_opts)),
            layout,
            installation_id,
            launcher_version: env!("CARGO_PKG_VERSION").to_string(),
            check_source,
            keys_dir,
            run_inline,
            inner: Mutex::new(Inner::default()),
        })
    }

    fn store(&self) -> StateStore {
        StateStore::new(&self.layout.root)
    }

    /// 处理一帧请求载荷，返回响应帧 + 是否断开。`token` 为本次会话令牌
    /// （逐帧校验，ipc-v1 §3.1）。
    pub fn handle(self: &Arc<Self>, raw: &[u8], token: &str) -> Reply {
        self.handle_with_seen(raw, token, &mut HashSet::new())
    }

    /// Enforce request_id uniqueness on one connection. A reconnect may reuse
    /// an id, which is handled by the dispatcher-wide idempotency cache.
    pub fn handle_with_seen(
        self: &Arc<Self>,
        raw: &[u8],
        token: &str,
        seen: &mut HashSet<String>,
    ) -> Reply {
        match parse_request(raw, token) {
            Ok(request) => {
                if let Err(err) = super::frames::check_request_id_unique(seen, &request.request_id)
                {
                    return Reply {
                        frame: error_frame(&request.request_id, err.code(), err.message()),
                        disconnect: false,
                    };
                }
                if request.operation == Operation::Status {
                    return Reply {
                        frame: success_frame(&request.request_id, self.status_result()),
                        disconnect: false,
                    };
                }
                self.submit(&request.request_id, request.operation)
            }
            Err((protocol_err, request_id)) => Reply {
                frame: error_frame(&request_id, protocol_err.code(), protocol_err.message()),
                disconnect: protocol_err.must_disconnect(),
            },
        }
    }

    /// 长操作提交：幂等（同 request_id → 原帧）/ 同类复用 / 冲突 update_busy /
    /// admission 矩阵（system-api §4.2 的 launcher 侧镜像）。
    fn submit(self: &Arc<Self>, request_id: &str, op: Operation) -> Reply {
        let mut inner = match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let now = Instant::now();
        inner.cache.retain(|_, c| c.expires > now);
        if let Some(cached) = inner.cache.get(request_id) {
            return Reply {
                frame: cached.frame.clone(),
                disconnect: false,
            };
        }
        // 同类并发：指向同一事务 → 复用原受理帧，不开第二次（ipc-v1 §5.3）
        if let Some(active) = &inner.active {
            if active.op == op {
                return Reply {
                    frame: success_frame(request_id, active.accepted_result.clone()),
                    disconnect: false,
                };
            }
            // 冲突操作：只有一个升级事务
            let reply = error_frame(
                request_id,
                codes::UPDATE_BUSY,
                "已有升级/修复事务进行中，请以 status 轮询后再试",
            );
            inner.cache.insert(
                request_id.to_string(),
                CachedReply {
                    frame: reply.clone(),
                    expires: Instant::now() + DEDUP_WINDOW,
                },
            );
            return Reply {
                frame: reply,
                disconnect: false,
            };
        }
        // Keep admission serialized with the active slot. Do not execute the
        // operation while holding this mutex: inline mode is used by tests and
        // the operation itself clears `active` on completion.
        let (reply_frame, should_run) = match self.admit(op) {
            Ok(Admission::Accept { state, update_id }) => {
                let accepted = json!({
                    "accepted": true,
                    "operation": op.as_str(),
                    "update_id": update_id,
                    "state": state,
                });
                let accepted_frame = success_frame(request_id, accepted);
                inner.active = Some(ActiveOp {
                    op,
                    accepted_result: accepted_frame.get("result").cloned().unwrap_or(Value::Null),
                });
                (accepted_frame, true)
            }
            Ok(Admission::NoOp { state, update_id }) => {
                // 如 staged + download = 无操作受理（§4.2）
                (
                    success_frame(
                        request_id,
                        json!({
                            "accepted": true,
                            "operation": op.as_str(),
                            "update_id": update_id,
                            "state": state,
                        }),
                    ),
                    false,
                )
            }
            Err(err) => (error_frame(request_id, &err.code, &err.message), false),
        };
        inner.cache.insert(
            request_id.to_string(),
            CachedReply {
                frame: reply_frame.clone(),
                expires: Instant::now() + DEDUP_WINDOW,
            },
        );
        drop(inner);
        if should_run {
            let dispatcher = Arc::clone(self);
            if self.run_inline {
                dispatcher.run_long_op(op);
            } else if let Err(e) = std::thread::Builder::new()
                .name(format!("ipc-op-{}", op.as_str()))
                .spawn(move || dispatcher.run_long_op(op))
            {
                tracing::error!("长操作线程启动失败: {e}");
                // A failed worker must not leave the dispatcher permanently
                // busy. Keep the original acceptance frame for idempotency,
                // but expose the failure through the journal/status path.
                let mut guard = match self.inner.lock() {
                    Ok(g) => g,
                    Err(poisoned) => poisoned.into_inner(),
                };
                if guard.active.as_ref().is_some_and(|active| active.op == op) {
                    guard.active = None;
                }
            }
        }
        Reply {
            frame: reply_frame,
            disconnect: false,
        }
    }

    /// admission（§4.2 状态 × 动作矩阵的 launcher 镜像；仅受理判断，不动状态机）。
    fn admit(&self, op: Operation) -> Result<Admission, BusinessError> {
        let journal = self
            .store()
            .load_journal()
            .map_err(|e| {
                BusinessError::new(
                    codes::LAUNCHER_UNREACHABLE,
                    format!("读取 journal 失败: {e}"),
                )
            })?
            .journal;
        if journal.state == UpdateState::ManualRecoveryRequired {
            return Err(BusinessError::new(
                codes::MANUAL_RECOVERY_REQUIRED,
                "升级与自动回滚均失败，已停止自动重试；请人工恢复",
            ));
        }
        let display = display_state(&journal);
        let update_id = journal.update_id.clone();
        match op {
            Operation::Status => unreachable!("status 在 handle 层短路"),
            Operation::Check => match display {
                "idle" | "failed" | "available" | "staged" | "waiting" => Ok(Admission::Accept {
                    state: "checking",
                    update_id,
                }),
                _ => Err(BusinessError::new(
                    codes::UPDATE_BUSY,
                    format!("状态 {display} 中不接受 check"),
                )),
            },
            Operation::Download => match display {
                "available" => Ok(Admission::Accept {
                    state: "downloading",
                    update_id,
                }),
                "staged" => Ok(Admission::NoOp {
                    state: "staged",
                    update_id,
                }),
                _ => Err(BusinessError::new(
                    codes::UPDATE_NOT_AVAILABLE,
                    format!("状态 {display} 无可下载候选"),
                )),
            },
            Operation::PrepareInstall => match display {
                "staged" => Ok(Admission::Accept {
                    state: "staged",
                    update_id,
                }),
                "available" => Err(BusinessError::new(
                    codes::UPDATE_NOT_READY,
                    "候选尚未下载就位（先 download）",
                )),
                _ => Err(BusinessError::new(
                    codes::UPDATE_BUSY,
                    format!("状态 {display} 不接受 prepare_install"),
                )),
            },
            Operation::Rollback => match display {
                "staged" | "waiting" => Ok(Admission::Accept {
                    state: "rolling_back",
                    update_id,
                }),
                "installing" | "restarting" | "rolling_back" => Err(BusinessError::new(
                    codes::UPDATE_BUSY,
                    format!("状态 {display} 中不接受 rollback"),
                )),
                _ => Err(BusinessError::new(
                    codes::ROLLBACK_UNAVAILABLE,
                    "无有效回滚点（无进行中事务或无已验证快照）",
                )),
            },
            Operation::RepairDependency(_) => Ok(Admission::Accept {
                state: display,
                update_id: None,
            }),
        }
    }

    /// 长操作执行（独立线程）：结束/失败都由 journal 体现，status 轮询可见；
    /// 结束后释放 active 槽位。
    fn run_long_op(self: &Arc<Self>, op: Operation) {
        match op {
            Operation::Status => {}
            Operation::Check => {
                if let Err(e) = self.engine.phase_check(&self.check_source) {
                    tracing::warn!(code = %e.code, %e.message, "IPC check 失败");
                }
            }
            Operation::Download => {
                if let Err(e) = self.engine.phase_download() {
                    tracing::warn!(code = %e.code, %e.message, "IPC download 失败");
                }
            }
            Operation::PrepareInstall => {
                if let Err(e) = self.engine.phase_prepare_install() {
                    tracing::warn!(code = %e.code, %e.message, "IPC prepare_install 失败");
                }
            }
            Operation::Rollback => {
                if let Err(e) = self.engine.phase_rollback() {
                    tracing::warn!(code = %e.code, %e.message, "IPC rollback 失败");
                }
            }
            Operation::RepairDependency(dep) => self.run_repair_dependency(dep),
        }
        let mut inner = match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if inner.active.as_ref().is_some_and(|a| a.op == op) {
            inner.active = None;
        }
    }

    /// repair_dependency：inventory→seed/cache→remote→probe 编排（复用 repair）。
    /// 不触碰升级 journal；结果经 status dependencies.*.status 观察。
    fn run_repair_dependency(&self, dep: Dependency) {
        let manifest = match self.load_repair_manifest() {
            Ok(m) => m,
            Err(msg) => {
                tracing::error!("repair_dependency 无法装载已验签 manifest: {msg}");
                return;
            }
        };
        let Some(platform) = manifest.platforms.get("windows-x86_64") else {
            tracing::error!("repair_dependency: manifest 缺少 windows-x86_64 平台");
            return;
        };
        let Some(comp) = platform.components.iter().find(|c| c.id == dep.as_str()) else {
            tracing::error!("repair_dependency: manifest 无组件 {}", dep.as_str());
            return;
        };
        let spec = match crate::inventory::ComponentSpec::from_model(comp) {
            Ok(s) => s,
            Err(msg) => {
                tracing::error!("repair_dependency: 组件规格非法: {msg}");
                return;
            }
        };
        let outcome = crate::repair::repair_component(
            &self.layout,
            &spec,
            &crate::repair::RepairOptions::default(),
        );
        match outcome.outcome {
            crate::repair::ComponentOutcome::Healthy => {
                tracing::info!(dep = dep.as_str(), "依赖完好，无需修复");
            }
            crate::repair::ComponentOutcome::Repaired { source } => {
                tracing::info!(dep = dep.as_str(), %source, "依赖修复完成");
            }
            crate::repair::ComponentOutcome::Failed { reason } => {
                tracing::error!(dep = dep.as_str(), %reason, "依赖修复失败");
            }
        }
    }

    fn load_repair_manifest(&self) -> Result<crate::manifest::model::Manifest, String> {
        let mut candidates: Vec<PathBuf> = std::fs::read_dir(self.layout.manifests_dir())
            .map_err(|e| format!("manifests/ 不可读: {e}"))?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
            .collect();
        candidates.sort();
        let opts = crate::manifest::ValidateOptions {
            keys_dir: Some(self.keys_dir.clone()),
            launcher_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            ..crate::manifest::ValidateOptions::default()
        };
        for path in candidates {
            let outcome = crate::manifest::validate_manifest_file(&path, &opts);
            if outcome.ok {
                let raw = std::fs::read(&path).map_err(|e| format!("读取失败: {e}"))?;
                let value: Value =
                    serde_json::from_slice(&raw).map_err(|e| format!("JSON 非法: {e}"))?;
                return crate::manifest::model::Manifest::parse(&value);
            }
        }
        Err("manifests/ 内无可信 manifest".to_string())
    }

    /// status 只读快照（ipc-v1 §4.1 冻结 result 形态）。
    pub fn status_result(&self) -> Value {
        let store = self.store();
        let (current, previous) = match store.load_current() {
            Ok(LoadOutcome::Present(c)) => (Some(c.current), c.previous),
            _ => (None, None),
        };
        let journal = store.load_journal().map(|j| j.journal).unwrap_or_default();
        let manifest_current = current
            .as_deref()
            .and_then(|v| read_json(&self.layout.manifests_dir().join(format!("{v}.json"))));
        let schema_db = journal
            .data_schema_before
            .or_else(|| {
                manifest_current
                    .as_ref()
                    .and_then(|m| m.get("release"))
                    .and_then(|r| r.get("data_schema"))
                    .and_then(Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok())
            })
            .map(serde_json::Number::from);
        let rollback_floor = manifest_current
            .as_ref()
            .and_then(|m| m.get("release"))
            .and_then(|r| r.get("rollback_floor"))
            .and_then(Value::as_u64)
            .map(serde_json::Number::from);
        let candidate_version = journal.to_version.clone();
        let candidate = candidate_version.as_deref().and_then(|v| {
            let m = read_json(&self.layout.manifests_dir().join(format!("{v}.json")))?;
            let release = m.get("release")?;
            Some(json!({
                "version": v,
                "channel": release.get("channel").cloned().unwrap_or(Value::Null),
                "published_at": release.get("published_at").cloned().unwrap_or(Value::Null),
            }))
        });
        let state = display_state(&journal);
        let progress = if state == "downloading" {
            let (done, total) = self.engine.progress.snapshot();
            Some(json!({"bytes_done": done, "bytes_total": total}))
        } else {
            None
        };
        let last_error = journal
            .error
            .as_ref()
            .map(|e| json!({"code": e.code, "message": e.message}));
        json!({
            "launcher_version": self.launcher_version,
            "installation_id": self.installation_id,
            "protocol_version": PROTOCOL_VERSION,
            "versions": {
                "current": current,
                "previous": previous,
            },
            "schema": {
                "db": schema_db,
                "file": 1,
                "rollback_floor": rollback_floor,
            },
            "update": {
                "state": state,
                "detail": display_detail(&journal),
                "update_id": journal.update_id,
                "candidate": candidate,
                "progress": progress,
                "last_error": last_error,
            },
            "dependencies": {
                "adb": dependency_status(&self.layout, "adb", "adb.exe"),
                "ffmpeg": dependency_status(&self.layout, "ffmpeg", "ffmpeg.exe"),
            },
        })
    }
}

/// 单条依赖状态（exe 存在 = ready，版本取目录名；缺失 = missing）。
fn dependency_status(layout: &InstallLayout, id: &str, exe: &str) -> Value {
    match latest_component_exe(layout, id, exe) {
        Some(path) => {
            let version = path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .map(str::to_string);
            json!({"status": "ready", "version": version})
        }
        None => json!({"status": "missing", "version": null}),
    }
}

fn read_json(path: &std::path::Path) -> Option<Value> {
    let raw = std::fs::read(path).ok()?;
    serde_json::from_slice(&raw).ok()
}

enum Admission {
    Accept {
        state: &'static str,
        update_id: Option<String>,
    },
    NoOp {
        state: &'static str,
        update_id: Option<String>,
    },
}
