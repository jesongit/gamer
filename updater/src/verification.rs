//! 增量 SHA-256 校验记录。缓存损坏/丢失自动全检；手动深检仍直接调用 digest。
use crate::{
    digest::{to_hex, verify_file, VerifyFileError},
    layout::InstallLayout,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Stamp {
    size: u64,
    modified: u128,
    identity: (u32, u64),
}
fn stamp(path: &Path) -> std::io::Result<Stamp> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };
    let file = fs::File::open(path)?;
    let meta = file.metadata()?;
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(Stamp {
        size: meta.len(),
        modified: meta
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        identity: (
            info.dwVolumeSerialNumber,
            (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        ),
    })
}
#[derive(Serialize, Deserialize)]
struct Record {
    path: String,
    hash: String,
    stamp: Stamp,
}

/// 返回 true 表示复用了成功记录，false 表示本次实际计算了 SHA-256。
pub fn verify(
    layout: &InstallLayout,
    path: &Path,
    hash: &str,
    size: u64,
) -> Result<bool, VerifyFileError> {
    let before = stamp(path).map_err(VerifyFileError::Io)?;
    if before.size != size {
        return Err(VerifyFileError::SizeMismatch {
            actual: before.size,
            expected: size,
        });
    }
    let path_key = path.to_string_lossy().into_owned();
    let hash = hash.to_ascii_lowercase();
    // 同卷原子移动不会改变文件身份。以身份寻址缓存，staging 中已经校验的文件
    // 安装到 versions/ 后直接复用；同路径替换文件则因身份变化重新校验。
    let key = to_hex(&Sha256::digest(format!(
        "{}:{}\n{hash}\n{size}",
        before.identity.0, before.identity.1
    )));
    let receipt = layout
        .state_dir()
        .join("verified")
        .join(format!("{key}.json"));
    let cached: Option<Record> = fs::read(&receipt)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok());
    if cached.is_some_and(|r| r.hash == hash && r.stamp == before) {
        return Ok(true);
    }
    verify_file(path, &hash, size)?;
    let after = stamp(path).map_err(VerifyFileError::Io)?;
    if before != after {
        return Err(VerifyFileError::Io(std::io::Error::other(
            "文件在校验过程中发生变化",
        )));
    }
    // 记录失败只损失加速能力，不将成功校验变成安装失败。
    if let Err(e) = crate::state::atomic::write_json_atomic(
        &receipt,
        &Record {
            path: path_key,
            hash,
            stamp: after,
        },
    ) {
        tracing::warn!(%e, "无法保存校验缓存");
    }
    Ok(false)
}
