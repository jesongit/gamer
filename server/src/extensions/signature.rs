//! Signature and official-registry verification for `.gplugin` packages.
//!
//! A package carries a detached `signature.sig` (or the compatible
//! `manifest.toml.sig`) whose Ed25519 signature covers the exact raw
//! `manifest.toml` bytes. Official registry installs additionally carry a
//! signed, per-version registry claim binding id/version/download URL and the
//! archive SHA-256. The server verifies both; browser status text is never a
//! trust decision.

use std::fs;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zip::ZipArchive;

use super::error::{ExtensionError, ExtensionResult};
use super::manifest::MANIFEST_FILE_NAME;
use super::model::{ExtensionId, ExtensionVersion};

const SIG_MAGIC: &str = "gamebot-gplugin-sig-1";
const SIGNATURE_FILE: &str = "signature.sig";
const LEGACY_SIGNATURE_FILE: &str = "manifest.toml.sig";
const REGISTRY_CLAIM_MAGIC: &str = "gamebot-gplugin-registry-entry-1";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SignatureStatus {
    Valid,
    Unsigned,
    Invalid,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct SignatureInfo {
    pub(crate) status: SignatureStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) key_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) algorithm: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) detail: Option<String>,
}

impl SignatureInfo {
    fn unsigned() -> Self {
        Self {
            status: SignatureStatus::Unsigned,
            key_id: None,
            algorithm: None,
            detail: None,
        }
    }

    fn invalid(detail: impl Into<String>, key_id: Option<String>) -> Self {
        Self {
            status: SignatureStatus::Invalid,
            key_id,
            algorithm: Some("ed25519".to_string()),
            detail: Some(detail.into()),
        }
    }

    fn valid(key_id: String) -> Self {
        Self {
            status: SignatureStatus::Valid,
            key_id: Some(key_id),
            algorithm: Some("ed25519".to_string()),
            detail: None,
        }
    }

    pub(crate) fn unknown(detail: impl Into<String>) -> Self {
        Self {
            status: SignatureStatus::Unknown,
            key_id: None,
            algorithm: None,
            detail: Some(detail.into()),
        }
    }
}

#[derive(Clone, Debug)]
struct ParsedSignature {
    key_id: String,
    signature: [u8; 64],
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct RegistryProof {
    pub(crate) id: String,
    pub(crate) version: String,
    pub(crate) download_url: String,
    pub(crate) sha256: String,
    pub(crate) key_id: String,
    pub(crate) signature: String,
}

impl RegistryProof {
    pub(crate) fn from_base64(value: &str) -> ExtensionResult<Self> {
        let bytes = B64.decode(value.trim()).map_err(|error| {
            ExtensionError::InvalidRegistryProof(format!("proof base64 无效: {error}"))
        })?;
        serde_json::from_slice(&bytes)
            .map_err(|error| ExtensionError::InvalidRegistryProof(error.to_string()))
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct TrustStore {
    entries: Vec<(String, Option<VerifyingKey>)>,
}

impl TrustStore {
    pub(crate) fn from_dir(dir: &Path) -> std::io::Result<Self> {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default())
            }
            Err(error) => return Err(error),
        };
        let mut trusted = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if !path.is_file()
                || !path
                    .extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.eq_ignore_ascii_case("pem"))
            {
                continue;
            }
            let key_id = path
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or_default()
                .to_string();
            let key = fs::read_to_string(&path)
                .ok()
                .and_then(|pem| parse_public_key_pem(&pem));
            trusted.push((key_id, key));
        }
        trusted.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(Self { entries: trusted })
    }

    #[cfg(test)]
    fn from_keys(entries: Vec<(String, VerifyingKey)>) -> Self {
        Self {
            entries: entries
                .into_iter()
                .map(|(id, key)| (id, Some(key)))
                .collect(),
        }
    }

    fn get(&self, key_id: &str) -> Option<&VerifyingKey> {
        self.entries
            .iter()
            .find(|(id, _)| id == key_id)
            .and_then(|(_, key)| key.as_ref())
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SignatureVerifier {
    trust_store: TrustStore,
}

impl SignatureVerifier {
    pub(crate) fn from_data_root(data_root: impl AsRef<Path>) -> Self {
        let trust_dir = std::env::var_os("GAMER_PLUGIN_TRUST_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| data_root.as_ref().join("plugin-trust"));
        let trust_store = TrustStore::from_dir(&trust_dir).unwrap_or_else(|error| {
            tracing::warn!(path = %trust_dir.display(), %error, "插件信任库不可读，官方包将被拒绝");
            TrustStore::default()
        });
        Self { trust_store }
    }

    #[cfg(test)]
    fn with_trust_store(trust_store: TrustStore) -> Self {
        Self { trust_store }
    }

    pub(crate) fn verify_archive(&self, bytes: &[u8]) -> ExtensionResult<SignatureInfo> {
        let mut archive = ZipArchive::new(std::io::Cursor::new(bytes))?;
        let mut manifest = Vec::new();
        archive
            .by_name(MANIFEST_FILE_NAME)?
            .read_to_end(&mut manifest)
            .map_err(ExtensionError::Io)?;
        let signature_name = [SIGNATURE_FILE, LEGACY_SIGNATURE_FILE]
            .into_iter()
            .find(|name| archive.by_name(name).is_ok());
        let Some(signature_name) = signature_name else {
            return Ok(SignatureInfo::unsigned());
        };
        let mut signature = Vec::new();
        archive
            .by_name(signature_name)?
            .read_to_end(&mut signature)
            .map_err(ExtensionError::Io)?;
        let parsed = parse_signature(&signature).map_err(ExtensionError::InvalidSignature)?;
        let Some(key) = self.trust_store.get(&parsed.key_id) else {
            return Err(ExtensionError::InvalidSignature(format!(
                "未知 key_id: {}",
                parsed.key_id
            )));
        };
        if key
            .verify(&manifest, &Signature::from_bytes(&parsed.signature))
            .is_err()
        {
            return Err(ExtensionError::InvalidSignature(format!(
                "manifest.toml 验签失败 (key_id={})",
                parsed.key_id
            )));
        }
        Ok(SignatureInfo::valid(parsed.key_id))
    }

    pub(crate) fn verify_installed(&self, root: &Path, manifest_path: &Path) -> SignatureInfo {
        let manifest = match fs::read(manifest_path) {
            Ok(bytes) => bytes,
            Err(error) => return SignatureInfo::unknown(error.to_string()),
        };
        let signature_path = [root.join(SIGNATURE_FILE), root.join(LEGACY_SIGNATURE_FILE)]
            .into_iter()
            .find(|path| path.is_file());
        let Some(signature_path) = signature_path else {
            return SignatureInfo::unsigned();
        };
        let signature = match fs::read(signature_path) {
            Ok(bytes) => bytes,
            Err(error) => return SignatureInfo::invalid(error.to_string(), None),
        };
        let parsed = match parse_signature(&signature) {
            Ok(value) => value,
            Err(error) => return SignatureInfo::invalid(error, None),
        };
        let Some(key) = self.trust_store.get(&parsed.key_id) else {
            return SignatureInfo::invalid(
                format!("未知 key_id: {}", parsed.key_id),
                Some(parsed.key_id),
            );
        };
        if key
            .verify(&manifest, &Signature::from_bytes(&parsed.signature))
            .is_err()
        {
            return SignatureInfo::invalid("manifest.toml 验签失败", Some(parsed.key_id));
        }
        SignatureInfo::valid(parsed.key_id)
    }

    pub(crate) fn verify_registry_proof(
        &self,
        proof: &RegistryProof,
        id: &ExtensionId,
        version: &ExtensionVersion,
        archive: &[u8],
    ) -> ExtensionResult<()> {
        if proof.id != id.as_str() || proof.version != version.as_str() {
            return Err(ExtensionError::InvalidRegistryProof(
                "proof 的插件 id/version 与归档不一致".into(),
            ));
        }
        if !proof.download_url.starts_with("https://") {
            return Err(ExtensionError::InvalidRegistryProof(
                "官方 download_url 必须是 https://".into(),
            ));
        }
        if !is_sha256(&proof.sha256) {
            return Err(ExtensionError::InvalidRegistryProof(
                "proof.sha256 必须是小写 64 位 hex".into(),
            ));
        }
        let actual = format!("{:x}", Sha256::digest(archive));
        if actual != proof.sha256 {
            return Err(ExtensionError::InvalidRegistryProof(
                "归档 SHA-256 与 Registry proof 不一致".into(),
            ));
        }
        let parsed = parse_keyed_base64(&proof.key_id, &proof.signature)
            .map_err(ExtensionError::InvalidRegistryProof)?;
        let Some(key) = self.trust_store.get(&parsed.key_id) else {
            return Err(ExtensionError::InvalidRegistryProof(format!(
                "未知 Registry key_id: {}",
                parsed.key_id
            )));
        };
        let claim = registry_claim(proof);
        key.verify(&claim, &Signature::from_bytes(&parsed.signature))
            .map_err(|_| ExtensionError::InvalidRegistryProof("Registry proof 验签失败".into()))
    }
}

fn parse_signature(bytes: &[u8]) -> Result<ParsedSignature, String> {
    let text = std::str::from_utf8(bytes).map_err(|error| format!("签名不是 UTF-8: {error}"))?;
    let mut lines = text
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect::<Vec<_>>();
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    if lines.len() != 2 {
        return Err("签名必须恰好两行".into());
    }
    let header = lines[0].split_whitespace().collect::<Vec<_>>();
    if header.len() != 2 || header[0] != SIG_MAGIC || !valid_key_id(header[1]) {
        return Err(format!("签名头无效，应为 {SIG_MAGIC} <key_id>"));
    }
    let decoded = decode_signature_base64(lines[1].trim())?;
    let mut signature = [0u8; 64];
    signature.copy_from_slice(&decoded);
    Ok(ParsedSignature {
        key_id: header[1].to_string(),
        signature,
    })
}

fn parse_keyed_base64(key_id: &str, value: &str) -> Result<ParsedSignature, String> {
    if !valid_key_id(key_id) {
        return Err("key_id 无效".into());
    }
    let decoded = decode_signature_base64(value)?;
    let mut signature = [0u8; 64];
    signature.copy_from_slice(&decoded);
    Ok(ParsedSignature {
        key_id: key_id.to_string(),
        signature,
    })
}

fn decode_signature_base64(value: &str) -> Result<Vec<u8>, String> {
    let bytes = value.as_bytes();
    if bytes.is_empty()
        || !bytes.len().is_multiple_of(4)
        || !bytes
            .iter()
            .take_while(|byte| **byte != b'=')
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'+' || *byte == b'/')
    {
        return Err("签名不是规范 base64".into());
    }
    let decoded = B64.decode(bytes).map_err(|error| error.to_string())?;
    if decoded.len() != 64 {
        return Err(format!("签名必须解码为 64 字节，实际 {}", decoded.len()));
    }
    Ok(decoded)
}

fn valid_key_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'.' | b'_' | b'-'))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn registry_claim(proof: &RegistryProof) -> Vec<u8> {
    format!(
        "{REGISTRY_CLAIM_MAGIC}\nid={}\nversion={}\ndownload_url={}\nsha256={}\n",
        proof.id, proof.version, proof.download_url, proof.sha256
    )
    .into_bytes()
}

fn parse_public_key_pem(pem: &str) -> Option<VerifyingKey> {
    const BEGIN: &str = "-----BEGIN PUBLIC KEY-----";
    const END: &str = "-----END PUBLIC KEY-----";
    let start = pem.find(BEGIN)? + BEGIN.len();
    let rest = &pem[start..];
    let end = rest.find(END)?;
    let body = rest[..end]
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    let der = B64.decode(body.as_bytes()).ok()?;
    let marker = der
        .windows(3)
        .position(|bytes| bytes == [0x03, 0x21, 0x00])?;
    let key_end = marker.checked_add(3 + 32)?;
    if key_end > der.len() {
        return None;
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&der[marker + 3..key_end]);
    VerifyingKey::from_bytes(&key).ok()
}

use std::io::Read;

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn package(signing: &SigningKey, manifest: &[u8]) -> Vec<u8> {
        package_with_signed_manifest(signing, manifest, manifest)
    }

    fn package_with_signed_manifest(
        signing: &SigningKey,
        manifest: &[u8],
        signed_manifest: &[u8],
    ) -> Vec<u8> {
        let signature = signing.sign(signed_manifest);
        let sig = format!(
            "{SIG_MAGIC} official-test-1\n{}\n",
            B64.encode(signature.to_bytes())
        );
        let mut bytes = Vec::new();
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
        let options = SimpleFileOptions::default();
        writer.start_file(MANIFEST_FILE_NAME, options).unwrap();
        writer.write_all(manifest).unwrap();
        writer.start_file("plugin.wasm", options).unwrap();
        writer.write_all(b"\0asm\x01\0\0\0").unwrap();
        writer.start_file(SIGNATURE_FILE, options).unwrap();
        writer.write_all(sig.as_bytes()).unwrap();
        writer.finish().unwrap();
        bytes
    }

    fn verifier(signing: &SigningKey) -> SignatureVerifier {
        SignatureVerifier::with_trust_store(TrustStore::from_keys(vec![(
            "official-test-1".into(),
            signing.verifying_key(),
        )]))
    }

    #[test]
    fn verifies_gplugin_manifest_signature_and_rejects_tampering() {
        let signing = SigningKey::from_bytes(&[7u8; 32]);
        let manifest = b"manifest_version = 1\nid = \"demo.plugin\"\nversion = \"1.0.0\"\nname = \"Demo\"\nentry = \"plugin.wasm\"\n";
        let bytes = package(&signing, manifest);
        let verifier = verifier(&signing);
        assert_eq!(
            verifier.verify_archive(&bytes).unwrap().status,
            SignatureStatus::Valid
        );

        let tampered_manifest = manifest
            .iter()
            .map(|byte| if *byte == b'D' { b'E' } else { *byte })
            .collect::<Vec<_>>();
        let tampered = package_with_signed_manifest(&signing, &tampered_manifest, manifest);
        assert!(matches!(
            verifier.verify_archive(&tampered),
            Err(ExtensionError::InvalidSignature(_))
        ));
    }

    #[test]
    fn registry_proof_binds_archive_hash_and_version() {
        let signing = SigningKey::from_bytes(&[9u8; 32]);
        let verifier = verifier(&signing);
        let manifest = b"manifest_version = 1\nid = \"demo.plugin\"\nversion = \"1.0.0\"\nname = \"Demo\"\nentry = \"plugin.wasm\"\n";
        let archive = package(&signing, manifest);
        let mut proof = RegistryProof {
            id: "demo.plugin".into(),
            version: "1.0.0".into(),
            download_url: "https://registry.example/demo.gplugin".into(),
            sha256: format!("{:x}", Sha256::digest(&archive)),
            key_id: "official-test-1".into(),
            signature: String::new(),
        };
        proof.signature = B64.encode(signing.sign(&registry_claim(&proof)).to_bytes());
        verifier
            .verify_registry_proof(
                &proof,
                &ExtensionId::parse("demo.plugin").unwrap(),
                &ExtensionVersion::parse("1.0.0").unwrap(),
                &archive,
            )
            .unwrap();
        proof.sha256.replace_range(0..1, "0");
        assert!(matches!(
            verifier.verify_registry_proof(
                &proof,
                &ExtensionId::parse("demo.plugin").unwrap(),
                &ExtensionVersion::parse("1.0.0").unwrap(),
                &archive,
            ),
            Err(ExtensionError::InvalidRegistryProof(_))
        ));
    }
}
