//! SemVer 2.0.0 解析与比较（无第三方依赖，规则与 release/contracts/validate-manifest.mjs 一致）。

/// SemVer 预发布/构建段的一个标识符。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identifier {
    /// 数字标识（无前导零）；按 (长度, 字典序) 比较避免超精度。
    Numeric(String),
    /// 字母数字标识（必须含至少一个非数字字符）。
    Alphanumeric(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Semver {
    pub major: String,
    pub minor: String,
    pub patch: String,
    pub pre: Option<Vec<Identifier>>,
}

impl Semver {
    fn core(&self) -> (u64, u64, u64) {
        (
            self.major.parse().unwrap_or(0),
            self.minor.parse().unwrap_or(0),
            self.patch.parse().unwrap_or(0),
        )
    }
}

fn parse_numeric(ident: &str) -> Option<Identifier> {
    // 0 或无前导零的十进制数字
    if ident.is_empty() || !ident.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if ident.len() > 1 && ident.starts_with('0') {
        return None;
    }
    Some(Identifier::Numeric(ident.to_string()))
}

fn parse_pre_identifier(ident: &str) -> Option<Identifier> {
    if ident.is_empty()
        || !ident
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return None;
    }
    if ident.bytes().all(|b| b.is_ascii_digit()) {
        return parse_numeric(ident);
    }
    Some(Identifier::Alphanumeric(ident.to_string()))
}

/// 解析 SemVer 2.0.0（含 prerelease/build metadata，数字部分禁前导零）。
pub fn parse(s: &str) -> Option<Semver> {
    // build metadata：最多一个 '+'，其后至少一个合法标识
    let (core_pre, build) = match s.split_once('+') {
        Some((c, b)) => (c, Some(b)),
        None => (s, None),
    };
    if let Some(build) = build {
        if build.is_empty() || !build.split('.').all(valid_build_ident) {
            return None;
        }
    }
    // prerelease：core 之后最多一个 '-' 段
    let (core, pre) = match core_pre.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (core_pre, None),
    };
    let mut parts = core.split('.');
    let major = parts.next()?;
    let minor = parts.next()?;
    let patch = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let major = parse_numeric(major)?;
    let minor = parse_numeric(minor)?;
    let patch = parse_numeric(patch)?;
    let Identifier::Numeric(major) = major else {
        unreachable!()
    };
    let Identifier::Numeric(minor) = minor else {
        unreachable!()
    };
    let Identifier::Numeric(patch) = patch else {
        unreachable!()
    };
    let pre = match pre {
        None => None,
        Some(p) => Some(
            p.split('.')
                .map(parse_pre_identifier)
                .collect::<Option<Vec<_>>>()?,
        ),
    };
    Some(Semver {
        major,
        minor,
        patch,
        pre,
    })
}

fn valid_build_ident(ident: &str) -> bool {
    !ident.is_empty()
        && ident
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn ident_lt(a: &Identifier, b: &Identifier) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (Identifier::Numeric(x), Identifier::Numeric(y)) => {
            // 长度短者数值小；等长按字典序（两者均无前导零）
            x.len().cmp(&y.len()).then_with(|| x.cmp(y))
        }
        // 数字标识 < 字母数字标识
        (Identifier::Numeric(_), Identifier::Alphanumeric(_)) => Ordering::Less,
        (Identifier::Alphanumeric(_), Identifier::Numeric(_)) => Ordering::Greater,
        (Identifier::Alphanumeric(x), Identifier::Alphanumeric(y)) => x.cmp(y),
    }
}

/// a < b？（SemVer 2.0.0 优先级：prerelease < 正式版；正式版相等时 prerelease 逐标识比较，
/// 标识更多且前缀相同者更大。）
pub fn is_lt(a: &Semver, b: &Semver) -> bool {
    use std::cmp::Ordering;
    if a.core() != b.core() {
        return a.core() < b.core();
    }
    match (&a.pre, &b.pre) {
        (Some(pa), Some(pb)) => {
            let n = pa.len().max(pb.len());
            for i in 0..n {
                match (pa.get(i), pb.get(i)) {
                    (None, Some(_)) => return true, // 标识更少的一组更小
                    (Some(_), None) => return false,
                    (Some(x), Some(y)) => match ident_lt(x, y) {
                        Ordering::Less => return true,
                        Ordering::Greater => return false,
                        Ordering::Equal => continue,
                    },
                    (None, None) => return false,
                }
            }
            false
        }
        (Some(_), None) => true,
        (None, Some(_)) => false,
        (None, None) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lt(a: &str, b: &str) -> bool {
        match (parse(a), parse(b)) {
            (Some(x), Some(y)) => is_lt(&x, &y),
            _ => panic!("解析失败: {a} / {b}"),
        }
    }

    #[test]
    fn official_prerelease_ordering() {
        // SemVer 规范 §11 示例序列
        assert!(lt("1.0.0-alpha", "1.0.0-alpha.1"));
        assert!(lt("1.0.0-alpha.1", "1.0.0-alpha.beta"));
        assert!(lt("1.0.0-alpha.beta", "1.0.0-beta"));
        assert!(lt("1.0.0-beta", "1.0.0-beta.2"));
        assert!(lt("1.0.0-beta.2", "1.0.0-beta.11"));
        assert!(lt("1.0.0-beta.11", "1.0.0-rc.1"));
        assert!(lt("1.0.0-rc.1", "1.0.0"));
    }

    #[test]
    fn basic_ordering() {
        assert!(lt("0.1.0", "0.2.0"));
        assert!(lt("0.1.9", "0.1.10"));
        assert!(lt("0.9.0", "1.0.0"));
        assert!(!lt("0.2.0", "0.2.0"));
        assert!(lt("1.0.0-rc.1", "1.0.0+build.1")); // prerelease < 正式版（build 不参与）
    }

    #[test]
    fn numeric_identifiers_compare_by_value() {
        assert!(lt("1.0.0-2", "1.0.0-10"));
        assert!(lt("1.0.0-9", "1.0.0-11"));
        assert!(lt("1.0.0-1", "1.0.0-a"));
        assert!(lt("1.0.0-aaa", "1.0.0-aab"));
    }

    #[test]
    fn rejects_malformed() {
        for bad in [
            "0.2",
            "1",
            "01.0.0",
            "1.0",
            "v1.0.0",
            "1.0.0-",
            "1.0.0-01",
            "1.0.0-.",
            "1.0.0+",
            "1.0.0-a..b",
            " 1.0.0",
            "1.0.0 ",
            "1.0.0-α",
        ] {
            assert!(parse(bad).is_none(), "{bad} 应被拒绝");
        }
    }

    #[test]
    fn accepts_valid_forms() {
        for good in [
            "0.0.0",
            "0.2.0",
            "10.20.30",
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-0.3.7",
            "1.0.0-x.7.z.92",
            "1.0.0-alpha+001",
            "1.0.0+20130313144700",
            "1.0.0-beta+exp.sha.5114f85",
            "1.0.0-01a", // 含字母时前导零允许
        ] {
            assert!(parse(good).is_some(), "{good} 应可解析");
        }
    }
}
