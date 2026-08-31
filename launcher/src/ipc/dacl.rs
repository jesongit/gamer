//! named pipe 创建（ipc-v1 §1）：SECURITY_ATTRIBUTES 的 DACL 仅当前用户 + SYSTEM
//! （SDDL 构造），远端连接拒绝；构造失败即报错，不静默退回默认 DACL。

#![cfg(windows)]

use std::io;

use tokio::net::windows::named_pipe::ServerOptions;

use crate::winutil;

/// 以「仅当前用户 + SYSTEM」DACL 创建一个 pipe server 实例。
/// `first_pipe_instance`：首个实例置 true（独占名字，防抢注）；
/// 后续实例（多连接并发）置 false。
pub fn create_pipe_server(
    pipe_name: &str,
    first: bool,
) -> io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
    let mut security = winutil::pipe_security_current_user()?;
    let mut options = ServerOptions::new();
    options
        .first_pipe_instance(first)
        .reject_remote_clients(true)
        .access_inbound(true)
        .access_outbound(true)
        .in_buffer_size(64 * 1024)
        .out_buffer_size(64 * 1024);
    // SAFETY：security 在调用栈内存活，指针仅被 CreateNamedPipeW 读取。
    unsafe { options.create_with_security_attributes_raw(pipe_name, security.as_mut_ptr()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pipe_creates_with_user_dacl() {
        let name = format!(
            "\\\\.\\pipe\\gamebot-launcher-test-{}-{}",
            std::process::id(),
            crate::state::atomic::now_unix_millis()
        );
        let server = create_pipe_server(&name, true).expect("DACL pipe 应可创建");
        drop(server);
        // 同名第二实例（first=false）
        let _second = create_pipe_server(&name, false).expect("非首个实例应可创建");
    }
}
