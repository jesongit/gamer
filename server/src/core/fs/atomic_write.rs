use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(windows)]
use std::sync::{Mutex, OnceLock};

/// Write a file through a same-directory temporary file and an atomic replace.
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    atomic_write_with(path, bytes, replace_file)
}

fn atomic_write_with(
    path: &Path,
    bytes: &[u8],
    replace: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("目标路径没有父目录: {}", path.display()))?;
    std::fs::create_dir_all(parent)?;

    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("目标文件名无效: {}", path.display()))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let mut temp = None;
    let mut file = None;
    for attempt in 0..16u32 {
        let candidate = parent.join(format!(
            ".{name}.tmp-{}-{nonce}-{attempt}",
            std::process::id()
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(f) => {
                temp = Some(candidate);
                file = Some(f);
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    let temp = temp.ok_or_else(|| anyhow::anyhow!("无法创建临时文件: {}", path.display()))?;
    let mut file = file.expect("临时文件句柄必须与路径同时创建");
    let result = (|| -> anyhow::Result<()> {
        std::io::Write::write_all(&mut file, bytes)?;
        std::io::Write::flush(&mut file)?;
        file.sync_all()?;
        drop(file);
        #[cfg(windows)]
        let _guard = replace_lock().lock().unwrap();
        replace(&temp, path)?;
        sync_parent(parent)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

#[cfg(not(windows))]
fn replace_file(temp: &Path, path: &Path) -> std::io::Result<()> {
    std::fs::rename(temp, path)
}

#[cfg(windows)]
fn replace_file(temp: &Path, path: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
    }

    // Rust file IO accepts long paths, but this direct Win32 call needs the
    // verbatim absolute prefix. The target may not exist yet, so resolve its
    // existing parent rather than canonicalizing the target itself.
    let from_path = std::fs::canonicalize(temp)?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "replacement target has no filename",
        )
    })?;
    let to_path = std::fs::canonicalize(parent)?.join(name);
    let from: Vec<u16> = from_path.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to_path.as_os_str().encode_wide().chain(Some(0)).collect();
    // MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH
    for attempt in 0..8 {
        let ok = unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0x1 | 0x8) };
        if ok != 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        // Windows readers / indexers may briefly hold the old target. Never
        // delete it first: failed replacement must preserve its complete bytes.
        if attempt == 7 || !matches!(error.raw_os_error(), Some(5 | 32 | 33)) {
            return Err(error);
        }
        std::thread::sleep(std::time::Duration::from_millis(5 * (attempt + 1)));
    }
    unreachable!()
}

#[cfg(unix)]
fn sync_parent(parent: &Path) -> std::io::Result<()> {
    std::fs::File::open(parent)?.sync_all()
}

#[cfg(windows)]
fn replace_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[cfg(not(unix))]
fn sync_parent(_parent: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(all(test, windows))]
mod long_path_tests {
    use super::*;
    use std::os::windows::ffi::OsStrExt;

    #[test]
    fn atomic_replacement_supports_long_unicode_history_paths() {
        let root = tempfile::tempdir().unwrap();
        let path = root
            .path()
            .join("版本历史".repeat(32))
            .join("保存回退".repeat(32))
            .join("真实模板".repeat(32))
            .join("template.png");
        assert!(path.as_os_str().encode_wide().count() > 260);
        atomic_write(&path, b"original pixels").unwrap();
        atomic_write(&path, b"replacement pixels").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"replacement pixels");
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
    }
}
