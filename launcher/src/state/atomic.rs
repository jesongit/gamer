//! 原子 JSON 读写：同目录临时文件 + 落盘 flush + rename 原子替换（UPDATE_CONTRACT §5.1）。
//! 半截/损坏 JSON 的恢复策略：备份到 <名>.corrupt-<unix-ms> 并按空状态处理。

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use serde::Serialize;

/// JSON 载入结果。
#[derive(Debug)]
pub enum LoadOutcome<T> {
    Present(T),
    Missing,
    /// 文件存在但半截/损坏；原文件已被移到 backup_path。
    Corrupted {
        backup_path: PathBuf,
    },
}

pub fn now_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 读 JSON；损坏（含空文件、半截 JSON）时把原文件改名备份后返回 `Corrupted`。
pub fn load_json_recover<T: DeserializeOwned>(path: &Path) -> io::Result<LoadOutcome<T>> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(LoadOutcome::Missing),
        Err(e) => return Err(e),
    };
    match serde_json::from_slice::<T>(&bytes) {
        Ok(v) => Ok(LoadOutcome::Present(v)),
        // 解析失败一律视为损坏：先备份再按空状态处理，绝不在原路径上复用半截内容。
        Err(_) => {
            let backup_path = backup_to_corrupt(path)?;
            Ok(LoadOutcome::Corrupted { backup_path })
        }
    }
}

/// 把损坏文件改名到 <名>.corrupt-<unix-ms>，返回备份路径。
pub fn backup_to_corrupt(path: &Path) -> io::Result<PathBuf> {
    let name = path
        .file_name()
        .map(|s| s.to_os_string())
        .unwrap_or_else(|| "state.json".into());
    let mut backup_name = name;
    backup_name.push(format!(".corrupt-{}", now_unix_millis()));
    let backup_path = path.with_file_name(backup_name);
    rename_with_retry(path, &backup_path)?;
    Ok(backup_path)
}

/// 原子写：临时文件写全 + sync_all + 同目录 rename 覆盖。
/// Windows 上 rename 走 MOVEFILE_REPLACE_EXISTING；被杀毒等短暂占用时有界重试（契约 §5.2）。
pub fn write_json_atomic<T: Serialize + ?Sized>(path: &Path, value: &T) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let tmp = temp_path(path);
    let result = (|| -> io::Result<()> {
        let mut bytes = serde_json::to_vec_pretty(value)?;
        bytes.push(b'\n');
        let mut file = fs::File::create(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        Ok(())
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    match rename_with_retry(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

fn temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|s| s.to_os_string())
        .unwrap_or_else(|| "state.json".into());
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(&name);
    tmp_name.push(format!(".tmp-{}-{}", std::process::id(), now_unix_millis()));
    path.with_file_name(tmp_name)
}

/// rename 有界重试：仅针对 ERROR_ACCESS_DENIED(5) / ERROR_SHARING_VIOLATION(32) /
/// ERROR_LOCK_VIOLATION(33) 的短暂占用；重试耗尽返回最后一个错误。
pub fn rename_with_retry(from: &Path, to: &Path) -> io::Result<()> {
    let mut last_err: Option<io::Error> = None;
    for attempt in 0..10u32 {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) => {
                let retryable = matches!(e.raw_os_error(), Some(5 | 32 | 33))
                    || e.kind() == io::ErrorKind::PermissionDenied;
                if !retryable || attempt == 9 {
                    return Err(e);
                }
                last_err = Some(e);
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }
    Err(last_err.unwrap_or_else(|| io::Error::other("rename 重试耗尽")))
}
