//! Independent MCP credentials, with encrypted secrets for administrator viewing.
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Mutex,
};

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::types::{ErrorKind, McpError};
const MAX_CREDENTIAL_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct AllowedObjects {
    pub devices: BTreeSet<String>,
    pub packages: BTreeSet<String>,
    pub plugins: BTreeSet<String>,
    pub runners: BTreeSet<String>,
    pub runs: BTreeSet<String>,
    pub tasks: BTreeSet<String>,
    pub media: BTreeSet<String>,
}
impl AllowedObjects {
    fn is_unrestricted(&self) -> bool {
        [
            &self.devices,
            &self.packages,
            &self.plugins,
            &self.runners,
            &self.runs,
            &self.tasks,
            &self.media,
        ]
        .iter()
        .all(|ids| ids.len() == 1 && ids.contains("*"))
    }
    pub(crate) fn authorize(&self, kind: &str, id: &str) -> Result<(), McpError> {
        let allowed = match kind {
            "device" => &self.devices,
            "package" => &self.packages,
            "plugin" => &self.plugins,
            "runner" => &self.runners,
            "run" => &self.runs,
            "task" => &self.tasks,
            "media" => &self.media,
            _ => return Err(McpError::forbidden("Unknown object kind")),
        };
        if !id.is_empty() && (allowed.contains(id) || allowed.contains("*")) {
            Ok(())
        } else {
            Err(McpError::forbidden(format!(
                "{kind} is outside the credential's allowed objects"
            )))
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ExternalPrincipal {
    pub token_id: String,
    pub label: String,
    pub scopes: BTreeSet<String>,
    pub allowed: AllowedObjects,
    pub expires_at: i64,
}
impl ExternalPrincipal {
    pub(crate) fn has_scope(&self, scope: &str) -> bool {
        self.scopes.contains("*") || self.scopes.contains(scope)
    }
    pub(crate) fn has_scopes(&self, required: &BTreeSet<String>) -> bool {
        self.scopes.contains("*") || required.is_subset(&self.scopes)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct TokenMetadata {
    pub token_id: String,
    pub label: String,
    pub scopes: BTreeSet<String>,
    pub allowed: AllowedObjects,
    pub created_at: i64,
    pub expires_at: i64,
    pub revoked: bool,
    /// Missing only in pre-0.5.9 credentials. False preserves explicit restrictions.
    #[serde(default)]
    pub full_access: Option<bool>,
}
impl TokenMetadata {
    fn is_full_access(&self) -> bool {
        self.full_access.unwrap_or_else(|| {
            // The old settings page always emitted exactly these object ranges
            // and control plus the current catalog. No secret/file rewrite is needed.
            self.allowed.is_unrestricted() && self.scopes.contains("control")
        })
    }
    fn principal(&self, now: i64) -> Result<ExternalPrincipal, McpError> {
        if self.revoked || (self.expires_at != 0 && self.expires_at <= now) {
            return Err(McpError::unauthorized());
        }
        let mut scopes = self.scopes.clone();
        if self.is_full_access() {
            scopes.insert("*".into());
        }
        Ok(ExternalPrincipal {
            token_id: self.token_id.clone(),
            label: self.label.clone(),
            scopes,
            allowed: self.allowed.clone(),
            expires_at: self.expires_at,
        })
    }
}

fn default_lifetime() -> u64 {
    86_400
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateTokenRequest {
    pub label: String,
    pub scopes: BTreeSet<String>,
    #[serde(default)]
    pub allowed: AllowedObjects,
    #[serde(default = "default_lifetime")]
    pub expires_in_seconds: u64,
}

/// Secrets are intentionally not Debug or included in metadata/list responses.
#[derive(Serialize)]
pub(crate) struct CreatedToken {
    pub token: String,
    pub metadata: TokenMetadata,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredToken {
    metadata: TokenMetadata,
    digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    encrypted_token: Option<String>,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredCredentials {
    version: u32,
    tokens: BTreeMap<String, StoredToken>,
}

pub(crate) struct CredentialStore {
    path: PathBuf,
    tokens: Mutex<BTreeMap<String, StoredToken>>,
}
impl CredentialStore {
    pub(crate) fn open(mcp_directory: &Path) -> Result<Self, McpError> {
        std::fs::create_dir_all(mcp_directory)
            .map_err(|_| McpError::internal("Cannot create MCP credential directory"))?;
        let path = mcp_directory.join("tokens.json");
        let tokens = match std::fs::read(&path) {
            Ok(bytes) => {
                if bytes.len() > MAX_CREDENTIAL_BYTES {
                    return Err(McpError::internal(
                        "MCP credential file exceeds its size limit",
                    ));
                }
                let stored: StoredCredentials = serde_json::from_slice(&bytes)
                    .map_err(|_| McpError::internal("MCP credential file is invalid"))?;
                if stored.version != 1 {
                    return Err(McpError::internal("Unsupported MCP credential format"));
                }
                for (id, token) in &stored.tokens {
                    if id != &token.metadata.token_id
                        || token.digest.len() != 64
                        || !token.digest.bytes().all(|b| b.is_ascii_hexdigit())
                    {
                        return Err(McpError::internal(
                            "MCP credential identity or digest is invalid",
                        ));
                    }
                }
                stored.tokens
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(_) => return Err(McpError::internal("Cannot read MCP credentials")),
        };
        Ok(Self {
            path,
            tokens: Mutex::new(tokens),
        })
    }
    pub(crate) fn create(&self, request: CreateTokenRequest) -> Result<CreatedToken, McpError> {
        self.create_at(request, chrono::Utc::now().timestamp())
    }
    fn create_at(&self, request: CreateTokenRequest, now: i64) -> Result<CreatedToken, McpError> {
        if request.label.trim().is_empty()
            || request.label.chars().count() > 80
            || request.label.chars().any(char::is_control)
        {
            return Err(McpError::invalid(
                "Credential label must contain 1 to 80 printable characters",
            ));
        }
        if request.scopes.is_empty()
            || request.scopes.len() > 128
            || request.scopes.iter().any(|s| {
                s != "*"
                    && (s.is_empty()
                        || s.len() > 128
                        || !s.bytes().all(|b| {
                            b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b':')
                        }))
            })
        {
            return Err(McpError::invalid("Invalid credential scopes"));
        }
        for values in [
            &request.allowed.devices,
            &request.allowed.packages,
            &request.allowed.plugins,
            &request.allowed.runners,
            &request.allowed.runs,
            &request.allowed.tasks,
            &request.allowed.media,
        ] {
            if values.len() > 256
                || values
                    .iter()
                    .any(|s| s.is_empty() || s.len() > 1024 || s.chars().any(char::is_control))
            {
                return Err(McpError::invalid("Invalid credential object allowlist"));
            }
        }
        if request.expires_in_seconds > 366 * 86_400 {
            return Err(McpError::invalid(
                "Credential lifetime must be zero (no expiry) or at most 366 days",
            ));
        }
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let metadata = TokenMetadata {
            token_id: uuid::Uuid::new_v4().to_string(),
            label: request.label.trim().into(),
            full_access: Some(request.scopes.contains("*")),
            scopes: request.scopes,
            allowed: request.allowed,
            created_at: now,
            expires_at: if request.expires_in_seconds == 0 {
                0
            } else {
                now.saturating_add(request.expires_in_seconds as i64)
            },
            revoked: false,
        };
        let stored = StoredToken {
            metadata: metadata.clone(),
            digest: digest(&token),
            encrypted_token: Some(
                base64::engine::general_purpose::STANDARD.encode(
                    crate::core::secrets::protect_file(&self.path, token.as_bytes(), true)
                        .map_err(|_| McpError::internal("Cannot protect MCP credential"))?,
                ),
            ),
        };
        let mut tokens = self.tokens.lock().unwrap();
        if tokens.len() >= 1024 {
            return Err(McpError::new(
                ErrorKind::Conflict,
                "credential_limit",
                "MCP credential limit reached",
            ));
        }
        let mut next = tokens.clone();
        next.insert(metadata.token_id.clone(), stored);
        self.persist(&next)?;
        *tokens = next;
        Ok(CreatedToken { token, metadata })
    }
    pub(crate) fn authenticate(&self, secret: &str) -> Result<ExternalPrincipal, McpError> {
        self.authenticate_at(secret, chrono::Utc::now().timestamp())
    }
    fn authenticate_at(&self, secret: &str, now: i64) -> Result<ExternalPrincipal, McpError> {
        if secret.len() != 64 || !secret.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(McpError::unauthorized());
        }
        let candidate = digest(secret);
        let tokens = self.tokens.lock().unwrap();
        // Compare digests without stopping at the first differing byte. No
        // secret or digest is exposed in errors.
        let found = tokens
            .values()
            .find(|token| constant_time_equal(token.digest.as_bytes(), candidate.as_bytes()));
        found
            .ok_or_else(McpError::unauthorized)?
            .metadata
            .principal(now)
    }
    pub(crate) fn list(&self) -> Vec<TokenMetadata> {
        self.tokens
            .lock()
            .unwrap()
            .values()
            .map(|s| s.metadata.clone())
            .collect()
    }
    pub(crate) fn by_token_id(&self, id: &str) -> Option<TokenMetadata> {
        self.tokens
            .lock()
            .unwrap()
            .get(id)
            .map(|s| s.metadata.clone())
    }
    pub(crate) fn can_reveal(&self, id: &str) -> bool {
        self.tokens
            .lock()
            .unwrap()
            .get(id)
            .is_some_and(|stored| !stored.metadata.revoked && stored.encrypted_token.is_some())
    }
    pub(crate) fn reveal(&self, id: &str) -> Result<String, McpError> {
        let tokens = self.tokens.lock().unwrap();
        let stored = tokens
            .get(id)
            .ok_or_else(|| McpError::new(ErrorKind::NotFound, "token_not_found", "令牌不存在"))?;
        if stored.metadata.revoked {
            return Err(McpError::unauthorized());
        }
        let encrypted = stored.encrypted_token.as_ref().ok_or_else(|| {
            McpError::new(
                ErrorKind::Conflict,
                "token_not_recoverable",
                "旧令牌仅保存了哈希，无法查看原文；请创建新令牌",
            )
        })?;
        let encrypted = base64::engine::general_purpose::STANDARD
            .decode(encrypted)
            .map_err(|_| McpError::internal("Cannot read protected MCP credential"))?;
        let plain =
            crate::core::secrets::protect_file(&self.path, &encrypted, false).map_err(|_| {
                McpError::internal("Cannot decrypt MCP credential; restore its original key")
            })?;
        let token = String::from_utf8(plain)
            .map_err(|_| McpError::internal("Invalid protected MCP credential"))?;
        if !constant_time_equal(digest(&token).as_bytes(), stored.digest.as_bytes()) {
            return Err(McpError::internal(
                "Protected MCP credential does not match its digest",
            ));
        }
        Ok(token)
    }
    pub(crate) fn principal_for_id(&self, id: &str) -> Result<ExternalPrincipal, McpError> {
        self.by_token_id(id)
            .ok_or_else(McpError::unauthorized)?
            .principal(chrono::Utc::now().timestamp())
    }
    pub(crate) fn is_active(&self, id: &str) -> bool {
        self.principal_for_id(id).is_ok()
    }
    pub(crate) fn revoke(&self, id: &str) -> Result<bool, McpError> {
        let mut tokens = self.tokens.lock().unwrap();
        let Some(existing) = tokens.get(id) else {
            return Ok(false);
        };
        if existing.metadata.revoked {
            return Ok(true);
        }
        let mut next = tokens.clone();
        next.get_mut(id).unwrap().metadata.revoked = true;
        self.persist(&next)?;
        *tokens = next;
        Ok(true)
    }
    /// Called only after revocation and cancellation of token-owned temporary work.
    pub(crate) fn remove_revoked(&self, id: &str) -> Result<(), McpError> {
        let mut tokens = self.tokens.lock().unwrap();
        let Some(stored) = tokens.get(id) else {
            return Ok(());
        };
        if !stored.metadata.revoked {
            return Err(McpError::internal(
                "Revoke MCP credential before deleting it",
            ));
        }
        let mut next = tokens.clone();
        next.remove(id);
        self.persist(&next)?;
        *tokens = next;
        Ok(())
    }
    fn persist(&self, tokens: &BTreeMap<String, StoredToken>) -> Result<(), McpError> {
        let bytes = serde_json::to_vec_pretty(&StoredCredentials {
            version: 1,
            tokens: tokens.clone(),
        })
        .map_err(|_| McpError::internal("Cannot serialize MCP credentials"))?;
        if bytes.len() > MAX_CREDENTIAL_BYTES {
            return Err(McpError::new(
                ErrorKind::Conflict,
                "credential_limit",
                "MCP credential store exceeds its size limit",
            ));
        }
        crate::core::fs::atomic_write(&self.path, &bytes)
            .map_err(|_| McpError::internal("Cannot persist MCP credentials"))
    }
}
fn digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
fn constant_time_equal(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b)
        .fold(0u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn request() -> CreateTokenRequest {
        CreateTokenRequest {
            label: "test".into(),
            scopes: ["resource.read".into()].into(),
            allowed: AllowedObjects {
                packages: ["a".into()].into(),
                ..Default::default()
            },
            expires_in_seconds: 100,
        }
    }
    #[test]
    fn default_tokens_cover_future_scopes_and_legacy_defaults_keep_the_same_secret() {
        let root = tempfile::tempdir().unwrap();
        let store = CredentialStore::open(root.path()).unwrap();
        let all = ["*".into()].into();
        let ranges = AllowedObjects {
            devices: all,
            packages: ["*".into()].into(),
            plugins: ["*".into()].into(),
            runners: ["*".into()].into(),
            runs: ["*".into()].into(),
            tasks: ["*".into()].into(),
            media: ["*".into()].into(),
        };
        let mut req = request();
        req.scopes = ["*".into()].into();
        req.allowed = ranges.clone();
        req.expires_in_seconds = 0;
        let full = store.create(req.clone()).unwrap();
        assert!(store
            .authenticate(&full.token)
            .unwrap()
            .has_scope("future.plugin.publish"));
        req.scopes = ["control".into(), "observe".into()].into();
        let legacy = store.create(req.clone()).unwrap();
        assert!(!store
            .authenticate(&legacy.token)
            .unwrap()
            .has_scope("future.plugin.publish"));
        // Only absence of the new marker identifies the old settings format.
        let path = root.path().join("tokens.json");
        let mut doc: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        doc["tokens"][&legacy.metadata.token_id]["metadata"]
            .as_object_mut()
            .unwrap()
            .remove("full_access");
        let original = serde_json::to_vec(&doc).unwrap();
        std::fs::write(&path, &original).unwrap();
        let reopened = CredentialStore::open(root.path()).unwrap();
        let p = reopened.authenticate(&legacy.token).unwrap();
        assert!(p.has_scope("plugin.gamer-yaml.publish"));
        assert!(p.has_scope("plugin.update"));
        assert_eq!(
            reopened.reveal(&legacy.metadata.token_id).unwrap(),
            legacy.token
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            original,
            "Authentication must not rewrite old credential bytes"
        );
        reopened.revoke(&legacy.metadata.token_id).unwrap();
        assert!(reopened.authenticate(&legacy.token).is_err());
        assert!(reopened.authenticate(&full.token).is_ok());
    }

    #[test]
    fn legacy_limited_ranges_and_scopes_do_not_become_full_access() {
        let root = tempfile::tempdir().unwrap();
        let store = CredentialStore::open(root.path()).unwrap();
        let mut req = request();
        req.scopes.insert("control".into());
        let limited = store.create(req).unwrap();
        let path = root.path().join("tokens.json");
        let mut doc: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        doc["tokens"][&limited.metadata.token_id]["metadata"]
            .as_object_mut()
            .unwrap()
            .remove("full_access");
        std::fs::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
        let reopened = CredentialStore::open(root.path()).unwrap();
        let p = reopened.authenticate(&limited.token).unwrap();
        assert!(!p.has_scope("plugin.update"));
        assert!(p.allowed.authorize("package", "b").is_err());
    }

    #[test]
    fn secrets_are_encrypted_and_scope_is_not_inferred_from_empty_allowlists() {
        let root = tempfile::tempdir().unwrap();
        let store = CredentialStore::open(root.path()).unwrap();
        let created = store.create(request()).unwrap();
        let bytes = std::fs::read_to_string(root.path().join("tokens.json")).unwrap();
        assert!(!bytes.contains(&created.token));
        let reopened = CredentialStore::open(root.path()).unwrap();
        assert_eq!(
            reopened.reveal(&created.metadata.token_id).unwrap(),
            created.token
        );
        assert!(!serde_json::to_string(&reopened.list())
            .unwrap()
            .contains(&created.token));
        let principal = reopened.authenticate(&created.token).unwrap();
        assert!(principal.allowed.authorize("package", "a").is_ok());
        assert!(principal.allowed.authorize("package", "b").is_err());
        assert!(principal.allowed.authorize("device", "a").is_err());
        reopened.revoke(&principal.token_id).unwrap();
        assert!(reopened.authenticate(&created.token).is_err());
        assert!(reopened.by_token_id(&principal.token_id).unwrap().revoked);
        assert!(reopened.reveal(&principal.token_id).is_err());
        reopened.remove_revoked(&principal.token_id).unwrap();
        assert!(reopened.list().is_empty());
        assert!(CredentialStore::open(root.path())
            .unwrap()
            .list()
            .is_empty());
    }
    #[test]
    fn zero_lifetime_never_expires_and_legacy_hashes_remain_usable() {
        let root = tempfile::tempdir().unwrap();
        let store = CredentialStore::open(root.path()).unwrap();
        let mut request = request();
        request.expires_in_seconds = 0;
        let created = store.create_at(request, 10).unwrap();
        assert_eq!(created.metadata.expires_at, 0);
        assert!(store.authenticate_at(&created.token, i64::MAX).is_ok());
        // Simulate a pre-simplification store: the original digest still works,
        // but no plaintext can be recovered from an old one-way hash.
        let path = root.path().join("tokens.json");
        let mut persisted: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        persisted["tokens"][&created.metadata.token_id]
            .as_object_mut()
            .unwrap()
            .remove("encrypted_token");
        std::fs::write(&path, serde_json::to_vec(&persisted).unwrap()).unwrap();
        let reopened = CredentialStore::open(root.path()).unwrap();
        assert!(reopened.authenticate(&created.token).is_ok());
        assert!(!reopened.can_reveal(&created.metadata.token_id));
        assert_eq!(
            reopened
                .reveal(&created.metadata.token_id)
                .unwrap_err()
                .code,
            "token_not_recoverable"
        );
    }
    #[test]
    fn absolute_expiry_is_checked_without_sleep_and_failed_writes_do_not_activate_tokens() {
        let root = tempfile::tempdir().unwrap();
        let store = CredentialStore::open(root.path()).unwrap();
        let created = store.create_at(request(), 10).unwrap();
        assert!(store.authenticate_at(&created.token, 109).is_ok());
        assert!(store.authenticate_at(&created.token, 110).is_err());
        let broken = root.path().join("file");
        std::fs::write(&broken, b"x").unwrap();
        let store = CredentialStore {
            path: broken.join("tokens.json"),
            tokens: Mutex::new(BTreeMap::new()),
        };
        assert!(store.create(request()).is_err());
        assert!(store.list().is_empty());
    }
}
