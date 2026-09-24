use super::*;
use sha2::{Digest, Sha256};

const REMEMBER_SECS: u64 = 30 * 24 * 3600;

#[derive(serde::Serialize, serde::Deserialize)]
struct SessionFile {
    schema_version: u32,
    credential: String,
    sessions: HashMap<String, Session>,
}

pub(super) fn session_key(sid: &str) -> String {
    format!("{:x}", Sha256::digest(sid.as_bytes()))
}

pub(super) fn session_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl AuthState {
    /// 会话文件只保存随机令牌摘要；凭据更换即清空，不持久化明文密码。
    pub fn with_session_store(self, root: &Path) -> anyhow::Result<Self> {
        let password = std::env::var("GAMER_ADMIN_PASSWORD").ok();
        self.open_session_store(root, password.as_deref())
    }

    fn open_session_store(
        mut self,
        root: &Path,
        dev_password: Option<&str>,
    ) -> anyhow::Result<Self> {
        let path = root.join("auth-sessions.json");
        if path.exists() {
            let file: SessionFile = serde_json::from_slice(&fs::read(&path)?)?;
            anyhow::ensure!(file.schema_version == 1, "不支持的登录会话存储版本");
            let same_credential = match &*self.credential.read().unwrap() {
                Credential::Argon2 {
                    encoded,
                    source: CredentialSource::Config,
                } => encoded == &file.credential,
                Credential::Argon2 {
                    source: CredentialSource::DevEnv,
                    ..
                } => {
                    // 开发凭据每次启动的随机盐不同；用当前开发密码验证旧 PHC。
                    dev_password.is_some_and(|password| {
                        self.verify_credentials(password)
                            && parse_fixed_argon2_phc(&file.credential).is_ok_and(|hash| {
                                fixed_argon2()
                                    .verify_password(password.as_bytes(), &hash)
                                    .is_ok()
                            })
                    })
                }
                Credential::Unavailable => false,
            };
            if same_credential {
                let now = session_now();
                self.inner.get_mut().unwrap().sessions = file
                    .sessions
                    .into_iter()
                    .filter(|(key, s)| {
                        key.len() == 64
                            && key.bytes().all(|v| v.is_ascii_hexdigit())
                            && s.username == "admin"
                            && self.session_active(s, now)
                    })
                    .map(|(key, mut session)| {
                        // Freeze the pre-settings session policy when loading older session files.
                        session.idle_secs.get_or_insert(self.cfg.session_idle_secs);
                        (key, session)
                    })
                    .collect();
            }
        }
        self.session_path = Some(path);
        self.persist_sessions(&self.inner.lock().unwrap())?;
        Ok(self)
    }

    pub(super) fn session_active(&self, s: &Session, now: u64) -> bool {
        let idle = if s.remember {
            REMEMBER_SECS
        } else {
            s.idle_secs.unwrap_or(self.cfg.session_idle_secs).max(1)
        };
        now < s.abs_expire && now.saturating_sub(s.last_seen) < idle * 1000
    }

    pub(super) fn make_session(&self, remember: bool) -> Session {
        let now = session_now();
        let ttl = if remember {
            REMEMBER_SECS
        } else {
            self.login_settings().session_abs_secs.max(1)
        };
        Session {
            username: "admin".into(),
            abs_expire: now.saturating_add(ttl * 1000),
            last_seen: now,
            remember,
            idle_secs: Some(self.login_settings().session_idle_secs),
        }
    }

    pub(super) fn persist_sessions(&self, inner: &Inner) -> anyhow::Result<()> {
        let Some(path) = &self.session_path else {
            return Ok(());
        };
        let credential = match &*self.credential.read().unwrap() {
            Credential::Argon2 { encoded, .. } => encoded.clone(),
            Credential::Unavailable => String::new(),
        };
        let bytes = serde_json::to_vec(&SessionFile {
            schema_version: 1,
            credential,
            sessions: inner.sessions.clone(),
        })?;
        crate::core::fs::atomic_write(path, &bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credential(password: &str) -> Credential {
        parse_password_hash(&hash_password(password).unwrap()).unwrap()
    }

    #[test]
    fn live_settings_keep_existing_sessions_and_change_new_logins_and_rate_limits() {
        let root = tempfile::tempdir().unwrap();
        let cfg = crate::config::Config {
            source_path: Some(root.path().join("config.toml")),
            ..Default::default()
        };
        fs::write(
            cfg.source_path.as_ref().unwrap(),
            toml::to_string(&cfg).unwrap(),
        )
        .unwrap();
        let cred = credential("settings-login-password");
        let auth = AuthState::new(cred.clone(), cfg.auth.clone(), false, None)
            .with_live_settings(cfg.live_settings.clone())
            .open_session_store(root.path(), None)
            .unwrap();
        let (old_id, _) = auth
            .attempt_login("admin", "settings-login-password", "ip")
            .unwrap();
        let view = cfg.settings_view().unwrap();
        let mut next = view.saved;
        next.auth.session_abs_secs = 120;
        next.auth.session_idle_secs = 60;
        next.auth.login_max_fails = 1;
        cfg.save_settings(next, &view.revision).unwrap();
        let (new_id, _) = auth
            .attempt_login("admin", "settings-login-password", "ip")
            .unwrap();
        {
            let sessions = &auth.inner.lock().unwrap().sessions;
            let old = &sessions[&session_key(&old_id)];
            let new = &sessions[&session_key(&new_id)];
            assert_eq!(old.idle_secs, Some(7200));
            assert_eq!(new.idle_secs, Some(60));
            assert!(old.abs_expire > new.abs_expire + 3600000);
        }
        assert!(matches!(
            auth.attempt_login("admin", "wrong", "limited-ip"),
            Err(LoginError::Invalid)
        ));
        assert!(matches!(
            auth.attempt_login("admin", "wrong", "limited-ip"),
            Err(LoginError::RateLimited { .. })
        ));
        drop(auth);
        let restarted = AuthState::new(cred, cfg.auth.clone(), false, None)
            .open_session_store(root.path(), None)
            .unwrap();
        assert_eq!(
            restarted.inner.lock().unwrap().sessions[&session_key(&new_id)].idle_secs,
            Some(60)
        );
        assert!(restarted.validate(&old_id).is_some());
    }

    #[test]
    fn persisted_sessions_survive_restart_and_logout_revokes_them() {
        for remember in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let cred = credential("persistent-password-fixture");
            let open = || {
                AuthState::new(cred.clone(), Default::default(), true, None)
                    .open_session_store(dir.path(), None)
                    .unwrap()
            };
            let auth = open();
            let (sid, _) = auth
                .attempt_login_remember("admin", "persistent-password-fixture", "ip", remember)
                .unwrap();
            let cookie = auth.session_cookie_for(&sid);
            assert_eq!(cookie.contains("Max-Age="), remember);
            assert!(
                cookie.contains("HttpOnly")
                    && cookie.contains("SameSite=Strict")
                    && cookie.contains("Secure")
            );
            let disk = fs::read_to_string(dir.path().join("auth-sessions.json")).unwrap();
            assert!(!disk.contains(&sid));
            assert!(!disk.contains("persistent-password-fixture"));
            drop(auth);
            let restarted = open();
            assert_eq!(restarted.validate(&sid).as_deref(), Some("admin"));
            restarted.destroy(&sid).unwrap();
            drop(restarted);
            assert!(open().validate(&sid).is_none());
        }
    }

    #[test]
    fn remember_uses_thirty_days_and_password_change_invalidates_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let cred = credential("first-password");
        let auth = AuthState::new(
            cred.clone(),
            AuthConfig {
                session_abs_secs: 60,
                session_idle_secs: 60,
                ..Default::default()
            },
            false,
            None,
        )
        .open_session_store(dir.path(), None)
        .unwrap();
        let (sid, _) = auth
            .attempt_login_remember("admin", "first-password", "ip", true)
            .unwrap();
        let mut g = auth.inner.lock().unwrap();
        let s = g.sessions.get_mut(&session_key(&sid)).unwrap();
        assert!(s.abs_expire.saturating_sub(session_now()) > (REMEMBER_SECS - 5) * 1000);
        s.last_seen -= 24 * 3600 * 1000;
        drop(g);
        assert!(
            auth.validate(&sid).is_some(),
            "记住登录不受普通会话空闲限制"
        );
        drop(auth);
        let changed = AuthState::new(credential("new-password"), Default::default(), false, None)
            .open_session_store(dir.path(), None)
            .unwrap();
        assert!(changed.validate(&sid).is_none());
    }

    #[test]
    fn dev_password_random_salt_does_not_invalidate_unchanged_password() {
        let dir = tempfile::tempdir().unwrap();
        let open = |password: &str| {
            AuthState::new(
                Credential::Argon2 {
                    encoded: hash_password(password).unwrap(),
                    source: CredentialSource::DevEnv,
                },
                Default::default(),
                false,
                None,
            )
            .open_session_store(dir.path(), Some(password))
            .unwrap()
        };
        let auth = open("dev-password");
        let (sid, _) = auth.attempt_login("admin", "dev-password", "ip").unwrap();
        drop(auth);
        assert!(open("dev-password").validate(&sid).is_some());
        assert!(open("different-password").validate(&sid).is_none());
    }

    #[test]
    fn expired_sessions_do_not_revive_and_failed_persistence_does_not_issue_cookie() {
        let dir = tempfile::tempdir().unwrap();
        let cred = credential("expiry-password");
        let mut auth = AuthState::new(cred.clone(), Default::default(), false, None)
            .open_session_store(dir.path(), None)
            .unwrap();
        let (sid, _) = auth
            .attempt_login("admin", "expiry-password", "ip")
            .unwrap();
        {
            let mut g = auth.inner.lock().unwrap();
            g.sessions.get_mut(&session_key(&sid)).unwrap().abs_expire = 0;
            auth.persist_sessions(&g).unwrap();
        }
        let restarted = AuthState::new(cred, Default::default(), false, None)
            .open_session_store(dir.path(), None)
            .unwrap();
        assert!(restarted.validate(&sid).is_none());
        fs::write(dir.path().join("blocked"), b"file").unwrap();
        auth.session_path = Some(dir.path().join("blocked/sessions.json"));
        let before = auth.sessions_len();
        assert!(matches!(
            auth.attempt_login("admin", "expiry-password", "ip"),
            Err(LoginError::Persist)
        ));
        assert_eq!(auth.sessions_len(), before);
    }
}
