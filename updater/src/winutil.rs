//! Windows 原生能力的小封装（批次 3）：
//! - `random_bytes`：BCryptGenRandom（IPC 会话令牌 / installation-id 的熵源）
//! - `free_disk_bytes`：GetDiskFreeSpaceExW（升级前空间预估，insufficient_space）
//! - `process_image_path` / `terminate_pid_if_image`：按 journal 记录的准确 PID/exe
//!   探测与终止孤儿候选进程（计划 §6.6：不用进程名/端口猜测，exe 匹配才动手）
//! - `pipe_security_current_user`：SDDL 构造仅含 SYSTEM + 当前用户的 DACL，
//!   供 named pipe 的 SECURITY_ATTRIBUTES 使用（ipc-v1.md §1.2）
//! - `extended_len_path`：绝对路径 → `\\?\` 扩展长度（verbatim）形态。
//!   LongPathsEnabled=0 的主机上 >260 字符路径只有 verbatim 形态可用（不过
//!   Win32 归一化层）；安装根统一经 [`crate::layout::InstallLayout::resolve`]
//!   转成该形态后，std::fs 与 CreateProcessW（lpApplicationName）均可正常工作。

#![allow(unknown_lints)]
#![allow(clippy::unsafe_derive_send_sync)]

use std::ffi::c_void;
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{LocalFree, HANDLE, HLOCAL};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
};
use windows_sys::Win32::Security::Cryptography::BCryptGenRandom;
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenUser, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows_sys::Win32::System::Memory;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW, TerminateProcess,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
};

const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 0x0000_0002;
const SDDL_REVISION_1: u32 = 1;

const BACKSLASH: u16 = b'\\' as u16;
const SLASH: u16 = b'/' as u16;

fn wide_to_path(wide: &[u16]) -> PathBuf {
    PathBuf::from(std::ffi::OsString::from_wide(wide))
}

fn starts_with_device_prefix(wide: &[u16]) -> bool {
    // `\\?\`（verbatim DOS）与 `\\.\`（设备）形态不做二次处理
    wide.len() >= 4
        && wide[0] == BACKSLASH
        && wide[1] == BACKSLASH
        && (wide[2] == b'?' as u16 || wide[2] == b'.' as u16)
        && wide[3] == BACKSLASH
}

/// 绝对路径 → 扩展长度（verbatim）形态：`C:\...` → `\\?\C:\...`、
/// UNC `\\server\share\...` → `\\?\UNC\server\share\...`。
///
/// LongPathsEnabled=0 的主机上，>260 字符的路径只有 verbatim 形态能通过
/// Win32 文件 API 与 CreateProcessW（lpApplicationName 显式给出时）；本函数
/// 同时做词法归一化（`/` → `\`）——verbatim 形态下系统不再做任何运行期
/// 归一化，混入的 `/` 会被当成字面字符。相对路径与已有 `\\?\` / `\\.\`
/// 前缀的输入原样返回（幂等）。
pub fn extended_len_path(path: &Path) -> PathBuf {
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    for ch in wide.iter_mut() {
        if *ch == SLASH {
            *ch = BACKSLASH;
        }
    }
    if starts_with_device_prefix(&wide) {
        return wide_to_path(&wide);
    }
    if wide.len() >= 2 && wide[0] == BACKSLASH && wide[1] == BACKSLASH {
        // UNC：\\server\share\... → \\?\UNC\server\share\...
        let mut out = vec![
            BACKSLASH,
            BACKSLASH,
            b'?' as u16,
            BACKSLASH,
            b'U' as u16,
            b'N' as u16,
            b'C' as u16,
            BACKSLASH,
        ];
        out.extend_from_slice(&wide[2..]);
        return wide_to_path(&out);
    }
    let drive_absolute = wide.len() >= 2
        && wide[1] == b':' as u16
        && (wide[0] >= b'A' as u16 && wide[0] <= b'Z' as u16
            || wide[0] >= b'a' as u16 && wide[0] <= b'z' as u16);
    if !drive_absolute {
        // 相对路径无法安全加前缀（语义依赖 cwd），原样返回
        return path.to_path_buf();
    }
    let mut out = vec![BACKSLASH, BACKSLASH, b'?' as u16, BACKSLASH];
    out.extend_from_slice(&wide);
    // `C:` → `\\?\C:\`（盘相对形态没有意义，补齐根分隔符）
    if out.len() == 6 {
        out.push(BACKSLASH);
    }
    wide_to_path(&out)
}

/// 进程镜像路径比较用的归一化形态：小写 + 去掉 verbatim 前缀
/// （`\\?\UNC\server\share` → `\\server\share`，`\\?\C:\x` → `C:\x`）。
/// spawn 用 verbatim 形态、而 QueryFullProcessImageNameW 的返回形态随系统
/// 而异，逐字节比较会误判「PID 镜像不一致」。
fn normalize_image_path(path: &Path) -> String {
    let lower = path.to_string_lossy().to_ascii_lowercase();
    let back = r"\\";
    if let Some(rest) = lower.strip_prefix(r"\\?\unc\") {
        return format!("{back}{rest}");
    }
    if let Some(rest) = lower.strip_prefix(r"\\?\") {
        return rest.to_string();
    }
    lower
}

/// CreateProcessW 的 lpCurrentDirectory 受 DOS 当前目录长度上限（~260，实测
/// verbatim 形态超限同样报 ERROR_DIRECTORY/267）——长路径安装根下不能把
/// `versions/<v>` 直接交给子进程当 cwd。回退策略：取同一目录树中不超过
/// `max_len` 的最近祖先目录（它是真实存在的目录的前缀，必然存在）；server
/// 的全部可解析路径经 GAMER_*/GB_* 环境变量绝对注入，不依赖 cwd。
pub fn fallback_current_dir(path: &Path, max_len: usize) -> PathBuf {
    let extended = extended_len_path(path);
    let text = extended.to_string_lossy().into_owned();
    if text.chars().count() <= max_len {
        return extended;
    }
    let mut candidate = extended.clone();
    while let Some(parent) = candidate.parent() {
        if parent == candidate {
            break;
        }
        candidate = parent.to_path_buf();
        if candidate.to_string_lossy().chars().count() <= max_len {
            return candidate;
        }
    }
    extended
}

fn to_wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// CSPRNG 随机字节（BCryptGenRandom，系统首选 RNG）；API 失败时返回错误，
/// 调用方不得退化到弱熵（令牌/id 生成宁可不完成）。
pub fn random_bytes(len: usize) -> io::Result<Vec<u8>> {
    let mut buf = vec![0u8; len];
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            buf.as_mut_ptr(),
            u32::try_from(len).unwrap_or(u32::MAX),
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status != 0 {
        return Err(io::Error::other(format!(
            "BCryptGenRandom 失败: 0x{status:08x}"
        )));
    }
    Ok(buf)
}

/// 路径所在卷的可用字节数（当前用户可用额度）。
pub fn free_disk_bytes(path: &Path) -> io::Result<u64> {
    let wide = to_wide(&path.to_string_lossy());
    let mut free: u64 = 0;
    let mut total: u64 = 0;
    let mut total_free: u64 = 0;
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, &mut total, &mut total_free) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(free)
}

fn wide_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// 按 PID 查询进程镜像完整路径（进程不存在/无权限 → None）。
pub fn process_image_path(pid: u32) -> Option<PathBuf> {
    if pid == 0 {
        return None;
    }
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let mut buf = [0u16; 1024];
    let mut size = u32::try_from(buf.len()).unwrap_or(u32::MAX);
    let ok = unsafe {
        QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut size)
    };
    unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) };
    if ok == 0 {
        return None;
    }
    Some(PathBuf::from(wide_to_string(&buf[..size as usize])))
}

/// 终止 PID 指向的进程，但**仅当**其镜像路径与 `expected_exe` 一致（不区分大小写；
/// 防 PID 复用误杀；verbatim 前缀差异经 normalize_image_path 归一后比较）。
/// 返回是否执行了终止。
pub fn terminate_pid_if_image(pid: u32, expected_exe: &Path) -> bool {
    let Some(actual) = process_image_path(pid) else {
        return false;
    };
    let matches = normalize_image_path(&actual) == normalize_image_path(expected_exe);
    if !matches {
        tracing::warn!(
            pid,
            expect = %expected_exe.display(),
            actual = %actual.display(),
            "PID 镜像与 journal 记录不一致，拒绝终止（防 PID 复用误杀）"
        );
        return false;
    }
    let handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
    if handle.is_null() {
        return false;
    }
    let ok = unsafe { TerminateProcess(handle, 1) };
    unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) };
    if ok == 0 {
        tracing::warn!(pid, "TerminateProcess 失败: {}", io::Error::last_os_error());
        return false;
    }
    true
}

/// 当前用户 SID 的字符串形式（S-1-5-…）；失败返回错误。
pub fn current_user_sid_string() -> io::Result<String> {
    let mut token: HANDLE = std::ptr::null_mut();
    let ok = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let result = token_user_sid(token);
    unsafe { windows_sys::Win32::Foundation::CloseHandle(token) };
    result
}

fn token_user_sid(token: HANDLE) -> io::Result<String> {
    // 第一次调用取所需长度
    let mut needed: u32 = 0;
    let ok = unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed) };
    if ok == 0 && needed == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut buf = vec![0u8; needed as usize];
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            buf.as_mut_ptr().cast::<c_void>(),
            needed,
            &mut needed,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let user = unsafe { &*(buf.as_ptr().cast::<TOKEN_USER>()) };
    let mut sid_str: windows_sys::core::PWSTR = std::ptr::null_mut();
    let ok = unsafe { ConvertSidToStringSidW(user.User.Sid, &mut sid_str) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    unsafe {
        let mut len = 0usize;
        while *sid_str.add(len) != 0 {
            len += 1;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(sid_str, len));
        LocalFree(sid_str as HLOCAL);
        Ok(s)
    }
}

/// named pipe 的 SECURITY_ATTRIBUTES（DACL = 仅 SYSTEM + 当前用户，GENERIC_ALL）。
/// 构造失败时返回错误，调用方不得静默退回默认 DACL（ipc-v1 §1.2：任一失败即拒绝）。
pub struct PipeSecurity {
    attrs: SECURITY_ATTRIBUTES,
}

// SECURITY_ATTRIBUTES 指向的系统内部分配的描述符在 Drop 时释放；
// 结构体不跨线程共享（仅在创建 pipe 的调用栈内使用）。
unsafe impl Send for PipeSecurity {}

impl PipeSecurity {
    pub fn as_mut_ptr(&mut self) -> *mut c_void {
        &mut self.attrs as *mut SECURITY_ATTRIBUTES as *mut c_void
    }
}

impl Drop for PipeSecurity {
    fn drop(&mut self) {
        unsafe { LocalFree(self.attrs.lpSecurityDescriptor as HLOCAL) };
    }
}

pub fn pipe_security_current_user() -> io::Result<PipeSecurity> {
    let sid = current_user_sid_string()?;
    // D:P = DACL protected（不继承可继承 ACE）；仅 SYSTEM 与当前用户授予 GA。
    let sddl = format!("D:P(A;;GA;;;SY)(A;;GA;;;{sid})");
    let wide = to_wide(&sddl);
    let mut desc: *mut c_void = std::ptr::null_mut();
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            wide.as_ptr(),
            SDDL_REVISION_1,
            &mut desc,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(PipeSecurity {
        attrs: SECURITY_ATTRIBUTES {
            nLength: u32::try_from(std::mem::size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(0),
            lpSecurityDescriptor: desc,
            bInheritHandle: 0,
        },
    })
}

/// 仅为让 `Memory` 导入被使用到的稳定锚（LocalFree 来自 Foundation，Memory 里
/// 目前无直接调用；保留特性位以便后续扩展）。编译期检查，无运行期成本。
#[allow(dead_code)]
const _MEMORY_FEATURE_ANCHOR: () = {
    let _ = std::mem::size_of::<Memory::PROCESS_HEAP_ENTRY>();
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extended_len_path_prefixes_drive_absolute_paths() {
        assert_eq!(
            extended_len_path(Path::new(r"C:\x\y")),
            PathBuf::from(r"\\?\C:\x\y")
        );
        // 盘相对形态补齐根分隔符
        assert_eq!(
            extended_len_path(Path::new(r"C:")),
            PathBuf::from(r"\\?\C:\")
        );
        // 幂等：verbatim / 设备形态原样返回
        assert_eq!(
            extended_len_path(Path::new(r"\\?\C:\x")),
            PathBuf::from(r"\\?\C:\x")
        );
        assert_eq!(
            extended_len_path(Path::new(r"\\.\pipe\gamebot")),
            PathBuf::from(r"\\.\pipe\gamebot")
        );
        // verbatim 下不再做归一化，这里提前统一分隔符
        assert_eq!(
            extended_len_path(Path::new(r"C:/x/y")),
            PathBuf::from(r"\\?\C:\x\y")
        );
        // UNC → \\?\UNC\
        assert_eq!(
            extended_len_path(Path::new(r"\\srv\share\x")),
            PathBuf::from(r"\\?\UNC\srv\share\x")
        );
        // 相对路径无法安全加前缀，原样返回
        assert_eq!(
            extended_len_path(Path::new(r"rel\dir")),
            PathBuf::from(r"rel\dir")
        );
    }

    #[test]
    fn image_path_comparison_ignores_verbatim_prefix_and_case() {
        assert_eq!(
            normalize_image_path(Path::new(r"\\?\C:\Inst\gamer-server.exe")),
            r"c:\inst\gamer-server.exe"
        );
        assert_eq!(
            normalize_image_path(Path::new(r"\\?\UNC\srv\share\x")),
            r"\\srv\share\x"
        );
        assert_eq!(
            normalize_image_path(Path::new(r"C:\Inst\GAMER-SERVER.EXE")),
            normalize_image_path(Path::new(r"\\?\c:\inst\gamer-server.exe"))
        );
    }

    #[test]
    fn fallback_current_dir_truncates_to_existing_ancestor() {
        let long = Path::new(r"C:\base").join("a").join("bbbbbbbbbbbbbbbbbbbb");
        let fb = fallback_current_dir(&long.join("c").join("d"), 24);
        let s = fb.to_string_lossy().into_owned();
        assert!(s.chars().count() <= 24 + 4, "回退 cwd 超限: {s}");
        assert!(s.starts_with(r"\\?\C:\base"), "必须是同树祖先: {s}");
        // 短路径原样返回
        assert_eq!(
            fallback_current_dir(Path::new(r"C:\base\a"), 24),
            PathBuf::from(r"\\?\C:\base\a")
        );
    }

    #[test]
    fn random_bytes_are_nonzero_and_sized() {
        let a = random_bytes(32).expect("CSPRNG 应可用");
        let b = random_bytes(32).expect("CSPRNG 应可用");
        assert_eq!(a.len(), 32);
        assert_ne!(a, b, "32 字节随机两次相同的概率可忽略");
    }

    #[test]
    fn current_user_sid_is_wellformed() {
        let sid = current_user_sid_string().expect("当前用户 SID 应可取");
        assert!(sid.starts_with("S-1-"), "SID 形态异常: {sid}");
        assert!(sid
            .chars()
            .all(|c| c.is_ascii_digit() || c == '-' || c == 'S'));
    }

    #[test]
    fn pipe_security_builds_for_current_user() {
        let mut sec = pipe_security_current_user().expect("SDDL DACL 构造应成功");
        let _ = sec.as_mut_ptr();
    }

    #[test]
    fn process_image_path_rejects_pid_zero() {
        assert!(process_image_path(0).is_none());
        assert!(!terminate_pid_if_image(0, Path::new("x.exe")));
    }

    #[test]
    fn free_disk_bytes_on_system_drive() {
        let free = free_disk_bytes(Path::new(&std::env::temp_dir())).expect("临时目录卷应可查");
        assert!(free > 0);
    }
}
