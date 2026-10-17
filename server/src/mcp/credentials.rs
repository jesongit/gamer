//! Independent MCP credentials. Only one-way digests are persisted.
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Mutex,
};

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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct TokenMetadata {
    pub token_id: String,
    pub label: String,
    pub scopes: BTreeSet<String>,
    pub allowed: AllowedObjects,
    pub created_at: i64,
    pub expires_at: i64,
    pub revoked: bool,
}
impl TokenMetadata {
    fn principal(&self, now: i64) -> Result<ExternalPrincipal, McpError> {
        if self.revoked || self.expires_at <= now {
            return Err(McpError::unauthorized());
        }
        Ok(ExternalPrincipal {
            token_id: self.token_id.clone(),
            label: self.label.clone(),
            scopes: self.scopes.clone(),
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

/// The secret is intentionally not Debug and is returned only at creation.
#[derive(Serialize)]
pub(crate) struct CreatedToken {
    pub token: String,
    pub metadata: TokenMetadata,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredToken {
    metadata: TokenMetadata,
    digest: String,
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
                s.is_empty()
                    || s.len() > 128
                    || !s.bytes().all(|b| {
                        b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b':')
                    })
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
        if request.expires_in_seconds == 0 || request.expires_in_seconds > 366 * 86_400 {
            return Err(McpError::invalid(
                "Credential lifetime must be between one second and 366 days",
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
            scopes: request.scopes,
            allowed: request.allowed,
            created_at: now,
            expires_at: now.saturating_add(request.expires_in_seconds as i64),
            revoked: false,
        };
        let stored = StoredToken {
            metadata: metadata.clone(),
            digest: digest(&token),
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
    fn secrets_are_shown_once_and_scope_is_not_inferred_from_empty_allowlists() {
        let root = tempfile::tempdir().unwrap();
        let store = CredentialStore::open(root.path()).unwrap();
        let created = store.create(request()).unwrap();
        let bytes = std::fs::read_to_string(root.path().join("tokens.json")).unwrap();
        assert!(!bytes.contains(&created.token));
        let reopened = CredentialStore::open(root.path()).unwrap();
        let principal = reopened.authenticate(&created.token).unwrap();
        assert!(principal.allowed.authorize("package", "a").is_ok());
        assert!(principal.allowed.authorize("package", "b").is_err());
        assert!(principal.allowed.authorize("device", "a").is_err());
        reopened.revoke(&principal.token_id).unwrap();
        assert!(reopened.authenticate(&created.token).is_err());
        assert!(reopened.by_token_id(&principal.token_id).unwrap().revoked);
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
