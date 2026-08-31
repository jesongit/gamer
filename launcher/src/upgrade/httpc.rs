//! 极简阻塞 HTTP/1.1 客户端（仅回环探测用）：GET/POST + 逐请求超时 +
//! Connection: close。支持 Content-Length 与 chunked 两种 body 形态，
//! 响应上限 8 MiB（防异常对端撑爆内存）。健康探针/activate/shutdown 专用，
//! 不承载产物下载（产物走 fetch.rs）。

use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

/// 响应体积上限（回环健康/activate 响应远小于此）。
const MAX_BODY: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn body_json(&self) -> Option<serde_json::Value> {
        serde_json::from_slice(&self.body).ok()
    }
}

/// 发送一个 HTTP/1.1 请求并读取完整响应（Connection: close）。
pub fn http_request(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    timeout: Duration,
) -> Result<HttpResponse, String> {
    let deadline = Instant::now() + timeout;
    let mut stream =
        TcpStream::connect_timeout(&addr, timeout).map_err(|e| format!("连接 {addr} 失败: {e}"))?;
    stream
        .set_read_timeout(Some(remaining(deadline)))
        .map_err(|e| format!("设置读超时失败: {e}"))?;
    stream
        .set_write_timeout(Some(remaining(deadline)))
        .map_err(|e| format!("设置写超时失败: {e}"))?;

    let mut request =
        format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
    for (k, v) in headers {
        // 防注入：头值禁止 CR/LF
        if v.contains('\r') || v.contains('\n') || k.contains('\r') || k.contains('\n') {
            return Err("HTTP 头含非法换行".to_string());
        }
        request.push_str(&format!("{k}: {v}\r\n"));
    }
    request.push_str("\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("发送请求失败: {e}"))?;

    let mut raw = Vec::new();
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        if Instant::now() >= deadline {
            return Err("HTTP 响应整体超时".to_string());
        }
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if raw.len() + n > MAX_BODY {
                    return Err("HTTP 响应超过 8 MiB 上限".to_string());
                }
                raw.extend_from_slice(&buf[..n]);
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                return Err("HTTP 响应读取超时".to_string());
            }
            Err(e) => return Err(format!("读取响应失败: {e}")),
        }
    }

    let text_end = find_subslice(&raw, b"\r\n\r\n").ok_or("HTTP 响应缺少头部分隔")?;
    let head = String::from_utf8_lossy(&raw[..text_end]);
    let mut lines = head.lines();
    let status_line = lines.next().ok_or("HTTP 响应为空")?;
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("无法解析状态行: {status_line:?}"))?;
    let mut content_length: Option<usize> = None;
    let mut chunked = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim();
        if name == "content-length" {
            content_length = value.parse().ok();
        } else if name == "transfer-encoding" && value.to_ascii_lowercase().contains("chunked") {
            chunked = true;
        }
    }
    let rest = &raw[text_end + 4..];
    let body = if chunked {
        decode_chunked(rest, deadline)?
    } else if let Some(len) = content_length {
        rest.get(..len.min(rest.len()))
            .map(<[u8]>::to_vec)
            .unwrap_or_else(|| rest.to_vec())
    } else {
        rest.to_vec()
    };
    Ok(HttpResponse { status, body })
}

fn remaining(deadline: Instant) -> Duration {
    deadline
        .saturating_duration_since(Instant::now())
        .max(Duration::from_millis(1))
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn decode_chunked(mut data: &[u8], deadline: Instant) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        if Instant::now() >= deadline {
            return Err("chunked 解码超时".to_string());
        }
        let Some(line_end) = find_subslice(data, b"\r\n") else {
            break;
        };
        let size_text = String::from_utf8_lossy(&data[..line_end]);
        let size = usize::from_str_radix(size_text.trim().split(';').next().unwrap_or("0"), 16)
            .map_err(|_| format!("chunk 大小非法: {size_text:?}"))?;
        data = &data[line_end + 2..];
        if size == 0 {
            break;
        }
        if out.len() + size > MAX_BODY {
            return Err("chunked body 超过上限".to_string());
        }
        let end = size.min(data.len());
        out.extend_from_slice(&data[..end]);
        data = &data[end..];
        // 跳过 chunk 结尾 CRLF
        if data.starts_with(b"\r\n") {
            data = &data[2..];
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn get_round_trip_with_content_length() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let n = sock.read(&mut buf).unwrap();
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let body = br#"{"ready":true,"boot_id":"b-1"}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            sock.write_all(resp.as_bytes()).unwrap();
            sock.write_all(body).unwrap();
            req
        });
        let resp = http_request(addr, "GET", "/health/ready", &[], Duration::from_secs(5))
            .expect("请求应成功");
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body_json().unwrap()["boot_id"], "b-1");
        let req = server.join().unwrap();
        assert!(req.starts_with("GET /health/ready HTTP/1.1\r\n"));
        assert!(req.contains("Connection: close"));
    }

    #[test]
    fn chunked_body_is_decoded() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf).unwrap();
            sock.write_all(
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n4\r\n{\"a\"\r\n3\r\n:1}\r\n0\r\n\r\n",
            )
            .unwrap();
        });
        let resp = http_request(addr, "GET", "/", &[], Duration::from_secs(5)).unwrap();
        server.join().unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body_json().unwrap()["a"], 1);
    }

    #[test]
    fn header_value_injection_rejected() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let err = http_request(
            addr,
            "POST",
            "/x",
            &[("X-Launcher-Token", "a\r\nEvil: 1")],
            Duration::from_secs(2),
        );
        assert!(err.is_err(), "CRLF 注入必须被拒绝");
    }

    #[test]
    fn post_round_trip_with_header() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let n = sock.read(&mut buf).unwrap();
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            sock.write_all(
                b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
            req
        });
        let resp = http_request(
            addr,
            "POST",
            "/api/system/activate",
            &[("X-Launcher-Token", "tok")],
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(resp.status, 204);
        let req = server.join().unwrap();
        assert!(req.starts_with("POST /api/system/activate"));
        assert!(req.contains("X-Launcher-Token: tok"));
    }
}
