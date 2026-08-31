//! LCH-005：组件产物获取（seeds → cache/artifacts → 远端 HTTP）与有界下载。
//!
//! 优先级固定：`seeds/<name>` → `cache/artifacts/<name>` → 远端（manifest artifact.url）。
//! 所有本地命中与下载结果都过 sha256+size 校验；下载先写 `<目标>.part`，全部校验通过
//! 才原子改名入库——截断/hash 不符/超时/超长均不污染 seeds、cache 与 runtime。
//! 代理：按 URL scheme 依次读取 HTTPS_PROXY/HTTP_PROXY/ALL_PROXY（含小写），
//! NO_PROXY 支持精确主机与后缀匹配。信任只来自 hash，URL scheme（http/https）不限。

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::digest::{to_hex, verify_file};
use crate::layout::InstallLayout;
use crate::manifest::pathsafe;
use crate::state::atomic::rename_with_retry;

pub const PART_SUFFIX: &str = ".part";

/// 下载边界参数（全部有界：连接/读取/整体超时 + 进度日志间隔）。
#[derive(Debug, Clone)]
pub struct FetchOptions {
    pub connect_timeout: Duration,
    pub read_timeout: Duration,
    pub overall_timeout: Duration,
    pub progress_interval_bytes: u64,
}

impl Default for FetchOptions {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(15),
            read_timeout: Duration::from_secs(30),
            overall_timeout: Duration::from_secs(600),
            progress_interval_bytes: 8 * 1024 * 1024,
        }
    }
}

/// 产物获取结果（path 指向已通过 sha256+size 校验的压缩包）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Obtained {
    Seed { path: PathBuf },
    Cache { path: PathBuf },
    Downloaded { cache_path: PathBuf },
}

impl Obtained {
    pub fn path(&self) -> &Path {
        match self {
            Obtained::Seed { path } | Obtained::Cache { path } => path,
            Obtained::Downloaded { cache_path } => cache_path,
        }
    }

    pub fn source_label(&self) -> &'static str {
        match self {
            Obtained::Seed { .. } => "seed",
            Obtained::Cache { .. } => "cache",
            Obtained::Downloaded { .. } => "remote",
        }
    }
}

#[derive(Debug)]
pub enum FetchError {
    /// artifact.name 不是单一安全文件名（manifest 校验已拦截，此处兜底）。
    InvalidName(String),
    /// 所有来源都不可用（含逐次尝试的原因）。
    AllSourcesExhausted {
        attempts: Vec<String>,
    },
    Download(DownloadError),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::InvalidName(name) => {
                write!(f, "artifact.name 非法（须为单一文件名）: {name:?}")
            }
            FetchError::AllSourcesExhausted { attempts } => {
                write!(f, "seeds/cache/远端均不可用: {}", attempts.join("; "))
            }
            FetchError::Download(e) => write!(f, "{e}"),
        }
    }
}

#[derive(Debug)]
pub enum DownloadError {
    InvalidUrl(String),
    HttpStatus(u16),
    /// 响应 content-length 超过声明 size（下载前即可判定，不读 body）。
    OversizedContentLength {
        declared: u64,
        expected: u64,
    },
    /// body 实际字节数超过声明 size（防谎报 header 的流）。
    OversizedBody {
        limit: u64,
    },
    Truncated {
        received: u64,
        expected: u64,
    },
    HashMismatch {
        actual: String,
        expected: String,
    },
    Timeout,
    Io(std::io::Error),
    Transport(String),
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DownloadError::InvalidUrl(url) => write!(f, "URL 非法: {url:?}"),
            DownloadError::HttpStatus(code) => write!(f, "HTTP 状态 {code}"),
            DownloadError::OversizedContentLength { declared, expected } => {
                write!(f, "content-length {declared} 超过声明 size {expected}")
            }
            DownloadError::OversizedBody { limit } => {
                write!(f, "响应 body 超过声明 size {limit}（疑似篡改/炸弹）")
            }
            DownloadError::Truncated { received, expected } => {
                write!(f, "响应被截断（收到 {received}，声明 {expected}）")
            }
            DownloadError::HashMismatch { actual, expected } => {
                write!(f, "sha256 不符（实际 {actual}，声明 {expected}）")
            }
            DownloadError::Timeout => write!(f, "下载超时（整体 deadline）"),
            DownloadError::Io(e) => write!(f, "IO 失败: {e}"),
            DownloadError::Transport(e) => write!(f, "HTTP 传输失败: {e}"),
        }
    }
}

fn is_plain_file_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && Path::new(name).file_name().is_some_and(|f| f == name)
        && pathsafe::check_single_path(name).is_none()
}

/// 按 seeds → cache → 远端 顺序获取组件压缩包；`remote_url` 为 None 时离线
/// （只用本地来源，典型于 QA 与无网修复）。任一来源命中即返回，全部失败时报
/// 逐次原因。
pub fn obtain_artifact(
    layout: &InstallLayout,
    name: &str,
    expected_sha256: &str,
    expected_size: u64,
    remote_url: Option<&str>,
    opts: &FetchOptions,
) -> Result<Obtained, FetchError> {
    if !is_plain_file_name(name) {
        return Err(FetchError::InvalidName(name.to_string()));
    }
    let expected_sha256 = expected_sha256.to_ascii_lowercase();
    let mut attempts: Vec<String> = Vec::new();

    // 1) seeds/（full 包内置，只读；命中同样过 hash 校验）
    let seed = layout.seeds_dir().join(name);
    if seed.is_file() {
        match verify_file(&seed, &expected_sha256, expected_size) {
            Ok(()) => {
                tracing::info!(seed = %seed.display(), "seed 命中且校验通过");
                return Ok(Obtained::Seed { path: seed });
            }
            Err(reason) => {
                tracing::warn!(seed = %seed.display(), %reason, "seed 校验失败，跳过");
                attempts.push(format!("seed {} 校验失败: {reason}", seed.display()));
            }
        }
    } else {
        attempts.push(format!("seed {} 不存在", seed.display()));
    }

    // 2) cache/artifacts/（可清理重建区；损坏即删除，避免反复命中坏文件）
    let cache_path = layout.artifacts_dir().join(name);
    if cache_path.is_file() {
        match verify_file(&cache_path, &expected_sha256, expected_size) {
            Ok(()) => {
                tracing::info!(cache = %cache_path.display(), "cache 命中且校验通过");
                return Ok(Obtained::Cache { path: cache_path });
            }
            Err(reason) => {
                tracing::warn!(cache = %cache_path.display(), %reason, "cache 校验失败，删除坏文件");
                attempts.push(format!("cache {} 校验失败: {reason}", cache_path.display()));
                let _ = fs::remove_file(&cache_path);
            }
        }
    } else {
        attempts.push(format!("cache {} 不存在", cache_path.display()));
    }

    // 3) 远端下载 → cache/artifacts/<name>.part → 校验 → 原子改名入库
    let Some(url) = remote_url else {
        return Err(FetchError::AllSourcesExhausted { attempts });
    };
    if let Err(e) = fs::create_dir_all(layout.artifacts_dir()) {
        return Err(FetchError::Download(DownloadError::Io(e)));
    }
    let mut part_name = name.to_string();
    part_name.push_str(PART_SUFFIX);
    let part_path = layout.artifacts_dir().join(&part_name);
    match download_bounded(url, &part_path, &expected_sha256, expected_size, opts) {
        Ok(bytes) => {
            rename_with_retry(&part_path, &cache_path)
                .map_err(|e| FetchError::Download(DownloadError::Io(e)))?;
            tracing::info!(cache = %cache_path.display(), bytes, "远端下载完成并入库");
            Ok(Obtained::Downloaded { cache_path })
        }
        Err(e) => {
            attempts.push(format!("远端 {url}: {e}"));
            Err(FetchError::AllSourcesExhausted { attempts })
        }
    }
}

/// 有界下载：只允许恰好 `expected_size` 字节且 sha256 匹配；先写 `dest`（.part），
/// 校验全过才返回；任何失败都会删除 .part，不产生半截文件。
pub fn download_bounded(
    url: &str,
    dest: &Path,
    expected_sha256: &str,
    expected_size: u64,
    opts: &FetchOptions,
) -> Result<u64, DownloadError> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(DownloadError::InvalidUrl(url.to_string()));
    }
    let expected_sha256 = expected_sha256.to_ascii_lowercase();
    let agent = build_agent(url, opts);
    let deadline = Instant::now() + opts.overall_timeout;

    let response = match agent.get(url).call() {
        Ok(r) => r,
        Err(ureq::Error::Status(code, _)) => return Err(DownloadError::HttpStatus(code)),
        Err(e) => return Err(DownloadError::Transport(e.to_string())),
    };
    if let Some(cl) = response
        .header("content-length")
        .and_then(|v| v.trim().parse::<u64>().ok())
    {
        if cl > expected_size {
            return Err(DownloadError::OversizedContentLength {
                declared: cl,
                expected: expected_size,
            });
        }
        if cl < expected_size {
            return Err(DownloadError::Truncated {
                received: cl,
                expected: expected_size,
            });
        }
    }

    let mut reader = response.into_reader();
    let mut file = match fs::File::create(dest) {
        Ok(f) => f,
        Err(e) => return Err(DownloadError::Io(e)),
    };
    let mut hasher = Sha256::new();
    let mut total: u64 = 0;
    let mut next_progress = opts.progress_interval_bytes;
    let mut buf = vec![0u8; 64 * 1024];
    let result = loop {
        if total > expected_size {
            break Err(DownloadError::OversizedBody {
                limit: expected_size,
            });
        }
        if Instant::now() > deadline {
            break Err(DownloadError::Timeout);
        }
        match reader.read(&mut buf) {
            Ok(0) => {
                break finish_download(
                    &mut file,
                    &mut hasher,
                    total,
                    &expected_sha256,
                    expected_size,
                )
            }
            Ok(n) => {
                if total + n as u64 > expected_size {
                    break Err(DownloadError::OversizedBody {
                        limit: expected_size,
                    });
                }
                if let Err(e) = file.write_all(&buf[..n]) {
                    break Err(DownloadError::Io(e));
                }
                hasher.update(&buf[..n]);
                total += n as u64;
                if total >= next_progress {
                    tracing::info!(
                        downloaded = total,
                        expected = expected_size,
                        url,
                        "下载进度"
                    );
                    next_progress += opts.progress_interval_bytes;
                }
            }
            Err(e) => break Err(classify_read_error(e)),
        }
    };
    drop(file);
    if result.is_err() {
        let _ = fs::remove_file(dest);
    }
    result
}

fn finish_download(
    file: &mut fs::File,
    hasher: &mut Sha256,
    total: u64,
    expected_sha256: &str,
    expected_size: u64,
) -> Result<u64, DownloadError> {
    if total != expected_size {
        return Err(DownloadError::Truncated {
            received: total,
            expected: expected_size,
        });
    }
    let actual = to_hex(&std::mem::take(hasher).finalize());
    if actual != expected_sha256 {
        return Err(DownloadError::HashMismatch {
            actual,
            expected: expected_sha256.to_string(),
        });
    }
    file.sync_all().map_err(DownloadError::Io)?;
    Ok(total)
}

fn classify_read_error(e: std::io::Error) -> DownloadError {
    if matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    ) {
        DownloadError::Timeout
    } else {
        DownloadError::Io(e)
    }
}

fn build_agent(url: &str, opts: &FetchOptions) -> ureq::Agent {
    let mut builder = ureq::AgentBuilder::new()
        .timeout_connect(opts.connect_timeout)
        .timeout_read(opts.read_timeout)
        .user_agent(concat!("gamer-launcher/", env!("CARGO_PKG_VERSION")));
    if let Some(proxy_url) = resolve_proxy(url, &|key| std::env::var(key).ok()) {
        match ureq::Proxy::new(&proxy_url) {
            Ok(proxy) => builder = builder.proxy(proxy),
            Err(e) => tracing::warn!(proxy = %proxy_url, %e, "代理 URL 无法解析，按直连处理"),
        }
    }
    builder.build()
}

/// 解析某 URL 应使用的代理地址（`lookup` 注入环境变量读取，供测试）。
/// 规则：NO_PROXY 命中 → 直连；https 走 HTTPS_PROXY→ALL_PROXY，http 走
/// HTTP_PROXY→ALL_PROXY（均含小写）。
pub fn resolve_proxy(url: &str, lookup: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = host_port.rsplit_once(':').map_or(host_port, |(h, _)| h);
    let host = host.trim_matches(['[', ']']);
    if host.is_empty() {
        return None;
    }
    if no_proxy_matches(host, lookup) {
        return None;
    }
    let chain: &[&str] = if scheme.eq_ignore_ascii_case("https") {
        &["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"]
    } else {
        &["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"]
    };
    for key in chain {
        if let Some(value) = lookup(key) {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// NO_PROXY：逗号分隔；`*` 全排除；条目可带前导点（按域后缀匹配），否则精确或
/// 后缀匹配（`example.com` 命中 `api.example.com`）。
fn no_proxy_matches(host: &str, lookup: &dyn Fn(&str) -> Option<String>) -> bool {
    let Some(raw) = lookup("NO_PROXY").or_else(|| lookup("no_proxy")) else {
        return false;
    };
    raw.split(',')
        .map(str::trim)
        .filter(|pat| !pat.is_empty())
        .any(|pat| {
            let pat = pat.trim_start_matches('.');
            pat == "*"
                || host.eq_ignore_ascii_case(pat)
                || host
                    .to_ascii_lowercase()
                    .ends_with(&format!(".{}", pat.to_ascii_lowercase()))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup<'a>(map: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| {
            map.iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn resolve_proxy_prefers_scheme_specific_vars() {
        let env = lookup(&[
            ("HTTP_PROXY", "http://proxy-http:8080"),
            ("HTTPS_PROXY", "http://proxy-https:8443"),
            ("ALL_PROXY", "http://proxy-all:1"),
        ]);
        assert_eq!(
            resolve_proxy("https://example.invalid/a.zip", &env).as_deref(),
            Some("http://proxy-https:8443")
        );
        assert_eq!(
            resolve_proxy("http://example.invalid/a.zip", &env).as_deref(),
            Some("http://proxy-http:8080")
        );
    }

    #[test]
    fn resolve_proxy_falls_back_to_all_proxy() {
        let env = lookup(&[("all_proxy", "socks5://gw:1080")]);
        assert_eq!(
            resolve_proxy("https://example.invalid/a.zip", &env).as_deref(),
            Some("socks5://gw:1080")
        );
        let none = lookup(&[]);
        assert_eq!(resolve_proxy("https://example.invalid/a.zip", &none), None);
    }

    #[test]
    fn no_proxy_excludes_host() {
        let env = lookup(&[
            ("HTTPS_PROXY", "http://proxy:1"),
            ("NO_PROXY", "localhost, .example.invalid, 10.0.0.0/8"),
        ]);
        assert_eq!(resolve_proxy("https://example.invalid/a.zip", &env), None);
        assert_eq!(
            resolve_proxy("https://api.example.invalid/a.zip", &env),
            None
        );
        assert_eq!(resolve_proxy("https://localhost/a.zip", &env), None);
        assert_eq!(
            resolve_proxy("https://other.invalid/a.zip", &env).as_deref(),
            Some("http://proxy:1")
        );
    }

    #[test]
    fn no_proxy_star_disables_proxy() {
        let env = lookup(&[("HTTPS_PROXY", "http://proxy:1"), ("NO_PROXY", "*")]);
        assert_eq!(resolve_proxy("https://any.invalid/a.zip", &env), None);
    }

    #[test]
    fn rejects_non_plain_names() {
        assert!(!is_plain_file_name(""));
        assert!(!is_plain_file_name("a/b.zip"));
        assert!(!is_plain_file_name("a\\b.zip"));
        assert!(!is_plain_file_name(".."));
        assert!(!is_plain_file_name("a:stream"));
        assert!(is_plain_file_name("gamer-adb-1.0.0-windows-x64.zip"));
    }
}
