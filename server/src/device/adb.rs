//! adb 命令封装（通过 adb 可执行文件）
//!
//! 支持：redroid / USB / 无线 adb / 模拟器统一接入。
//! Docker 镜像内置 android-tools-adb；USB 设备需 --device 直通。

use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::config::Config;

#[derive(Debug, Clone)]
pub struct Adb {
    bin: String,
}

impl Adb {
    pub fn new(cfg: &Config) -> Self {
        Self {
            bin: cfg.adb_path.clone(),
        }
    }

    /// 运行 adb 命令并返回 stdout（UTF-8）
    pub async fn run(&self, args: &[&str], timeout: Duration) -> anyhow::Result<String> {
        let mut cmd = Command::new(&self.bin);
        cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn()?;
        let (mut so, mut se) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
        let (mut out_buf, mut err_buf) = (Vec::new(), Vec::new());
        // 超时必须 kill 子进程：泄漏的 adb.exe 会持有 USB transport，可能卡死后续 adb 调用。
        // 不用 select! 双分支（timeout(inner) vs sleep 同时到期时竞态走 Elapsed 分支：
        // 报裸 "deadline has elapsed" 且不 kill）
        match tokio::time::timeout(timeout, async {
            so.read_to_end(&mut out_buf).await?;
            se.read_to_end(&mut err_buf).await?;
            anyhow::Ok(())
        })
        .await
        {
            Ok(r) => {
                r?;
            }
            Err(_) => {
                let _ = child.kill().await;
                anyhow::bail!("adb timeout: {:?}", args);
            }
        }
        let out = String::from_utf8_lossy(&out_buf).into_owned();
        let err = String::from_utf8_lossy(&err_buf).into_owned();
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
        // 同 run()：超时统一 kill 子进程 + 明确报错（不用 select! 双分支，见 run() 注释）
        match tokio::time::timeout(timeout, async {
            so.read_to_end(&mut buf).await?;
            se.read_to_end(&mut err_buf).await?;
            anyhow::Ok(())
        })
        .await
        {
            Ok(r) => {
                r?;
            }
            Err(_) => {
                let _ = child.kill().await;
                anyhow::bail!("adb timeout: {:?}", args);
            }
        }
        let status = child.wait().await?;
        if !status.success() {
            anyhow::bail!(
                "adb {:?} failed: {}",
                args,
                String::from_utf8_lossy(&err_buf).trim()
            );
        }
        Ok(buf)
    }

    /// 轻量健康探测（2s 超时）：adb 客户端能否与 server 快速往返一次。
    /// server 主循环被卡（scrcpy 隧道 teardown 楔死，见 AGENTS 已知坑）时该调用超时
    pub async fn probe(&self) -> bool {
        self.run(&["version"], Duration::from_secs(2)).await.is_ok()
    }

    /// 重置 adb server（楔死自愈）：礼貌 kill-server（3s 超时，server 主循环被卡时
    /// 不响应）；仅当 kill-server 超时（进程未退出）才强制结束全部 adb 进程兜底
    /// （Windows taskkill /F /IM，Linux pkill -9）。下次任何 adb 调用自动拉起全新
    /// server。调用方应在 probe/连接连续超时（疑似楔死）时调用。
    /// 注意：强制结束是最后手段——传输中途强杀可能把设备端 USB 状态搞差
    /// （重插变"设备描述符请求失败"，见 AGENTS 已知坑），能靠 kill-server 就别强杀
    pub async fn reset_server(&self) {
        let graceful = self.run(&["kill-server"], Duration::from_secs(3)).await;
        if graceful.is_ok() {
            return;
        }
        // kill-server 挂起 = server 主循环被卡，进程仍活着 → 强制结束兜底
        let bin = self.bin.clone();
        let (prog, args): (String, Vec<String>) = if cfg!(windows) {
            let name = std::path::Path::new(&bin)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "adb.exe".to_string());
            (
                "taskkill".to_string(),
                vec!["/F".into(), "/IM".into(), name],
            )
        } else {
            let name = std::path::Path::new(&bin)
                .file_stem()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "adb".to_string());
            ("pkill".to_string(), vec!["-9".into(), name])
        };
        match tokio::process::Command::new(&prog)
            .args(&args)
            .output()
            .await
        {
            Ok(o) if o.status.success() => tracing::warn!(
                "adb reset: kill-server 超时，已强制结束 adb 进程（{}）",
                prog
            ),
            Ok(_) => tracing::debug!("adb reset: 无残留 adb 进程（{} 未找到目标）", prog),
            Err(e) => tracing::warn!("adb reset: 强制结束失败: {}", e),
        }
    }

    /// 建立 TCP 隧道：设备端 localabstract -> 本机 localhost:port（adb reverse）
    pub async fn reverse(
        &self,
        serial: &str,
        abstract_name: &str,
        port: u16,
    ) -> anyhow::Result<()> {
        self.run(
            &[
                "-s",
                serial,
                "reverse",
                &format!("localabstract:{}", abstract_name),
                &format!("tcp:{}", port),
            ],
            Duration::from_secs(10),
        )
        .await?;
        Ok(())
    }

    /// 推送文件到设备
    pub async fn push(&self, serial: &str, local: &str, remote: &str) -> anyhow::Result<()> {
        self.run(
            &["-s", serial, "push", local, remote],
            Duration::from_secs(60),
        )
        .await?;
        Ok(())
    }

    /// 执行设备端 shell 命令
    pub async fn shell(
        &self,
        serial: &str,
        cmd: &str,
        timeout: Duration,
    ) -> anyhow::Result<String> {
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
        let bytes = self
            .run_bytes(
                &["-s", serial, "exec-out", "screencap", "-p"],
                Duration::from_secs(15),
            )
            .await?;
        Ok(bytes)
    }

    /// 指定 display 截屏（虚拟屏模式）
    pub async fn screencap_display(
        &self,
        serial: &str,
        display_id: i64,
    ) -> anyhow::Result<Vec<u8>> {
        let bytes = self
            .run_bytes(
                &[
                    "-s",
                    serial,
                    "exec-out",
                    "screencap",
                    "-d",
                    &display_id.to_string(),
                    "-p",
                ],
                Duration::from_secs(15),
            )
            .await?;
        Ok(bytes)
    }

    /// 连接网络设备（redroid / 无线 adb / 模拟器）
    pub async fn connect(&self, addr: &str) -> anyhow::Result<()> {
        let out = self
            .run(&["connect", addr], Duration::from_secs(15))
            .await?;
        if out.contains("failed") || out.contains("cannot") {
            anyhow::bail!("adb connect {} failed: {}", addr, out.trim());
        }
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

    /// 解析设备配置 serial → 实际 adb transport（`adb devices -l` 的显示名）。
    ///
    /// 设备连接方式变化后（USB 直连 ↔ Android 11+ 无线调试），`adb devices`
    /// 的显示名与配置里的 serial 会失配：
    /// - USB 直连：显示为 serial（如 `HIUWUCNJOBEEOZDY`）→ 精确匹配
    /// - 无线调试 mDNS：显示为 `adb-<serial>-<token>._adb-tls-connect._tcp` → 子串匹配
    /// - 无线 adb IP:port：显示为 `192.168.31.96:43461` → 按 model 匹配
    ///
    /// 匹配优先级：精确 → 子串（双向）→ model == 设备 name。
    /// 全部失配时原样返回配置 serial（保持旧行为，错误信息更清晰）。
    pub async fn resolve_serial(&self, configured: &str, name: &str) -> String {
        let out = self
            .run(&["devices", "-l"], Duration::from_secs(10))
            .await
            .unwrap_or_default();
        let mut by_model: Option<String> = None;
        for line in out.lines().skip(1) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 2 || parts[1] != "device" {
                continue;
            }
            let ts = parts[0].to_string();
            if ts == configured {
                return ts;
            }
            // mDNS 名包含 serial（adb-<serial>-...）；反向子串（serial 恰好是 transport 前缀等）
            if (ts.contains(configured) && !configured.is_empty())
                || (configured.contains(&ts) && !ts.is_empty())
            {
                return ts;
            }
            let model = parts
                .iter()
                .find_map(|p| p.strip_prefix("model:"))
                .unwrap_or("");
            if !model.is_empty() && model == name && by_model.is_none() {
                by_model = Some(ts);
            }
        }
        by_model.unwrap_or_else(|| configured.to_string())
    }

    /// serial 是否已连接（避免重复 connect / mDNS serial 无法 connect）
    pub async fn is_connected(&self, serial: &str) -> bool {
        match self.list_devices().await {
            // 子串匹配兜底：传入的 serial 可能是 mDNS 名/IP:port（与配置 serial 不同）
            Ok(list) => list
                .iter()
                .any(|s| s == serial || (s.contains(serial) && !serial.is_empty())),
            Err(_) => false,
        }
    }
}

/// 拼进 adb shell 命令的包名安全校验：仅 [A-Za-z0-9_.]，防注入；
/// 非包名输入（如中文应用名）返回 false，调用方据此跳过 pidof 探测等拼串操作
pub fn is_safe_pkg(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
}
