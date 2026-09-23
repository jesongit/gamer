//! Plugin-owned defaults. Each run freezes a snapshot before loading its program.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Mutex;

pub const GET_SETTINGS: &str = "settings.get";
pub const SAVE_SETTINGS: &str = "settings.save";
pub const DEFAULT_TIMEOUT_SECS: u64 = 10;
pub const DEFAULT_CLICK_DELAY_MS: u64 = 300;
fn default_click_delay_ms() -> u64 {
    DEFAULT_CLICK_DELAY_MS
}
static SAVE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub default_timeout_secs: u64,
    #[serde(default = "default_click_delay_ms")]
    pub before_click_ms: u64,
    #[serde(default = "default_click_delay_ms")]
    pub after_click_ms: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            default_timeout_secs: DEFAULT_TIMEOUT_SECS,
            before_click_ms: DEFAULT_CLICK_DELAY_MS,
            after_click_ms: DEFAULT_CLICK_DELAY_MS,
        }
    }
}

impl Settings {
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            (1..=3600).contains(&self.default_timeout_secs),
            "默认模板等待超时须为 1～3600 秒的整数"
        );
        anyhow::ensure!(
            self.before_click_ms <= 60_000 && self.after_click_ms <= 60_000,
            "点击前后延迟须为 0～60000 毫秒的整数"
        );
        Ok(())
    }
}

pub fn load(data_dir: &Path) -> anyhow::Result<Settings> {
    let path = data_dir.join("extension-data/gamer-yaml/settings.json");
    match std::fs::read(path) {
        Ok(bytes) => {
            let settings: Settings = serde_json::from_slice(&bytes)?;
            settings.validate()?;
            Ok(settings)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(e) => Err(e.into()),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveRequest {
    settings: Settings,
    expected: Settings,
}

pub fn dispatch(action: &str, values: &Value, data_dir: &Path) -> anyhow::Result<Value> {
    let _guard = SAVE_LOCK.lock().unwrap();
    let current = load(data_dir)?;
    if action == GET_SETTINGS {
        return Ok(view(current));
    }
    let request: SaveRequest = serde_json::from_value(values.clone())?;
    request.settings.validate()?;
    anyhow::ensure!(
        current == request.expected,
        "自动化设置已发生变化，请重新读取后再保存"
    );
    crate::core::fs::atomic_write(
        &data_dir.join("extension-data/gamer-yaml/settings.json"),
        &serde_json::to_vec_pretty(&request.settings)?,
    )?;
    Ok(view(request.settings))
}

fn view(settings: Settings) -> Value {
    json!({
        "title": "自动化", "values": settings,
        "fields": [{"key":"default_timeout_secs", "label":"默认模板等待超时", "unit":"秒",
            "min":1, "max":3600, "effect":"下次运行生效",
            "help":"用于 wait_find、tap_template、wait_disappear；步骤指定的 timeout 优先，正在运行的任务不受影响。"},
            {"key":"before_click_ms", "label":"点击前延迟", "unit":"毫秒", "min":0, "max":60000,
                "effect":"下次运行生效", "help":"自动化确定点击位置后、按下前等待；0 表示关闭，不影响投屏手动操作。"},
            {"key":"after_click_ms", "label":"点击后延迟", "unit":"毫秒", "min":0, "max":60000,
                "effect":"下次运行生效", "help":"自动化点击松开后等待；适用于坐标点击、模板自动点击和障碍模板点击，0 表示关闭。"}]
    })
}

pub fn uses_timeout(name: &str) -> bool {
    matches!(name, "wait_find" | "tap_template" | "wait_disappear")
}

/// Explicit arguments (including references already evaluated by the guest) always win.
pub fn bind_timeout(name: &str, args: Value, timeout_secs: u64) -> Value {
    if !uses_timeout(name) {
        return args;
    }
    let mut args = match args {
        Value::Object(map) => map,
        Value::Null => serde_json::Map::new(),
        shorthand => serde_json::Map::from_iter([("template".into(), shorthand)]),
    };
    args.entry("timeout")
        .or_insert_with(|| json!(format!("{timeout_secs}s")));
    Value::Object(args)
}

pub fn describe_functions(data_dir: &Path) -> Value {
    let settings = match load(data_dir) {
        Ok(settings) => settings,
        Err(error) => {
            tracing::warn!(%error, "cannot describe automation defaults");
            return Value::Null; // Never advertise a value the runner would not use.
        }
    };
    Value::Array(
        super::native_funcs::native_functions()
            .iter()
            .map(|function| {
                let mut schema = super::native_funcs::native_schema_json(function);
                if uses_timeout(function.name) {
                    if let Some(params) = schema["params"].as_array_mut() {
                        for param in params {
                            if param["name"] == "timeout" {
                                param["default"] =
                                    json!(format!("{}s", settings.default_timeout_secs));
                            }
                        }
                    }
                }
                schema
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_defaults_persist_validate_and_do_not_add_step_parameters() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("extension-data/gamer-yaml/settings.json");
        crate::core::fs::atomic_write(&path, br#"{"default_timeout_secs":25}"#).unwrap();
        let first = load(root.path()).unwrap();
        assert_eq!((first.before_click_ms, first.after_click_ms), (300, 300));
        let changed = Settings {
            before_click_ms: 0,
            after_click_ms: 450,
            ..first.clone()
        };
        let response = dispatch(
            SAVE_SETTINGS,
            &json!({"settings":changed,"expected":first}),
            root.path(),
        )
        .unwrap();
        assert_eq!(response["fields"].as_array().unwrap().len(), 3);
        assert_eq!(load(root.path()).unwrap(), changed);
        assert_eq!(
            (first.before_click_ms, first.after_click_ms),
            (300, 300),
            "运行快照不随保存改变"
        );
        for value in [json!(-1), json!(1.5), json!(60001)] {
            let mut invalid = serde_json::to_value(&changed).unwrap();
            invalid["before_click_ms"] = value;
            assert!(dispatch(
                SAVE_SETTINGS,
                &json!({"settings":invalid,"expected":changed}),
                root.path()
            )
            .is_err());
        }
        assert_eq!(load(root.path()).unwrap(), changed);
        let catalog = describe_functions(root.path());
        for function in catalog.as_array().unwrap() {
            assert!(function["params"]
                .as_array()
                .unwrap()
                .iter()
                .all(|p| !matches!(
                    p["name"].as_str(),
                    Some("before_click" | "after_click" | "before_click_ms" | "after_click_ms")
                )));
        }
    }

    #[test]
    fn persisted_defaults_are_frozen_per_run_and_explicit_args_win() {
        let root = tempfile::tempdir().unwrap();
        let first_run = load(root.path()).unwrap();
        dispatch(
            SAVE_SETTINGS,
            &json!({"settings":{"default_timeout_secs":25},"expected":first_run}),
            root.path(),
        )
        .unwrap();
        let next_run = load(root.path()).unwrap();
        for name in ["wait_find", "tap_template", "wait_disappear"] {
            assert_eq!(
                bind_timeout(name, json!("home"), first_run.default_timeout_secs)["timeout"],
                "10s"
            );
            assert_eq!(
                bind_timeout(
                    name,
                    json!({"template":"home"}),
                    next_run.default_timeout_secs
                )["timeout"],
                "25s"
            );
            assert_eq!(
                bind_timeout(
                    name,
                    json!({"timeout":"0ms"}),
                    next_run.default_timeout_secs
                )["timeout"],
                "0ms"
            );
        }
        assert_eq!(
            bind_timeout("custom", json!({"timeout":"3s"}), 25),
            json!({"timeout":"3s"})
        );
        let catalog = describe_functions(root.path());
        let wait = catalog
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["name"] == "wait_find")
            .unwrap();
        assert_eq!(
            wait["params"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["name"] == "timeout")
                .unwrap()["default"],
            "25s"
        );
        assert!(dispatch(
            SAVE_SETTINGS,
            &json!({"settings":{"default_timeout_secs":5},"expected":first_run}),
            root.path()
        )
        .is_err());
        assert!(dispatch(
            SAVE_SETTINGS,
            &json!({"settings":{"default_timeout_secs":0},"expected":next_run}),
            root.path()
        )
        .is_err());
        assert_eq!(load(root.path()).unwrap().default_timeout_secs, 25);
    }
}
