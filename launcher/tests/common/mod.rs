//! QA-002 共享夹具：临时安装根、zip 构造器、std::net 手写极简 HTTP 服务。
//! 测试不得依赖 release/vendor 存在：组件/压缩包/HTTP 响应全部在测试内自造。

// 各测试 crate 只用到本模块的一部分，未用项在编译期按 dead_code 报警。
#![allow(dead_code)]

use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;

/// 唯一临时目录（进程内自增序号 + 进程号 + 毫秒时间戳，防同毫秒并发碰撞）。
pub fn unique_root(tag: &str) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "gamer-launcher-qa2-{tag}-{}-{}-{}",
        seq,
        std::process::id(),
        gamer_launcher::state::atomic::now_unix_millis()
    ));
    fs::create_dir_all(&dir).expect("创建临时目录");
    dir
}

pub fn cleanup(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// zip 条目规格（含目录/符号链接变体，供危险条目夹具使用）。
pub enum ZipEntrySpec {
    File {
        name: String,
        content: Vec<u8>,
        unix_mode: Option<u32>,
    },
    Dir {
        name: String,
    },
}

impl ZipEntrySpec {
    pub fn file(name: &str, content: &[u8]) -> Self {
        ZipEntrySpec::File {
            name: name.to_string(),
            content: content.to_vec(),
            unix_mode: None,
        }
    }

    pub fn dir(name: &str) -> Self {
        let mut name = name.to_string();
        if !name.ends_with('/') {
            name.push('/');
        }
        ZipEntrySpec::Dir { name }
    }

    /// unix mode 标记为符号链接的条目（读取侧 is_symlink() = true）。
    pub fn symlink(name: &str, target: &str) -> Self {
        ZipEntrySpec::File {
            name: name.to_string(),
            content: target.as_bytes().to_vec(),
            unix_mode: Some(0o120_777),
        }
    }
}

/// 构造 zip 夹具（Stored，不压缩；文件内容即字节）。
pub fn build_zip(zip_path: &Path, entries: &[ZipEntrySpec]) {
    let file = fs::File::create(zip_path).expect("创建 zip 文件");
    let mut writer = zip::ZipWriter::new(file);
    let base =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for entry in entries {
        match entry {
            ZipEntrySpec::Dir { name } => {
                writer.add_directory(name, base).expect("写目录条目");
            }
            ZipEntrySpec::File {
                name,
                content,
                unix_mode,
            } => {
                let opts = match unix_mode {
                    Some(mode) => base.unix_permissions(*mode),
                    None => base,
                };
                writer
                    .start_file(name.as_str(), opts)
                    .expect("写文件条目头");
                writer.write_all(content).expect("写文件条目内容");
            }
        }
    }
    writer.finish().expect("完成 zip 写入");
}

/// 原始 zip 条目：`unix_mode` 为 Some 时按 Unix 系统（version made by 高字节=3）
/// 写入 external_attributes，使读取侧 `unix_mode()`/`is_symlink()` 生效
/// （zip crate 写入器做不到这一点——它把 system 字节写成 0/DOS）。
pub struct RawZipEntry {
    pub name: String,
    pub data: Vec<u8>,
    pub unix_mode: Option<u32>,
}

impl RawZipEntry {
    pub fn file(name: &str, data: &[u8]) -> Self {
        RawZipEntry {
            name: name.to_string(),
            data: data.to_vec(),
            unix_mode: None,
        }
    }

    pub fn with_unix_mode(name: &str, data: &[u8], mode: u32) -> Self {
        RawZipEntry {
            name: name.to_string(),
            data: data.to_vec(),
            unix_mode: Some(mode),
        }
    }
}

/// 手工拼装 STORE zip 原始字节（zip crate 写入器会拒绝重复条目名，
/// 因此「重复条目」「符号链接条目」夹具绕过写入器直拼字节；CRC-32 按 STORE 规范实算）。
pub fn build_raw_zip(path: &Path, entries: &[RawZipEntry]) {
    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<(u32, u32, u32, u16, u32, String)> = Vec::new(); // (offset, crc, size, version_made_by, ext_attrs, name)
    for entry in entries {
        let offset = out.len() as u32;
        let crc = crc32fast::hash(&entry.data);
        let name_bytes = entry.name.as_bytes();
        out.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04]); // local file header
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0u16.to_le_bytes()); // flags
        out.extend_from_slice(&0u16.to_le_bytes()); // method = store
        out.extend_from_slice(&0u16.to_le_bytes()); // mod time
        out.extend_from_slice(&0u16.to_le_bytes()); // mod date
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(entry.data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(entry.data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra len
        out.extend_from_slice(name_bytes);
        out.extend_from_slice(&entry.data);
        let (version_made_by, ext_attrs) = match entry.unix_mode {
            Some(mode) => ((3u16 << 8) | 20, mode << 16),
            None => (20, 0),
        };
        central.push((
            offset,
            crc,
            entry.data.len() as u32,
            version_made_by,
            ext_attrs,
            entry.name.clone(),
        ));
    }
    let cd_start = out.len() as u32;
    for (offset, crc, size, version_made_by, ext_attrs, name) in &central {
        let name_bytes = name.as_bytes();
        out.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02]); // central directory header
        out.extend_from_slice(&version_made_by.to_le_bytes()); // version made by（高字节 = system）
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0u16.to_le_bytes()); // flags
        out.extend_from_slice(&0u16.to_le_bytes()); // method
        out.extend_from_slice(&0u16.to_le_bytes()); // mod time
        out.extend_from_slice(&0u16.to_le_bytes()); // mod date
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes()); // compressed size
        out.extend_from_slice(&size.to_le_bytes()); // uncompressed size
        out.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra len
        out.extend_from_slice(&0u16.to_le_bytes()); // comment len
        out.extend_from_slice(&0u16.to_le_bytes()); // disk start
        out.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        out.extend_from_slice(&ext_attrs.to_le_bytes()); // external attrs
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(name_bytes);
    }
    let cd_size = out.len() as u32 - cd_start;
    out.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06]); // EOCD
    out.extend_from_slice(&0u16.to_le_bytes()); // disk
    out.extend_from_slice(&0u16.to_le_bytes()); // cd disk
    out.extend_from_slice(&(central.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u16).to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_start.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // comment len
    fs::write(path, &out).expect("写原始 zip");
}

/// 手写极简 HTTP 夹具服务：handler 收到「请求头结束」的原始请求字节与流，
/// 自行写响应（失败响应完全可控）。连接逐个处理，不保持 keep-alive。
pub type HttpHandler = Arc<dyn Fn(&[u8], &mut TcpStream) + Send + Sync>;

pub fn http_server(handler: HttpHandler) -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("绑定测试端口");
    let addr = listener.local_addr().expect("取本地端口");
    thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let handler = handler.clone();
            thread::spawn(move || handle_conn(conn, handler));
        }
    });
    addr
}

fn handle_conn(mut stream: TcpStream, handler: HttpHandler) {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 2048];
    loop {
        match stream.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    handler(&buf, &mut stream);
    let _ = stream.flush();
}

/// 标准 200 响应（带 body，Connection: close）。
pub fn http_ok(body: &str) -> String {
    format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
}

/// 写响应字节的便捷函数。
pub fn write_response(stream: &mut TcpStream, response: &str) {
    let _ = stream.write_all(response.as_bytes());
}
