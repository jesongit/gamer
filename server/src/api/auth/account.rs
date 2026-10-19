//! Management credentials are configuration, never Package data or MCP tools.
use super::*;

pub(crate) fn valid_username(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::api) struct UpdateAccount {
    username: String,
    current_password: String,
    #[serde(default)]
    new_password: String,
    #[serde(default)]
    confirm_password: String,
    expected_revision: String,
}

pub(in crate::api) async fn get_account(State(st): State<AppState>) -> Response {
    let account = st.auth.account.read().unwrap();
    Json(json!({
        "username": account.username,
        "revision": account.revision,
        "password_editable": matches!(account.credential, Credential::Argon2 { source: CredentialSource::Config, .. }),
    })).into_response()
}

pub(in crate::api) async fn update_account(
    State(st): State<AppState>,
    Extension(ip): Extension<IpKey>,
    headers: HeaderMap,
    Json(req): Json<UpdateAccount>,
) -> Response {
    if !origin_allows_headers(&headers) {
        return err_response(StatusCode::FORBIDDEN, "forbidden_origin");
    }
    let Ok(_gate) = st.auth.account_update_gate.try_lock() else {
        return err_response(StatusCode::CONFLICT, "account_update_busy");
    };
    let auth = st.auth.clone();
    let sid = AuthState::extract_sid(&headers);
    match tokio::task::spawn_blocking(move || auth.change_account(req, &ip.0, sid.as_deref())).await
    {
        Ok(Ok(())) => (
            [(header::SET_COOKIE, st.auth.expired_cookie())],
            Json(json!({"ok":true,"reauthenticate":true})),
        )
            .into_response(),
        Ok(Err((status, code))) => err_response(status, code),
        Err(_) => err_response(StatusCode::INTERNAL_SERVER_ERROR, "account_update_failed"),
    }
}

type AccountResult = Result<(), (StatusCode, &'static str)>;

impl AuthState {
    fn change_account(&self, req: UpdateAccount, ip: &str, sid: Option<&str>) -> AccountResult {
        let bad = |code| (StatusCode::BAD_REQUEST, code);
        if !valid_username(&req.username) {
            return Err(bad("invalid_username"));
        }
        if req.current_password.is_empty() || req.current_password.len() > 1024 {
            return Err(bad("current_password_required"));
        }
        if !req.new_password.is_empty() && !(8..=1024).contains(&req.new_password.len()) {
            return Err(bad("weak_password"));
        }
        if req.new_password != req.confirm_password {
            return Err(bad("password_mismatch"));
        }
        // The same lock order as login/setup: sessions -> configuration -> account.
        // A request admitted before a concurrent rotation must revalidate its cookie here.
        let mut inner = self.inner.lock().unwrap();
        if sid.is_some_and(|sid| {
            !inner
                .sessions
                .get(&session_key(sid))
                .is_some_and(|s| self.session_active(s, session_now()))
        }) {
            return Err((StatusCode::UNAUTHORIZED, "unauthorized"));
        }
        let _config = self.live_settings.lock_config();
        let account = self.account.read().unwrap();
        if req.expected_revision != account.revision {
            return Err((StatusCode::CONFLICT, "account_revision_conflict"));
        }
        if req.username == account.username && req.new_password.is_empty() {
            return Err(bad("account_unchanged"));
        }
        if !req.new_password.is_empty()
            && !matches!(
                account.credential,
                Credential::Argon2 {
                    source: CredentialSource::Config,
                    ..
                }
            )
        {
            return Err((StatusCode::CONFLICT, "password_managed_by_environment"));
        }
        let key = LoginKey {
            ip: ip.to_owned(),
            username: account.username.clone(),
        };
        drop(account);
        let now = Instant::now();
        self.prune_fails(&mut inner.fails, now);
        if inner
            .fails
            .get(&key)
            .is_some_and(|f| f.attempts.len() >= self.login_settings().login_max_fails as usize)
        {
            return Err((StatusCode::TOO_MANY_REQUESTS, "too_many_attempts"));
        }
        if !self.verify_credentials(&req.current_password) {
            inner.fails.entry(key).or_default().attempts.push_back(now);
            return Err((StatusCode::FORBIDDEN, "invalid_current_password"));
        }
        let current = fs::read_to_string(&self.config_path).map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "account_config_read_failed",
            )
        })?;
        let mut document = current
            .parse::<toml_edit::DocumentMut>()
            .map_err(|_| (StatusCode::CONFLICT, "account_config_invalid"))?;
        let saved: crate::config::Config = toml::from_str(&current)
            .map_err(|_| (StatusCode::CONFLICT, "account_config_invalid"))?;
        let account = self.account.read().unwrap();
        let source_matches = match &account.credential {
            Credential::Argon2 {
                source: CredentialSource::Config,
                encoded,
            } => saved.auth.password_hash == *encoded,
            Credential::Argon2 {
                source: CredentialSource::DevEnv,
                ..
            } => true,
            Credential::Unavailable => false,
        };
        if saved.auth.username != account.username
            || saved.auth.account_revision != account.revision
            || !source_matches
        {
            return Err((StatusCode::CONFLICT, "account_config_changed"));
        }
        let encoded = if req.new_password.is_empty() {
            None
        } else {
            Some(
                hash_password(&req.new_password)
                    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "account_update_failed"))?,
            )
        };
        drop(account);
        let revision = uuid::Uuid::new_v4().to_string();
        document["auth"]["username"] = toml_edit::value(&req.username);
        document["auth"]["account_revision"] = toml_edit::value(&revision);
        if let Some(encoded) = &encoded {
            document["auth"]["password_hash"] = toml_edit::value(encoded);
        }
        write_config_atomically(&self.config_path, &document.to_string()).map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "account_config_write_failed",
            )
        })?;
        // Atomic rename is the commit point. No fallible operation may roll back only memory.
        let mut account = self.account.write().unwrap();
        account.username = req.username;
        account.revision = revision;
        if let Some(encoded) = encoded {
            account.credential = Credential::Argon2 {
                encoded,
                source: CredentialSource::Config,
            };
        }
        drop(account);
        inner.sessions.clear();
        inner.fails.clear();
        self.account_changes
            .send_modify(|generation| *generation = generation.wrapping_add(1));
        if self.persist_sessions(&inner).is_err() {
            // The persisted account revision also rejects every old disk session on restart.
            warn!("account updated; stale session file could not be cleared");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, AuthState) {
        let dir = tempfile::tempdir().unwrap();
        let cfg = crate::config::Config {
            auth: AuthConfig {
                password_hash: hash_password("original-password").unwrap(),
                ..Default::default()
            },
            ..Default::default()
        };
        fs::write(
            dir.path().join("config.toml"),
            format!("# keep this comment\n{}", toml::to_string(&cfg).unwrap()),
        )
        .unwrap();
        fs::write(
            dir.path().join("business-data"),
            b"tasks and packages stay intact",
        )
        .unwrap();
        let auth = reopen(dir.path(), None);
        (dir, auth)
    }

    fn reopen(root: &Path, dev_password: Option<&str>) -> AuthState {
        let cfg: crate::config::Config =
            toml::from_str(&fs::read_to_string(root.join("config.toml")).unwrap()).unwrap();
        let credential = if let Some(password) = dev_password {
            Credential::Argon2 {
                encoded: hash_password(password).unwrap(),
                source: CredentialSource::DevEnv,
            }
        } else {
            parse_password_hash(&cfg.auth.password_hash).unwrap()
        };
        AuthState::new(credential, cfg.auth, false, None)
            .with_config_path(Some(&root.join("config.toml")))
            .open_session_store(root, dev_password)
            .unwrap()
    }

    fn change(auth: &AuthState, username: &str, password: &str) -> UpdateAccount {
        UpdateAccount {
            username: username.into(),
            current_password: "original-password".into(),
            new_password: password.into(),
            confirm_password: password.into(),
            expected_revision: auth.account.read().unwrap().revision.clone(),
        }
    }

    #[test]
    fn account_change_revokes_all_sessions_notifies_connections_and_survives_restart() {
        let (dir, auth) = fixture();
        let old_hash = auth.account.read().unwrap().credential.clone();
        let original: crate::config::Config =
            toml::from_str(&fs::read_to_string(&auth.config_path).unwrap()).unwrap();
        let ordinary = auth
            .attempt_login("admin", "original-password", "a")
            .unwrap()
            .0;
        let remembered = auth
            .attempt_login_remember("admin", "original-password", "b", true)
            .unwrap()
            .0;
        let changes = auth.subscribe_account_changes();
        auth.change_account(change(&auth, "Owner.1", ""), "a", Some(&ordinary))
            .unwrap();
        assert!(changes.has_changed().unwrap());
        assert_eq!(auth.validate(&ordinary), None);
        assert_eq!(auth.validate(&remembered), None);
        assert!(matches!(
            auth.attempt_login("admin", "original-password", "a"),
            Err(LoginError::Invalid)
        ));
        let renamed_sid = auth
            .attempt_login("Owner.1", "original-password", "a")
            .unwrap()
            .0;
        assert!(
            matches!((&old_hash, &auth.account.read().unwrap().credential),
            (Credential::Argon2 { encoded: a, .. }, Credential::Argon2 { encoded: b, .. }) if a==b)
        );
        let mut req = change(&auth, "Owner.1", "replacement-password");
        req.expected_revision = String::new();
        assert_eq!(
            auth.change_account(req, "a", Some(&renamed_sid))
                .unwrap_err()
                .0,
            StatusCode::CONFLICT
        );
        auth.change_account(
            change(&auth, "Owner.1", "replacement-password"),
            "a",
            Some(&renamed_sid),
        )
        .unwrap();
        assert_eq!(auth.validate(&renamed_sid), None);
        let restarted = reopen(dir.path(), None);
        assert!(!restarted.verify_credentials("original-password"));
        assert!(restarted
            .attempt_login("Owner.1", "replacement-password", "a")
            .is_ok());
        assert_eq!(
            fs::read(dir.path().join("business-data")).unwrap(),
            b"tasks and packages stay intact"
        );
        let updated = fs::read_to_string(&auth.config_path).unwrap();
        assert!(updated.starts_with("# keep this comment"));
        let mut updated: crate::config::Config = toml::from_str(&updated).unwrap();
        updated.auth = original.auth.clone();
        assert_eq!(
            toml::to_string(&updated).unwrap(),
            toml::to_string(&original).unwrap()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&auth.config_path)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn account_change_validation_and_wrong_current_password_preserve_credentials_and_session() {
        let (_dir, auth) = fixture();
        let original = fs::read(&auth.config_path).unwrap();
        let sid = auth
            .attempt_login("admin", "original-password", "a")
            .unwrap()
            .0;
        for (username, new, confirm, code) in [
            ("a b", "", "", "invalid_username"),
            ("", "", "", "invalid_username"),
            ("owner", "short", "short", "weak_password"),
            (
                "owner",
                "valid-password",
                "other-password",
                "password_mismatch",
            ),
            ("admin", "", "", "account_unchanged"),
        ] {
            let mut req = change(&auth, username, new);
            req.confirm_password = confirm.into();
            assert_eq!(
                auth.change_account(req, "a", Some(&sid)).unwrap_err().1,
                code
            );
        }
        let mut req = change(&auth, "owner", "");
        req.current_password = "wrong".into();
        assert_eq!(
            auth.change_account(req, "a", Some(&sid)).unwrap_err(),
            (StatusCode::FORBIDDEN, "invalid_current_password")
        );
        assert_eq!(fs::read(&auth.config_path).unwrap(), original);
        assert!(auth.validate(&sid).is_some());
    }

    #[test]
    fn account_change_shares_login_rate_limit_and_rejects_a_revoked_request() {
        let (_dir, auth) = fixture();
        let sid = auth
            .attempt_login("admin", "original-password", "a")
            .unwrap()
            .0;
        for _ in 0..auth.cfg.login_max_fails {
            let _ = auth.attempt_login("admin", "wrong", "a");
        }
        assert_eq!(
            auth.change_account(change(&auth, "owner", ""), "a", Some(&sid))
                .unwrap_err()
                .0,
            StatusCode::TOO_MANY_REQUESTS
        );
        auth.change_account(change(&auth, "owner", ""), "b", Some(&sid))
            .unwrap();
        assert_eq!(
            auth.change_account(change(&auth, "owner2", ""), "b", Some(&sid))
                .unwrap_err()
                .0,
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn account_change_failed_write_keeps_memory_and_external_config_edit_is_rejected() {
        let (dir, auth) = fixture();
        let original = fs::read(&auth.config_path).unwrap();
        let sid = auth
            .attempt_login("admin", "original-password", "a")
            .unwrap()
            .0;
        fs::write(
            &auth.config_path,
            String::from_utf8(original.clone())
                .unwrap()
                .replace("username = \"admin\"", "username = \"external\""),
        )
        .unwrap();
        assert_eq!(
            auth.change_account(change(&auth, "owner", ""), "a", Some(&sid))
                .unwrap_err()
                .1,
            "account_config_changed"
        );
        fs::write(&auth.config_path, original).unwrap();
        // A valid TOML directory cannot be read as a config; the account must remain active.
        fs::rename(&auth.config_path, dir.path().join("saved.toml")).unwrap();
        fs::create_dir(&auth.config_path).unwrap();
        assert_eq!(
            auth.change_account(change(&auth, "owner", ""), "a", Some(&sid))
                .unwrap_err()
                .1,
            "account_config_read_failed"
        );
        assert_eq!(auth.validate(&sid).as_deref(), Some("admin"));
        assert!(auth.verify_credentials("original-password"));
        assert!(write_config_atomically(&auth.config_path, "config").is_err());
    }

    #[test]
    fn account_revision_blocks_stale_disk_sessions_even_when_username_changes_back() {
        let (dir, auth) = fixture();
        let sid = auth
            .attempt_login_remember("admin", "original-password", "a", true)
            .unwrap()
            .0;
        let stale_sessions = fs::read(dir.path().join("auth-sessions.json")).unwrap();
        auth.change_account(change(&auth, "owner", ""), "a", Some(&sid))
            .unwrap();
        let new_sid = auth
            .attempt_login("owner", "original-password", "a")
            .unwrap()
            .0;
        fs::remove_file(dir.path().join("auth-sessions.json")).unwrap();
        fs::create_dir(dir.path().join("auth-sessions.json")).unwrap();
        auth.change_account(change(&auth, "admin", ""), "a", Some(&new_sid))
            .unwrap();
        fs::remove_dir(dir.path().join("auth-sessions.json")).unwrap();
        fs::write(dir.path().join("auth-sessions.json"), stale_sessions).unwrap();
        let restarted = reopen(dir.path(), None);
        assert_eq!(restarted.validate(&sid), None);
        assert_eq!(restarted.validate(&new_sid), None);
        assert!(restarted
            .attempt_login("admin", "original-password", "a")
            .is_ok());
    }

    #[test]
    fn account_environment_password_cannot_be_overridden_but_username_can_change() {
        let (dir, _auth) = fixture();
        let auth = reopen(dir.path(), Some("original-password"));
        assert_eq!(
            auth.change_account(change(&auth, "owner", "replacement-password"), "a", None)
                .unwrap_err()
                .1,
            "password_managed_by_environment"
        );
        auth.change_account(change(&auth, "owner", ""), "a", None)
            .unwrap();
        let restarted = reopen(dir.path(), Some("original-password"));
        assert!(restarted
            .attempt_login("owner", "original-password", "a")
            .is_ok());
        assert!(matches!(
            restarted.account.read().unwrap().credential,
            Credential::Argon2 {
                source: CredentialSource::DevEnv,
                ..
            }
        ));
    }
}
