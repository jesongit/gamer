//! Small, explicitly supported system settings. No plugin configuration lives here.
use std::path::Path;
use std::sync::{Mutex, RwLock};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::Config;

#[derive(Debug, Default)]
pub struct LiveSettings {
    values: RwLock<Option<SystemSettings>>,
    save_lock: Mutex<()>,
}

impl LiveSettings {
    pub fn snapshot(&self) -> Option<SystemSettings> {
        self.values.read().unwrap().clone()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoginSettings {
    pub session_abs_secs: u64,
    pub session_idle_secs: u64,
    pub login_max_fails: u32,
    pub login_window_secs: u64,
}

impl From<&crate::config::AuthConfig> for LoginSettings {
    fn from(auth: &crate::config::AuthConfig) -> Self {
        Self {
            session_abs_secs: auth.session_abs_secs,
            session_idle_secs: auth.session_idle_secs,
            login_max_fails: auth.login_max_fails,
            login_window_secs: auth.login_window_secs,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemSettings {
    pub idle_power_secs: u64,
    pub log_retain_days: u32,
    pub compute_max_concurrency: u32,
    pub auth: LoginSettings,
}

impl SystemSettings {
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            idle_power_secs: cfg.idle_power_secs,
            log_retain_days: cfg.log_retain_days,
            compute_max_concurrency: cfg.compute_max_concurrency,
            auth: (&cfg.auth).into(),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.idle_power_secs > 604800 {
            return Err("空闲省电时间须在 0～604800 秒之间".into());
        }
        if self.log_retain_days > 36500 {
            return Err("日志保留天数须在 0～36500 之间".into());
        }
        let mut cfg = Config {
            compute_max_concurrency: self.compute_max_concurrency,
            ..Default::default()
        };
        cfg.auth.session_abs_secs = self.auth.session_abs_secs;
        cfg.auth.session_idle_secs = self.auth.session_idle_secs;
        cfg.auth.login_max_fails = self.auth.login_max_fails;
        cfg.auth.login_window_secs = self.auth.login_window_secs;
        let errors = cfg.validate();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("；"))
        }
    }
}

#[derive(Serialize)]
pub struct SettingsView {
    pub saved: SystemSettings,
    pub active: SystemSettings,
    pub revision: String,
    pub restart_required: bool,
}

#[derive(Debug)]
pub enum SaveError {
    Invalid(String),
    Conflict,
    Io(anyhow::Error),
}

fn read_file(path: &Path) -> anyhow::Result<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e.into()),
    }
}

fn revision(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

impl Config {
    pub fn current_settings(&self) -> SystemSettings {
        self.live_settings
            .snapshot()
            .unwrap_or_else(|| SystemSettings::from_config(self))
    }

    pub fn settings_view(&self) -> anyhow::Result<SettingsView> {
        let text = read_file(&self.settings_path())?;
        self.settings_view_for(&text)
    }

    fn settings_view_for(&self, text: &str) -> anyhow::Result<SettingsView> {
        let saved = if text.is_empty() {
            SystemSettings::from_config(self)
        } else {
            SystemSettings::from_config(&toml::from_str::<Config>(text)?)
        };
        let mut active = self.current_settings();
        active.compute_max_concurrency = self.compute_max_concurrency;
        Ok(SettingsView {
            restart_required: saved.compute_max_concurrency != active.compute_max_concurrency,
            saved,
            active,
            revision: revision(text),
        })
    }

    fn settings_path(&self) -> std::path::PathBuf {
        self.source_path.clone().unwrap_or_else(|| {
            std::env::var("GB_CONFIG")
                .unwrap_or_else(|_| "config.toml".into())
                .into()
        })
    }

    pub fn save_settings(
        &self,
        values: SystemSettings,
        expected: &str,
    ) -> Result<SettingsView, SaveError> {
        values.validate().map_err(SaveError::Invalid)?;
        let _guard = self.live_settings.save_lock.lock().unwrap();
        let path = self.settings_path();
        let old = read_file(&path).map_err(SaveError::Io)?;
        if revision(&old) != expected {
            return Err(SaveError::Conflict);
        }
        let initial = if old.is_empty() {
            toml::to_string_pretty(self).map_err(|e| SaveError::Io(e.into()))?
        } else {
            old
        };
        let mut doc = initial
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| SaveError::Io(e.into()))?;
        for (key, number) in [
            ("idle_power_secs", values.idle_power_secs),
            ("log_retain_days", values.log_retain_days as u64),
            (
                "compute_max_concurrency",
                values.compute_max_concurrency as u64,
            ),
        ] {
            doc[key] = toml_edit::value(number as i64);
        }
        for (key, number) in [
            ("session_abs_secs", values.auth.session_abs_secs),
            ("session_idle_secs", values.auth.session_idle_secs),
            ("login_max_fails", values.auth.login_max_fails as u64),
            ("login_window_secs", values.auth.login_window_secs),
        ] {
            doc["auth"][key] = toml_edit::value(number as i64);
        }
        let updated = doc.to_string();
        // Validate before touching disk or live values. Credentials are never returned by this API.
        let parsed: Config = toml::from_str(&updated).map_err(|e| SaveError::Io(e.into()))?;
        let errors = parsed.validate();
        if !errors.is_empty() {
            return Err(SaveError::Invalid(errors.join("；")));
        }
        crate::core::fs::atomic_write(&path, updated.as_bytes()).map_err(SaveError::Io)?;
        *self.live_settings.values.write().unwrap() = Some(values);
        self.settings_view_for(&updated).map_err(SaveError::Io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_save_preserves_credentials_and_checks_revision_before_hot_apply() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut cfg = Config {
            source_path: Some(path.clone()),
            ..Default::default()
        };
        cfg.auth.password_hash = crate::api::auth::hash_password("settings-test-password").unwrap();
        let original = format!(
            "# keep this comment\n{}\n[custom]\nkeep = 'unchanged'\n",
            toml::to_string_pretty(&cfg).unwrap()
        );
        std::fs::write(&path, &original).unwrap();
        let consumer = cfg.clone();
        let before = cfg.settings_view().unwrap();
        let mut updated = before.saved.clone();
        updated.idle_power_secs = 90;
        updated.log_retain_days = 0;
        updated.compute_max_concurrency = 2;
        let result = cfg
            .save_settings(updated.clone(), &before.revision)
            .unwrap();
        assert_eq!(consumer.current_settings().idle_power_secs, 90);
        assert_eq!(consumer.current_settings().log_retain_days, 0);
        assert_eq!(result.active.compute_max_concurrency, 0);
        assert!(result.restart_required);
        let disk = std::fs::read_to_string(&path).unwrap();
        assert!(disk.contains("# keep this comment"));
        assert!(disk.contains("keep = 'unchanged'"));
        assert!(disk.contains(&cfg.auth.password_hash));
        let response = serde_json::to_string(&result).unwrap();
        assert!(!response.contains("password_hash"));
        assert!(!response.contains(&cfg.auth.password_hash));
        let restarted: Config = toml::from_str(&disk).unwrap();
        assert_eq!(restarted.current_settings(), updated);
        assert!(matches!(
            cfg.save_settings(before.saved.clone(), &before.revision),
            Err(SaveError::Conflict)
        ));
        updated.auth.login_max_fails = 0;
        assert!(matches!(
            cfg.save_settings(updated, &result.revision),
            Err(SaveError::Invalid(_))
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), disk);
        assert_eq!(consumer.current_settings().idle_power_secs, 90);
        // Failed persistence must not publish new live settings.
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(matches!(
            cfg.save_settings(before.saved, &result.revision),
            Err(SaveError::Io(_))
        ));
        assert_eq!(consumer.current_settings().idle_power_secs, 90);
    }
}
