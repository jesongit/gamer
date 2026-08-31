//! 子命令实现：start / status / doctor / repair / upgrade（LCH-001）。

use std::path::{Path, PathBuf};

use crate::cli::{Cli, Command};
use crate::layout::InstallLayout;
use crate::manifest::{validate_manifest_file, ValidateOptions};
use crate::state::atomic::LoadOutcome;
use crate::state::lock::InstanceLock;
use crate::state::StateStore;

/// doctor --manifest 的参数集合。
#[derive(Debug, Clone)]
pub struct DoctorInvocation {
    pub manifest: Option<PathBuf>,
    pub sig: Option<PathBuf>,
    pub key: Option<PathBuf>,
    pub expect_current_version: Option<String>,
    pub expect_channel: Option<String>,
}

pub fn dispatch(cli: &Cli, layout: &InstallLayout) -> i32 {
    match &cli.command {
        Command::Start => not_implemented("start", "批次 2 LCH-008：server supervisor"),
        Command::Status => cmd_status(layout),
        Command::Doctor {
            manifest,
            sig,
            key,
            expect_current_version,
            expect_channel,
        } => {
            let invocation = DoctorInvocation {
                manifest: manifest.clone(),
                sig: sig.clone(),
                key: key.clone(),
                expect_current_version: expect_current_version.clone(),
                expect_channel: expect_channel.clone(),
            };
            match invocation.manifest.as_deref() {
                Some(path) => cmd_doctor_manifest(layout, cli, &invocation, path),
                None => cmd_doctor_inventory(layout),
            }
        }
        Command::Repair => not_implemented("repair", "批次 2 LCH-007：依赖修复编排"),
        Command::Upgrade => not_implemented("upgrade", "批次 3 LCH-010：升级状态机编排"),
    }
}

fn not_implemented(name: &str, plan: &str) -> i32 {
    tracing::warn!(command = name, "子命令尚未实现");
    println!("{name}：尚未实现（计划 {plan} 后续批次提供）。");
    println!("本次未执行任何动作，安装目录未被修改。");
    1
}

/// status：只读查询，不获取单实例锁；current.json 损坏时报错而非崩溃。
fn cmd_status(layout: &InstallLayout) -> i32 {
    println!("安装根: {}", layout.root.display());
    let store = StateStore::new(&layout.root);
    match store.load_current() {
        Ok(LoadOutcome::Present(current)) => {
            println!("当前版本: {}", current.current);
            println!(
                "上一版本: {}",
                current.previous.as_deref().unwrap_or("（无）")
            );
            if let Some(ts) = current.updated_at_unix_ms {
                println!("指针更新时间: {ts} (unix ms)");
            }
            println!("应用目录: versions/{}", current.current);
        }
        Ok(LoadOutcome::Missing) => {
            println!("当前状态: 未安装（state/current.json 不存在）");
        }
        Ok(LoadOutcome::Corrupted { backup_path }) => {
            tracing::error!(backup = %backup_path.display(), "current.json 损坏");
            eprintln!(
                "错误: state/current.json 损坏（半截/非法 JSON），已按空状态处理并备份到 {}；请运行 repair（后续批次）或重新安装。",
                backup_path.display()
            );
            return 1;
        }
        Err(e) => {
            tracing::error!(error = %e, "读取 current.json 失败");
            eprintln!("错误: 读取 state/current.json 失败: {e}");
            return 1;
        }
    }
    match store.load_journal() {
        Ok(jl) => {
            let mut line = format!("升级状态机: {}", jl.journal.state.as_str());
            if let Some(id) = &jl.journal.update_id {
                line.push_str(&format!("（update id: {id}）"));
            }
            if let Some(backup) = &jl.reset_from {
                line.push_str(&format!(
                    "（journal 损坏已重置，备份: {}）",
                    backup.display()
                ));
            }
            println!("{line}");
        }
        Err(e) => println!("升级状态机: 读取失败（{e}）"),
    }
    let held = InstanceLock::is_locked(&store.lock_path());
    println!(
        "实例锁: {}",
        if held {
            "被其他实例持有"
        } else {
            "空闲"
        }
    );
    0
}

/// doctor（不带 --manifest）：安装库存快速检查。
/// 深检（逐文件 hash/probe）为 LCH-004 的占位；本批先做 manifest/runtime 等目录存在性。
fn cmd_doctor_inventory(layout: &InstallLayout) -> i32 {
    println!("doctor：安装库存快速检查（深检为 LCH-004 占位，后续批次实现）");
    println!("安装根: {}", layout.root.display());
    if !layout.root.is_dir() {
        println!("[FAIL] 安装根目录不存在");
        return 1;
    }
    println!("[PASS] 安装根目录存在");
    let store = StateStore::new(&layout.root);
    let mut fail = 0usize;
    let mut warn = 0usize;

    // state/（单实例锁与版本指针的落点）
    if layout.state_dir().is_dir() {
        println!("[PASS] state/ 目录存在");
    } else {
        println!("[FAIL] state/ 目录不存在（单实例锁与版本指针无落点）");
        fail += 1;
    }

    match store.load_current() {
        Ok(LoadOutcome::Present(current)) => {
            println!("[PASS] 当前版本指针: {}", current.current);
        }
        Ok(LoadOutcome::Missing) => {
            println!("[WARN] 尚未安装（state/current.json 不存在）");
            warn += 1;
        }
        Ok(LoadOutcome::Corrupted { backup_path }) => {
            println!(
                "[FAIL] state/current.json 损坏（已备份: {}）",
                backup_path.display()
            );
            fail += 1;
        }
        Err(e) => {
            println!("[FAIL] state/current.json 读取失败: {e}");
            fail += 1;
        }
    }

    // manifests/
    let manifests = layout.manifests_dir();
    if manifests.is_dir() {
        let count = count_files_with_ext(&manifests, "json");
        println!("[PASS] manifests/ 目录存在（{count} 份缓存 manifest）");
    } else {
        println!("[WARN] manifests/ 目录不存在（尚未缓存任何 release manifest）");
        warn += 1;
    }

    // runtime/
    let runtime = layout.runtime_dir();
    if runtime.is_dir() {
        let subs = subdir_names(&runtime);
        let detail = if subs.is_empty() {
            "（空）".to_string()
        } else {
            subs.join(", ")
        };
        println!("[PASS] runtime/ 目录存在（依赖: {detail}）");
    } else {
        println!("[WARN] runtime/ 目录不存在（managed 依赖尚未安装）");
        warn += 1;
    }

    // versions/
    let versions = layout.versions_dir();
    if versions.is_dir() {
        let subs = subdir_names(&versions);
        let detail = if subs.is_empty() {
            "（空）".to_string()
        } else {
            subs.join(", ")
        };
        println!("[PASS] versions/ 目录存在（已装版本: {detail}）");
    } else {
        println!("[WARN] versions/ 目录不存在（尚未安装任何应用版本）");
        warn += 1;
    }

    println!("库存检查完成: {fail} 项失败 / {warn} 项警告");
    i32::from(fail > 0)
}

/// doctor --manifest：对任意 manifest 文件跑完整校验（先验签、后解析，fail closed）。
fn cmd_doctor_manifest(
    layout: &InstallLayout,
    cli: &Cli,
    invocation: &DoctorInvocation,
    manifest: &Path,
) -> i32 {
    let keys_dir = match resolve_keys_dir(cli.keys_dir.as_ref(), layout) {
        Ok(dir) => Some(dir),
        Err(msg) => {
            eprintln!("错误: {msg}");
            return 2;
        }
    };
    let opts = ValidateOptions {
        sig_path: invocation.sig.clone(),
        keys_dir,
        key_path: invocation.key.clone(),
        expect_current_version: invocation.expect_current_version.clone(),
        expect_channel: invocation.expect_channel.clone(),
        launcher_version: Some(env!("CARGO_PKG_VERSION").to_string()),
    };
    let outcome = validate_manifest_file(manifest, &opts);
    println!("manifest: {}", manifest.display());
    if outcome.ok {
        println!(
            "signature: verified (key_id={})",
            outcome.info.key_id.as_deref().unwrap_or("?")
        );
        println!(
            "release: {} ({}); platforms: {}",
            outcome.info.version.as_deref().unwrap_or("?"),
            outcome.info.channel.as_deref().unwrap_or("?"),
            outcome.info.platforms.join(", ")
        );
        println!("OK — release manifest v1 valid（校验通过）");
        0
    } else {
        println!("FAIL — {} error(s)", outcome.errors.len());
        for e in &outcome.errors {
            println!("  [{}] {}", e.code, e.detail);
        }
        1
    }
}

/// 公钥目录解析顺序：--keys-dir > GAMER_LAUNCHER_KEYS_DIR > <安装根>/keys > <exe 目录>/keys。
fn resolve_keys_dir(explicit: Option<&PathBuf>, layout: &InstallLayout) -> Result<PathBuf, String> {
    if let Some(dir) = explicit {
        return Ok(dir.clone());
    }
    if let Ok(env_dir) = std::env::var("GAMER_LAUNCHER_KEYS_DIR") {
        if !env_dir.trim().is_empty() {
            return Ok(PathBuf::from(env_dir));
        }
    }
    let mut candidates: Vec<PathBuf> = vec![layout.root.join("keys")];
    if let Some(exe_keys) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("keys")))
    {
        candidates.push(exe_keys);
    }
    for candidate in candidates {
        if candidate.is_dir() {
            return Ok(candidate);
        }
    }
    Err(
        "未找到可信公钥目录：请用 --keys-dir 指定（例如 release/contracts/fixtures/keys），\
         或设置 GAMER_LAUNCHER_KEYS_DIR，或在安装根下放置 keys/*.pem"
            .to_string(),
    )
}

fn count_files_with_ext(dir: &Path, ext: &str) -> usize {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|e| {
                    e.path()
                        .extension()
                        .and_then(|x| x.to_str())
                        .is_some_and(|x| x.eq_ignore_ascii_case(ext))
                })
                .count()
        })
        .unwrap_or(0)
}

fn subdir_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|e| e.path().is_dir())
                .filter_map(|e| e.file_name().to_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}
