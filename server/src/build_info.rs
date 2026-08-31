//! 构建信息模块（VER-002 / 计划 §6.1）。
//!
//! `version` 以 `server/Cargo.toml` package.version 为唯一权威源（编译期
//! `CARGO_PKG_VERSION`）；`commit`/`built_at`/`channel`/`target` 由
//! `server/build.rs` 在编译期注入（发布流水线显式 env → git 自动探测 →
//! cargo `TARGET`），取值校验失败一律回落明确的缺省形态：
//!
//! | 字段 | 注入缺失时的缺省 | 说明 |
//! |---|---|---|
//! | `commit` | `"dev"` | git 不可用 / SKIP 开关 / 值非法 → dev，绝不伪装正式构建 |
//! | `built_at` | `"unknown"` | 仅发布流水线显式注入 |
//! | `channel` | `"dev"` | 只接受 stable / beta / dev / unknown，其余一律按 dev |
//! | `target` | 当前 target triple | build.rs 注入；再缺省退化为 `<arch>-<os>` |
//!
//! 同名环境变量在**运行期**也可注入（仅编译期缺失时生效——编译期值代表
//! 二进制的真实构建信息，运行期不允许覆盖改写）。消费方接线属 SYS-001 /
//! 后续批次；本批次交付模块与单测，不改动既有启动日志。
//!
//! 注意：`api::system` 另有一个面向 `/api/system/info` 的旧版 JSON 拼装
//! `build_info()`（独立字段探测链），本模块与其互不依赖；后续 SYS-001
//! 收口时再统一。

// 消费方接线（SYS-001 /api/system/info、启动日志收口）属后续批次；本批次
// 交付模块本体 + 单测，运行路径暂无引用，整体豁免 dead_code。
#![allow(dead_code)]

use std::sync::OnceLock;

/// 进程级唯一构建信息快照（OnceLock：首次访问解析并固化）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildInfo {
    /// 产品版本（server/Cargo.toml package.version，权威源）
    pub version: String,
    /// git 提交（短/全长 sha 均可）；无构建元数据时明确为 "dev"
    pub commit: String,
    /// 构建时间（发布流水线注入）；缺省 "unknown"
    pub built_at: String,
    /// 发布通道：stable / beta / dev；缺省 "dev"
    pub channel: String,
    /// 编译 target triple（如 x86_64-pc-windows-msvc）；缺省当前 target
    pub target: String,
}

const DEV_COMMIT: &str = "dev";
const UNKNOWN_BUILT_AT: &str = "unknown";
const DEV_CHANNEL: &str = "dev";

/// 取进程级唯一构建信息。解析顺序：编译期（build.rs 注入）→ 运行期环境变量
/// （仅作编译期缺失时的补充注入口）→ dev 缺省。
pub fn build_info() -> &'static BuildInfo {
    static INFO: OnceLock<BuildInfo> = OnceLock::new();
    INFO.get_or_init(|| {
        resolve(
            pick(
                "GAMER_GIT_COMMIT",
                option_env!("GAMER_GIT_COMMIT"),
                valid_commit,
            ),
            pick(
                "GAMER_BUILD_TIME",
                option_env!("GAMER_BUILD_TIME"),
                valid_timestamp,
            ),
            pick("GAMER_CHANNEL", option_env!("GAMER_CHANNEL"), valid_channel),
            pick(
                "GAMER_BUILD_TARGET",
                option_env!("GAMER_BUILD_TARGET"),
                valid_target,
            ),
        )
    })
}

/// 纯函数装配：编译期/运行期候选就绪后统一套用缺省规则（单测直接驱动）
fn resolve(
    commit: Option<String>,
    built_at: Option<String>,
    channel: Option<String>,
    target: Option<String>,
) -> BuildInfo {
    BuildInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        commit: commit
            .filter(|v| valid_commit(v))
            .unwrap_or_else(|| DEV_COMMIT.into()),
        built_at: built_at
            .filter(|v| valid_timestamp(v))
            .unwrap_or_else(|| UNKNOWN_BUILT_AT.into()),
        channel: channel
            .filter(|v| valid_channel(v))
            .unwrap_or_else(|| DEV_CHANNEL.into()),
        target: target
            .filter(|v| valid_target(v))
            .unwrap_or_else(runtime_target),
    }
}

/// 单字段取值：编译期 option_env! 优先，其次运行期环境变量；空白视为未设置，
/// 非法值整体丢弃（回落缺省），不做部分修正。
fn pick(key: &str, compiled: Option<&'static str>, valid: fn(&str) -> bool) -> Option<String> {
    let mut candidates = compiled
        .into_iter()
        .map(|v| v.trim().to_string())
        .chain(std::env::var(key).ok().map(|v| v.trim().to_string()));
    candidates.find(|v| !v.is_empty() && valid(v))
}

/// git commit：7~64 位十六进制（短 sha 到全长 sha1/sha256 均接受）
fn valid_commit(value: &str) -> bool {
    (7..=64).contains(&value.len()) && value.chars().all(|ch| ch.is_ascii_hexdigit())
}

/// 构建时间戳：自由格式文本（RFC3339 / unix 秒等），仅限长度与字符集白名单
fn valid_timestamp(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ".:+-".contains(ch))
}

/// 发布通道：白名单枚举（与发布契约一致），未知值一律回落 dev
fn valid_channel(value: &str) -> bool {
    matches!(value, "stable" | "beta" | "dev" | "unknown")
}

/// target triple / 任意构建令牌
fn valid_target(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "._-".contains(ch))
}

/// target 缺省退化：无编译期 triple 时用运行平台近似（明确非完整 triple）
fn runtime_target() -> String {
    format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_defaults_when_no_build_metadata() {
        // 本地开发缺省形态（计划 §6.1）：明确 dev/unknown，绝不伪装正式构建
        let info = resolve(None, None, None, None);
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(info.commit, "dev");
        assert_eq!(info.built_at, "unknown");
        assert_eq!(info.channel, "dev");
        assert_eq!(
            info.target,
            format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
        );
    }

    #[test]
    fn explicit_metadata_propagates_verbatim() {
        let info = resolve(
            Some("9f3ba2cd1e445566778899aabbccddeeff001122".into()),
            Some("2026-08-31T12:00:00+08:00".into()),
            Some("stable".into()),
            Some("x86_64-pc-windows-msvc".into()),
        );
        assert_eq!(info.commit, "9f3ba2cd1e445566778899aabbccddeeff001122");
        assert_eq!(info.built_at, "2026-08-31T12:00:00+08:00");
        assert_eq!(info.channel, "stable");
        assert_eq!(info.target, "x86_64-pc-windows-msvc");
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn invalid_or_empty_values_fall_back_instead_of_partial_fix() {
        // commit：非十六进制 / 过短 → dev
        assert_eq!(
            resolve(Some("release-2026-08-31".into()), None, None, None).commit,
            "dev"
        );
        assert_eq!(resolve(Some("abc".into()), None, None, None).commit, "dev");
        // channel：白名单外 → dev
        assert_eq!(
            resolve(None, None, Some("nightly".into()), None).channel,
            "dev"
        );
        // built_at：空白 → unknown
        assert_eq!(
            resolve(None, Some("   ".into()), None, None).built_at,
            "unknown"
        );
        // target：带路径分隔符 → 退化 arch-os（不泄露/不传播路径形态）
        let info = resolve(None, None, None, Some("../../evil/path".into()));
        assert_eq!(
            info.target,
            format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
        );
    }

    #[test]
    fn process_build_info_is_coherent() {
        let info = build_info();
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert!(
            info.commit == "dev" || valid_commit(&info.commit),
            "commit 应为 dev 或合法 sha：{}",
            info.commit
        );
        assert!(
            matches!(info.channel.as_str(), "stable" | "beta" | "dev" | "unknown"),
            "channel 白名单外：{}",
            info.channel
        );
        assert!(!info.built_at.is_empty());
        assert!(!info.target.is_empty());
        assert!(
            !info.target.contains('/') && !info.target.contains('\\'),
            "target 不得携带路径形态：{}",
            info.target
        );
    }

    #[test]
    fn build_info_is_process_wide_singleton() {
        // OnceLock 语义：两次取同一引用（无 Clone 开销、进程内常量）
        assert!(std::ptr::eq(build_info(), build_info()));
    }
}
