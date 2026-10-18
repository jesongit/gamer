//! Local credentials: existing Windows DPAPI plus portable authenticated envelopes.
use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use anyhow::{ensure, Context, Result};
use rand::RngCore;
use std::{
    io::Write,
    path::{Path, PathBuf},
};

const MAGIC: &[u8; 8] = b"GMRSEC01";

/// A configured installation key makes newly written credentials portable on every OS.
/// Without one, Windows retains DPAPI; Unix uses a private sibling key. Old files
/// are never rewritten on read. Keys must travel with the encrypted data at restore.
pub(crate) fn protect_file(path: &Path, bytes: &[u8], encrypt: bool) -> Result<Vec<u8>> {
    let configured = std::env::var_os("GAMER_SECRETS_KEY_FILE").map(PathBuf::from);
    #[cfg(windows)]
    if configured.is_none() && (encrypt || !bytes.starts_with(MAGIC)) {
        return windows_protect(bytes, encrypt);
    }
    let key_path = configured.unwrap_or_else(|| path.with_file_name(".credentials.key"));
    portable(&key_path, bytes, encrypt)
}

fn key(path: &Path, create: bool) -> Result<[u8; 32]> {
    if create && !path.exists() {
        let parent = path.parent().context("凭据密钥路径无效")?;
        std::fs::create_dir_all(parent)?;
        // Publish a complete private key atomically, so concurrent readers cannot
        // observe an empty/partially written key. Never replace an existing key.
        let mut file = tempfile::NamedTempFile::new_in(parent).context("无法创建凭据密钥")?;
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        file.write_all(&bytes)?;
        file.as_file().sync_all()?;
        match file.persist_noclobber(path) {
            Ok(_) => {}
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.error).context("无法创建凭据密钥"),
        }
    }
    let metadata = std::fs::symlink_metadata(path).context("凭据密钥缺失；请恢复原始密钥")?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "凭据密钥必须是普通文件"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        ensure!(
            metadata.permissions().mode() & 0o077 == 0,
            "凭据密钥权限过宽，须为 0600"
        );
    }
    std::fs::read(path)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("凭据密钥长度无效，拒绝替换"))
}

fn portable(path: &Path, bytes: &[u8], encrypt: bool) -> Result<Vec<u8>> {
    ensure!(
        encrypt || (bytes.starts_with(MAGIC) && bytes.len() >= 36),
        "凭据格式不可跨系统读取，请先使用显式迁移工具转换"
    );
    let secret = key(path, encrypt)?;
    let cipher = Aes256Gcm::new_from_slice(&secret).map_err(|_| anyhow::anyhow!("凭据密钥无效"))?;
    if encrypt {
        let mut nonce = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce);
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: bytes,
                    aad: MAGIC,
                },
            )
            .map_err(|_| anyhow::anyhow!("凭据加密失败"))?;
        let mut result = MAGIC.to_vec();
        result.extend_from_slice(&nonce);
        result.extend_from_slice(&ciphertext);
        Ok(result)
    } else {
        cipher
            .decrypt(
                Nonce::from_slice(&bytes[8..20]),
                Payload {
                    msg: &bytes[20..],
                    aad: MAGIC,
                },
            )
            .map_err(|_| anyhow::anyhow!("凭据解密失败，请检查原始密钥和文件完整性"))
    }
}

#[cfg(windows)]
fn windows_protect(bytes: &[u8], encrypt: bool) -> Result<Vec<u8>> {
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
        },
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len().try_into()?,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // DPAPI is scoped to the current Windows account; never prompts or uses machine-wide protection.
    let ok = unsafe {
        if encrypt {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    ensure!(ok != 0, "Windows 凭据保护操作失败");
    let result = if output.cbData == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() }
    };
    unsafe {
        LocalFree(output.pbData as _);
    }
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn portable_envelope_survives_reopen_and_rejects_corruption_and_wrong_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key");
        let plain = b"private connection fixture";
        let encrypted = portable(&path, plain, true).unwrap();
        assert!(encrypted.starts_with(MAGIC));
        assert!(!encrypted.windows(plain.len()).any(|part| part == plain));
        assert_eq!(portable(&path, &encrypted, false).unwrap(), plain);
        let mut damaged = encrypted.clone();
        *damaged.last_mut().unwrap() ^= 1;
        assert!(portable(&path, &damaged, false).is_err());
        let other = dir.path().join("other-key");
        portable(&other, b"fixture", true).unwrap();
        assert!(portable(&other, &encrypted, false).is_err());
        assert!(portable(&path, b"old plaintext", false).is_err());
        assert!(portable(&dir.path().join("missing"), &encrypted, false).is_err());
    }
}
