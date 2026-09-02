/// Content version used by resource conflict checks: SHA-256 truncated to 12 hex digits.
pub(crate) fn content_version(content: &str) -> String {
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(content.as_bytes());
    let mut version = String::with_capacity(12);
    for byte in digest.iter().take(6) {
        version.push_str(&format!("{byte:02x}"));
    }
    version
}
