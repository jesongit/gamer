//! 子命令实现：start / status / doctor / repair / upgrade。

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::cli::{Cli, Command};
use crate::inventory::{CheckOptions, ComponentSpec, ComponentStatus, FileCheck, ProbeCheck};
use crate::layout::InstallLayout;
use crate::manifest::model::Manifest;
use crate::manifest::{validate_manifest_file, ValidateOptions};
use crate::repair::{self, RepairGate, RepairOptions};
use crate::state::atomic::LoadOutcome;
use crate::state::lock::InstanceLock;
use crate::state::StateStore;
use crate::supervisor::{self, LaunchPlan, ReadyProbe};

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
        Command::Start => cmd_start(layout),
        Command::Status => cmd_status(layout),
        Command::Doctor {
            manifest,
            sig,
            key,
            expect_current_version,
            expect_channel,
            deep,
            probe,
        } => {
            let invocation = DoctorInvocation {
                manifest: manifest.clone(),
                sig: sig.clone(),
                key: key.clone(),
                expect_current_version: expect_current_version.clone(),
                expect_channel: expect_channel.clone(),
            };
            match invocation.manifest.as_deref() {
                Some(path) if !deep && !probe => {
                    cmd_doctor_manifest(layout, cli, &invocation, path)
                }
                _ => {
                    cmd_doctor_inventory(layout, cli, *deep || *probe, *probe, manifest.as_deref())
                }
            }
        }
        Command::Repair { manifest, probe } => cmd_repair(layout, cli, manifest.as_deref(), *probe),
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

/// doctor（库存检查）：目录级检查恒做；能解析到 release manifest（--manifest 或
/// manifests/ 缓存）时叠加组件级检查——quick = 存在 + size，deep = 逐文件 sha256，
/// probe = 附 adb/ffmpeg 版本探针。
fn cmd_doctor_inventory(
    layout: &InstallLayout,
    cli: &Cli,
    deep: bool,
    probe: bool,
    explicit_manifest: Option<&Path>,
) -> i32 {
    let mode = if deep {
        "深检（逐文件 sha256）"
    } else {
        "快速检查（存在 + size）"
    };
    println!("doctor：安装库存{mode}（probe={probe}）");
    println!("安装根: {}", layout.root.display());
    if !layout.root.is_dir() {
        println!("[FAIL] 安装根目录不存在");
        return 1;
    }
    println!("[PASS] 安装根目录存在");
    let mut fail = run_layout_checks(layout);
    let mut warn = 0usize;

    let bundle = match load_manifest_model(layout, cli, explicit_manifest) {
        Ok(bundle) => Some(bundle),
        Err(msg) => {
            if deep {
                eprintln!("错误: 深检需要 release manifest：{msg}");
                return 1;
            }
            println!("[WARN] 未找到可用 release manifest，跳过组件级检查（{msg}）");
            warn += 1;
            None
        }
    };

    if let Some(bundle) = bundle {
        let Some(platform) = bundle.model.platforms.get("windows-x86_64") else {
            println!("[FAIL] manifest 缺少 windows-x86_64 平台");
            return 1;
        };
        println!(
            "manifest: {}（release {}）",
            bundle.path.display(),
            bundle.model.release.version
        );
        for comp in &platform.components {
            let spec = match ComponentSpec::from_model(comp) {
                Ok(s) => s,
                Err(msg) => {
                    println!("[FAIL] {msg}");
                    fail += 1;
                    continue;
                }
            };
            let finding =
                crate::inventory::check_installed(layout, &spec, CheckOptions { deep, probe });
            print_component_finding(&finding);
            if finding.status != ComponentStatus::Ok {
                fail += 1;
            }
        }
    }

    println!("库存检查完成: {fail} 项失败 / {warn} 项警告");
    i32::from(fail > 0)
}

/// 目录级检查（state/manifests/runtime/versions），返回失败数。
fn run_layout_checks(layout: &InstallLayout) -> usize {
    let store = StateStore::new(&layout.root);
    let mut fail = 0usize;

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
        Ok(LoadOutcome::Missing) => println!("[WARN] 尚未安装（state/current.json 不存在）"),
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

    let manifests = layout.manifests_dir();
    if manifests.is_dir() {
        let count = count_files_with_ext(&manifests, "json");
        println!("[PASS] manifests/ 目录存在（{count} 份缓存 manifest）");
    } else {
        println!("[WARN] manifests/ 目录不存在（尚未缓存任何 release manifest）");
    }

    let runtime = layout.runtime_dir();
    if runtime.is_dir() {
        let detail = subdir_names(&runtime).join(", ");
        println!("[PASS] runtime/ 目录存在（依赖: {detail}）");
    } else {
        println!("[WARN] runtime/ 目录不存在（managed 依赖尚未安装）");
    }

    let versions = layout.versions_dir();
    if versions.is_dir() {
        let detail = subdir_names(&versions).join(", ");
        println!("[PASS] versions/ 目录存在（已装版本: {detail}）");
    } else {
        println!("[WARN] versions/ 目录不存在（尚未安装任何应用版本）");
    }
    fail
}

fn print_component_finding(finding: &crate::inventory::ComponentFinding) {
    println!("组件目录 {}", finding.dir.display());
    for f in &finding.files {
        match &f.check {
            FileCheck::Ok => println!("  [PASS] {}", f.path),
            FileCheck::Missing => println!("  [FAIL] {}: 文件缺失", f.path),
            FileCheck::SizeMismatch { actual, expected } => {
                println!(
                    "  [FAIL] {}: size 不符（实际 {actual}，声明 {expected}）",
                    f.path
                )
            }
            FileCheck::HashMismatch { actual, expected } => {
                println!(
                    "  [FAIL] {}: sha256 不符（实际 {actual}，声明 {expected}）",
                    f.path
                )
            }
            FileCheck::Io(e) => println!("  [FAIL] {}: 读取失败（{e}）", f.path),
        }
    }
    match &finding.probe {
        Some(ProbeCheck::Match { reported }) => println!("  [PASS] 探针: 版本匹配（{reported}）"),
        Some(ProbeCheck::Mismatch { reported }) => {
            println!("  [FAIL] 探针: 版本不符（运行输出 {reported}）")
        }
        Some(ProbeCheck::Failed { reason }) => println!("  [FAIL] 探针: {reason}"),
        Some(ProbeCheck::Unsupported) | None => {}
    }
    if finding.status == ComponentStatus::Ok {
        println!("  => 组件完好");
    } else {
        println!("  => 组件缺失/损坏（可运行 repair 修复）");
    }
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

/// repair（LCH-007）：深检 → seed/cache/remote 修复 → 复验探针。
fn cmd_repair(layout: &InstallLayout, cli: &Cli, manifest: Option<&Path>, probe: bool) -> i32 {
    println!("repair：依赖修复（probe={probe}）");
    println!("安装根: {}", layout.root.display());
    let bundle = match load_manifest_model(layout, cli, manifest) {
        Ok(b) => b,
        Err(msg) => {
            eprintln!("错误: {msg}");
            return 2;
        }
    };
    println!(
        "manifest: {}（release {}）",
        bundle.path.display(),
        bundle.model.release.version
    );
    let Some(platform) = bundle.model.platforms.get("windows-x86_64") else {
        eprintln!("错误: manifest 缺少 windows-x86_64 平台");
        return 2;
    };
    let mut specs = Vec::new();
    let mut spec_errors = 0usize;
    for comp in &platform.components {
        match ComponentSpec::from_model(comp) {
            Ok(spec) => specs.push(spec),
            Err(msg) => {
                println!("[FAIL] {msg}");
                spec_errors += 1;
            }
        }
    }
    if spec_errors > 0 {
        return 1;
    }

    let opts = RepairOptions {
        fetch: Default::default(),
        probe,
    };
    match repair::repair_with_lock(layout, &specs, &opts) {
        Err(RepairGate::Locked { path }) => {
            println!(
                "已有 launcher 实例持有安装根（{}），本次未执行任何动作。",
                path.display()
            );
            1
        }
        Err(RepairGate::Io(e)) => {
            eprintln!("错误: 单实例锁操作失败: {e}");
            1
        }
        Ok(report) => {
            for cr in &report.components {
                match &cr.outcome {
                    repair::ComponentOutcome::Healthy => {
                        println!("[PASS] {} {}: 组件完好，无需修复", cr.id, cr.version);
                    }
                    repair::ComponentOutcome::Repaired { source } => {
                        println!("[PASS] {} {}: 已修复（来源 {source}）", cr.id, cr.version);
                    }
                    repair::ComponentOutcome::Failed { reason } => {
                        println!("[FAIL] {} {}: {reason}", cr.id, cr.version);
                    }
                }
            }
            let failed = report.failed_count();
            let repaired = report.repaired_count();
            if failed == 0 {
                println!("修复完成：{repaired} 个组件恢复 / 全部组件可用。");
                0
            } else {
                println!("修复完成：{repaired} 个组件恢复 / {failed} 个失败（失败组件的上一份 runtime 未被破坏）。");
                1
            }
        }
    }
}

/// start（LCH-008 + OPS-003）：env 注入 + 最小 PATH + 句柄等待 + 就绪探测。
fn cmd_start(layout: &InstallLayout) -> i32 {
    println!("start：启动并监管 gamer-server");
    println!("安装根: {}", layout.root.display());
    let lock = match InstanceLock::acquire(&layout.state_dir()) {
        Ok(l) => l,
        Err(crate::state::lock::LockError::Held { path }) => {
            eprintln!("错误: 已有 launcher 实例持有安装根（{}）", path.display());
            return 1;
        }
        Err(crate::state::lock::LockError::Io(e)) => {
            eprintln!("错误: 单实例锁操作失败: {e}");
            return 1;
        }
    };
    let _ = lock;

    let store = StateStore::new(&layout.root);
    let current = match store.load_current() {
        Ok(LoadOutcome::Present(c)) => c,
        Ok(LoadOutcome::Missing) => {
            eprintln!("错误: 尚未安装（state/current.json 不存在），无版本可启动。");
            return 1;
        }
        Ok(LoadOutcome::Corrupted { backup_path }) => {
            eprintln!(
                "错误: state/current.json 损坏（已备份到 {}），拒绝启动以避免版本指针不可信。",
                backup_path.display()
            );
            return 1;
        }
        Err(e) => {
            eprintln!("错误: 读取 state/current.json 失败: {e}");
            return 1;
        }
    };
    let version = current.current.clone();
    let app_dir = layout.versions_dir().join(&version);
    let exe = match supervisor::resolve_entrypoint(layout, &version) {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("错误: {msg}");
            return 1;
        }
    };

    let adb = supervisor::latest_component_exe(layout, "adb", "adb.exe");
    let ffmpeg = supervisor::latest_component_exe(layout, "ffmpeg", "ffmpeg.exe");
    if adb.is_none() {
        tracing::warn!(
            "runtime/adb 未安装，GAMER_ADB_PATH 不注入（server readiness 将报 adb not_ready）"
        );
    }
    if ffmpeg.is_none() {
        tracing::warn!("runtime/ffmpeg 未安装，GAMER_FFMPEG_PATH 不注入（server readiness 将报 ffmpeg not_ready）");
    }
    let _ = fs::create_dir_all(layout.logs_dir());
    let _ = fs::create_dir_all(layout.data_dir());

    let plan = LaunchPlan {
        exe: exe.clone(),
        cwd: app_dir.clone(),
        app_dir: app_dir.clone(),
        data_dir: layout.data_dir(),
        adb_path: adb,
        ffmpeg_path: ffmpeg,
        scrcpy_server: app_dir.join("assets").join("scrcpy-server.jar"),
        config_path: layout.config_file(),
        log_path: layout.logs_dir().join("gamer-server.log"),
    };
    let port = supervisor::read_configured_port(&plan.config_path);
    println!("当前版本: {version}");
    println!("入口程序: {}", plan.exe.display());
    println!(
        "就绪探测: http://127.0.0.1:{port}{}",
        supervisor::HEALTH_PATH
    );

    let mut child = match supervisor::spawn_supervised(&plan) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("错误: 启动 server 子进程失败: {e}");
            return 1;
        }
    };
    tracing::info!(pid = child.id(), exe = %plan.exe.display(), "server 子进程已启动");
    println!("server 子进程已启动 (pid={})，等待就绪…", child.id());
    match supervisor::wait_for_ready(port, &ReadyProbe::default()) {
        Ok(()) => println!(
            "[PASS] server 已就绪 (http://127.0.0.1:{port}{})",
            supervisor::HEALTH_PATH
        ),
        Err(reason) => {
            tracing::error!(%reason, "就绪探测未通过（继续持有子进程监管）");
            println!("[WARN] 就绪探测未通过: {reason}（继续持有子进程监管，不按端口判定进程死活）");
        }
    }

    // OPS-003：持有子进程句柄等待退出（不按端口/进程名判定）
    match child.wait() {
        Ok(status) => {
            let code = status.code();
            tracing::info!(?code, "server 子进程退出");
            println!(
                "server 子进程退出（退出码: {}）",
                code.map(|c| c.to_string())
                    .unwrap_or_else(|| "非正常终止".to_string())
            );
            code.unwrap_or(1)
        }
        Err(e) => {
            tracing::error!(error = %e, "等待子进程退出失败");
            eprintln!("错误: 等待子进程退出失败: {e}");
            1
        }
    }
}

// -- manifest 装载（doctor 深检 / repair 共用） -------------------------------

struct ManifestBundle {
    path: PathBuf,
    model: Manifest,
}

/// 装载并完整校验（验签）release manifest：显式 --manifest 优先；否则扫描
/// manifests/ 缓存（匹配当前版本的优先，其余按 SemVer 降序），取第一份通过者。
fn load_manifest_model(
    layout: &InstallLayout,
    cli: &Cli,
    explicit: Option<&Path>,
) -> Result<ManifestBundle, String> {
    let keys_dir = Some(resolve_keys_dir(cli.keys_dir.as_ref(), layout)?);
    let opts = ValidateOptions {
        keys_dir,
        launcher_version: Some(env!("CARGO_PKG_VERSION").to_string()),
        ..ValidateOptions::default()
    };
    let candidates: Vec<PathBuf> = match explicit {
        Some(p) => vec![p.to_path_buf()],
        None => cached_manifest_candidates(layout),
    };
    if candidates.is_empty() {
        return Err(
            "未找到可用 release manifest：用 --manifest 指定，或先把已验签 manifest 放入 manifests/。"
                .to_string(),
        );
    }
    let mut failures = Vec::new();
    for path in candidates {
        let outcome = validate_manifest_file(&path, &opts);
        if outcome.ok {
            let raw = fs::read(&path)
                .map_err(|e| format!("读取 manifest 失败 {}: {e}", path.display()))?;
            let value: Value = serde_json::from_slice(&raw)
                .map_err(|e| format!("manifest 不是合法 JSON（{}）: {e}", path.display()))?;
            let model = Manifest::parse(&value)
                .map_err(|e| format!("manifest 模型解析失败（{}）: {e}", path.display()))?;
            return Ok(ManifestBundle { path, model });
        }
        let codes: Vec<&str> = outcome.errors.iter().map(|e| e.code.as_str()).collect();
        failures.push(format!("{}: {}", path.display(), codes.join(",")));
    }
    Err(format!(
        "候选 manifest 全部校验失败（信任库/签名不匹配或内容非法）:\n  {}",
        failures.join("\n  ")
    ))
}

/// manifests/ 缓存候选：匹配 state/current.json 当前版本的排前，各组内 SemVer 降序。
fn cached_manifest_candidates(layout: &InstallLayout) -> Vec<PathBuf> {
    let dir = layout.manifests_dir();
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    files.sort();
    let current_version = StateStore::new(&layout.root)
        .load_current()
        .ok()
        .and_then(|o| match o {
            LoadOutcome::Present(c) => Some(c.current),
            _ => None,
        });
    let score = |p: &Path| -> (u8, Option<crate::manifest::semver::Semver>) {
        let version = fs::read(p)
            .ok()
            .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
            .and_then(|v| {
                v.get("release")
                    .and_then(|r| r.get("version"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            });
        let matches_current = version
            .as_deref()
            .zip(current_version.as_deref())
            .is_some_and(|(v, c)| v == c);
        (
            u8::from(!matches_current),
            version.as_deref().and_then(crate::manifest::semver::parse),
        )
    };
    let mut scored: Vec<_> = files.into_iter().map(|p| (score(&p), p)).collect();
    scored.sort_by(|a, b| {
        a.0 .0.cmp(&b.0 .0).then_with(|| match (&a.0 .1, &b.0 .1) {
            (Some(sa), Some(sb)) => {
                if crate::manifest::semver::is_lt(sa, sb) {
                    std::cmp::Ordering::Greater
                } else if crate::manifest::semver::is_lt(sb, sa) {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Equal
                }
            }
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        })
    });
    scored.into_iter().map(|(_, p)| p).collect()
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
