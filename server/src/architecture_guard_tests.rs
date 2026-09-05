//! P11.9 Architecture Guard Tests（ADR-11 Core/Extension 最终边界 + ADR-13
//! Runner 归插件 + plan §14）。
//!
//! 把 B1-B4 / W4-A 已落地的架构边界焊死成可回归测试，防止将来把 YAML / Keymap
//! 语义写回 Core：
//!
//! - §14.1 Source Boundary：Core 源码（非扩展内容目录）不得出现 YAML/Keymap
//!   语义符号（`ScriptStore` / `KeymapStore` / `script_v2` / `yaml_vnext` /
//!   `MappingRule` / `YamlTimerRunner` / `parse_script*` …）；`script_id` 按
//!   P11.9 审计结论单列白名单（schema v1 日志列名 + RunRecord 兼容展示字段，
//!   更名属数据迁移另案）。已知合法残留走行级白名单（文件 + 内容片段，双向
//!   校验：条目失效即失败，杜绝死条目）。
//! - §14.2 Dependency Direction：Core 模块（含 capabilities SDK 侧）不得出现
//!   `extensions::gamer_yaml` / `extensions::keymap` 路径；extensions→core 方向
//!   恒合法（编译器强制）。唯一例外是组合根 `main.rs`（装配点）与 `#[cfg(test)]`
//!   夹具，逐行白名单申报。
//! - §14.3 Extension Lifecycle：install→enable→start→stop→disable→uninstall
//!   全链（HTTP 层），每步断言 UI contribution / TimerRunner 注册（含
//!   owner_extension_id）/ 任务 dependency_missing 与自动恢复；另核 disable
//!   运行中=自动 stop 与 reconcile_startup 恢复路径。
//! - §14.4 Bare Core：零已装扩展时全部基础 API 可用（system/info、tasks 全
//!   CRUD、通用资源存取、runners 空、设备列表空），不要求 gamer.yaml /
//!   gamer.keymap 存在。
//! - §14.5 YAML Isolation：无 gamer.yaml → 任务保存成功、派发 424
//!   dependency_missing 且任务保留；安装+启动 → 同一任务自动恢复 Active 且
//!   派发进入执行层（202 + run 记录）。
//! - §14.6 Keymap Isolation：无 gamer.keymap → 输入事件直通（pass-through）。
//!   「有扩展 → 经 keymap runtime 消费」的整合入口是
//!   `extensions/keymap/mod.rs` 的 `real_keymap_gplugin_invokes_wit_and_native_capabilities`
//!   与 `real_keymap_guest_consumes_user_profile_yaml`（真实 fixture guest），
//!   此处不重复造轮子。

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request as HttpRequest, Response as HttpResponse, StatusCode};
use tower::ServiceExt;
use zip::write::SimpleFileOptions;

use crate::config::Config;
use crate::device::DeviceManager;
use crate::extensions::{ExtensionService, InputEvent, InputResult, ScreenSize};
use crate::resources::ResourceStore;
use crate::scheduler::Scheduler;
use crate::store::Db;

// ===========================================================================
// 共享扫描基础设施（§14.1 / §14.2）
// ===========================================================================

/// 本守卫文件自身（扫描豁免：夹具装配必须引用扩展类型构造生产形态环境）。
const SELF_FILE: &str = "architecture_guard_tests.rs";
/// 扩展内容语义目录：YAML 栈与 Keymap 栈的物理归属（ADR-11）。
const EXTENSION_SOURCE_DIRS: &[&str] = &["extensions/gamer_yaml", "extensions/keymap"];
/// 若存在亦豁免的扩展内文件（当前不存在，防未来漂移）。
const EXTENSION_SOURCE_FILES: &[&str] = &["extensions/gamer_yaml/wasm_host.rs"];

struct SourceFile {
    /// 相对 server/src/ 的 POSIX 风格路径（与白名单条目的 file 字段比对）。
    file: String,
    lines: Vec<String>,
}

/// 收集受守卫的源文件：server/src/**/*.rs，减去扩展内容目录与本文件。
fn guarded_sources() -> Vec<SourceFile> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("读取目录 {} 失败: {e}", dir.display()))
            .map(|entry| entry.expect("目录项可读").path())
            .collect();
        entries.sort();
        for entry in entries {
            let rel = relative_to_src(&entry);
            if entry.is_dir() {
                if !EXTENSION_SOURCE_DIRS.contains(&rel.as_str()) {
                    stack.push(entry);
                }
                continue;
            }
            if !entry.extension().is_some_and(|ext| ext == "rs") {
                continue;
            }
            if rel == SELF_FILE || EXTENSION_SOURCE_FILES.contains(&rel.as_str()) {
                continue;
            }
            let text = std::fs::read_to_string(&entry)
                .unwrap_or_else(|e| panic!("读取 {} 失败: {e}", entry.display()));
            files.push(SourceFile {
                file: rel,
                lines: text.lines().map(str::to_string).collect(),
            });
        }
    }
    files
}

/// 相对 server/src/ 的路径（POSIX 分隔符），如 `api/tests.rs`。
fn relative_to_src(path: &Path) -> String {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    path.strip_prefix(&src)
        .expect("路径必在 src/ 下")
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("/")
}

/// 行级白名单条目：`file` 内 `snippet`（对原始行做 contains 匹配）。
/// 每条必须带 reason；双向校验（见 [`assert_whitelist_alive`]）保证条目不腐烂。
struct Allow {
    file: &'static str,
    snippet: &'static str,
    reason: &'static str,
}

/// 白名单是否放行一行。
fn allowed(allows: &[Allow], file: &str, line: &str) -> bool {
    allows
        .iter()
        .any(|a| a.file == file && line.contains(a.snippet))
}

/// 白名单活性校验：每条 entry 必须仍命中其文件中的至少一行，否则说明引用的
/// 代码已改/已删——删除或更新条目，而不是留着永不命中的死条目。
fn assert_whitelist_alive(sources: &[SourceFile], allows: &[Allow]) {
    for allow in allows {
        let hit = sources.iter().any(|source| {
            source.file == allow.file
                && source.lines.iter().any(|line| line.contains(allow.snippet))
        });
        assert!(
            hit,
            "白名单条目已失效：{} 中的 {:?}（{}）不再命中任何行；请删除条目或复核该处代码",
            allow.file, allow.snippet, allow.reason
        );
    }
}

/// 词边界命中（手写，避免引入 regex 依赖）：`parse_script` 不误伤
/// `parse_script_file`。
fn contains_word(hay: &str, needle: &str) -> bool {
    let bytes = hay.as_bytes();
    let mut start = 0;
    while let Some(pos) = hay[start..].find(needle) {
        let abs = start + pos;
        let end = abs + needle.len();
        let boundary_before = abs == 0 || !is_word_byte(bytes[abs - 1]);
        let boundary_after = end >= bytes.len() || !is_word_byte(bytes[end]);
        if boundary_before && boundary_after {
            return true;
        }
        start = abs + 1;
    }
    false
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

// ===========================================================================
// §14.1 Source Boundary Test
// ===========================================================================

/// Core 源码禁符（子串匹配；均为唯一性足够强的语义符号）。
const BOUNDARY_SUBSTRINGS: &[&str] = &[
    "ScriptStore",
    "KeymapStore",
    "MappingRule",
    "YamlTimerRunner",
    "yaml_vnext",
    "script_v2",
];
/// Core 源码禁符（词边界匹配）。
const BOUNDARY_WORDS: &[&str] = &["parse_script", "parse_function_file"];

/// §14.1 已知合法残留（行级）：见各条 reason。纯陈旧引用（把已删类型当现役
/// 实现来描述）不进白名单——已随本守卫一并改写（capabilities/adapters/mod.rs、
/// app_packages/composite.rs）。
const BOUNDARY_ALLOWS: &[Allow] = &[
    // —— 生产：组合根（main.rs）是全仓唯一允许点名扩展类型的装配点 ——
    Allow {
        file: "main.rs",
        snippet: "YamlTimerRunnerRegistrar::new(",
        reason: "组合根：gamer.yaml 的 TimerRunner registrar 注入 ExtensionService（ADR-13 注册缝）",
    },
    Allow {
        file: "main.rs",
        snippet: "executor.attach_yaml_vnext(",
        reason: "组合根：v3 适配器装配（EngineExecutor 门面方法，v3 脚本运行必需）",
    },
    // —— 生产：历史命名注释（「消解后」框架，陈述迁移事实） ——
    Allow {
        file: "resources.rs",
        snippet: "//! ScriptStore / KeymapStore 消解后的内容无关资源层",
        reason: "历史注释：说明本层是两 Store 消解后的替代物",
    },
    Allow {
        file: "api/mod.rs",
        snippet: "P11.3：ScriptStore/KeymapStore 消解后的 Core 侧唯一资源层",
        reason: "历史注释：AppState.resources 字段文档",
    },
    // —— 扩展机制文件（extensions/service.rs，非 gamer_yaml 目录）——
    Allow {
        file: "extensions/service.rs",
        snippet: "gamer.yaml 的 run_yaml_vnext",
        reason: "扩展机制 doc 注释：instance_free 模型举例（门面函数名）",
    },
    Allow {
        file: "extensions/service.rs",
        snippet: "YamlTimerRunnerRegistrar::new(",
        reason: "extensions/service.rs #[cfg(test)]：启动对账测试的 gamer.yaml registrar 夹具（两处同形）",
    },
    // —— 测试代码（#[cfg(test)] 模块/测试文件；夹具允许构造扩展类型） ——
    Allow {
        file: "phase0_tests.rs",
        snippet: "use crate::extensions::gamer_yaml::script_v2::{",
        reason: "Phase 0 夹具护栏（cfg(test)）：调用正式装载器校验仓库 fixtures",
    },
    Allow {
        file: "phase0_tests.rs",
        snippet: "parse_script_file, serialize_script, InMemoryResources,",
        reason: "Phase 0 夹具护栏：use 续行",
    },
    Allow {
        file: "phase0_tests.rs",
        snippet: "parse_script_file(&source",
        reason: "Phase 0 夹具护栏：fixture round-trip",
    },
    Allow {
        file: "phase0_tests.rs",
        snippet: "parse_script_file(&serialized",
        reason: "Phase 0 夹具护栏：fixture 回读",
    },
    Allow {
        file: "api/tests.rs",
        snippet: "YamlTimerRunner::new(",
        reason: "api 测试装配：与生产等价预注册 gamer.yaml runner（HTTP 集成夹具）",
    },
    Allow {
        file: "api/tests/app_packages_lifecycle.rs",
        snippet: "script_v2::validate::TemplateAvail::Found",
        reason: "app-packages 生命周期测试：模板可用性桩",
    },
    Allow {
        file: "api/tests/extensions.rs",
        snippet: "run_yaml_vnext",
        reason: "扩展 API 测试注释：指向 yaml 运行验收的整合入口",
    },
    Allow {
        file: "api/tests/update.rs",
        snippet: "YamlTimerRunner::new(",
        reason: "update API 测试装配：gamer.yaml runner 夹具",
    },
];

/// `script_id` 白名单——审计结论（2026-09-05，P11.9）：Core 侧 `script_id` 仅剩
/// （a）`RunRecord.script_id` 兼容展示字段及其 HTTP/busy/日志映射（RunManager
/// 明确「not interpreted」，无任何解析语义）；（b）schema v1 运行日志表列名与
/// legacy 表 SQL 字符串。**schema v1 日志列名，更名属数据迁移另案**；除此之外
/// 新增的任何 `script_id` 出现都会被本测试拦下复核。
const SCRIPT_ID_ALLOWS: &[Allow] = &[
    // —— run_manager.rs：RunRecord 兼容展示字段 + busy/日志映射 + 测试 ——
    Allow {
        file: "run_manager.rs",
        snippet: "`script_id` below is retained as the legacy",
        reason: "RunRecord 字段文档：兼容展示字段声明",
    },
    Allow {
        file: "run_manager.rs",
        snippet: "pub script_id: String,",
        reason: "RunRecord.script_id 字段定义（HTTP 契约兼容展示字段）",
    },
    Allow {
        file: "run_manager.rs",
        snippet: "\"script_id\": self.script_id,",
        reason: "busy 409 响应 JSON 键映射（前端契约）",
    },
    Allow {
        file: "run_manager.rs",
        snippet: "script_id: req.display_label().to_string(),",
        reason: "RunRecord 构造：展示标签透传（RunManager 不解读内容）",
    },
    Allow {
        file: "run_manager.rs",
        snippet: "script_id: String::new(),",
        reason: "查无 run 时的占位 RunRecord",
    },
    Allow {
        file: "run_manager.rs",
        snippet: "script = %rec.script_id,",
        reason: "tracing 日志字段映射",
    },
    Allow {
        file: "run_manager.rs",
        snippet: "fn req(device_id: &str, script_id: &str",
        reason: "cfg(test) 夹具：StartRequest 构造助手",
    },
    Allow {
        file: "run_manager.rs",
        snippet: "script_id.split('/')",
        reason: "cfg(test) 夹具：从 id 提取分区名造 AppContext",
    },
    Allow {
        file: "run_manager.rs",
        snippet: "script_id,",
        reason: "cfg(test) 断言：展示标签回归（busy/历史记录）",
    },
    // —— store.rs：schema v1 运行日志列名 + legacy 表 SQL（数据迁移另案） ——
    Allow {
        file: "store.rs",
        snippet: "script_id: String,",
        reason: "RunLogRecord 字段（logs 表 schema v1 列映射）",
    },
    Allow {
        file: "store.rs",
        snippet: "script_id TEXT NOT NULL,",
        reason: "schema v1 DDL：logs/tasks 表列",
    },
    Allow {
        file: "store.rs",
        snippet: "(\"script_id\", \"TEXT\", 1, 0),",
        reason: "schema v1 列清单断言",
    },
    Allow {
        file: "store.rs",
        snippet: "instr(script_id, '/')",
        reason: "legacy 视图 SQL：从 script_id 提取分区（v1 数据只读迁移路径）",
    },
    Allow {
        file: "store.rs",
        snippet: "substr(script_id, 1, instr(script_id, '/') - 1)",
        reason: "legacy 视图 SQL：同上",
    },
    Allow {
        file: "store.rs",
        snippet: "删除 legacy `tasks` 表（script_id+cron 旧契约整体退役",
        reason: "迁移注释：legacy 契约退役说明",
    },
    Allow {
        file: "store.rs",
        snippet: "INSERT INTO logs (time, device_id, script_id, level, msg)",
        reason: "运行日志写入：schema v1 列名",
    },
    Allow {
        file: "store.rs",
        snippet: "log.record.script_id,",
        reason: "日志写入参数绑定",
    },
    Allow {
        file: "store.rs",
        snippet: "script_id: &str,",
        reason: "日志查询助手参数（v1 列名）",
    },
    Allow {
        file: "store.rs",
        snippet: "script_id: script_id.to_string(),",
        reason: "日志记录构造（v1 列名映射）",
    },
    Allow {
        file: "store.rs",
        snippet: "SELECT id, time, device_id, script_id, level, msg FROM logs",
        reason: "日志查询 SQL（v1 列名）",
    },
    Allow {
        file: "store.rs",
        snippet: "script_id: r.get(3)?,",
        reason: "日志行反序列化（v1 列名）",
    },
    Allow {
        file: "store.rs",
        snippet: "INSERT INTO logs(time, device_id, script_id, level, msg)",
        reason: "cfg(test)：日志写入夹具",
    },
    Allow {
        file: "store.rs",
        snippet: "device_id TEXT NOT NULL, script_id TEXT NOT NULL, level TEXT NOT NULL,",
        reason: "cfg(test)：v1 表 DDL 夹具",
    },
    Allow {
        file: "store.rs",
        snippet: "(id, name, cron, script_id, device_id, enabled, created_at, args_json, param_signature)",
        reason: "cfg(test)：legacy tasks 表行夹具（该表已随迁移退役）",
    },
    Allow {
        file: "store.rs",
        snippet: "\"script_id\", \"type\": \"TEXT\"",
        reason: "cfg(test)：PRAGMA 列断言（v1 schema 锚定）",
    },
    Allow {
        file: "store.rs",
        snippet: "    script_id,",
        reason: "legacy 视图 SQL 列清单（v1 只读迁移路径）",
    },
    // —— 其他映射点 ——
    Allow {
        file: "capabilities/adapters/run.rs",
        snippet: "record.run_id, record.script_id",
        reason: "capability run.submit 冲突消息映射（busy 展示字段透传）",
    },
    Allow {
        file: "update/service.rs",
        snippet: "device_id=system / script_id=update",
        reason: "审计日志落点注释：复用 v1 日志表列约定",
    },
    Allow {
        file: "api/mod.rs",
        snippet: "script_id+cron",
        reason: "路由文档注释：legacy /api/tasks 契约已收口",
    },
    Allow {
        file: "api/tests/runs.rs",
        snippet: "j[\"script_id\"]",
        reason: "HTTP 契约断言：RunRecord 兼容展示字段",
    },
    Allow {
        file: "api/tests/tasks.rs",
        snippet: "script_id/cron",
        reason: "注释：旧平铺字段不得复现",
    },
    Allow {
        file: "api/tests/tasks.rs",
        snippet: "created.get(\"script_id\")",
        reason: "HTTP 契约断言：任务响应不含 legacy 平铺字段",
    },
];

/// §14.1：Core（非扩展内容目录）源码不得出现 YAML/Keymap 语义符号。
/// 新增命中 = 把业务语义写回了 Core，一律视为退化（ADR-11）。
#[test]
fn architecture_guard_source_boundary_core_free_of_yaml_keymap_semantics() {
    let sources = guarded_sources();
    let mut violations: Vec<String> = Vec::new();

    for source in &sources {
        for (no, line) in source.lines.iter().enumerate() {
            let mut hit_tokens: Vec<&str> = Vec::new();
            for token in BOUNDARY_SUBSTRINGS {
                if line.contains(token) {
                    hit_tokens.push(token);
                }
            }
            for token in BOUNDARY_WORDS {
                if contains_word(line, token) {
                    hit_tokens.push(token);
                }
            }
            if hit_tokens.is_empty() || allowed(BOUNDARY_ALLOWS, &source.file, line) {
                continue;
            }
            violations.push(format!(
                "{}:{}: 禁符 {:?}（ADR-11：该语义归扩展；如属合法残留请加行级白名单并写明理由）\n    {}",
                source.file,
                no + 1,
                hit_tokens,
                line.trim()
            ));
        }
    }

    // script_id 单独审计：仅允许「schema v1 日志列名 + RunRecord 兼容展示字段」。
    for source in &sources {
        for (no, line) in source.lines.iter().enumerate() {
            if !line.contains("script_id") {
                continue;
            }
            if allowed(SCRIPT_ID_ALLOWS, &source.file, line) {
                continue;
            }
            violations.push(format!(
                "{}:{}: Core 出现未申报的 script_id（审计结论：Core 侧仅存 RunRecord 兼容展示字段与 schema v1 日志列名，更名属数据迁移另案；新增出现需复核是否把脚本语义写回 Core）\n    {}",
                source.file,
                no + 1,
                line.trim()
            ));
        }
    }

    assert_whitelist_alive(&sources, BOUNDARY_ALLOWS);
    assert_whitelist_alive(&sources, SCRIPT_ID_ALLOWS);

    assert!(
        violations.is_empty(),
        "Core 源码边界被破坏（{} 处）：\n{}",
        violations.len(),
        violations.join("\n")
    );
}

// ===========================================================================
// §14.2 Dependency Direction Test
// ===========================================================================

/// Core 模块禁止出现的扩展内部路径（`use` 语句与内联路径同等对待；
/// `crate::extensions::X` 门面再导出不算违规——那是机制层定义的窄缝）。
const DEPENDENCY_PATTERNS: &[&str] = &["extensions::gamer_yaml", "extensions::keymap"];

/// §14.2 白名单：组合根装配点 + cfg(test) 夹具，逐条申报。
const DEPENDENCY_ALLOWS: &[Allow] = &[
    // —— 生产：组合根（main.rs 的 RuntimeServices::start）是唯一装配点 ——
    Allow {
        file: "main.rs",
        snippet: "extensions::gamer_yaml::register_resource_handlers",
        reason: "组合根：注册 gamer.yaml 资源内容校验钩子（裸 Core 不注册则保存不做内容校验）",
    },
    Allow {
        file: "main.rs",
        snippet: "extensions::gamer_yaml::engine::Runner::new",
        reason: "组合根：gamer.yaml 引擎 Runner 注入 RunManager 执行器",
    },
    Allow {
        file: "main.rs",
        snippet: "extensions::gamer_yaml::engine::EngineExecutor::new",
        reason: "组合根：RunExecutor 适配器构造",
    },
    Allow {
        file: "main.rs",
        snippet: "extensions::gamer_yaml::timer_yaml::YamlTimerRunnerRegistrar::new",
        reason: "组合根：ADR-13 registrar 钩子注入 ExtensionService",
    },
    // —— 注释 ——
    Allow {
        file: "api/runs.rs",
        snippet: "`extensions::gamer_yaml::timer_yaml`",
        reason: "api/runs.rs 文档注释：指向 runner 的实现归属",
    },
    // —— cfg(test) 夹具（构造与生产等价的运行环境） ——
    Allow {
        file: "phase0_tests.rs",
        snippet: "use crate::extensions::gamer_yaml::script_v2::{",
        reason: "Phase 0 夹具护栏：调用正式装载器",
    },
    Allow {
        file: "api/tests.rs",
        snippet: "gamer_yaml::engine::EngineExecutor::new",
        reason: "api 测试装配（生产等价执行器）",
    },
    Allow {
        file: "api/tests.rs",
        snippet: "gamer_yaml::engine::Runner::new",
        reason: "api 测试装配（生产等价执行器）",
    },
    Allow {
        file: "api/tests.rs",
        snippet: "gamer_yaml::timer_yaml::YamlTimerRunner::new",
        reason: "api 测试装配：预注册 gamer.yaml runner",
    },
    Allow {
        file: "api/tests.rs",
        snippet: "gamer_yaml::register_resource_handlers",
        reason: "api 测试装配：与生产一致的资源内容钩子",
    },
    Allow {
        file: "api/update.rs",
        snippet: "gamer_yaml::engine::EngineExecutor::new",
        reason: "update API cfg(test) 装配（两处同形）",
    },
    Allow {
        file: "api/update.rs",
        snippet: "gamer_yaml::engine::Runner::new",
        reason: "update API cfg(test) 装配（两处同形）",
    },
    Allow {
        file: "api/tests/app_packages_edit.rs",
        snippet: "gamer_yaml::engine::snapshot::RunSnapshot::capture",
        reason: "编辑提取测试：构造运行快照夹具",
    },
    Allow {
        file: "api/tests/app_packages_edit.rs",
        snippet: "gamer_yaml::engine::snapshot::RunResources::new",
        reason: "编辑提取测试：快照资源视图夹具",
    },
    Allow {
        file: "api/tests/app_packages_lifecycle.rs",
        snippet: "gamer_yaml::engine::snapshot::RunSnapshot::capture",
        reason: "包生命周期测试：运行快照夹具",
    },
    Allow {
        file: "api/tests/app_packages_lifecycle.rs",
        snippet: "gamer_yaml::engine::snapshot::RunResources::new",
        reason: "包生命周期测试：快照资源视图夹具",
    },
    Allow {
        file: "api/tests/app_packages_lifecycle.rs",
        snippet: "gamer_yaml::script_v2::validate::TemplateAvail::Found",
        reason: "包生命周期测试：模板可用性桩",
    },
    Allow {
        file: "api/tests/extensions.rs",
        snippet: "gamer_yaml::engine::EngineExecutor::new",
        reason: "extensions API 测试装配（capability registry 检查）",
    },
    Allow {
        file: "api/tests/extensions.rs",
        snippet: "gamer_yaml::engine::Runner::new",
        reason: "extensions API 测试装配（capability registry 检查）",
    },
    Allow {
        file: "api/tests/update.rs",
        snippet: "gamer_yaml::timer_yaml::YamlTimerRunner::new",
        reason: "update API 测试装配：gamer.yaml runner 夹具",
    },
    Allow {
        file: "app_packages/builder.rs",
        snippet: "gamer_yaml::register_resource_handlers",
        reason: "PackageBuilder cfg(test)：与生产一致的资源内容钩子",
    },
];

/// §14.2：Core 模块（含 capabilities SDK 侧）不得路径依赖扩展内部；
/// extensions → core 方向恒合法（编译期即强制，无需测试）。唯一例外是组合根
/// 装配点与 cfg(test) 夹具，逐行白名单申报。扫描范围为全部非 extensions/ 模块。
#[test]
fn architecture_guard_dependency_direction_core_never_paths_into_extension_internals() {
    let sources = guarded_sources();
    let mut violations: Vec<String> = Vec::new();

    for source in &sources {
        // 扩展机制自身（extensions/）不在本测试范围（ADR-11 机制层归 Core，
        // 但其内部引用扩展实现属同目录装配；§14.1 已覆盖其禁符）。
        if source.file.starts_with("extensions/") {
            continue;
        }
        for (no, line) in source.lines.iter().enumerate() {
            let hits: Vec<&str> = DEPENDENCY_PATTERNS
                .iter()
                .filter(|pattern| line.contains(*pattern))
                .copied()
                .collect();
            if hits.is_empty() || allowed(DEPENDENCY_ALLOWS, &source.file, line) {
                continue;
            }
            violations.push(format!(
                "{}:{}: Core 路径依赖扩展内部 {:?}（依赖方向必须 extensions→core；组合根装配与测试夹具以外的引用一律违规）\n    {}",
                source.file,
                no + 1,
                hits,
                line.trim()
            ));
        }
    }

    assert_whitelist_alive(&sources, DEPENDENCY_ALLOWS);

    assert!(
        violations.is_empty(),
        "Core→Extension 依赖方向被破坏（{} 处）：\n{}",
        violations.len(),
        violations.join("\n")
    );
}

// ===========================================================================
// HTTP 集成共享装配（§14.3 / §14.4 / §14.5）
// ===========================================================================

/// gamer.yaml 扩展 id（安装/生命周期路由参数）。
const YAML_ID: &str = "gamer.yaml";
/// gamer.yaml 市场包 manifest 版本（见 YAML_EXTENSION_MANIFEST_TOML）。
const YAML_VERSION: &str = "3.0.0";

/// 生产形态 Core 依赖（目录生命周期归 CoreDeps：GuardApp 只是路由视图，同一
/// CoreDeps 可先后装配多个「进程」——启动对账路径依赖这一点）。
struct CoreDeps {
    dir: PathBuf,
    cfg: Config,
    db: Db,
    resources: Arc<ResourceStore>,
    devices: Arc<DeviceManager>,
    viewers: crate::webrtc::ViewerMap,
}

impl Drop for CoreDeps {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// 生产形态 Core 依赖（DeviceManager 只构造不 start，无 adb 扫描副作用）。
fn build_core(tag: &str) -> CoreDeps {
    let dir = std::env::temp_dir().join(format!(
        "gamer-guard-{tag}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = Config {
        data_dir: dir.clone(),
        ..Default::default()
    };
    let db: Db = Arc::new(crate::store::Store::open(&cfg).unwrap());
    let resources = Arc::new(ResourceStore::open(&cfg).unwrap());
    let viewers: crate::webrtc::ViewerMap =
        Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let devices = Arc::new(DeviceManager::new(db.clone(), cfg.clone()));
    CoreDeps {
        dir,
        cfg,
        db,
        resources,
        devices,
        viewers,
    }
}

struct GuardApp {
    app: axum::Router,
    extensions: Arc<ExtensionService>,
}

/// 与生产组合根 `RuntimeServices::start` 同构的 HTTP 装配。
fn build_app(core: &CoreDeps) -> GuardApp {
    // 与生产一致：注册扩展内容校验钩子（保存校验/注记）。
    crate::extensions::gamer_yaml::register_resource_handlers(&core.resources);
    crate::extensions::register_resource_handlers(&core.resources);
    let runner = Arc::new(crate::extensions::gamer_yaml::engine::Runner::new(
        core.devices.clone(),
        Arc::new(crate::webrtc::ViewerEventSink::new(core.viewers.clone())),
        core.resources.clone(),
    ));
    let executor = Arc::new(crate::extensions::gamer_yaml::engine::EngineExecutor::new(
        runner,
        core.devices.clone(),
        core.db.clone(),
    ));
    let runs = Arc::new(crate::run_manager::RunManager::new(executor.clone()));
    // ADR-13：裸 Core 组合——Scheduler 不预置任何 runner；gamer.yaml 的定时
    // runner 由扩展 start 生命周期经 registrar 钩子注册。
    let scheduler = Arc::new(Scheduler::new(core.db.clone()));
    let capabilities = crate::capabilities::adapters::build_registry(
        core.devices.clone(),
        core.resources.clone(),
        core.db.clone(),
        runs.clone(),
    );
    let registrar = Arc::new(
        crate::extensions::gamer_yaml::timer_yaml::YamlTimerRunnerRegistrar::new(
            scheduler.clone(),
            core.db.clone(),
            runs.clone(),
            core.resources.clone(),
        ),
    );
    let extensions = Arc::new(
        ExtensionService::for_data_root(core.cfg.data_dir.clone(), capabilities)
            .with_runner_registrar(registrar),
    );
    executor.attach_yaml_vnext(core.resources.clone(), extensions.clone(), None);
    let auth = Arc::new(crate::api::auth::AuthState::new(
        test_credential(),
        Default::default(),
        false,
        Some("guard-test-token".into()),
    ));
    let shutdown = Arc::new(crate::shutdown::ShutdownCoordinator::new(Arc::new(|| {
        Box::pin(async {})
    })));
    let policy_store = crate::update::policy::PolicyStore::load_blocking(
        &core.cfg.data_dir,
        crate::update::policy::UpdatePolicy::default(),
    );
    let update = Arc::new(crate::update::service::UpdateService::new(
        Arc::new(crate::update::controller::UnsupportedController),
        policy_store,
        Arc::new(crate::update::service::UpdateTxn::default()),
        Arc::new(crate::update::workload::Workload::default),
        core.db.clone(),
    ));
    let app = crate::api::build_router_with_extensions(
        core.db.clone(),
        core.devices.clone(),
        runs,
        scheduler,
        core.cfg.clone(),
        core.viewers.clone(),
        core.resources.clone(),
        shutdown,
        auth,
        update,
        extensions.clone(),
    );
    GuardApp { app, extensions }
}

fn test_credential() -> crate::api::auth::Credential {
    crate::api::auth::parse_password_hash(
        &crate::api::auth::hash_password("guard-admin123").unwrap(),
    )
    .unwrap()
}

// ---------- HTTP 助手（tower oneshot 直驱，无真实端口） ----------

async fn send(app: &axum::Router, request: HttpRequest<Body>) -> HttpResponse<Body> {
    app.clone().oneshot(request).await.unwrap()
}

fn request(
    method: &str,
    uri: &str,
    headers: &[(String, String)],
    body: Option<Vec<u8>>,
) -> HttpRequest<Body> {
    let mut builder = HttpRequest::builder().method(method).uri(uri);
    for (key, value) in headers {
        builder = builder.header(key.as_str(), value);
    }
    match body {
        Some(bytes) => builder.body(Body::from(bytes)).unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

fn json_headers(cookie: &str) -> Vec<(String, String)> {
    vec![
        (header::COOKIE.to_string(), cookie.to_string()),
        (header::CONTENT_TYPE.to_string(), "application/json".into()),
    ]
}

fn zip_headers(cookie: &str) -> Vec<(String, String)> {
    vec![
        (header::COOKIE.to_string(), cookie.to_string()),
        (header::CONTENT_TYPE.to_string(), "application/zip".into()),
        // gamer.yaml manifest 声明完整权限集：全新安装需要显式权限确认。
        ("x-gamer-permission-confirm".to_string(), "1".into()),
    ]
}

/// 登录并返回会话 cookie（guard 装配的凭据 = guard-admin123）。
async fn login(app: &axum::Router) -> String {
    let response = send(
        app,
        request(
            "POST",
            "/api/login",
            &[(header::CONTENT_TYPE.to_string(), "application/json".into())],
            Some(br#"{"username":"admin","password":"guard-admin123"}"#.to_vec()),
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK, "登录必须成功");
    response
        .headers()
        .get(header::SET_COOKIE)
        .map(|value| {
            value
                .to_str()
                .unwrap()
                .split(';')
                .next()
                .unwrap()
                .to_string()
        })
        .expect("登录响应必带 Set-Cookie")
}

async fn get_json(app: &axum::Router, cookie: &str, uri: &str) -> serde_json::Value {
    let response = send(app, request("GET", uri, &json_headers(cookie), None)).await;
    let status = response.status();
    let value = body_json(response).await;
    assert!(
        status.is_success(),
        "GET {uri} 应成功，得到 {status}：{value}"
    );
    value
}

async fn body_json(response: HttpResponse<Body>) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("响应不是合法 JSON: {e}"))
}

async fn post_json(
    app: &axum::Router,
    cookie: &str,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response = send(
        app,
        request(
            "POST",
            uri,
            &json_headers(cookie),
            Some(body.to_string().into_bytes()),
        ),
    )
    .await;
    let status = response.status();
    (status, body_json(response).await)
}

fn task_body(name: &str, runner_id: &str, entrypoint: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "app": {
            "device_id": "device-1",
            "android_package": "com.guard.app",
            "content_package": "com.guard.app"
        },
        "runner": {"runner_id": runner_id, "entrypoint": entrypoint, "payload": {"args": {}}},
        "schedule": {"provider_id": "cron", "config": {"expression": "0 8 * * *"}},
        "enabled": true
    })
}

/// gamer.yaml 安装包：无实例执行模型下 start 不读 guest 字节，占位 wasm 即可
/// 完成完整生命周期（真实 v3 运行验收在 gamer_yaml 扩展自身的端到端测试）。
fn gamer_yaml_archive() -> Vec<u8> {
    let mut archive = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut archive));
        let options = SimpleFileOptions::default();
        writer
            .start_file("manifest.toml", options)
            .expect("zip 写入 manifest");
        writer
            .write_all(crate::extensions::gamer_yaml::YAML_EXTENSION_MANIFEST_TOML.as_bytes())
            .unwrap();
        writer.start_file("plugin.wasm", options).unwrap();
        writer.write_all(b"\0asm\x01\0\0\0").unwrap();
        writer.finish().unwrap();
    }
    archive
}

async fn install_enable_start(app: &axum::Router, cookie: &str) {
    let response = send(
        app,
        request(
            "POST",
            "/api/extensions",
            &zip_headers(cookie),
            Some(gamer_yaml_archive()),
        ),
    )
    .await;
    let status = response.status();
    assert_eq!(status, StatusCode::CREATED, "安装 gamer.yaml");
    for action in ["enable", "start"] {
        let (status, body) = post_json(
            app,
            cookie,
            &format!("/api/extensions/{YAML_ID}/{action}"),
            serde_json::json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{action}: {body}");
    }
}

// ===========================================================================
// §14.3 Extension Lifecycle Test
// ===========================================================================

/// ADR-13 全链生命周期（HTTP 层）：install→enable→start→stop→disable→uninstall，
/// 每步断言 UI contribution、TimerRunner 注册（owner_extension_id）、任务
/// dependency_missing / 自动恢复；另核 disable 运行中=自动 stop，以及
/// reconcile_startup 恢复路径（遗留 Running 记录 → runner 重注册 + 任务恢复）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn architecture_guard_lifecycle_extension_full_chain_binds_ui_runner_and_tasks() {
    let core = build_core("lifecycle");
    let guard = build_app(&core);
    let cookie = login(&guard.app).await;

    // ---- install：只落盘，无 UI、无 runner；任务可先于 runner 存在（ADR-12） ----
    let response = send(
        &guard.app,
        request(
            "POST",
            "/api/extensions",
            &zip_headers(&cookie),
            Some(gamer_yaml_archive()),
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED, "安装 gamer.yaml");
    assert_eq!(body_json(response).await["state"], "installed");

    let list = get_json(&guard.app, &cookie, "/api/extensions").await;
    assert_eq!(list["extensions"].as_array().unwrap().len(), 1);
    assert_eq!(list["extensions"][0]["state"], "installed");
    assert!(
        list["ui_contributions"].as_array().unwrap().is_empty(),
        "install 不发布 UI"
    );
    assert!(
        get_json(&guard.app, &cookie, "/api/runners")
            .await
            .as_array()
            .unwrap()
            .is_empty(),
        "install 后 runner 注册表为空"
    );

    let (status, created) = post_json(
        &guard.app,
        &cookie,
        "/api/tasks",
        task_body("Guard Daily", YAML_ID, "com.guard.app/daily.yaml"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let task_id = created["id"].as_str().unwrap().to_string();

    // ---- enable：UI contribution 出现，runner 仍不注册（enable ≠ start） ----
    let (status, body) = post_json(
        &guard.app,
        &cookie,
        &format!("/api/extensions/{YAML_ID}/enable"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let ui = get_json(&guard.app, &cookie, "/api/extensions/ui").await;
    let mut panels: Vec<String> = ui
        .as_array()
        .unwrap()
        .iter()
        .map(|panel| panel["panel_id"].as_str().unwrap().to_string())
        .collect();
    panels.sort();
    assert_eq!(
        panels,
        vec!["automation", "functions", "templates"],
        "gamer.yaml 的 core-runtime 面板随 enable 发布"
    );
    assert!(
        get_json(&guard.app, &cookie, "/api/runners")
            .await
            .as_array()
            .unwrap()
            .is_empty(),
        "enable 不注册 runner（ADR-13：runner 随 start 生命周期）"
    );

    // ---- start：runner 注册（owner_extension_id 锁归属） ----
    let (status, started) = post_json(
        &guard.app,
        &cookie,
        &format!("/api/extensions/{YAML_ID}/start"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{started}");
    assert_eq!(started["state"], "running");
    let runners = get_json(&guard.app, &cookie, "/api/runners").await;
    assert_eq!(runners.as_array().unwrap().len(), 1);
    assert_eq!(runners[0]["runner_id"], YAML_ID);
    assert_eq!(runners[0]["owner_extension_id"], YAML_ID);

    // ---- stop：runner 注销 → 任务转 dependency_missing 但保留；UI 仍在（enabled） ----
    let (status, body) = post_json(
        &guard.app,
        &cookie,
        &format!("/api/extensions/{YAML_ID}/stop"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        get_json(&guard.app, &cookie, "/api/runners")
            .await
            .as_array()
            .unwrap()
            .is_empty(),
        "stop 注销扩展拥有的 runner"
    );
    let task = get_json(&guard.app, &cookie, &format!("/api/tasks/{task_id}")).await;
    assert_eq!(task["state"], "dependency_missing");
    assert_eq!(
        task["suspend_reason"],
        serde_json::json!(format!("missing_dependency={YAML_ID}")),
        "ADR-13：runner 缺失 → 任务挂起且记录缺失依赖"
    );
    assert_eq!(
        get_json(&guard.app, &cookie, "/api/extensions/ui")
            .await
            .as_array()
            .unwrap()
            .len(),
        3,
        "stop 只摘 runner；enabled 状态下 UI 贡献保留"
    );

    // ---- start 再启：runner 重注册 → 任务自动恢复 Active（无需人工 enable） ----
    let (status, body) = post_json(
        &guard.app,
        &cookie,
        &format!("/api/extensions/{YAML_ID}/start"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let task = get_json(&guard.app, &cookie, &format!("/api/tasks/{task_id}")).await;
    assert_eq!(
        task["state"], "active",
        "runner 重注册自动恢复 dependency_missing 任务"
    );
    assert!(task["suspend_reason"].is_null());
    assert!(!task["next_wakeup"].is_null(), "恢复必须重算唤醒游标");

    // ---- disable 运行中 = 自动 stop（ADR-13 disable 语义）：runner/UI 一并摘除 ----
    let (status, disabled) = post_json(
        &guard.app,
        &cookie,
        &format!("/api/extensions/{YAML_ID}/disable"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{disabled}");
    assert_eq!(disabled["state"], "disabled");
    assert!(
        get_json(&guard.app, &cookie, "/api/runners")
            .await
            .as_array()
            .unwrap()
            .is_empty(),
        "disable 运行中必须先 stop（注销 runner）"
    );
    assert!(
        get_json(&guard.app, &cookie, "/api/extensions/ui")
            .await
            .as_array()
            .unwrap()
            .is_empty(),
        "disable 摘除 UI 贡献"
    );
    let task = get_json(&guard.app, &cookie, &format!("/api/tasks/{task_id}")).await;
    assert_eq!(task["state"], "dependency_missing");
    assert_eq!(
        task["suspend_reason"],
        serde_json::json!(format!("missing_dependency={YAML_ID}"))
    );

    // ---- uninstall：扩展、UI、runner 全清，任务（用户资产）保留 ----
    let response = send(
        &guard.app,
        request(
            "DELETE",
            &format!("/api/extensions/{YAML_ID}/{YAML_VERSION}"),
            &json_headers(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let list = get_json(&guard.app, &cookie, "/api/extensions").await;
    assert!(list["extensions"].as_array().unwrap().is_empty());
    assert!(list["ui_contributions"].as_array().unwrap().is_empty());
    assert!(get_json(&guard.app, &cookie, "/api/runners")
        .await
        .as_array()
        .unwrap()
        .is_empty());
    let task = get_json(&guard.app, &cookie, &format!("/api/tasks/{task_id}")).await;
    assert_eq!(
        task["state"], "dependency_missing",
        "卸载插件不删除用户任务（ADR-13：任务配置是资产）"
    );

    // ---- reconcile_startup 恢复路径：重装 → 启动 → 制造崩溃窗口（磁盘遗留
    //   Running）→「重启后的进程」对账 → runner 重注册 + 任务恢复 Active ----
    install_enable_start(&guard.app, &cookie).await;
    let (status, body) = post_json(
        &guard.app,
        &cookie,
        &format!("/api/extensions/{YAML_ID}/stop"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // 崩溃窗口：不经生命周期把磁盘状态改写为 Running（实例与 runner 均不存在）。
    {
        let store = guard.extensions.store();
        let mut states = store.read_state().unwrap();
        let record = states
            .get_mut(&crate::extensions::ExtensionId::parse(YAML_ID).unwrap())
            .expect("gamer.yaml 状态记录");
        record.state = crate::extensions::ExtensionState::Running;
        store.write_state(&states).unwrap();
    }
    drop(guard);

    // 「重启后的进程」：裸 runner 注册表 + 遗留 Running 记录。
    let restarted = build_app(&core);
    let cookie2 = login(&restarted.app).await;
    assert!(
        get_json(&restarted.app, &cookie2, "/api/runners")
            .await
            .as_array()
            .unwrap()
            .is_empty(),
        "重启后 runner 注册表为空（Scheduler 不持久化 runner）"
    );
    restarted.extensions.reconcile_startup().await;
    let runners = get_json(&restarted.app, &cookie2, "/api/runners").await;
    assert_eq!(
        runners.as_array().unwrap().len(),
        1,
        "对账后 runner 已重注册"
    );
    assert_eq!(runners[0]["runner_id"], YAML_ID);
    assert_eq!(runners[0]["owner_extension_id"], YAML_ID);
    let task = get_json(&restarted.app, &cookie2, &format!("/api/tasks/{task_id}")).await;
    assert_eq!(
        task["state"], "active",
        "reconcile_startup 恢复 dependency_missing 任务"
    );
    assert!(!task["next_wakeup"].is_null());
}

// ===========================================================================
// §14.4 Bare Core Test
// ===========================================================================

/// 零已装扩展：全部基础 API 可用，任何端点不因 gamer.yaml / gamer.keymap 缺席
/// 而失败。投屏只测 API 面（设备列表空；真机链路见 tests/README.md 外部边界）。
#[tokio::test]
async fn architecture_guard_bare_core_serves_full_base_api_with_zero_extensions() {
    let core = build_core("bare");
    let guard = build_app(&core);
    let cookie = login(&guard.app).await;

    // system/info 正常（契约字段齐备）。
    let info = get_json(&guard.app, &cookie, "/api/system/info").await;
    assert!(!info["app"]["version"].is_null());
    assert!(!info["schema"]["db"].is_null());
    assert!(!info["capabilities"].is_null());

    // 扩展视图为空且接口正常。
    let list = get_json(&guard.app, &cookie, "/api/extensions").await;
    assert_eq!(list["extensions"], serde_json::json!([]));
    assert_eq!(list["ui_contributions"], serde_json::json!([]));
    assert!(
        get_json(&guard.app, &cookie, "/api/runners")
            .await
            .as_array()
            .unwrap()
            .is_empty(),
        "裸 Core：没有任何 runner（Scheduler 不预置，扩展未 start）"
    );

    // /api/tasks 全 CRUD 可用（runner 未注册只影响派发，不影响存取）。
    let (status, created) = post_json(
        &guard.app,
        &cookie,
        "/api/tasks",
        task_body("Bare Task", YAML_ID, "com.guard.app/daily.yaml"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let task_id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["state"], "active");

    let tasks = get_json(&guard.app, &cookie, "/api/tasks").await;
    assert_eq!(tasks.as_array().unwrap().len(), 1);

    let mut updated = task_body("Bare Task Renamed", YAML_ID, "com.guard.app/daily.yaml");
    updated["id"] = serde_json::json!(task_id);
    let response = send(
        &guard.app,
        request(
            "PUT",
            &format!("/api/tasks/{task_id}"),
            &json_headers(&cookie),
            Some(updated.to_string().into_bytes()),
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let (status, suspended) = post_json(
        &guard.app,
        &cookie,
        &format!("/api/tasks/{task_id}/disable"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(suspended["state"], "suspended");
    let (status, enabled) = post_json(
        &guard.app,
        &cookie,
        &format!("/api/tasks/{task_id}/enable"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(enabled["state"], "active");

    // runner 缺失只把派发挡在 424 dependency_missing，不影响任务存取。
    let (status, denied) = post_json(
        &guard.app,
        &cookie,
        &format!("/api/tasks/{task_id}/run"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::FAILED_DEPENDENCY, "{denied}");
    assert_eq!(denied["code"], "dependency_unavailable");

    let response = send(
        &guard.app,
        request(
            "DELETE",
            &format!("/api/tasks/{task_id}"),
            &json_headers(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let response = send(
        &guard.app,
        request(
            "GET",
            &format!("/api/tasks/{task_id}"),
            &json_headers(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // 通用资源：文本 kind（presets，无内容钩子 = 裸 Core 不校验）JSON 创建 + 内容往返。
    let preset_content = "version: 1\nname: guard-bare-preset\n";
    let (status, saved) = post_json(
        &guard.app,
        &cookie,
        "/api/apps/com.guard.app/resources/presets",
        serde_json::json!({"name": "bare", "content": preset_content}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{saved}");
    let preset = get_json(
        &guard.app,
        &cookie,
        "/api/apps/com.guard.app/resources/presets/com.guard.app%2Fbare.yaml",
    )
    .await;
    assert_eq!(preset["content"], preset_content);

    // 字节 kind：PNG 模板创建 + 跨分区通配（app = "-"）读回（字节 kind 走
    // templates 的归一化存储，读回可解码为同尺寸图片）。
    let response = send(
        &guard.app,
        request(
            "POST",
            "/api/apps/com.guard.app/resources/templates?name=guard_bare.png",
            &[
                (header::COOKIE.to_string(), cookie.clone()),
                (header::CONTENT_TYPE.to_string(), "image/png".into()),
            ],
            Some(valid_png()),
        ),
    )
    .await;
    let status = response.status();
    assert_eq!(status, StatusCode::CREATED, "{}", body_json(response).await);
    let response = send(
        &guard.app,
        request(
            "GET",
            // 字节 kind 的 id 是 "<pkg>/<文件名>"；app = "-" 为跨分区通配。
            "/api/apps/-/resources/templates/com.guard.app%2Fguard_bare.png",
            &json_headers(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let decoded = image::load_from_memory(&bytes).expect("字节 kind 读回必须是合法图片");
    assert_eq!(
        (decoded.width(), decoded.height()),
        (8, 8),
        "字节内容往返一致"
    );

    // 投屏 API 面：设备列表为空但不报错。
    let devices = get_json(&guard.app, &cookie, "/api/devices").await;
    assert_eq!(devices, serde_json::json!([]));
}

/// 8x8 合法灰度 PNG（字节 kind 上传夹具）。
fn valid_png() -> Vec<u8> {
    let mut img = image::GrayImage::new(8, 8);
    for (x, y, pixel) in img.enumerate_pixels_mut() {
        pixel.0[0] = if (x + y) % 2 == 0 { 32 } else { 224 };
    }
    let mut bytes = Vec::new();
    image::DynamicImage::ImageLuma8(img)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    bytes
}

// ===========================================================================
// §14.5 YAML Isolation Test
// ===========================================================================

/// 无 gamer.yaml：runner=gamer.yaml 任务保存成功；派发 → 424 dependency_missing
/// 且任务保留；安装（占位 wasm 即可）+ 启动 → 同一任务**自动**恢复 Active 且
/// 派发进入执行层（202 + run 记录）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn architecture_guard_isolation_yaml_task_survives_extension_absence_and_recovers() {
    let core = build_core("yaml-isolation");
    let guard = build_app(&core);
    let cookie = login(&guard.app).await;

    // 无 gamer.yaml（未安装）：任务保存成功（ADR-12：保存边界不拒绝未知 runner）。
    let (status, created) = post_json(
        &guard.app,
        &cookie,
        "/api/tasks",
        task_body("Isolated", YAML_ID, "com.guard.app/daily.yaml"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let task_id = created["id"].as_str().unwrap().to_string();

    // 派发 → 424 dependency_unavailable，任务进入 dependency_missing 且保留。
    let (status, denied) = post_json(
        &guard.app,
        &cookie,
        &format!("/api/tasks/{task_id}/run"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::FAILED_DEPENDENCY);
    assert_eq!(denied["runner_id"], YAML_ID);
    let task = get_json(&guard.app, &cookie, &format!("/api/tasks/{task_id}")).await;
    assert_eq!(task["state"], "dependency_missing");
    assert_eq!(
        task["suspend_reason"],
        serde_json::json!(format!("missing_dependency={YAML_ID}"))
    );
    assert!(task["next_wakeup"].is_null(), "依赖缺失任务必须休眠");
    let tasks = get_json(&guard.app, &cookie, "/api/tasks").await;
    assert_eq!(tasks.as_array().unwrap().len(), 1, "任务保留，不删除");

    // 安装 gamer.yaml（占位 wasm 即可：无实例执行模型）+ 启动。
    install_enable_start(&guard.app, &cookie).await;

    // 同一任务自动恢复 Active（无需人工 enable/resume）。
    let task = get_json(&guard.app, &cookie, &format!("/api/tasks/{task_id}")).await;
    assert_eq!(task["state"], "active", "runner 注册自动恢复任务");
    assert!(task["suspend_reason"].is_null());
    assert!(!task["next_wakeup"].is_null());

    // 保存脚本资源后派发进入执行层：202 + run 记录（不再 424）。
    let script = "steps:\n  - log: guard isolation\n";
    let (status, saved) = post_json(
        &guard.app,
        &cookie,
        "/api/apps/com.guard.app/resources/scripts",
        serde_json::json!({"name": "daily", "content": script}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{saved}");
    let (status, dispatched) = post_json(
        &guard.app,
        &cookie,
        &format!("/api/tasks/{task_id}/run"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{dispatched}");
    let run_id = dispatched["run_id"].as_str().unwrap().to_string();
    let run = get_json(&guard.app, &cookie, &format!("/api/runs/{run_id}")).await;
    assert_eq!(run["run_id"], run_id);
    assert_eq!(run["runner_id"], YAML_ID);
    assert_eq!(
        run["entrypoint"], "com.guard.app/daily.yaml",
        "run 记录携带任务入口"
    );
}

// ===========================================================================
// §14.6 Keymap Isolation Test
// ===========================================================================

/// 无 gamer.keymap（未安装/未启动）：键盘事件直通（`dispatch_keymap_input`
/// 返回 pass：不消费、无动作）——「有扩展 → 经 keymap runtime 消费」的整合
/// 入口见 `extensions/keymap/mod.rs` 的
/// `real_keymap_gplugin_invokes_wit_and_native_capabilities` /
/// `real_keymap_guest_consumes_user_profile_yaml`（真实 fixture guest；WASM
/// 消费与 profile 覆盖语义由它们锁定），此处不重复。
#[tokio::test]
async fn architecture_guard_isolation_keymap_missing_extension_passes_input_through() {
    let dir = tempfile::tempdir().expect("keymap isolation 临时目录");
    let service = ExtensionService::for_data_root(
        dir.path(),
        crate::capabilities::CapabilityRegistry::default(),
    );

    // 零扩展：未安装 gamer.keymap，也未安装任何其他扩展。
    assert!(service.list().unwrap().is_empty());

    let device =
        crate::capabilities::DeviceHandle::new(crate::capabilities::DeviceId::new("device-1"));
    let screen = ScreenSize::new(1000, 500);
    for event in [
        InputEvent::key_down("KeyW"),
        InputEvent::key_up("KeyW"),
        InputEvent::key_down("Space"),
    ] {
        let result: InputResult = service
            .dispatch_keymap_input(device.clone(), screen, event, None)
            .await
            .expect("缺失 keymap 的派发必须是正常直通而非错误");
        assert!(
            !result.consume,
            "裸 Core 语义：无 keymap 时输入必须直通设备，不得吞掉"
        );
        assert!(result.actions.is_empty(), "直通不得产生映射动作");
    }
}
