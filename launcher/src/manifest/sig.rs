//! 签名文件解析、信任库与 Ed25519 验签（release/contracts/manifest-v1.md §3）。
//!
//! `.sig` 恰好两行（UTF-8，`\n`，容忍结尾空行/`\r\n`）：
//!   行1: `gamebot-manifest-sig-1 <key_id>`
//!   行2: 规范 base64（解码后恰为 64 字节 Ed25519 签名）
//! `key_id` 位于签名文件头（manifest 内无 key_id 字段）；launcher 按 key_id 在
//! 内置信任库（当前 + 下一把，本批以 `<keys-dir>/<key_id>.pem` 目录承载）查找公钥，
//! 未知 key_id 直接拒绝。Ed25519 签名覆盖 manifest 文件原始字节。

use std::fs;
use std::path::Path;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};

pub const SIG_MAGIC: &str = "gamebot-manifest-sig-1";

/// 解析后的分离签名。
#[derive(Debug, Clone)]
pub struct ParsedSignature {
    pub key_id: String,
    pub signature: [u8; 64],
}

/// 解析 .sig 文件字节；任何偏离冻结格式都返回 `Err(detail)`（错误码 sig-format-invalid）。
pub fn parse_signature_file(buf: &[u8]) -> Result<ParsedSignature, String> {
    let text = std::str::from_utf8(buf).map_err(|e| format!("签名文件不是 UTF-8: {e}"))?;
    let mut lines: Vec<&str> = text
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    if lines.len() != 2 {
        return Err(format!(
            "签名文件必须恰好 2 个非空行，实际 {} 行",
            lines.len()
        ));
    }
    // split_whitespace 本身容忍首尾空白（等价 node 实现里的 trim + split /\s+/）
    let head: Vec<&str> = lines[0].split_whitespace().collect();
    if head.len() != 2 || head[0] != SIG_MAGIC {
        return Err(format!("签名头必须为 \"{SIG_MAGIC} <key_id>\""));
    }
    let key_id = head[1];
    if !is_valid_key_id(key_id) {
        return Err(format!("key_id 非法: {key_id:?}"));
    }
    let b64 = lines[1].trim();
    if !is_canonical_b64(b64) {
        return Err(
            "签名第 2 行不是规范 base64（[A-Za-z0-9+/]+={0,2}，长度 4 的倍数）".to_string(),
        );
    }
    let decoded = B64
        .decode(b64)
        .map_err(|e| format!("base64 解码失败: {e}"))?;
    if decoded.len() != 64 {
        return Err(format!(
            "签名解码后必须为 64 字节，实际 {} 字节",
            decoded.len()
        ));
    }
    let mut signature = [0u8; 64];
    signature.copy_from_slice(&decoded);
    Ok(ParsedSignature {
        key_id: key_id.to_string(),
        signature,
    })
}

/// key_id 字符集 `[A-Za-z0-9][A-Za-z0-9._-]{0,63}`（禁 `/` `\` `..`，防路径穿越）。
pub fn is_valid_key_id(key_id: &str) -> bool {
    let b = key_id.as_bytes();
    if b.is_empty() || b.len() > 64 || !b[0].is_ascii_alphanumeric() {
        return false;
    }
    b[1..]
        .iter()
        .all(|&c| c.is_ascii_alphanumeric() || c == b'.' || c == b'_' || c == b'-')
}

fn is_canonical_b64(s: &str) -> bool {
    let b = s.as_bytes();
    if b.is_empty() || !b.len().is_multiple_of(4) {
        return false;
    }
    let content = b.iter().take_while(|&&c| c != b'=').count();
    let pads = b.len() - content;
    if content == 0 || pads > 2 {
        return false;
    }
    b[..content]
        .iter()
        .all(|&c| c.is_ascii_alphanumeric() || c == b'+' || c == b'/')
}

/// 可信公钥库（当前 + 下一把）。加载 `<dir>/<key_id>.pem`（SPKI PEM）。
pub struct TrustStore {
    entries: Vec<(String, Option<VerifyingKey>)>,
}

impl TrustStore {
    pub fn from_dir(dir: &Path) -> std::io::Result<TrustStore> {
        let mut entries = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let ext_ok = path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("pem"));
            if !ext_ok {
                continue;
            }
            let key_id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            let pem = fs::read_to_string(&path)?;
            let key = parse_public_key_pem(&pem);
            tracing::debug!(key_id = %key_id, parseable = key.is_some(), "加载信任库公钥");
            entries.push((key_id, key));
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(TrustStore { entries })
    }

    pub fn empty() -> TrustStore {
        TrustStore {
            entries: Vec::new(),
        }
    }

    /// 未知 key_id 或该 key 的 PEM 不可解析 → None（fail closed，按未知 key 拒绝）。
    pub fn get(&self, key_id: &str) -> Option<&VerifyingKey> {
        self.entries
            .iter()
            .find(|(k, _)| k == key_id)
            .and_then(|(_, v)| v.as_ref())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// 解析 SPKI PEM 为 Ed25519 公钥。fixture/生产公钥统一为 44 字节 DER：
/// `30 .. 30 05 06 03 2b 65 70 03 21 00 <32 字节原始公钥>`。
pub fn parse_public_key_pem(pem: &str) -> Option<VerifyingKey> {
    const BEGIN: &str = "-----BEGIN PUBLIC KEY-----";
    const END: &str = "-----END PUBLIC KEY-----";
    let start = pem.find(BEGIN)? + BEGIN.len();
    let rest = &pem[start..];
    let end = rest.find(END)?;
    let b64: String = rest[..end].chars().filter(|c| !c.is_whitespace()).collect();
    let der = B64.decode(b64.as_bytes()).ok()?;
    extract_ed25519_key(&der)
}

fn extract_ed25519_key(der: &[u8]) -> Option<VerifyingKey> {
    if der.first() != Some(&0x30) {
        return None;
    }
    let mut i = 0;
    while i + 3 + 32 <= der.len() {
        // BIT STRING (0x03)，长度 0x21，unused bits 0x00，随后 32 字节公钥
        if der[i] == 0x03 && der[i + 1] == 0x21 && der[i + 2] == 0x00 {
            let mut key = [0u8; 32];
            key.copy_from_slice(&der[i + 3..i + 3 + 32]);
            return VerifyingKey::from_bytes(&key).ok();
        }
        i += 1;
    }
    None
}

/// Ed25519 验签：覆盖 manifest 文件原始字节。
pub fn verify_signature(message: &[u8], key: &VerifyingKey, signature: &[u8; 64]) -> bool {
    key.verify(message, &Signature::from_bytes(signature))
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("release")
            .join("contracts")
            .join("fixtures")
    }

    fn b64(bytes: &[u8]) -> String {
        B64.encode(bytes)
    }

    #[test]
    fn fixture_key_verifies_valid_manifest() {
        let pem =
            fs::read_to_string(fixtures().join("keys").join("test-ed25519-public-1.pem")).unwrap();
        let key = parse_public_key_pem(&pem).expect("fixture 公钥应可解析");
        let raw = fs::read(
            fixtures()
                .join("manifest")
                .join("valid")
                .join("manifest-valid-basic.json"),
        )
        .unwrap();
        let sig = fs::read(
            fixtures()
                .join("manifest")
                .join("valid")
                .join("manifest-valid-basic.sig"),
        )
        .unwrap();
        let parsed = parse_signature_file(&sig).expect("fixture 签名应可解析");
        assert_eq!(parsed.key_id, "test-ed25519-public-1");
        assert!(verify_signature(&raw, &key, &parsed.signature));
    }

    #[test]
    fn tampered_bytes_fail_verification() {
        let pem =
            fs::read_to_string(fixtures().join("keys").join("test-ed25519-public-1.pem")).unwrap();
        let key = parse_public_key_pem(&pem).unwrap();
        let mut raw = fs::read(
            fixtures()
                .join("manifest")
                .join("valid")
                .join("manifest-valid-basic.json"),
        )
        .unwrap();
        let sig = fs::read(
            fixtures()
                .join("manifest")
                .join("valid")
                .join("manifest-valid-basic.sig"),
        )
        .unwrap();
        let parsed = parse_signature_file(&sig).unwrap();
        // 翻转第一个字母数字字节
        for byte in raw.iter_mut() {
            if byte.is_ascii_alphanumeric() {
                *byte = if *byte == b'a' { b'b' } else { b'a' };
                break;
            }
        }
        assert!(!verify_signature(&raw, &key, &parsed.signature));
    }

    #[test]
    fn parses_crlf_and_trailing_newline() {
        let sig_bytes = [64u8; 64];
        let text = format!(
            "gamebot-manifest-sig-1 key-1\r\n{}\r\n\r\n",
            b64(&sig_bytes)
        );
        let parsed = parse_signature_file(text.as_bytes()).expect("CRLF + 结尾空行应容忍");
        assert_eq!(parsed.key_id, "key-1");
    }

    #[test]
    fn rejects_bad_signature_formats() {
        // 不是签名
        assert!(parse_signature_file(b"this is not a signature").is_err());
        // key_id 含路径穿越字符
        assert!(parse_signature_file(b"gamebot-manifest-sig-1 ../evil\nAAAA\n").is_err());
        // key_id 以非字母数字开头
        assert!(parse_signature_file(b"gamebot-manifest-sig-1 _k\nAAAA\n").is_err());
        // base64 非法字符
        assert!(parse_signature_file(b"gamebot-manifest-sig-1 k1\n****\n").is_err());
        // base64 长度不是 4 的倍数
        assert!(parse_signature_file(b"gamebot-manifest-sig-1 k1\nAAAAA\n").is_err());
        // 内容不足（"AA==" 解码只有 1 字节）
        assert!(parse_signature_file(b"gamebot-manifest-sig-1 k1\nAA==\n").is_err());
        // 多余行
        assert!(parse_signature_file(b"gamebot-manifest-sig-1 k1\nAAAA\nextra\n").is_err());
        // 解码后不是 64 字节
        assert!(parse_signature_file(b"gamebot-manifest-sig-1 k1\nAAAA\n").is_err());
        let short = b64(&[0x41; 63]);
        assert!(
            parse_signature_file(format!("gamebot-manifest-sig-1 k1\n{short}\n").as_bytes())
                .is_err()
        );
    }

    #[test]
    fn trust_store_lookups() {
        let dir = fixtures().join("keys");
        let store = TrustStore::from_dir(&dir).expect("信任库目录应可读");
        assert!(!store.is_empty());
        assert!(store.get("test-ed25519-public-1").is_some());
        assert!(store.get("missing-key").is_none());
        assert!(TrustStore::empty().is_empty());
    }
}
