//! LCH-002：安装根单实例写锁（state/launcher.lock）。
//!
//! Windows 实现：CreateFile 独占打开（dwShareMode=0，`OpenOptionsExt::share_mode(0)`），
//! 排他性由句柄保证，持有进程退出（含被杀）即自动释放；锁文件本身是遗留物，
//! 无持有者时可被下一次 acquire 接管。`status` 永不取锁，只做只读探测。

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use super::LOCK_FILE;

/// ERROR_SHARING_VIOLATION / ERROR_LOCK_VIOLATION
const ERRORS_CONTENTION: [i32; 2] = [32, 33];

#[derive(Debug)]
pub enum LockError {
    /// 另一个 launcher 实例持有锁（status 可只读继续，写动作必须拒绝）。
    Held {
        path: PathBuf,
    },
    Io(io::Error),
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockError::Held { path } => {
                write!(
                    f,
                    "安装根已被另一个 launcher 实例持有（{}）",
                    path.display()
                )
            }
            LockError::Io(e) => write!(f, "锁文件操作失败: {e}"),
        }
    }
}

/// 单实例锁句柄；Drop 时先关句柄再尽力删除锁文件（删除失败无碍正确性）。
pub struct InstanceLock {
    file: Option<File>,
    path: PathBuf,
}

impl InstanceLock {
    /// 取锁：先尝试全新创建（CREATEx 首次安装），已存在（崩溃遗留）则尝试独占打开接管；
    /// 打开遇 sharing/lock violation → 另一实例在持有，返回 `LockError::Held`。
    pub fn acquire(state_dir: &Path) -> Result<InstanceLock, LockError> {
        fs::create_dir_all(state_dir).map_err(LockError::Io)?;
        let path = state_dir.join(LOCK_FILE);
        let create_err = match open_exclusive_create(&path) {
            Ok(file) => return Ok(Self::locked(file, path)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => None,
            Err(e) => Some(e),
        };
        match open_exclusive_existing(&path) {
            Ok(file) => Ok(Self::locked(file, path)),
            Err(e) if is_contention(&e) => Err(LockError::Held { path }),
            // 两个错误都不是占用：优先回报创建阶段的原始错误。
            Err(e) => Err(match create_err {
                Some(c) => LockError::Io(c),
                None => LockError::Io(e),
            }),
        }
    }

    fn locked(mut file: File, path: PathBuf) -> InstanceLock {
        // 写入持有者信息便于人工诊断；失败不影响锁的排他性。
        let _ = writeln!(
            file,
            "pid={} acquired_at_unix_ms={}",
            std::process::id(),
            super::atomic::now_unix_millis()
        );
        let _ = file.flush();
        InstanceLock {
            file: Some(file),
            path,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 只读探测：锁当前是否被某个实例持有（不取锁、不改动文件）。
    pub fn is_locked(path: &Path) -> bool {
        match File::open(path) {
            // 能以共享方式打开 → 无独占持有者（可能是崩溃遗留的空锁文件）。
            Ok(_) => false,
            Err(e) => is_contention(&e),
        }
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        // 先关句柄再删文件，否则自己持有的句柄会挡住删除。
        if let Some(file) = self.file.take() {
            drop(file);
        }
        let _ = fs::remove_file(&self.path);
    }
}

fn open_exclusive_create(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .open(path)
}

fn open_exclusive_existing(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create(false)
        .share_mode(0)
        .open(path)
}

fn is_contention(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(code) if ERRORS_CONTENTION.contains(&code))
}
