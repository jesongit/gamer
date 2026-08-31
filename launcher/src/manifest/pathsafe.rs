//! manifest 路径安全（release/contracts/manifest-v1.md §4）。
//! 仅接受规范化相对路径（'/' 分隔）；绝对路径/盘符/ADS 冒号/反斜杠/`.` `..` 段/
//! 空段/段尾点空格/Windows 保留名/非法字符全部拒绝。
//! 跨条目的大小写碰撞与重复条目拒绝由 checks.rs 在安装树命名空间内执行
//! （符号链接/reparse point 在解包落地时拒绝，属 LCH-006）。

use crate::manifest::codes;

/// Windows 保留设备名（按“最后一个扩展名之前的主名”判断，大小写不敏感；
/// `con.nul`、`nul.txt` 同样命中）。
pub const RESERVED_BASES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM0", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
    "COM8", "COM9", "LPT0", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// 单条路径安全检查；`None` 表示安全，`Some(code)` 为拒绝码。
pub fn check_single_path(p: &str) -> Option<&'static str> {
    if p.is_empty() {
        return Some(codes::PATH_EMPTY);
    }
    if p.contains('\\') {
        return Some(codes::PATH_BACKSLASH);
    }
    if p.starts_with('/') {
        return Some(codes::PATH_ABSOLUTE);
    }
    let b = p.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return Some(codes::PATH_DRIVE_LETTER);
    }
    // 其余任何冒号都按 NTFS 备用数据流（ADS）拒绝
    if p.contains(':') {
        return Some(codes::PATH_ADS_COLON);
    }
    if p.chars()
        .any(|c| matches!(c, '\0'..='\u{1f}' | '<' | '>' | '|' | '"' | '?' | '*'))
    {
        return Some(codes::PATH_ILLEGAL_CHARS);
    }
    for seg in p.split('/') {
        if seg.is_empty() {
            // 'a//b'、尾随 '/'
            return Some(codes::PATH_NOT_NORMALIZED);
        }
        if seg == "." || seg == ".." {
            return Some(codes::PATH_DOTDOT);
        }
        // Windows 会剥掉段尾的点和空格
        if seg.ends_with('.') || seg.ends_with(' ') {
            return Some(codes::PATH_TRAILING_DOT_SPACE);
        }
        let base = seg.split('.').next().unwrap_or("").to_ascii_uppercase();
        if RESERVED_BASES.contains(&base.as_str()) {
            return Some(codes::PATH_RESERVED_NAME);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(p: &str) -> &'static str {
        check_single_path(p).expect("应当被拒绝")
    }

    #[test]
    fn accepts_normalized_relative_paths() {
        for ok in [
            "gamer-server.exe",
            "adb.exe",
            "AdbWinApi.dll",
            "assets/scrcpy-server.jar",
            "a/b/c/d.tar.gz",
            "console-helper.exe", // 首段主名 "CONSOLE-HELPER" 不是保留名
        ] {
            assert_eq!(check_single_path(ok), None, "{ok} 应安全");
        }
    }

    #[test]
    fn rejects_dangerous_paths() {
        assert_eq!(code(""), codes::PATH_EMPTY);
        assert_eq!(code("a\\evil.exe"), codes::PATH_BACKSLASH);
        assert_eq!(code("/abs/adb.exe"), codes::PATH_ABSOLUTE);
        assert_eq!(code("C:/evil/adb.exe"), codes::PATH_DRIVE_LETTER);
        assert_eq!(code("adb.exe:hidden"), codes::PATH_ADS_COLON);
        assert_eq!(code("a<b*.exe"), codes::PATH_ILLEGAL_CHARS);
        assert_eq!(code("a//b"), codes::PATH_NOT_NORMALIZED);
        assert_eq!(code("a/"), codes::PATH_NOT_NORMALIZED);
        assert_eq!(code("./adb.exe"), codes::PATH_DOTDOT);
        assert_eq!(code("../../../evil.exe"), codes::PATH_DOTDOT);
        assert_eq!(code("a/.."), codes::PATH_DOTDOT);
        assert_eq!(code("adb.exe."), codes::PATH_TRAILING_DOT_SPACE);
        assert_eq!(code("adb.exe "), codes::PATH_TRAILING_DOT_SPACE);
        assert_eq!(code("con.nul"), codes::PATH_RESERVED_NAME);
        assert_eq!(code("NUL.txt"), codes::PATH_RESERVED_NAME);
        assert_eq!(code("lpt1"), codes::PATH_RESERVED_NAME);
        assert_eq!(code("com9.dll"), codes::PATH_RESERVED_NAME);
    }
}
