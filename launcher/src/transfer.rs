//! 可中断产物下载。只有完整 SHA-256 通过后调用方才将 .part 安装入缓存。
use crate::fetch::{build_agent, DownloadError, FetchOptions};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use std::time::Instant;

#[derive(Debug, Clone, Default)]
pub struct TransferControl {
    paused: Arc<AtomicBool>,
    bytes: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
}
impl TransferControl {
    pub fn pause(&self) {
        self.paused.store(true, Ordering::Release);
    }
    pub fn resume(&self) {
        self.paused.store(false, Ordering::Release);
    }
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }
    pub fn progress(&self) -> (u64, u64) {
        (
            self.bytes.load(Ordering::Relaxed),
            self.total.load(Ordering::Relaxed),
        )
    }
}

#[derive(Serialize, Deserialize)]
struct Receipt {
    url: String,
    sha256: String,
    size: u64,
    etag: Option<String>,
}

pub fn download(
    url: &str,
    dest: &Path,
    hash: &str,
    size: u64,
    opts: &FetchOptions,
) -> Result<u64, DownloadError> {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err(DownloadError::InvalidUrl(url.into()));
    }
    if opts.control.is_paused() {
        return Err(DownloadError::Paused);
    }
    let metadata = dest.with_extension("part.json");
    let receipt: Option<Receipt> = fs::read(&metadata)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok());
    let mut offset = receipt
        .as_ref()
        .filter(|r| r.url == url && r.sha256 == hash && r.size == size)
        .and_then(|_| fs::metadata(dest).ok())
        .filter(|m| m.is_file() && m.len() <= size)
        .map_or(0, |m| m.len());
    if offset == size && dest.is_file() {
        if crate::digest::verify_file(dest, hash, size).is_ok() {
            let _ = fs::remove_file(&metadata);
            return Ok(size);
        }
        offset = 0;
    }
    let deadline = Instant::now() + opts.overall_timeout;
    let agent = build_agent(url, opts);
    let mut request = agent.get(url).set("Accept-Encoding", "identity");
    if offset > 0 {
        request = request.set("Range", &format!("bytes={offset}-"));
        if let Some(etag) = receipt.as_ref().and_then(|r| r.etag.as_deref()) {
            request = request.set("If-Range", etag);
        }
    }
    let response = request.call().map_err(|e| match e {
        ureq::Error::Status(c, _) => DownloadError::HttpStatus(c),
        e => DownloadError::Transport(e.to_string()),
    })?;
    match response.status() {
        200 => offset = 0, // 服务端不支持 Range / ETag 改变：重下，不拼接新旧字节。
        206 if offset > 0 => {
            let expected = format!("bytes {offset}-{}/{size}", size - 1);
            if response.header("Content-Range") != Some(expected.as_str()) {
                return Err(DownloadError::InvalidRange);
            }
        }
        _ => return Err(DownloadError::InvalidRange),
    }
    if let Some(length) = response
        .header("Content-Length")
        .and_then(|s| s.parse::<u64>().ok())
    {
        if length > size - offset {
            return Err(DownloadError::OversizedContentLength {
                declared: length,
                expected: size - offset,
            });
        }
        if length < size - offset {
            return Err(DownloadError::Truncated {
                received: offset + length,
                expected: size,
            });
        }
    }
    let etag = response
        .header("ETag")
        .filter(|s| !s.starts_with("W/"))
        .map(str::to_owned);
    // 重置旧 part 必须先于写新收据，避免此处崩溃后拿旧字节当成新产物续传。
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(offset == 0)
        .append(offset > 0)
        .open(dest)
        .map_err(DownloadError::Io)?;
    crate::state::atomic::write_json_atomic(
        &metadata,
        &Receipt {
            url: url.into(),
            sha256: hash.into(),
            size,
            etag,
        },
    )
    .map_err(DownloadError::Io)?;
    opts.control.total.store(size, Ordering::Relaxed);
    opts.control.bytes.store(offset, Ordering::Relaxed);
    let mut reader = response.into_reader();
    let mut buf = [0; 64 * 1024];
    let result = loop {
        if opts.control.is_paused() {
            break Err(DownloadError::Paused);
        }
        if Instant::now() >= deadline {
            break Err(DownloadError::Timeout);
        }
        let n = match reader.read(&mut buf) {
            Ok(n) => n,
            Err(e) => break Err(DownloadError::Io(e)),
        };
        if n == 0 {
            break if offset == size {
                Ok(size)
            } else {
                Err(DownloadError::Truncated {
                    received: offset,
                    expected: size,
                })
            };
        }
        if n as u64 > size - offset {
            break Err(DownloadError::OversizedBody { limit: size });
        }
        if let Err(e) = file.write_all(&buf[..n]) {
            break Err(DownloadError::Io(e));
        }
        offset += n as u64;
        opts.control.bytes.store(offset, Ordering::Relaxed);
    };
    file.sync_all().map_err(DownloadError::Io)?;
    drop(file);
    if matches!(result, Err(DownloadError::OversizedBody { .. })) {
        let _ = fs::remove_file(dest);
        let _ = fs::remove_file(&metadata);
    }
    result?;
    let actual = crate::digest::sha256_file_hex(dest).map_err(DownloadError::Io)?;
    if !actual.eq_ignore_ascii_case(hash) {
        let _ = fs::remove_file(dest);
        let _ = fs::remove_file(&metadata);
        return Err(DownloadError::HashMismatch {
            actual,
            expected: hash.into(),
        });
    }
    let _ = fs::remove_file(&metadata);
    Ok(size)
}
