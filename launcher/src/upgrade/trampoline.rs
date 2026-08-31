//! LCH-013：launcher 自更新 trampoline。
//!
//! 运行中的 Windows executable 不能可靠地覆盖自身，因此更新拆成两阶段：
//! 当前 launcher 只负责把候选复制到同卷临时目录并启动自身的 helper；helper
//! 等父进程退出后再原子替换目标。替换前不删除、不改名旧文件，替换失败时旧
//! launcher 仍保持原样可启动。

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::supervisor;

pub const TRAMPOLINE_MODE_ENV: &str = "GAMER_LAUNCHER_TRAMPOLINE";
pub const TRAMPOLINE_CURRENT_ENV: &str = "GAMER_LAUNCHER_TRAMPOLINE_CURRENT";
pub const TRAMPOLINE_STAGED_ENV: &str = "GAMER_LAUNCHER_TRAMPOLINE_STAGED";
pub const TRAMPOLINE_TEMP_ENV: &str = "GAMER_LAUNCHER_TRAMPOLINE_TEMP";
pub const TRAMPOLINE_PARENT_PID_ENV: &str = "GAMER_LAUNCHER_TRAMPOLINE_PARENT_PID";
pub const TRAMPOLINE_WAIT_MS_ENV: &str = "GAMER_LAUNCHER_TRAMPOLINE_WAIT_MS";

const MAX_REPLACE_ATTEMPTS: u32 = 10;
const RETRY_DELAY: Duration = Duration::from_millis(25);
const DEFAULT_PARENT_WAIT: Duration = Duration::from_secs(30);

/// 一次 launcher 自更新请求。`temp_parent` 只是临时目录的父目录；默认使用
/// current launcher 的父目录，保证 staging 与目标位于同一卷。
#[derive(Debug, Clone)]
pub struct LauncherUpdateRequest {
    pub current: PathBuf,
    pub candidate: PathBuf,
    pub temp_parent: Option<PathBuf>,
    pub parent_wait: Duration,
}

impl LauncherUpdateRequest {
    pub fn new(current: impl Into<PathBuf>, candidate: impl Into<PathBuf>) -> Self {
        Self {
            current: current.into(),
            candidate: candidate.into(),
            temp_parent: None,
            parent_wait: DEFAULT_PARENT_WAIT,
        }
    }

    #[cfg(test)]
    fn with_temp_parent(mut self, parent: PathBuf) -> Self {
        self.temp_parent = Some(parent);
        self
    }
}

#[derive(Debug)]
pub enum TrampolineError {
    Io(io::Error),
    Invalid(String),
}

impl std::fmt::Display for TrampolineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::Invalid(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for TrampolineError {}

impl From<io::Error> for TrampolineError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

#[derive(Debug)]
struct StagedLauncher {
    temp_dir: PathBuf,
    staged: PathBuf,
    cleanup: bool,
}

impl Drop for StagedLauncher {
    fn drop(&mut self) {
        if self.cleanup {
            if let Err(e) = fs::remove_dir_all(&self.temp_dir) {
                if e.kind() != io::ErrorKind::NotFound {
                    tracing::warn!(
                        path = %self.temp_dir.display(),
                        error = %e,
                        "清理 launcher trampoline 临时目录失败"
                    );
                }
            }
        }
    }
}

/// 把候选 launcher 复制到同卷临时目录。此函数不触碰 current。
fn stage_candidate(request: &LauncherUpdateRequest) -> Result<StagedLauncher, TrampolineError> {
    ensure_regular_file(&request.current, "当前 launcher")?;
    ensure_regular_file(&request.candidate, "候选 launcher")?;
    if same_path(&request.current, &request.candidate) {
        return Err(TrampolineError::Invalid(
            "当前 launcher 与候选 launcher 不能是同一路径".to_string(),
        ));
    }
    let current_parent = request.current.parent().ok_or_else(|| {
        TrampolineError::Invalid("当前 launcher 缺少父目录，无法建立同卷临时目录".to_string())
    })?;
    let temp_parent = request.temp_parent.as_deref().unwrap_or(current_parent);
    fs::create_dir_all(temp_parent)?;
    let temp_dir = create_unique_dir(temp_parent)?;
    let staged = temp_dir.join(
        request
            .current
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("gamer-launcher.exe")),
    );

    let result = (|| -> io::Result<()> {
        let mut source = File::open(&request.candidate)?;
        let mut target = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged)?;
        io::copy(&mut source, &mut target)?;
        target.flush()?;
        target.sync_all()?;
        // 读取一次以确保测试/mock 也只把完整文件视为 staged 产物；不做版本或
        // PE 解析，候选身份由上游签名 manifest 门禁负责。
        let mut check = File::open(&staged)?;
        let mut byte = [0u8; 1];
        let _ = check.read(&mut byte)?;
        Ok(())
    })();
    if let Err(e) = result {
        let _ = fs::remove_dir_all(&temp_dir);
        return Err(e.into());
    }
    Ok(StagedLauncher {
        temp_dir,
        staged,
        cleanup: true,
    })
}

/// 准备并启动 trampoline helper。成功返回后调用方应尽快退出，让 helper 接管
/// 文件替换；helper 失败不会删除 current。
pub fn schedule(request: &LauncherUpdateRequest) -> Result<Child, TrampolineError> {
    let mut staged = stage_candidate(request)?;
    let env = trampoline_environment(request, &staged);
    let child = supervisor::spawn_trampoline(&request.current, &env)?;
    staged.cleanup = false;
    Ok(child)
}

fn trampoline_environment(
    request: &LauncherUpdateRequest,
    staged: &StagedLauncher,
) -> std::collections::BTreeMap<String, String> {
    let mut env = std::collections::BTreeMap::new();
    env.insert(TRAMPOLINE_MODE_ENV.to_string(), "1".to_string());
    env.insert(
        TRAMPOLINE_CURRENT_ENV.to_string(),
        request.current.to_string_lossy().into_owned(),
    );
    env.insert(
        TRAMPOLINE_STAGED_ENV.to_string(),
        staged.staged.to_string_lossy().into_owned(),
    );
    env.insert(
        TRAMPOLINE_TEMP_ENV.to_string(),
        staged.temp_dir.to_string_lossy().into_owned(),
    );
    env.insert(
        TRAMPOLINE_PARENT_PID_ENV.to_string(),
        std::process::id().to_string(),
    );
    env.insert(
        TRAMPOLINE_WAIT_MS_ENV.to_string(),
        request
            .parent_wait
            .as_millis()
            .min(u128::from(u32::MAX))
            .to_string(),
    );
    env
}

/// 是否为 launcher 自身拉起的 helper invocation。
pub fn is_requested() -> bool {
    std::env::var(TRAMPOLINE_MODE_ENV).ok().as_deref() == Some("1")
}

/// 由 [`crate::commands::dispatch`] 调用的 helper 入口。
pub fn run_from_environment() -> Result<(), TrampolineError> {
    if !is_requested() {
        return Err(TrampolineError::Invalid(
            "不是 launcher trampoline invocation".to_string(),
        ));
    }
    let current = required_env_path(TRAMPOLINE_CURRENT_ENV)?;
    let staged = required_env_path(TRAMPOLINE_STAGED_ENV)?;
    let temp_dir = required_env_path(TRAMPOLINE_TEMP_ENV)?;
    let parent_pid = required_env_u32(TRAMPOLINE_PARENT_PID_ENV)?;
    let wait_ms = std::env::var(TRAMPOLINE_WAIT_MS_ENV)
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .map(|milliseconds| Duration::from_millis(u64::from(milliseconds)))
        .unwrap_or(DEFAULT_PARENT_WAIT);
    if !staged.starts_with(&temp_dir) {
        return Err(TrampolineError::Invalid(
            "staged launcher 不在 trampoline 临时目录内".to_string(),
        ));
    }
    if !wait_for_parent_exit(parent_pid, wait_ms) {
        return Err(TrampolineError::Invalid(
            "等待旧 launcher 退出超时；旧 launcher 未被替换".to_string(),
        ));
    }
    apply_staged(&staged, &temp_dir, &current)
}

/// 使用生产替换实现提交 staged launcher。失败时 current 保持不变（原子替换
/// API 的失败前提），并清理 helper 临时目录。
pub fn apply_staged(staged: &Path, temp_dir: &Path, current: &Path) -> Result<(), TrampolineError> {
    apply_staged_with(staged, temp_dir, current, replace_file)
}

/// 测试/故障注入入口：替换器由调用方 mock，仍走与生产一致的有界重试和清理。
pub fn apply_staged_with<F>(
    staged: &Path,
    temp_dir: &Path,
    current: &Path,
    mut replace: F,
) -> Result<(), TrampolineError>
where
    F: FnMut(&Path, &Path) -> io::Result<()>,
{
    ensure_regular_file(staged, "staged launcher")?;
    ensure_regular_file(current, "当前 launcher")?;
    if !staged.starts_with(temp_dir) {
        return Err(TrampolineError::Invalid(
            "staged launcher 不在 trampoline 临时目录内".to_string(),
        ));
    }
    let result = replace_with_retry(staged, current, &mut replace);
    let cleanup = fs::remove_dir_all(temp_dir);
    if let Err(e) = cleanup {
        if e.kind() != io::ErrorKind::NotFound {
            tracing::warn!(path = %temp_dir.display(), error = %e, "清理 trampoline 临时目录失败");
        }
    }
    result.map_err(TrampolineError::Io)
}

fn replace_with_retry<F>(from: &Path, to: &Path, replace: &mut F) -> io::Result<()>
where
    F: FnMut(&Path, &Path) -> io::Result<()>,
{
    let mut last = None;
    for attempt in 0..MAX_REPLACE_ATTEMPTS {
        match replace(from, to) {
            Ok(()) => return Ok(()),
            Err(e) => {
                let retryable = e.kind() == io::ErrorKind::PermissionDenied
                    || matches!(e.raw_os_error(), Some(5 | 32 | 33));
                if !retryable || attempt + 1 == MAX_REPLACE_ATTEMPTS {
                    return Err(e);
                }
                last = Some(e);
                std::thread::sleep(RETRY_DELAY);
            }
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("launcher 替换重试耗尽")))
}

#[cfg(windows)]
fn replace_file(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let from: Vec<u16> = from
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let to: Vec<u16> = to
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let ok = unsafe {
        MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_file(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)
}

fn ensure_regular_file(path: &Path, label: &str) -> Result<(), TrampolineError> {
    let metadata = fs::symlink_metadata(path).map_err(|e| {
        TrampolineError::Io(io::Error::new(
            e.kind(),
            format!("{label} {} 不可用: {e}", path.display()),
        ))
    })?;
    if !metadata.file_type().is_file() {
        return Err(TrampolineError::Invalid(format!(
            "{label} {} 不是普通文件",
            path.display()
        )));
    }
    Ok(())
}

fn same_path(a: &Path, b: &Path) -> bool {
    a == b
        || a.canonicalize()
            .ok()
            .zip(b.canonicalize().ok())
            .is_some_and(|(a, b)| a == b)
}

fn create_unique_dir(parent: &Path) -> io::Result<PathBuf> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    for attempt in 0..100u32 {
        let dir = parent.join(format!(
            ".gamer-launcher-trampoline-{}-{stamp}-{attempt}",
            std::process::id()
        ));
        match fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "无法创建唯一 launcher trampoline 临时目录",
    ))
}

fn required_env_path(name: &str) -> Result<PathBuf, TrampolineError> {
    let value = std::env::var_os(name)
        .ok_or_else(|| TrampolineError::Invalid(format!("trampoline 缺少环境变量 {name}")))?;
    if value.is_empty() {
        return Err(TrampolineError::Invalid(format!(
            "trampoline 环境变量 {name} 为空"
        )));
    }
    Ok(PathBuf::from(value))
}

fn required_env_u32(name: &str) -> Result<u32, TrampolineError> {
    let value = std::env::var(name)
        .map_err(|_| TrampolineError::Invalid(format!("trampoline 缺少环境变量 {name}")))?;
    value
        .parse()
        .map_err(|_| TrampolineError::Invalid(format!("trampoline 环境变量 {name} 不是合法 PID")))
}

#[cfg(windows)]
fn wait_for_parent_exit(pid: u32, timeout: Duration) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject};
    const PROCESS_SYNCHRONIZE: u32 = 0x0010_0000;
    const WAIT_OBJECT_0: u32 = 0;
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        // 父进程可能已经退出，或句柄权限不足；继续尝试原子替换，失败则仍保留旧文件。
        return true;
    }
    let wait_ms = timeout.as_millis().min(u128::from(u32::MAX)) as u32;
    let result = unsafe { WaitForSingleObject(handle, wait_ms) };
    unsafe { CloseHandle(handle) };
    result == WAIT_OBJECT_0
}

#[cfg(not(windows))]
fn wait_for_parent_exit(_pid: u32, timeout: Duration) -> bool {
    // 生产目标为 Windows；非 Windows 仅保留可编译的开发 fallback。
    std::thread::sleep(timeout.min(Duration::from_millis(50)));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(tag: &str) -> Self {
            let parent = std::env::temp_dir().join(format!(
                "gamer-launcher-lch013-{tag}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&parent);
            fs::create_dir_all(&parent).expect("创建测试临时目录");
            Self(parent)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn files(tag: &str) -> (TempRoot, PathBuf, PathBuf) {
        let root = TempRoot::new(tag);
        let current = root.path().join("gamer-launcher.exe");
        let candidate = root.path().join("download").join("gamer-launcher.exe");
        fs::create_dir_all(candidate.parent().unwrap()).expect("创建候选目录");
        fs::write(&current, b"old-launcher").expect("写旧 launcher");
        fs::write(&candidate, b"new-launcher").expect("写候选 launcher");
        (root, current, candidate)
    }

    #[test]
    fn staged_success_replaces_current_and_cleans_temp_directory() {
        let (root, current, candidate) = files("success");
        let request = LauncherUpdateRequest::new(&current, &candidate)
            .with_temp_parent(root.path().join("tmp"));
        let staged = stage_candidate(&request).expect("候选应可复制到临时目录");
        let temp_dir = staged.temp_dir.clone();
        apply_staged_with(&staged.staged, &staged.temp_dir, &current, |from, to| {
            fs::copy(from, to).map(|_| ())
        })
        .expect("mock 替换应成功");
        assert_eq!(fs::read(&current).unwrap(), b"new-launcher");
        assert_eq!(fs::read(&candidate).unwrap(), b"new-launcher");
        assert!(!temp_dir.exists(), "成功后不应残留 trampoline 临时目录");
    }

    #[test]
    fn occupied_retry_can_eventually_replace_without_touching_candidate() {
        let (root, current, candidate) = files("occupied-retry");
        let request = LauncherUpdateRequest::new(&current, &candidate);
        let staged = stage_candidate(&request).expect("候选应可复制到临时目录");
        let mut calls = 0;
        apply_staged_with(&staged.staged, &staged.temp_dir, &current, |from, to| {
            calls += 1;
            if calls < 3 {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "mock occupied",
                ))
            } else {
                fs::copy(from, to).map(|_| ())
            }
        })
        .expect("短暂占用应在有界重试内恢复");
        assert_eq!(calls, 3);
        assert_eq!(fs::read(&current).unwrap(), b"new-launcher");
        assert_eq!(fs::read(&candidate).unwrap(), b"new-launcher");
        drop(root);
    }

    #[test]
    fn replacement_failure_preserves_old_launcher_and_cleans_temp_directory() {
        let (root, current, candidate) = files("failure");
        let request = LauncherUpdateRequest::new(&current, &candidate);
        let staged = stage_candidate(&request).expect("候选应可复制到临时目录");
        let temp_dir = staged.temp_dir.clone();
        let err = apply_staged_with(&staged.staged, &staged.temp_dir, &current, |_from, _to| {
            Err(io::Error::other("mock replacement failure"))
        })
        .expect_err("替换失败应上报");
        assert!(err.to_string().contains("mock replacement failure"));
        assert_eq!(fs::read(&current).unwrap(), b"old-launcher");
        assert_eq!(fs::read(&candidate).unwrap(), b"new-launcher");
        assert!(!temp_dir.exists(), "失败后不应残留 trampoline 临时目录");
        drop(root);
    }

    #[test]
    fn candidate_and_current_must_be_distinct_regular_files() {
        let (root, current, _candidate) = files("invalid");
        let request = LauncherUpdateRequest::new(&current, &current);
        let err = stage_candidate(&request).expect_err("同一路径必须拒绝");
        assert!(err.to_string().contains("不能是同一路径"));
        drop(root);
    }
}
