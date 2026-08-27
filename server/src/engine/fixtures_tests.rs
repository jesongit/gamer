//! 前端共享 YAML fixture 的引擎侧一致校验（OPTIMIZATION_PLAN QG-003）
//!
//! 消费 `web/src/script-language/fixtures/` 稳定以例清单（9 个业务改编脚本 +
//! templates.txt 虚拟模板权威清单），使同一份输入同时驱动前端 Vitest
//! （`validate.test.js`）与本模块：合法 fixture 双端零错误、模板短名 / #区域
//! 后缀语义、normalize_top 归一化（省略 func: 的映射式函数库）、跨文件函数
//! 解析，任一侧判定翻转即在此报警。fixture 只读，不依赖设备 / ffmpeg。
//!
//! 引擎「零错误」判据口径：预置 stop 标志调用 `Runner::run`——文档级分析管线
//! （normalize_top → func 段摘取与 $N 实参替换 → config 解析 → parse_funcs →
//! steps 提取）与正常执行完全同路径，走完后步骤循环入口即停，
//! 不触发截图 / 点击等任何动作。

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde_yaml::Value;

use super::Runner;

/// 与前端 validate.test.js 一致的分区名（fixtures 内容改写自 com.miHoYo.hkrpg）
const PKG: &str = "com.example.game";
/// 全部 9 个脚本 fixture（含 misc.yml —— README「缺扩展名自动补全用例」目标）
const SCRIPT_FILES: [&str; 9] = [
    "lib_utils.yaml",
    "flow_daily.yaml",
    "common_account.yaml",
    "multi_account.yaml",
    "mail_only.yaml",
    "color_probe.yaml",
    "cn_names.yaml",
    "fn_lib_short.yaml",
    "misc.yml",
];

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../web/src/script-language/fixtures")
        .canonicalize()
        .expect("web/src/script-language/fixtures 目录存在")
}

fn load_script(name: &str) -> String {
    std::fs::read_to_string(fixtures_dir().join("scripts").join(name))
        .unwrap_or_else(|e| panic!("读取 fixture {name}: {e}"))
}

/// templates.txt 权威清单：每行一个磁盘文件名（# 后缀 = 区域元数据）
fn templates_list() -> Vec<String> {
    std::fs::read_to_string(fixtures_dir().join("templates.txt"))
        .expect("templates.txt 存在")
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

fn unique_tmp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "gamer-fixtures-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 消费环境：9 个 fixture 全部落入 ScriptStore 同一分区（跨文件互调可达）+
/// templates.txt 展开为分区虚拟模板目录（占位字节，只用于文件名 / 存在性解析）
fn fixture_env(tag: &str) -> (Runner, PathBuf) {
    let dir = unique_tmp_dir(tag);
    let cfg = crate::config::Config {
        data_dir: dir.clone(),
        ..Default::default()
    };
    let db: crate::store::Db = Arc::new(crate::store::Store::open(&cfg).unwrap());
    let viewers: crate::webrtc::ViewerMap =
        Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let devices = Arc::new(crate::device::DeviceManager::new(
        db,
        cfg.clone(),
        viewers.clone(),
    ));
    let scripts = Arc::new(crate::scripts::ScriptStore::open(&cfg).unwrap());
    for f in SCRIPT_FILES {
        scripts.save(None, PKG, f, &load_script(f)).unwrap();
    }
    let tmpl = scripts.tmpl_dir(PKG);
    std::fs::create_dir_all(&tmpl).unwrap();
    for name in templates_list() {
        std::fs::write(tmpl.join(&name), b"tpl").unwrap();
    }
    (Runner::new(devices, viewers, scripts), dir)
}

/// 预置 stop 的 Runner::run：文档级解析 + 校验全走、步骤循环入口即停。
/// 任何解析 / 校验错误直接 panic 带出 fixture 名与报错内容
async fn stopped_run(
    runner: &Runner,
    name: &str,
    content: &str,
    args: &[&str],
) -> Vec<(String, String)> {
    runner
        .run(
            "dev",
            &format!("{PKG}/{name}"),
            content,
            Arc::new(AtomicBool::new(true)),
            None,
            0,
            None,
            None,
            args.iter().map(|s| s.to_string()).collect(),
        )
        .await
        .unwrap_or_else(|e| {
            panic!("fixture {name} 应通过引擎解析+校验（双端零错误口径），实报: {e}")
        })
}

async fn full_run(runner: &Runner, yaml: &str) -> Vec<(String, String)> {
    runner
        .run(
            "dev",
            &format!("{PKG}/caller_test.yaml"),
            yaml,
            Arc::new(AtomicBool::new(false)),
            None,
            0,
            None,
            None,
            vec![],
        )
        .await
        .unwrap()
}

/// 模板引用采集：find 主模板单字符串（只支持单个，原样不做逗号拆分）、
/// block / cond 字符串（可逗号分隔）或列表；`$N` 占位（运行时实参替换）跳过；
/// 其余兄弟键值递归下钻（then/else/steps/loop/func 体等全部覆盖）
fn collect_tpl_refs(v: &Value, refs: &mut Vec<String>) {
    match v {
        Value::Sequence(seq) => {
            for item in seq {
                collect_tpl_refs(item, refs);
            }
        }
        Value::Mapping(m) => {
            for (k, val) in m {
                match k.as_str() {
                    Some("find") => {
                        if let Some(s) = val.as_str() {
                            let s = s.trim();
                            if !s.starts_with('$') && !s.is_empty() {
                                refs.push(s.to_string());
                            }
                        }
                    }
                    Some("block") | Some("cond") => {
                        let names: Vec<String> = match val {
                            Value::String(s) => {
                                s.split(',').map(|p| p.trim().to_string()).collect()
                            }
                            Value::Sequence(seq) => seq
                                .iter()
                                .filter_map(|i| i.as_str())
                                .map(|s| s.trim().to_string())
                                .collect(),
                            _ => Vec::new(), // 其他形态由执行期校验报错，不在静态扫描范围
                        };
                        refs.extend(
                            names
                                .into_iter()
                                .filter(|n| !n.is_empty() && !n.starts_with('$')),
                        );
                    }
                    _ => collect_tpl_refs(val, refs),
                }
            }
        }
        _ => {}
    }
}

/// 镜像 exec_cross_func 的被引用脚本 func 取段路径：
/// resolve_call（同分区优先、缺扩展名自动补全）→ normalize_top → parse_funcs
fn resolve_cross_fn(
    scripts: &Arc<crate::scripts::ScriptStore>,
    name: &str,
    func: &str,
) -> anyhow::Result<bool> {
    let Some(s) = scripts.resolve_call(PKG, name)? else {
        anyhow::bail!("子脚本不存在: {name}");
    };
    let doc: Value = serde_yaml::from_str(&s.content)?;
    let doc = Runner::normalize_top(doc)?;
    let funcs = Runner::parse_funcs(doc.get("func").filter(|v| !v.is_null()).cloned())?;
    Ok(funcs.contains_key(func))
}

/// templates.txt 权威清单形状：39 个虚拟模板名，覆盖中文稳定命名 / 半区码 /
/// ×1000 坐标后缀 / tpl_dup 歧义对（fixtures/README.md 规范约定）
#[test]
fn templates_txt_authoritative_shape() {
    let list = templates_list();
    assert_eq!(list.len(), 39, "模板清单数量与 README 口径一致");
    for must in [
        "plain_ref.png",
        "tpl_dup#l.png",
        "tpl_dup#r.png",
        "普通界面.png",
        "每日签到#d.png",
        "签到按钮#u.png",
        "acct_136#392_519_526_932.png",
    ] {
        assert!(list.iter().any(|n| n == must), "templates.txt 缺 {must}");
    }
}

/// QG-003 双侧零错误主断言：9 个 fixture 在引擎文档级解析+校验管线全部通过
/// （与前端 validate.test.js「fixture 全量双通过」同一批输入、同一结论）
#[tokio::test]
async fn all_fixtures_parse_and_validate_zero_errors() {
    let (runner, _dir) = fixture_env("zeroerr");
    for f in SCRIPT_FILES {
        // common_account 的 $1 为实参占位：按 multi_account 的真实传法给账号模板名
        let args: &[&str] = if f == "common_account.yaml" {
            &["acct_136.png"]
        } else {
            &[]
        };
        let logs = stopped_run(&runner, f, &load_script(f), args).await;
        // 有 steps 的脚本应停在 stop 门（解析完成后一步未跑）；纯函数库
        // （lib_utils / fn_lib_short / misc）无 steps：记一条提示日志不动作
        if matches!(f, "lib_utils.yaml" | "fn_lib_short.yaml" | "misc.yml") {
            assert!(
                logs.iter().any(|(_, m)| m.contains("纯函数库脚本")),
                "{f} 应识别为纯函数库脚本（无 steps 仅提供函数），logs={logs:?}"
            );
        } else {
            assert!(
                logs.iter().any(|(_, m)| m == "脚本被停止"),
                "{f} 应在解析完成后即被 stop 门停住（不执行任何步骤），logs={logs:?}"
            );
        }
    }
}

/// 每个 fixture 里出现的模板引用（find/block/cond，含函数体与 then/else 子树）
/// 都能在 templates.txt 虚拟清单中经引擎同名规则（精确 / 短名唯一）落到一个真实文件
#[tokio::test]
async fn fixture_template_references_resolve_in_vault() {
    let (_runner, dir) = fixture_env("refs");
    let vault = dir.join(PKG).join("tmpl");
    let mut total_refs = Vec::new();
    for f in SCRIPT_FILES {
        let doc: Value = serde_yaml::from_str(&load_script(f)).unwrap();
        let mut refs = Vec::new();
        collect_tpl_refs(&doc, &mut refs);
        for r in &refs {
            let resolved = Runner::resolve_template_file(&vault, r)
                .unwrap_or_else(|e| panic!("{f} 模板引用 {r} 应命中虚拟清单唯一文件: {e}"));
            assert!(resolved.is_file(), "{f} 模板 {r} 解析结果应为真实文件");
        }
        total_refs.extend(refs);
    }
    total_refs.sort();
    total_refs.dedup();
    // 覆盖面下限：全部 9 个 fixture 合计引用的去重模板数（防扫描静默失效）
    assert!(
        total_refs.len() >= 30,
        "引用覆盖面异常，仅收集到 {} 个不同模板: {total_refs:?}",
        total_refs.len()
    );
}

/// fixtures/README.md 记录的跨文件关系逐一验证：调用方经引擎同名解析
/// （call 全扩展名 / 脚本:函数 缺扩展名）解析到被引用脚本与其 func；
/// 纯日志型函数体 e2e 真跑通（无设备依赖）
#[tokio::test]
async fn cross_file_func_resolution_matches_readme_relations() {
    let (runner, dir) = fixture_env("xfile");

    // flow_daily → lib_utils（3 处跨文件调用）+ 库内第 4 函数 bp_recv 同样可达；
    // misc 与 fn_lib_short 作为被引用目标亦按同一规则命中
    for (sub, func) in [
        ("lib_utils", "mail_recv"),
        ("lib_utils", "itm_recv"),
        ("lib_utils", "ap_burn"),
        ("lib_utils", "bp_recv"),
        ("misc", "ping"),
        ("fn_lib_short", "noop_fn"),
        ("fn_lib_short", "ping_fn"),
    ] {
        assert!(
            resolve_cross_fn(&runner.scripts, sub, func).unwrap(),
            "{sub}:{func} 应解析到函数"
        );
    }
    // 未定义函数：判定报错（与前端「未定义函数 missing_fn」结论一致）
    assert!(
        !resolve_cross_fn(&runner.scripts, "fn_lib_short", "missing_fn").unwrap(),
        "missing_fn 不应存在"
    );

    // call 链：multi_account → common_account.yaml（带 $1 实参）→ flow_daily.yaml
    // call 目标写全扩展名；实参账号模板都在虚拟清单里
    let vault = dir.join(PKG).join("tmpl");
    let ma_doc: Value = serde_yaml::from_str(&load_script("multi_account.yaml")).unwrap();
    let steps = ma_doc.get("steps").unwrap().as_sequence().unwrap();
    let mut acct_args = Vec::new();
    for st in steps {
        let call = st
            .get("call")
            .and_then(Value::as_str)
            .expect("multi_account 步骤应全是 call");
        let mut parts = Runner::split_args(call);
        let target = parts.remove(0);
        let sub = runner
            .scripts
            .resolve_call(PKG, &target)
            .unwrap()
            .unwrap_or_else(|| panic!("call 目标 {target} 应存在"));
        assert_eq!(sub.name, "common_account.yaml", "call 应带扩展名精确命中");
        acct_args.extend(parts);
    }
    assert_eq!(acct_args.len(), 3, "三账号各带一个实参");
    for a in &acct_args {
        Runner::resolve_template_file(&vault, a)
            .unwrap_or_else(|e| panic!("call 实参模板 {a} 应存在于虚拟清单: {e}"));
    }

    // e2e：缺扩展名自动补全 —— `- misc:ping`（README：misc.yml 为该用例调用目标）
    let logs = full_run(&runner, "steps:\n  - misc:ping").await;
    assert!(logs.iter().any(|(_, m)| m == "pong"));

    // e2e：省略 func: 的映射式函数库被跨文件调用（README 已知分歧 2 的引擎口径：
    // exec_cross_func 先对被引用脚本 normalize_top 再取 func 段）
    let logs = full_run(&runner, "steps:\n  - fn_lib_short:noop_fn: 你好").await;
    assert!(logs.iter().any(|(_, m)| m == "你好"), "$1 由调用点实参替换");

    // e2e：无参调用带 then 必须写冒号；fall-through 默认 true → then 执行
    let logs = full_run(
        &runner,
        "steps:\n  - fn_lib_short:ping_fn:\n    then:\n      - log: PONG_THEN",
    )
    .await;
    assert!(logs.iter().any(|(_, m)| m == "pong"));
    assert!(logs.iter().any(|(_, m)| m == "PONG_THEN"));

    // 未定义函数报错文案关键片段与前端一致（未定义函数 missing_fn）
    let err = runner
        .run(
            "dev",
            &format!("{PKG}/caller_test.yaml"),
            "steps:\n  - fn_lib_short:missing_fn",
            Arc::new(AtomicBool::new(false)),
            None,
            0,
            None,
            None,
            vec![],
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("未定义函数 missing_fn"), "实际: {err}");
}

/// normalize_top 归一化语义锚定在 fixture 上：fn_lib_short.yaml（顶层映射、
/// 无段落键、多个函数键）归一化出 func 段并逐键拆成多函数；func 段豁免脚本级
/// $N 替换；run_func 直接运行模式（Console「从某行运行」点击函数名行）可用
#[tokio::test]
async fn normalized_shorthand_fn_lib_fixture_semantics() {
    let raw: Value = serde_yaml::from_str(&load_script("fn_lib_short.yaml")).unwrap();
    let norm = Runner::normalize_top(raw.clone()).unwrap();
    assert!(norm.get("steps").is_none(), "省略写法归一化为纯 func 段");
    let funcs = Runner::parse_funcs(norm.get("func").filter(|v| !v.is_null()).cloned()).unwrap();
    let mut names: Vec<&str> = funcs.keys().map(String::as_str).collect();
    names.sort_unstable();
    assert_eq!(names, ["noop_fn", "ping_fn"], "映射逐键拆分为多函数");
    // noop_fn 体内 $1 原样保留（func 段不参与脚本级替换，调用点才替换）
    assert_eq!(
        funcs["noop_fn"].body[0].get("log").and_then(Value::as_str),
        Some("$1")
    );

    // run_func 直接运行该库的函数体：整体语法与函数命中照常生效
    let (runner, _dir) = fixture_env("normfn");
    let content = load_script("fn_lib_short.yaml");
    let logs = runner
        .run(
            "dev",
            &format!("{PKG}/fn_lib_short.yaml"),
            &content,
            Arc::new(AtomicBool::new(false)),
            None,
            0,
            Some("ping_fn"),
            None,
            vec![],
        )
        .await
        .unwrap();
    assert!(logs.iter().any(|(_, m)| m == "pong"));
}

/// 区域后缀元数据：抽代表名断言 tpl_region_from_name 输出正确
///（半区码 + ×1000 坐标两种形态，含中文模板名的 Unicode 路径；无后缀 = 全屏）
#[test]
fn region_suffix_metadata_representatives() {
    let (w, h) = (1920u32, 1080u32);
    let r = |name: &str| Runner::tpl_region_from_name(name, w, h).unwrap();
    // 半区码（来自 templates.txt 中文稳定命名与歧义对）
    assert_eq!(r("签到按钮#u.png"), Some([0, 0, w, h / 2]));
    assert_eq!(r("每日签到#d.png"), Some([0, h / 2, w, h - h / 2]));
    assert_eq!(r("tpl_dup#r.png"), Some([w / 2, 0, w - w / 2, h]));
    // ×1000 相对坐标（acct 短名引用与区域框选自动命名形态）
    assert_eq!(
        r("acct_136#392_519_526_932.png"),
        Some([753, 561, 257, 446])
    );
    assert_eq!(r("tpl_phone#014_049_047_119.png"), Some([27, 53, 63, 76]));
    // 无后缀 = 全屏（#a 语义）
    assert_eq!(r("plain_ref.png"), None);
    assert_eq!(r("普通界面.png"), None);
}

/// tpl_dup 歧义对：短名解析走「匹配到多个候选」多候选报错路径要求写全名
/// （判定结论与前端 validate.test.js 相同，措辞允许两端差异）；
/// 附短名唯一 / 精确全名 / 不存在的正反例
#[test]
fn dup_pair_multi_candidate_and_short_name_resolution() {
    let dir = unique_tmp_dir("duppair");
    for name in templates_list() {
        std::fs::write(dir.join(&name), b"tpl").unwrap();
    }
    let err = Runner::resolve_template_file(&dir, "tpl_dup.png")
        .unwrap_err()
        .to_string();
    assert!(err.contains("匹配到多个候选"), "{err}");
    assert!(err.contains("请用完整文件名"), "{err}");
    assert!(
        err.contains("tpl_dup#l.png") && err.contains("tpl_dup#r.png"),
        "{err}"
    );

    // 短名唯一命中（英文基名 / 中文基名各一）
    assert_eq!(
        Runner::resolve_template_file(&dir, "tpl_mail_icon.png")
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy(),
        "tpl_mail_icon#946_270_990_343.png"
    );
    assert_eq!(
        Runner::resolve_template_file(&dir, "每日签到.png")
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy(),
        "每日签到#d.png"
    );
    // 精确全名直用
    assert_eq!(
        Runner::resolve_template_file(&dir, "签到按钮#u.png")
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy(),
        "签到按钮#u.png"
    );
    // 不存在报错（判定一致：错误；提示文案由两侧各自实现）
    assert!(Runner::resolve_template_file(&dir, "ghost.png").is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
