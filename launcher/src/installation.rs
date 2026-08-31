//! installation-id：launcher 首次初始化生成、此后恒定的安装实例标识
//! （ipc-v1.md §1.1：`[a-z0-9]`、8~32 字符，仅用于组成 pipe 名，不含机器敏感信息）。
//! 持久化到 `state/installation-id`（原子写，缺失时生成）。
//! IPC 会话令牌同样由此模块生成（≥32 字节随机 hex，ipc-v1 §1.1 建议值）。
//!
//! 回环管理通道令牌（`state/admin-token`）：launcher 注入 `GAMER_ADMIN_TOKEN`
//! 给 server，使升级 drain（POST /api/shutdown + X-Admin-Token）能通过服务端
//! 回环管理通道的鉴权；同样为本机敏感凭据，绝不写日志。

use std::io;
use std::path::{Path, PathBuf};

use crate::state::atomic::{load_json_recover, write_json_atomic, LoadOutcome};
use crate::state::StateStore;
use crate::winutil;

pub const INSTALLATION_ID_FILE: &str = "installation-id";
/// pipe 名前缀（ipc-v1 §1.1 冻结形态：\\.\pipe\gamebot-launcher-<installation-id>）。
pub const PIPE_NAME_PREFIX: &str = "\\\\.\\pipe\\gamebot-launcher-";
const ID_LEN: usize = 16;
const TOKEN_BYTES: usize = 32;

/// 安装标识文件结构（JSON 原子写；id 字段便于后续扩展元数据不破坏格式）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InstallationId {
    pub id: String,
}

fn id_path(state_dir: &Path) -> PathBuf {
    state_dir.join(INSTALLATION_ID_FILE)
}

fn generate_id() -> io::Result<String> {
    let bytes = winutil::random_bytes(ID_LEN)?;
    let id: String = bytes
        .iter()
        .map(|b| {
            let alphabet = b"0123456789abcdefghij";
            alphabet[(*b % 20) as usize] as char
        })
        .collect();
    Ok(id)
}

fn valid_id(id: &str) -> bool {
    id.len() >= 8
        && id.len() <= 32
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// 读取或生成 installation-id（幂等：已有合法 id 原样返回）。
pub fn load_or_create(store: &StateStore) -> io::Result<String> {
    let path = id_path(&store.state_dir());
    match load_json_recover::<InstallationId>(&path)? {
        LoadOutcome::Present(v) if valid_id(&v.id) => Ok(v.id),
        // 损坏/字段非法/JSON 不是对象 → 重新生成并覆盖（id 只用于组 pipe 名，可再生成；
        // 已有客户端重连旧 pipe 名会失败并按 launcher_unreachable 重试，无数据影响）。
        LoadOutcome::Present(_) | LoadOutcome::Corrupted { .. } => regenerate(&path),
        LoadOutcome::Missing => regenerate(&path),
    }
}

fn regenerate(path: &Path) -> io::Result<String> {
    let value = InstallationId { id: generate_id()? };
    write_json_atomic(path, &value)?;
    Ok(value.id)
}

/// 完整 pipe 名（ipc-v1 §1.1 冻结形态）。
pub fn pipe_name_for(installation_id: &str) -> String {
    format!("{PIPE_NAME_PREFIX}{installation_id}")
}

/// 生成本次启动的 IPC 会话令牌（32 字节随机 → 64 hex 字符）。
pub fn new_session_token() -> io::Result<String> {
    let bytes = winutil::random_bytes(TOKEN_BYTES)?;
    Ok(crate::digest::to_hex(&bytes))
}

const ADMIN_TOKEN_FILE: &str = "admin-token";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct AdminToken {
    token: String,
}

fn admin_token_path(state_dir: &Path) -> PathBuf {
    state_dir.join(ADMIN_TOKEN_FILE)
}

fn valid_token(token: &str) -> bool {
    token.len() == TOKEN_BYTES * 2
        && token
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// 读取或生成本安装的回环管理通道令牌（幂等；损坏/非法时重新生成——
/// 旧令牌随即失效，server 由 launcher 重新注入新值，无跨进程记忆）。
pub fn load_or_create_admin_token(store: &StateStore) -> io::Result<String> {
    let path = admin_token_path(&store.state_dir());
    match load_json_recover::<AdminToken>(&path)? {
        LoadOutcome::Present(v) if valid_token(&v.token) => Ok(v.token),
        LoadOutcome::Present(_) | LoadOutcome::Corrupted { .. } | LoadOutcome::Missing => {
            let value = AdminToken {
                token: new_session_token()?,
            };
            write_json_atomic(&path, &value)?;
            Ok(value.token)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::InstallLayout;

    fn temp_root(tag: &str) -> InstallLayout {
        let dir = std::env::temp_dir().join(format!(
            "gamer-installation-{tag}-{}-{}",
            std::process::id(),
            crate::state::atomic::now_unix_millis()
        ));
        InstallLayout { root: dir }
    }

    #[test]
    fn id_is_generated_once_and_stable() {
        let layout = temp_root("stable");
        let store = StateStore::new(&layout.root);
        let first = load_or_create(&store).expect("首次生成应成功");
        assert!(valid_id(&first), "id 字符集/长度非法: {first}");
        let second = load_or_create(&store).expect("二次读取应成功");
        assert_eq!(first, second, "installation-id 必须恒定");
        let _ = std::fs::remove_dir_all(&layout.root);
    }

    #[test]
    fn corrupted_id_file_regenerates() {
        let layout = temp_root("corrupt");
        let store = StateStore::new(&layout.root);
        std::fs::create_dir_all(store.state_dir()).unwrap();
        std::fs::write(id_path(&store.state_dir()), b"{broken").unwrap();
        let id = load_or_create(&store).expect("损坏后应重新生成");
        assert!(valid_id(&id));
        let _ = std::fs::remove_dir_all(&layout.root);
    }

    #[test]
    fn pipe_name_shape_is_frozen() {
        let name = pipe_name_for("a1b2c3d4e5f6a7b8");
        assert_eq!(name, "\\\\.\\pipe\\gamebot-launcher-a1b2c3d4e5f6a7b8");
    }

    #[test]
    fn session_token_is_64_hex() {
        let token = new_session_token().unwrap();
        assert_eq!(token.len(), 64);
        assert!(token
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
    }

    #[test]
    fn admin_token_is_generated_once_and_regenerated_when_invalid() {
        let layout = temp_root("admin-token");
        let store = StateStore::new(&layout.root);
        let first = load_or_create_admin_token(&store).expect("首次生成应成功");
        assert!(valid_token(&first), "令牌必须为 64 位小写 hex: {first}");
        let second = load_or_create_admin_token(&store).expect("二次读取应成功");
        assert_eq!(first, second, "admin-token 必须恒定");

        // 损坏 → 重新生成（新值合法且不同于旧值语义：旧令牌失效）
        std::fs::write(admin_token_path(&store.state_dir()), b"{broken").unwrap();
        let third = load_or_create_admin_token(&store).expect("损坏后应重新生成");
        assert!(valid_token(&third));
        let _ = std::fs::remove_dir_all(&layout.root);
    }
}
