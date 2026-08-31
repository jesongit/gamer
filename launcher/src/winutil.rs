//! Windows 原生能力的小封装（批次 3）：
//! - `random_bytes`：BCryptGenRandom（IPC 会话令牌 / installation-id 的熵源）
//! - `free_disk_bytes`：GetDiskFreeSpaceExW（升级前空间预估，insufficient_space）
//! - `process_image_path` / `terminate_pid_if_image`：按 journal 记录的准确 PID/exe
//!   探测与终止孤儿候选进程（计划 §6.6：不用进程名/端口猜测，exe 匹配才动手）
//! - `pipe_security_current_user`：SDDL 构造仅含 SYSTEM + 当前用户的 DACL，
//!   供 named pipe 的 SECURITY_ATTRIBUTES 使用（ipc-v1.md §1.2）

#![allow(unknown_lints)]
#![allow(clippy::unsafe_derive_send_sync)]

use std::ffi::c_void;
use std::io;
use std::os::windows::ffi::OsStrExt;
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
/// 防 PID 复用误杀）。返回是否执行了终止。
pub fn terminate_pid_if_image(pid: u32, expected_exe: &Path) -> bool {
    let Some(actual) = process_image_path(pid) else {
        return false;
    };
    let matches = actual
        .to_string_lossy()
        .eq_ignore_ascii_case(&expected_exe.to_string_lossy());
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
