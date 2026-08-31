//! 更新策略（SYS-005 / system-api-v1 §6 冻结形态）。
//!
//! `strategy` = off（不检查）| notify（自动检查+下载，用户确认安装，产品默认）
//! | auto（下载并在维护窗口 + 空闲门禁满足后安装）。
//!
//! 来源链：config.toml `[update]` 段为基线；`PUT /api/system/update/policy`
//! 热生效并**持久化到 `<data_dir>/state/update-policy.json`**（不改用户
//! config.toml）；启动时 state 文件存在则优先于配置段（显式保存过的策略
//! 胜出）。校验失败 → `invalid_argument`（§6，details.field 指明字段）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

/// 策略枚举（§6 冻结）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateStrategy {
    Off,
    Notify,
    Auto,
}

impl UpdateStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            UpdateStrategy::Off => "off",
            UpdateStrategy::Notify => "notify",
            UpdateStrategy::Auto => "auto",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "off" => UpdateStrategy::Off,
            "notify" => UpdateStrategy::Notify,
            "auto" => UpdateStrategy::Auto,
            _ => return None,
        })
    }
}

/// 维护窗口（`HH:MM` 24 小时制本地时间；允许跨午夜；start == end 非法）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaintenanceWindow {
    pub start: String,
    pub end: String,
}

impl Default for MaintenanceWindow {
    fn default() -> Self {
        Self {
            start: "02:00".into(),
            end: "06:00".into(),
        }
    }
}

/// `HH:MM` 解析（0≤HH≤23、0≤MM≤59，定长 5 字符）
pub fn parse_hh_mm(value: &str) -> Option<(u8, u8)> {
    let bytes = value.as_bytes();
    if bytes.len() != 5 || bytes[2] != b':' {
        return None;
    }
    let hh = value.get(0..2)?.parse::<u8>().ok()?;
    let mm = value.get(3..5)?.parse::<u8>().ok()?;
    if hh > 23 || mm > 59 || !value.is_ascii() {
        return None;
    }
    Some((hh, mm))
}

/// 策略对象（§6 PUT 请求/响应体同构）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdatePolicy {
    pub strategy: UpdateStrategy,
    pub maintenance_window: MaintenanceWindow,
    /// cron 冻结窗口分钟数（0~1440；建议默认 30）
    #[serde(rename = "freeze_window_minutes")]
    pub freeze_minutes: i64,
}

impl Default for UpdatePolicy {
    fn default() -> Self {
        Self {
            strategy: UpdateStrategy::Notify,
            maintenance_window: MaintenanceWindow::default(),
            freeze_minutes: 30,
        }
    }
}

impl UpdatePolicy {
    /// config.toml `[update]` 段 → 策略基线（[`PolicyStore::load`] 的回落基线；
    /// 段校验在 config 启动期已完成，这里只做透传）
    pub fn from_config(cfg: &crate::config::UpdateConfig) -> Self {
        Self {
            strategy: UpdateStrategy::parse(&cfg.strategy).unwrap_or(UpdateStrategy::Notify),
            maintenance_window: MaintenanceWindow {
                start: cfg.maintenance_window_start.clone(),
                end: cfg.maintenance_window_end.clone(),
            },
            freeze_minutes: cfg.freeze_minutes,
        }
    }

    /// 结构校验（§6）：返回 `Some(field)` = 非法字段名（invalid_argument 的
    /// details.field）；None = 合法。
    pub fn validate_field(&self) -> Option<&'static str> {
        if parse_hh_mm(&self.maintenance_window.start).is_none() {
            return Some("maintenance_window");
        }
        if parse_hh_mm(&self.maintenance_window.end).is_none() {
            return Some("maintenance_window");
        }
        if self.maintenance_window.start == self.maintenance_window.end {
            return Some("maintenance_window");
        }
        if !(0..=1440).contains(&self.freeze_minutes) {
            return Some("freeze_window_minutes");
        }
        None
    }

    /// 维护窗口判定（支持跨午夜；now 为本地 `HH:MM` 的分钟数）。窗口语义
    /// 左闭右开：`start ≤ t < end`。
    pub fn in_maintenance_window(&self, now_minutes: i64) -> bool {
        let (sh, sm) = parse_hh_mm(&self.maintenance_window.start).unwrap_or((2, 0));
        let (eh, em) = parse_hh_mm(&self.maintenance_window.end).unwrap_or((6, 0));
        let start = (sh as i64) * 60 + sm as i64;
        let end = (eh as i64) * 60 + em as i64;
        let t = now_minutes.rem_euclid(24 * 60);
        if start < end {
            t >= start && t < end
        } else {
            // 跨午夜（如 23:00→05:00）
            t >= start || t < end
        }
    }
}

/// 策略校验错误（API 层映射 400 invalid_argument）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyValidationError {
    pub field: &'static str,
}

/// 运行时策略存储：内部可变 + state/ JSON 持久化。
pub struct PolicyStore {
    current: Mutex<UpdatePolicy>,
    persist_path: PathBuf,
}

impl PolicyStore {
    /// 从配置段基线 + 既有 state 文件装配（state 文件存在且合法 → 胜出）。
    /// state 文件损坏/非法时忽略并保留配置基线（后续保存会覆盖它）。
    pub fn load_blocking(data_dir: &Path, baseline: UpdatePolicy) -> Arc<Self> {
        let persist_path = data_dir.join("state").join("update-policy.json");
        let mut initial = baseline;
        match std::fs::read_to_string(&persist_path) {
            Ok(raw) => match serde_json::from_str::<UpdatePolicy>(&raw) {
                Ok(saved) if saved.validate_field().is_none() => initial = saved,
                Ok(_) | Err(_) => {
                    tracing::warn!(
                        path = %persist_path.display(),
                        "update policy state file invalid; falling back to configured baseline"
                    );
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                tracing::warn!(error = %e, "read update policy state failed; using baseline");
            }
        }
        Arc::new(Self {
            current: Mutex::new(initial),
            persist_path,
        })
    }

    /// 当前生效策略
    pub async fn get(&self) -> UpdatePolicy {
        self.current.lock().await.clone()
    }

    /// 整对象替换（幂等）：校验失败返回字段名，不改动现有策略；成功则
    /// 写回运行时并持久化到 state/ JSON（原子写：tmp + rename）。
    pub async fn replace(
        &self,
        policy: UpdatePolicy,
    ) -> Result<UpdatePolicy, PolicyValidationError> {
        if let Some(field) = policy.validate_field() {
            return Err(PolicyValidationError { field });
        }
        {
            let mut cur = self.current.lock().await;
            *cur = policy.clone();
        }
        if let Err(e) = self.persist(&policy).await {
            // 运行时已生效；持久化失败只记日志（下次重启回落配置基线）
            tracing::warn!(error = %e, "persist update policy failed");
        }
        Ok(policy)
    }

    /// 原子持久化：state 目录 + tmp 文件 + rename（blocking 池执行，不占核心线程）
    async fn persist(&self, policy: &UpdatePolicy) -> std::io::Result<()> {
        let path = self.persist_path.clone();
        let bytes = serde_json::to_vec_pretty(policy).map_err(std::io::Error::other)?;
        tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut bytes = bytes;
            bytes.push(b'\n');
            let tmp = PathBuf::from(format!("{}.tmp", path.display()));
            {
                use std::io::Write as _;
                let mut f = std::fs::File::create(&tmp)?;
                f.write_all(&bytes)?;
                f.sync_all()?;
            }
            std::fs::rename(&tmp, &path)
        })
        .await
        .map_err(std::io::Error::other)?
    }

    /// 序列化供 API 直接回显
    pub fn to_json(policy: &UpdatePolicy) -> serde_json::Value {
        serde_json::json!({
            "strategy": policy.strategy.as_str(),
            "maintenance_window": {
                "start": policy.maintenance_window.start,
                "end": policy.maintenance_window.end,
            },
            "freeze_window_minutes": policy.freeze_minutes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(strategy: UpdateStrategy, start: &str, end: &str, freeze: i64) -> UpdatePolicy {
        UpdatePolicy {
            strategy,
            maintenance_window: MaintenanceWindow {
                start: start.into(),
                end: end.into(),
            },
            freeze_minutes: freeze,
        }
    }

    #[test]
    fn hh_mm_parse_accepts_valid_and_rejects_invalid() {
        assert_eq!(parse_hh_mm("02:00"), Some((2, 0)));
        assert_eq!(parse_hh_mm("23:59"), Some((23, 59)));
        assert_eq!(parse_hh_mm("00:00"), Some((0, 0)));
        assert_eq!(parse_hh_mm("2:00"), None);
        assert_eq!(parse_hh_mm("24:00"), None);
        assert_eq!(parse_hh_mm("12:60"), None);
        assert_eq!(parse_hh_mm("ab:cd"), None);
        assert_eq!(parse_hh_mm("1200"), None);
        assert_eq!(parse_hh_mm(""), None);
    }

    #[test]
    fn policy_validation_rejects_bad_fields() {
        assert_eq!(
            policy(UpdateStrategy::Auto, "02:00", "06:00", 30).validate_field(),
            None
        );
        // start == end 非法（fixture update-policy.invalid-argument 场景）
        assert_eq!(
            policy(UpdateStrategy::Auto, "02:00", "02:00", 30).validate_field(),
            Some("maintenance_window")
        );
        assert_eq!(
            policy(UpdateStrategy::Auto, "25:00", "06:00", 30).validate_field(),
            Some("maintenance_window")
        );
        assert_eq!(
            policy(UpdateStrategy::Auto, "02:00", "06:00", -1).validate_field(),
            Some("freeze_window_minutes")
        );
        assert_eq!(
            policy(UpdateStrategy::Auto, "02:00", "06:00", 1441).validate_field(),
            Some("freeze_window_minutes")
        );
    }

    #[test]
    fn maintenance_window_supports_normal_and_cross_midnight() {
        let normal = policy(UpdateStrategy::Auto, "02:00", "06:00", 30);
        assert!(!normal.in_maintenance_window(60 + 59));
        assert!(normal.in_maintenance_window(2 * 60));
        assert!(normal.in_maintenance_window(5 * 60 + 59));
        assert!(!normal.in_maintenance_window(6 * 60));

        let cross = policy(UpdateStrategy::Auto, "23:00", "05:00", 30);
        assert!(cross.in_maintenance_window(23 * 60));
        assert!(cross.in_maintenance_window(2 * 60 + 30));
        assert!(cross.in_maintenance_window(4 * 60 + 59));
        assert!(!cross.in_maintenance_window(5 * 60));
        assert!(!cross.in_maintenance_window(12 * 60));
        assert!(!cross.in_maintenance_window(22 * 60 + 59));
        assert!(cross.in_maintenance_window(23 * 60));
    }

    #[tokio::test]
    async fn policy_store_persists_and_state_file_wins_on_reload() {
        let dir =
            std::env::temp_dir().join(format!("gamer-policy-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();

        // 基线（默认 notify/02:00-06:00/30）
        let store = PolicyStore::load_blocking(&dir, UpdatePolicy::default());
        assert_eq!(store.get().await, UpdatePolicy::default());

        // 热更新为 auto + 跨午夜窗口并持久化
        let saved = store
            .replace(policy(UpdateStrategy::Auto, "23:00", "05:00", 15))
            .await
            .unwrap();
        assert_eq!(saved.strategy, UpdateStrategy::Auto);
        drop(store);

        // 重新加载：state 文件胜出
        let store = PolicyStore::load_blocking(&dir, UpdatePolicy::default());
        let cur = store.get().await;
        assert_eq!(cur.strategy, UpdateStrategy::Auto);
        assert_eq!(cur.maintenance_window.start, "23:00");
        assert_eq!(cur.maintenance_window.end, "05:00");
        assert_eq!(cur.freeze_minutes, 15);

        // 非法替换被拒绝且不落盘
        let err = store
            .replace(policy(UpdateStrategy::Auto, "08:00", "08:00", 15))
            .await
            .unwrap_err();
        assert_eq!(err.field, "maintenance_window");
        assert_eq!(store.get().await, cur);

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn policy_to_json_matches_contract_field_names() {
        let json = PolicyStore::to_json(&UpdatePolicy::default());
        assert_eq!(
            json,
            serde_json::json!({
                "strategy": "notify",
                "maintenance_window": { "start": "02:00", "end": "06:00" },
                "freeze_window_minutes": 30,
            })
        );
    }

    /// SYS-005：三种策略的动作边界固定；off 不检查，notify 只检查/下载，
    /// auto 只有窗口内且空闲时安装。
    #[test]
    fn strategy_modes_have_distinct_check_download_install_boundaries() {
        use crate::update::coordinator::{decide, Tick};
        use crate::update::model::UpdateState;
        use crate::update::workload::Workload;

        let idle = Workload::default();
        for state in [
            UpdateState::Idle,
            UpdateState::Available,
            UpdateState::Staged,
            UpdateState::Waiting,
        ] {
            assert_eq!(
                decide(UpdateStrategy::Off, true, state, &idle, 30),
                Tick::Noop,
                "off must never act in {state:?}"
            );
        }
        assert_eq!(
            decide(UpdateStrategy::Notify, false, UpdateState::Idle, &idle, 30),
            Tick::Check
        );
        assert_eq!(
            decide(
                UpdateStrategy::Notify,
                false,
                UpdateState::Available,
                &idle,
                30
            ),
            Tick::Download
        );
        assert_eq!(
            decide(UpdateStrategy::Notify, true, UpdateState::Staged, &idle, 30),
            Tick::Noop
        );
        assert_eq!(
            decide(UpdateStrategy::Auto, false, UpdateState::Staged, &idle, 30),
            Tick::Noop
        );
        assert_eq!(
            decide(UpdateStrategy::Auto, true, UpdateState::Staged, &idle, 30),
            Tick::Install
        );
    }

    /// SYS-005 / QA-006：下一次启用 cron 恰在冻结窗口边界及窗口内时，auto
    /// 只能等待；超过一秒的安全边际后才可安装。
    #[test]
    fn auto_waits_for_nearby_cron_and_installs_after_freeze_window() {
        use crate::update::coordinator::{decide, Tick};
        use crate::update::model::UpdateState;
        use crate::update::workload::Workload;

        let staged = UpdateState::Staged;
        let near = Workload {
            next_cron_secs: Some(30 * 60),
            ..Workload::default()
        };
        assert_eq!(
            decide(UpdateStrategy::Auto, true, staged, &near, 30),
            Tick::Noop
        );
        let safe = Workload {
            next_cron_secs: Some(30 * 60 + 1),
            ..Workload::default()
        };
        assert_eq!(
            decide(UpdateStrategy::Auto, true, staged, &safe, 30),
            Tick::Install
        );
    }
}
