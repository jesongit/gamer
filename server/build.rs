//! 构建脚本（VER-002 / 计划 §6.1）：把构建元数据注入编译期环境变量，
//! 供 `crate::build_info` 在运行期汇总输出。
//!
//! 输入与优先级：
//! 1. 发布流水线显式注入的环境变量：`GAMER_GIT_COMMIT` / `GAMER_BUILD_TIME` /
//!    `GAMER_CHANNEL`（透传为编译期常量；构建机进程环境里直接设置同名变量同样生效）；
//! 2. `GAMER_GIT_COMMIT` 未设置时自动执行 `git rev-parse HEAD`（仅本地/CI 仓库构建可用）；
//! 3. `GAMER_BUILD_TARGET` 未设置时取 cargo 提供的 `TARGET` triple。
//!
//! 降级承诺：**不做任何网络访问**；git 缺失/非仓库/失败一律静默跳过该字段，
//! `build_info.rs` 回落明确的 dev 缺省值（"dev"/"unknown"），绝不伪装正式构建。
//! 设 `GAMER_BUILD_INFO_SKIP`（非 0/false/off 即视为开启）可完全跳过注入——
//! 逃生开关：构建环境不适合探测 git、或发布流水线想完全接管元数据时使用。

use std::path::Path;
use std::process::Command;

fn main() {
    // 显式声明重跑条件后，cargo 不再按"包内任意文件变化"默认重跑本脚本：
    // 只跟踪下方输入，避免开发期无谓的全量重编译。
    println!("cargo:rerun-if-env-changed=GAMER_BUILD_INFO_SKIP");
    println!("cargo:rerun-if-env-changed=GAMER_GIT_COMMIT");
    println!("cargo:rerun-if-env-changed=GAMER_BUILD_TIME");
    println!("cargo:rerun-if-env-changed=GAMER_CHANNEL");
    println!("cargo:rerun-if-env-changed=GAMER_BUILD_TARGET");
    // .git/HEAD + refs 跟踪分支切换与提交推进（目录小，遍历代价可忽略）；
    // 源码 tarball 无 .git 时不声明，保持默认行为。
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    let repo_root = Path::new(&manifest_dir)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();
    for tracked in [".git/HEAD", ".git/refs"] {
        let path = repo_root.join(tracked);
        if path.exists() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }

    if skip_requested() {
        // 逃生开关：不注入任何构建元数据，build_info 全部落 dev 缺省。
        return;
    }

    // commit：显式 env 优先，其次自动探测 git（失败静默降级 → build_info 回落 "dev"）
    let commit = non_empty_env("GAMER_GIT_COMMIT").or_else(git_commit);
    if let Some(commit) = commit {
        println!("cargo:rustc-env=GAMER_GIT_COMMIT={commit}");
    }

    if let Some(built_at) = non_empty_env("GAMER_BUILD_TIME") {
        println!("cargo:rustc-env=GAMER_BUILD_TIME={built_at}");
    }
    if let Some(channel) = non_empty_env("GAMER_CHANNEL") {
        println!("cargo:rustc-env=GAMER_CHANNEL={channel}");
    }
    // target：显式 GAMER_BUILD_TARGET 优先，否则取 cargo 注入的 TARGET triple
    let target = non_empty_env("GAMER_BUILD_TARGET").or_else(|| non_empty_env("TARGET"));
    if let Some(target) = target {
        println!("cargo:rustc-env=GAMER_BUILD_TARGET={target}");
    }
}

/// 逃生开关解析：非空且不属于 0/false/off（大小写不敏感）即视为开启
fn skip_requested() -> bool {
    match std::env::var("GAMER_BUILD_INFO_SKIP") {
        Ok(value) => {
            let value = value.trim().to_ascii_lowercase();
            !value.is_empty() && value != "0" && value != "false" && value != "off"
        }
        Err(_) => false,
    }
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// 本地/CI 仓库构建的 commit 探测。git 不在 PATH、目录非 git 仓库或命令
/// 失败时返回 None——build.rs 自身绝不失败，也不做任何网络访问。
fn git_commit() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let commit = String::from_utf8(output.stdout).ok()?;
    let commit = commit.trim();
    if commit.is_empty() {
        None
    } else {
        Some(commit.to_string())
    }
}
