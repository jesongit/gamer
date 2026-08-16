//! adb 命令封装（通过 adb 可执行文件）
//!
//! 支持：redroid / USB / 无线 adb / 模拟器统一接入。
//! Docker 镜像内置 android-tools-adb；USB 设备需 --device 直通。

use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use crate::config::Config;

#[derive(Debug, Clone)]
pub struct Adb {
    bin: String,
}

impl Adb {
    pub fn new(cfg: &Config) -> Self {
        Self { bin: cfg.adb_path.clone() }
    }

    /// 运行 adb 命令并返回 stdout（UTF-8）
    pub async fn run(&self, args: &[&str], timeout: Duration) -> anyhow::Result<String> {
        let mut cmd = Command::new(&self.bin);
        cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn()?;
        let mut out = String::new();
        let mut err = String::new();
        let (mut so, mut se) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
        let (mut out_buf, mut err_buf) = (Vec::new(), Vec::new());
        tokio::select! {
            r = tokio::time::timeout(timeout, async {
                so.read_to_end(&mut out_buf).await?;
                se.read_to_end(&mut err_buf).await?;
                anyhow::Ok(())
            }) => { r??; }
            _ = tokio::time::sleep(timeout) => {
                let _ = child.kill().await;
                anyhow::bail!("adb timeout: {:?}", args);
            }
        }
        out = String::from_utf8_lossy(&out_buf).into_owned();
        err = String::from_utf8_lossy(&err_buf).into_owned();
        let status = child.wait().await?;
        if !status.success() && out.is_empty() {
            anyhow::bail!("adb {:?} failed: {}", args, err.trim());
        }
        Ok(out)
    }

    /// 运行 adb 命令并返回原始 stdout 字节（如截图 PNG）
    pub async fn run_bytes(&self, args: &[&str], timeout: Duration) -> anyhow::Result<Vec<u8>> {
        let mut cmd = Command::new(&self.bin);
        cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn()?;
        let (mut so, mut se) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
        let mut buf = Vec::new();
        let mut err_buf = Vec::new();
        tokio::select! {
            r = tokio::time::timeout(timeout, async {
                so.read_to_end(&mut buf).await?;
                se.read_to_end(&mut err_buf).await?;
                anyhow::Ok(())
            }) => { r??; }
            _ = tokio::time::sleep(timeout) => {
                let _ = child.kill().await;
                anyhow::bail!("adb timeout: {:?}", args);
            }
        }
        let status = child.wait().await?;
        if !status.success() {
            anyhow::bail!("adb {:?} failed: {}", args, String::from_utf8_lossy(&err_buf).trim());
        }
        Ok(buf)
    }

    /// 建立 TCP 隧道：设备端 localabstract -> 本机 localhost:port（adb reverse）
    pub async fn reverse(&self, serial: &str, abstract_name: &str, port: u16) -> anyhow::Result<()> {
        self.run(
            &["-s", serial, "reverse", &format!("localabstract:{}", abstract_name), &format!("tcp:{}", port)],
            Duration::from_secs(10),
        )
        .await?;
        Ok(())
    }

    /// 推送文件到设备
    pub async fn push(&self, serial: &str, local: &str, remote: &str) -> anyhow::Result<()> {
        self.run(&["-s", serial, "push", local, remote], Duration::from_secs(60)).await?;
        Ok(())
    }

    /// 执行设备端 shell 命令
    pub async fn shell(&self, serial: &str, cmd: &str, timeout: Duration) -> anyhow::Result<String> {
        self.run(&["-s", serial, "shell", cmd], timeout).await
    }

    /// 后台执行 shell 命令，stdout/stderr 逐行转发到 tracing 日志
    pub fn shell_logged(&self, serial: &str, cmd: &str, tag: &str) {
        let bin = self.bin.clone();
        let serial = serial.to_string();
        let cmd = cmd.to_string();
        let tag = tag.to_string();
        tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, BufReader};
            let mut child = match tokio::process::Command::new(&bin)
                .args(["-s", &serial, "shell", &cmd])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("[{}] spawn failed: {}", tag, e);
                    return;
                }
            };
            let so = child.stdout.take();
            let se = child.stderr.take();
            if let Some(so) = so {
                let tag2 = tag.clone();
                tokio::spawn(async move {
                    let mut lines = BufReader::new(so).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        tracing::info!("[{}] {}", tag2, line);
                    }
                });
            }
            if let Some(se) = se {
                let tag2 = tag.clone();
                tokio::spawn(async move {
                    let mut lines = BufReader::new(se).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        tracing::warn!("[{}] {}", tag2, line);
                    }
                });
            }
            let _ = child.wait().await;
            tracing::info!("[{}] process exited", tag);
        });
    }

    /// 截屏（PNG 字节）—— 视频流取帧的 fallback
    pub async fn screencap(&self, serial: &str) -> anyhow::Result<Vec<u8>> {
        let bytes = self.run_bytes(&["-s", serial, "exec-out", "screencap", "-p"], Duration::from_secs(15)).await?;
        Ok(bytes)
    }

    /// 指定 display 截屏（虚拟屏模式）
    pub async fn screencap_display(&self, serial: &str, display_id: i64) -> anyhow::Result<Vec<u8>> {
        let bytes = self
            .run_bytes(&["-s", serial, "exec-out", "screencap", "-d", &display_id.to_string(), "-p"], Duration::from_secs(15))
            .await?;
        Ok(bytes)
    }

    /// 连接网络设备（redroid / 无线 adb / 模拟器）
    pub async fn connect(&self, addr: &str) -> anyhow::Result<()> {
        let out = self.run(&["connect", addr], Duration::from_secs(15)).await?;
        if out.contains("failed") || out.contains("cannot") {
            anyhow::bail!("adb connect {} failed: {}", addr, out.trim());
        }
        Ok(())
    }

    pub async fn disconnect(&self, addr: &str) -> anyhow::Result<()> {
        let _ = self.run(&["disconnect", addr], Duration::from_secs(10)).await;
        Ok(())
    }

    /// 查询设备在线状态（serial 列表）
    pub async fn list_devices(&self) -> anyhow::Result<Vec<String>> {
        let out = self.run(&["devices"], Duration::from_secs(10)).await?;
        let mut list = Vec::new();
        for line in out.lines().skip(1) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 && parts[1] == "device" {
                list.push(parts[0].to_string());
            }
        }
        Ok(list)
    }

    /// serial 是否已连接（避免重复 connect / mDNS serial 无法 connect）
    pub async fn is_connected(&self, serial: &str) -> bool {
        match self.list_devices().await {
            Ok(list) => list.iter().any(|s| s == serial),
            Err(_) => false,
        }
    }

    /// 获取设备属性（ro.product.model 等）
    pub async fn getprop(&self, serial: &str, prop: &str) -> anyhow::Result<String> {
        let out = self.shell(serial, &format!("getprop {}", prop), Duration::from_secs(8)).await?;
        Ok(out.trim().to_string())
    }
}

/// 向已连接的 TCP socket 写入全部数据
pub async fn write_all(sock: &mut tokio::net::TcpStream, data: &[u8]) -> anyhow::Result<()> {
    sock.write_all(data).await?;
    Ok(())
}
