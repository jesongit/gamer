//! Windows named pipe 客户端传输（SYS-003 / ipc-v1 §1）。
//!
//! 生产 [`PipeTransport`]：连接 `GAMER_LAUNCHER_PIPE` 注入的完整 pipe 名
//! （server 不自行猜测/拼接）；令牌逐帧校验由协议层负责。单条长连接 +
//! 失败即断、下次交换重建（有界：连接 5s / 交换 30s 超时；ERROR_PIPE_BUSY
//! 小睡重试至连接超时）——launcher 不在场时快速失败，不做重连风暴。
//!
//! named pipe 是 Windows 专属传输：非 Windows 目标（Linux 容器镜像）编译
//! 本模块的桩实现，交换一律返回 `Unavailable`（Docker 部署本就是 external
//! 更新策略，launcher 不在场，该桩只为让 `LauncherController` 类型在
//! 全平台可装配）。
//!
//! 文件尾含进程内真 pipe 集成测试（#[cfg(all(test, windows))]）：起一个
//! tokio named pipe 服务端模拟 launcher 应答 status 帧，客户端全链路走通。

#[cfg(windows)]
use std::time::{Duration, Instant};

#[cfg(windows)]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(windows)]
use tokio::net::windows::named_pipe::ClientOptions;
#[cfg(windows)]
use tokio::sync::Mutex;

use super::ipc::{FrameError, FrameTransport};

#[cfg(windows)]
use super::ipc::{check_frame_limit, CONNECT_TIMEOUT, EXCHANGE_TIMEOUT};

#[cfg(windows)]
/// ERROR_PIPE_BUSY（winerror.h）：当前无可用 pipe 实例，稍候重试
const ERROR_PIPE_BUSY: i32 = 231 + 1;

pub struct PipeTransport {
    #[allow(dead_code)] // 非 Windows 平台保留名字仅用于错误信息可读性
    pipe_name: String,
    #[cfg(windows)]
    conn: Mutex<Option<tokio::net::windows::named_pipe::NamedPipeClient>>,
}

impl PipeTransport {
    pub fn new(pipe_name: impl Into<String>) -> Self {
        Self {
            pipe_name: pipe_name.into(),
            #[cfg(windows)]
            conn: Mutex::new(None),
        }
    }

    #[cfg(windows)]
    fn pipe_name(&self) -> &str {
        &self.pipe_name
    }

    #[cfg(windows)]
    async fn connect(
        &self,
    ) -> Result<tokio::net::windows::named_pipe::NamedPipeClient, FrameError> {
        let deadline = Instant::now() + CONNECT_TIMEOUT;
        loop {
            match ClientOptions::new().open(self.pipe_name()) {
                Ok(client) => return Ok(client),
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                    if Instant::now() >= deadline {
                        return Err(FrameError::Timeout("connect (pipe busy)"));
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // launcher 不在场：立即失败（不重试风暴，ipc-v1 §1.3）
                    return Err(FrameError::Unavailable(
                        "pipe not found (launcher not running)".into(),
                    ));
                }
                Err(e) => {
                    if Instant::now() >= deadline {
                        return Err(FrameError::Unavailable(e.to_string()));
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
        }
    }

    /// 单帧交换：确保连接（失败置空，下次重建）→ 写请求 → 读长度前缀 + 载荷。
    /// 连接 5s / 交换 30s 有界超时。
    #[cfg(windows)]
    async fn run_exchange(&self, request: Vec<u8>) -> Result<Vec<u8>, FrameError> {
        let mut guard = self.conn.lock().await;
        if guard.is_none() {
            *guard = Some(self.connect().await?);
        }
        let conn = guard.as_mut().expect("connection established above");
        conn.write_all(&request)
            .await
            .map_err(|e| FrameError::Unavailable(e.to_string()))?;
        conn.flush()
            .await
            .map_err(|e| FrameError::Unavailable(e.to_string()))?;
        // 长度前缀
        let mut prefix = [0u8; 4];
        tokio::time::timeout(EXCHANGE_TIMEOUT, conn.read_exact(&mut prefix))
            .await
            .map_err(|_| FrameError::Timeout("frame exchange"))?
            .map_err(|e| FrameError::Unavailable(e.to_string()))?;
        let len = u32::from_le_bytes(prefix);
        check_frame_limit(len)?;
        // 载荷
        let mut body = vec![0u8; len as usize];
        tokio::time::timeout(EXCHANGE_TIMEOUT, conn.read_exact(&mut body))
            .await
            .map_err(|_| FrameError::Timeout("frame exchange"))?
            .map_err(|e| FrameError::Unavailable(e.to_string()))?;
        // FrameTransport returns the complete frame, including the prefix;
        // LauncherClient performs the shared length/exactly-one-frame check.
        let mut frame = Vec::with_capacity(4 + body.len());
        frame.extend_from_slice(&prefix);
        frame.extend_from_slice(&body);
        Ok(frame)
    }
}

impl FrameTransport for PipeTransport {
    #[cfg(windows)]
    fn exchange(
        &self,
        request: Vec<u8>,
        _request_id: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>, FrameError>> + Send + '_>>
    {
        Box::pin(async move {
            // 通道损伤：丢弃长连接（下次交换重建）；结果原样上抛
            let outcome = self.run_exchange(request).await;
            if outcome.is_err() {
                *self.conn.lock().await = None;
            }
            outcome
        })
    }

    #[cfg(not(windows))]
    fn exchange(
        &self,
        request: Vec<u8>,
        _request_id: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>, FrameError>> + Send + '_>>
    {
        Box::pin(async move {
            let _ = request;
            let _ = &self.pipe_name;
            Err(FrameError::Unavailable(
                "launcher named-pipe IPC is Windows-only (linux container build)".into(),
            ))
        })
    }
}

#[cfg(all(test, windows))]
mod windows_pipe_tests {
    use super::*;
    use crate::update::ipc::{decode_payload, encode_frame, LauncherClient};
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::windows::named_pipe::ServerOptions;

    /// 进程内真 pipe 端到端：起服务端模拟 launcher（只应答一次 status），
    /// 客户端经 PipeTransport 完成一帧交换。launcher 并行轨不在场也自洽
    /// （测试自己扮演 pipe 服务端）。
    #[tokio::test]
    async fn real_pipe_status_exchange_roundtrip() {
        let pipe_name = format!(
            r"\\.\pipe\gamebot-server-test-{}",
            uuid::Uuid::new_v4().simple()
        );
        let server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&pipe_name)
            .unwrap();

        // 模拟 launcher：读一帧 status 请求 → 回一帧成功响应
        let serve = tokio::spawn(async move {
            let mut conn = server;
            conn.connect().await.unwrap();
            let mut prefix = [0u8; 4];
            conn.read_exact(&mut prefix).await.unwrap();
            let len = u32::from_le_bytes(prefix);
            let mut body = vec![0u8; len as usize];
            conn.read_exact(&mut body).await.unwrap();
            let req = decode_payload(&body).unwrap();
            assert_eq!(req["operation"], "status");
            assert_eq!(req["protocol_version"], 1);
            let resp = json!({
                "protocol_version": 1,
                "request_id": req["request_id"],
                "ok": true,
                "result": {
                    "launcher_version": "0.1.0",
                    "installation_id": "a1b2c3d4e5f6a7b8",
                    "protocol_version": 1,
                    "versions": {"current": "0.2.0", "previous": null},
                    "schema": {"db": 1, "file": 1, "rollback_floor": 1},
                    "update": {"state": "idle", "detail": "idle", "update_id": null,
                               "candidate": null, "progress": null, "last_error": null},
                    "dependencies": {},
                },
            });
            let frame = encode_frame(&resp);
            conn.write_all(&frame).await.unwrap();
            conn.flush().await.unwrap();
        });

        let transport = PipeTransport::new(pipe_name);
        let client = LauncherClient::new(transport, "session-token".into());
        let status = tokio::time::timeout(Duration::from_secs(10), client.status())
            .await
            .expect("exchange within timeout")
            .expect("status exchange succeeds");
        assert_eq!(status.state, Some(crate::update::model::UpdateState::Idle));
        serve.await.unwrap();
    }

    /// launcher 不在场：pipe 不存在 → 立即 Unavailable（不挂死、不重试风暴）
    #[tokio::test]
    async fn missing_pipe_reports_unavailable_quickly() {
        let transport = PipeTransport::new(r"\\.\pipe\gamebot-server-test-never-exists");
        let started = Instant::now();
        let result = tokio::time::timeout(Duration::from_secs(10), transport.connect()).await;
        let elapsed = started.elapsed();
        assert!(result.is_ok(), "connect should fail fast, not hang");
        assert!(matches!(result.unwrap(), Err(FrameError::Unavailable(_))));
        assert!(
            elapsed < Duration::from_secs(5),
            "未在场的 pipe 不应重试拖时长: {elapsed:?}"
        );
    }
}
