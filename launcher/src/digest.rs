//! SHA-256 摘要与逐文件校验（inventory / fetch / archive 共用）。

use std::fs;
use std::io::{self, Read};
use std::path::Path;

use sha2::{Digest, Sha256};

const READ_BUF: usize = 64 * 1024;
const HEX: &[u8; 16] = b"0123456789abcdef";

/// 小写 hex 编码。
pub fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

/// 流式计算文件 sha256（64 KiB 缓冲），返回小写 hex。
pub fn sha256_file_hex(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; READ_BUF];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(to_hex(&hasher.finalize()))
}

/// 逐文件校验结果（调用方负责映射为可读错误）。
#[derive(Debug)]
pub enum VerifyFileError {
    NotFound,
    SizeMismatch { actual: u64, expected: u64 },
    HashMismatch { actual: String, expected: String },
    Io(io::Error),
}

impl std::fmt::Display for VerifyFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VerifyFileError::NotFound => write!(f, "文件不存在"),
            VerifyFileError::SizeMismatch { actual, expected } => {
                write!(f, "size 不符（实际 {actual}，声明 {expected}）")
            }
            VerifyFileError::HashMismatch { actual, expected } => {
                write!(f, "sha256 不符（实际 {actual}，声明 {expected}）")
            }
            VerifyFileError::Io(e) => write!(f, "读取失败: {e}"),
        }
    }
}

/// 校验单个文件：存在 + size 一致 + sha256 一致（期望 hash 必须为小写 hex）。
pub fn verify_file(
    path: &Path,
    expected_sha256: &str,
    expected_size: u64,
) -> Result<(), VerifyFileError> {
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(VerifyFileError::NotFound),
        Err(e) => return Err(VerifyFileError::Io(e)),
    };
    if !meta.is_file() {
        return Err(VerifyFileError::NotFound);
    }
    let actual = meta.len();
    if actual != expected_size {
        return Err(VerifyFileError::SizeMismatch {
            actual,
            expected: expected_size,
        });
    }
    let actual = sha256_file_hex(path).map_err(VerifyFileError::Io)?;
    if actual != expected_sha256.to_ascii_lowercase() {
        return Err(VerifyFileError::HashMismatch {
            actual,
            expected: expected_sha256.to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_encoding_is_lowercase() {
        assert_eq!(to_hex(&[]), "");
        assert_eq!(to_hex(&[0x0f, 0xa0]), "0fa0");
    }

    #[test]
    fn verify_file_checks_size_and_hash() {
        let dir = std::env::temp_dir().join(format!("gamer-digest-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sample.bin");
        fs::write(&path, b"hello world").unwrap();
        // sha256("hello world") = b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9
        let good = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        assert!(verify_file(&path, good, 11).is_ok());
        // 期望 hash 用大写同样应匹配（内部归一化小写）
        assert!(verify_file(&path, &good.to_ascii_uppercase(), 11).is_ok());
        match verify_file(&path, good, 12) {
            Err(VerifyFileError::SizeMismatch {
                actual: 11,
                expected: 12,
            }) => {}
            other => panic!("应报 size 不符，实际 {other:?}"),
        }
        match verify_file(&path, &"0".repeat(64), 11) {
            Err(VerifyFileError::HashMismatch { actual, .. }) => assert_eq!(actual, good),
            other => panic!("应报 hash 不符，实际 {other:?}"),
        }
        assert!(matches!(
            verify_file(&dir.join("missing.bin"), good, 11),
            Err(VerifyFileError::NotFound)
        ));
        let _ = fs::remove_dir_all(&dir);
    }
}
