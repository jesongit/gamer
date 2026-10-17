//! 官方插件打包工具（tools/build-plugins.ps1 的后端）。
//!
//! Phase 1 免签名模型：默认链路（pack/inspect/verify）与私钥、签名、Registry
//! proof 完全解耦。manifest.toml（manifest v2）是插件元数据的唯一权威源，
//! `--meta-out` 把解析出的元数据写成 JSON 供 build-plugins.ps1 生成 registry v2。
//!
//! 子命令 — 默认链路（无私钥依赖）：
//!
//! - `pack`：`--manifest <toml> [--wasm <component.wasm>] --out <gplugin>
//!   [--file <归档路径>=<源文件>]... [--meta-out <meta.json>]`；打包 .gplugin
//!   （zip：manifest.toml + entry（如 plugin.wasm）+ 附加文件，无 signature.sig）。
//!   execution.kind=wasm 必须提供 --wasm 且 manifest 声明 entry；kind=builtin
//!   禁止 --wasm（宿主预置实现不携带 guest）。
//! - `inspect`：`--manifest <toml> [--meta-out <meta.json>]`；只解析 manifest
//!   元数据（id/version/execution.kind/permissions/...），不打包。
//! - `verify`：`--archive <gplugin>`；产物自检：重走 zip 中央目录校验、
//!   manifest.toml 可解析、wasm 包 entry 存在且为 `\0asm` magic（builtin 包跳过
//!   entry 校验）。打印 id=/version=/kind=/sha256=/size=。
//!
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{Read as _, Write as _};
use std::path::PathBuf;
use std::process::ExitCode;

const MANIFEST_FILE: &str = "manifest.toml";
const WASM_MAGIC: [u8; 4] = [0x00, b'a', b's', b'm'];
const MAX_SCHEMA_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ToolDeclaration {
    tool_id: String,
    action: String,
    description: String,
    input_schema: String,
    #[serde(default)]
    output_schema: Option<String>,
    required_scopes: Vec<String>,
    #[serde(default)]
    required_permissions: Vec<String>,
    effect: String,
    #[serde(default)]
    context: ToolContext,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct ToolContext {
    device_field: Option<String>,
    package_field: Option<String>,
    plugin_field: Option<String>,
    runner_field: Option<String>,
    run_field: Option<String>,
    task_field: Option<String>,
    media_field: Option<String>,
    control_session_field: Option<String>,
}
fn validate_archive_path(path: &str) -> Result<(), String> {
    if path.is_empty()
        || path.len() > 1024
        || path.contains(['\\', ':', '\0'])
        || path.chars().any(char::is_control)
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(format!("Invalid archive path: {path}"));
    }
    Ok(())
}
fn validate_tools(
    id: &str,
    permissions: &[String],
    tools: &[ToolDeclaration],
) -> Result<(), String> {
    let mut ids = std::collections::BTreeSet::new();
    let mut actions = std::collections::BTreeSet::new();
    for tool in tools {
        if tool.tool_id.is_empty()
            || !tool
                .tool_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
            || format!("gamer.plugin.{id}.{}", tool.tool_id).len() > 128
            || !ids.insert(&tool.tool_id)
            || tool.action.trim().is_empty()
            || tool.action.len() > 128
            || tool.action.chars().any(char::is_control)
            || !actions.insert(&tool.action)
            || tool.description.trim().is_empty()
            || tool.description.len() > 16 * 1024
        {
            return Err("Invalid or duplicate external tool identity/action/description".into());
        }
        if tool.required_scopes.is_empty()
            || tool.required_scopes.iter().any(|scope| {
                scope.is_empty()
                    || scope.len() > 128
                    || !scope
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._-:".contains(&b))
            })
        {
            return Err("Every tool requires explicit valid scopes".into());
        }
        if tool
            .required_permissions
            .iter()
            .any(|permission| !permissions.contains(permission))
        {
            return Err("Tool requires a permission not granted by its manifest".into());
        }
        if tool.effect == "read"
            && tool.required_permissions.iter().any(|permission| {
                ![
                    "device.read",
                    "vision.match",
                    "vision.color",
                    "vision.ocr",
                    "resource.read",
                    "runtime.sleep",
                    "media.read",
                    "media.events.read",
                    "ui.host",
                ]
                .contains(&permission.as_str())
            })
        {
            return Err("A read-only tool cannot declare write permissions".into());
        }
        if ![
            "read",
            "draft_write",
            "resource_write",
            "media_write",
            "input",
            "run_submit",
            "task_write",
            "external_send",
        ]
        .contains(&tool.effect.as_str())
        {
            return Err("Invalid tool effect".into());
        }
        validate_archive_path(&tool.input_schema)?;
        if let Some(path) = &tool.output_schema {
            validate_archive_path(path)?;
        }
        for field in [
            &tool.context.device_field,
            &tool.context.package_field,
            &tool.context.plugin_field,
            &tool.context.runner_field,
            &tool.context.run_field,
            &tool.context.task_field,
            &tool.context.media_field,
            &tool.context.control_session_field,
        ]
        .into_iter()
        .flatten()
        {
            if field.is_empty() || field.len() > 128 || field.chars().any(char::is_control) {
                return Err("Invalid tool context selector".into());
            }
        }
    }
    Ok(())
}
fn reject_external_refs(value: &Value, depth: usize) -> Result<(), String> {
    if depth > 64 {
        return Err("Tool schema exceeds nesting limit".into());
    }
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if ["$ref", "$dynamicRef", "$recursiveRef"].contains(&key.as_str())
                    && !value.as_str().is_some_and(|v| v.starts_with('#'))
                {
                    return Err("Tool schemas support only local references".into());
                }
                if key == "$id"
                    && value
                        .as_str()
                        .is_some_and(|id| !id.is_empty() && !id.starts_with('#'))
                {
                    return Err("External tool schema identifiers are forbidden".into());
                }
                reject_external_refs(value, depth + 1)?;
            }
        }
        Value::Array(items) => {
            for item in items {
                reject_external_refs(item, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}
fn validate_schema(content: &[u8], input: bool) -> Result<(), String> {
    let schema: Value = serde_json::from_slice(content)
        .map_err(|error| format!("Tool schema must be valid JSON: {error}"))?;
    reject_external_refs(&schema, 0)?;
    if input && schema["type"] != "object" {
        return Err("Tool input schema must declare type object".into());
    }
    jsonschema::draft202012::options()
        .build(&schema)
        .map_err(|error| format!("Invalid tool JSON Schema: {error}"))?;
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("plugin-packer: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let Some(command) = args.first() else {
        return Err("用法: plugin-packer <pack|inspect|verify> ...".into());
    };
    match command.as_str() {
        "pack" => pack(&flags(&args[1..])?),
        "inspect" => inspect(&flags(&args[1..])?),
        "verify" => verify(&flags(&args[1..])?),
        other => Err(format!("未知子命令: {other}")),
    }
}

#[derive(Default)]
struct Flags {
    values: std::collections::BTreeMap<String, String>,
    multi: Vec<(String, String)>,
}

fn flags(args: &[String]) -> Result<Flags, String> {
    let mut flags = Flags::default();
    let mut index = 0;
    while index < args.len() {
        let name = args[index]
            .strip_prefix("--")
            .ok_or_else(|| format!("参数必须以 -- 开头: {}", args[index]))?
            .to_string();
        let value = args
            .get(index + 1)
            .ok_or_else(|| format!("参数 --{name} 缺少取值"))?;
        if name == "file" {
            flags.multi.push((name.clone(), value.clone()));
        } else {
            flags.values.insert(name, value.clone());
        }
        index += 2;
    }
    Ok(flags)
}

fn require(flags: &Flags, name: &str) -> Result<String, String> {
    flags
        .values
        .get(name)
        .cloned()
        .ok_or_else(|| format!("缺少参数 --{name}"))
}

// ---------- manifest 元数据解析（registry 元数据收敛的唯一权威源） ----------

/// toml::Value → serde_json::Value（host_api / ui.contributions 原样透传）。
fn toml_to_json(value: &toml::Value) -> Value {
    match value {
        toml::Value::String(s) => Value::String(s.clone()),
        toml::Value::Integer(i) => Value::Number((*i).into()),
        toml::Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        toml::Value::Boolean(b) => Value::Bool(*b),
        toml::Value::Datetime(dt) => Value::String(dt.to_string()),
        toml::Value::Array(items) => Value::Array(items.iter().map(toml_to_json).collect()),
        toml::Value::Table(t) => Value::Object(
            t.iter()
                .map(|(k, v)| (k.clone(), toml_to_json(v)))
                .collect(),
        ),
    }
}

/// 解析 manifest.toml 为元数据 JSON。结构（缺省字段为 null / 空数组 / 空对象）：
/// `{ manifest_version, id, version, name, description, publisher, entry,
///    execution: { kind, builtin_id }, permissions: [], host_api: {},
///    ui: { contributions: [] } }`
/// - manifest v2：`[execution] kind = "wasm" | "builtin"`；builtin 无 entry。
/// - manifest v1（无 [execution]）：按历史语义推断 kind = "wasm"。
/// - entry 兼容顶层与 `[execution] entry` 两种写法（顶层优先）。
fn meta_from_manifest(text: &str) -> Result<Value, String> {
    let root: toml::Value =
        toml::from_str(text).map_err(|error| format!("manifest 不是合法 TOML: {error}"))?;
    let table = root
        .as_table()
        .ok_or_else(|| "manifest 顶层必须是 TOML 表".to_string())?;

    let string_field = |key: &str| -> Result<String, String> {
        table
            .get(key)
            .and_then(|value| value.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| format!("manifest 缺少非空字符串字段 {key}"))
    };
    let id = string_field("id")?;
    let version = string_field("version")?;
    let name = string_field("name")?;
    let optional_string = |key: &str| {
        table
            .get(key)
            .and_then(|value| value.as_str())
            .map(|s| s.to_string())
    };

    let execution = table.get("execution").and_then(|value| value.as_table());
    let kind = execution
        .and_then(|t| t.get("kind"))
        .and_then(|value| value.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "wasm".to_string());
    if kind != "wasm" && kind != "builtin" {
        return Err(format!(
            "manifest [execution] kind 必须是 \"wasm\" 或 \"builtin\"，当前为 \"{kind}\""
        ));
    }
    let builtin_id = execution
        .and_then(|t| t.get("builtin_id"))
        .and_then(|value| value.as_str())
        .map(|s| s.to_string());
    if kind == "builtin" && builtin_id.as_deref() == Some("gamer-ai") {
        return Err("gamer-ai builtin is retired; connect an external Agent to Core MCP".into());
    }
    let host_version = execution
        .and_then(|t| t.get("host_version"))
        .and_then(|value| value.as_str())
        .map(|s| s.to_string());
    let entry = optional_string("entry").or_else(|| {
        execution
            .and_then(|t| t.get("entry"))
            .and_then(|value| value.as_str())
            .map(|s| s.to_string())
    });

    let permissions: Vec<String> = match table.get("permissions") {
        Some(value) => serde_json::from_value(toml_to_json(value))
            .map_err(|_| "manifest permissions must be an array of strings".to_string())?,
        None => Vec::new(),
    };
    let permissions: Vec<String> = permissions
        .into_iter()
        .map(|permission| {
            let permission = permission.trim();
            if permission == "ai.connect" {
                return Err("ai.connect is retired; update the plugin before distribution".into());
            }
            if ![
                "device.read",
                "device.app",
                "vision.match",
                "vision.color",
                "vision.ocr",
                "input.tap",
                "input.swipe",
                "input.key",
                "input.text",
                "touch",
                "resource.read",
                "resource.write",
                "package.publish",
                "run.submit",
                "run.control",
                "runtime.sleep",
                "log.write",
                "media.stream",
                "live.connect",
                "notify.send",
                "media.read",
                "media.import",
                "media.record",
                "media.write",
                "media.events.read",
                "ui.host",
            ]
            .contains(&permission)
            {
                return Err(format!(
                    "Unknown or forbidden manifest permission: {permission}"
                ));
            }
            Ok(permission.to_owned())
        })
        .collect::<Result<_, String>>()?;
    let host_api = table
        .get("host_api")
        .map(toml_to_json)
        .unwrap_or_else(|| json!({}));
    let contributions = table
        .get("ui")
        .and_then(|ui| ui.get("contributions"))
        .and_then(|value| value.as_array())
        .map(|items| Value::Array(items.iter().map(toml_to_json).collect()))
        .unwrap_or_else(|| json!([]));
    let tools = table
        .get("tools")
        .map(toml_to_json)
        .unwrap_or_else(|| json!([]));
    let declarations: Vec<ToolDeclaration> = serde_json::from_value(tools.clone())
        .map_err(|error| format!("Invalid explicit tools declaration: {error}"))?;
    validate_tools(&id, &permissions, &declarations)?;

    Ok(json!({
        "manifest_version": table.get("manifest_version").and_then(|v| v.as_integer()),
        "id": id,
        "version": version,
        "name": name,
        "description": optional_string("description"),
        "publisher": optional_string("publisher"),
        "entry": entry,
        "execution": { "kind": kind, "builtin_id": builtin_id, "host_version": host_version },
        "permissions": permissions,
        "host_api": host_api,
        "ui": { "contributions": contributions },
        "tools": tools,
    }))
}

fn load_manifest_meta(manifest_path: &str) -> Result<(String, Value), String> {
    let text = std::fs::read_to_string(manifest_path)
        .map_err(|error| format!("读取 manifest 失败: {error}"))?;
    let meta = meta_from_manifest(&text)?;
    Ok((text, meta))
}

fn write_meta_out(flags: &Flags, meta: &Value) -> Result<(), String> {
    let Some(meta_out) = flags.values.get("meta-out") else {
        return Ok(());
    };
    if let Some(parent) = PathBuf::from(meta_out).parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("创建目录失败: {error}"))?;
    }
    let mut text =
        serde_json::to_string_pretty(meta).map_err(|error| format!("序列化元数据失败: {error}"))?;
    text.push('\n');
    std::fs::write(meta_out, text)
        .map_err(|error| format!("写入元数据 {meta_out} 失败: {error}"))?;
    Ok(())
}

// ---------- 默认链路：pack / inspect / verify ----------

fn inspect(flags: &Flags) -> Result<(), String> {
    let manifest_path = require(flags, "manifest")?;
    let (_, meta) = load_manifest_meta(&manifest_path)?;
    println!("id={}", meta["id"].as_str().unwrap_or_default());
    println!("version={}", meta["version"].as_str().unwrap_or_default());
    println!(
        "kind={}",
        meta["execution"]["kind"].as_str().unwrap_or("wasm")
    );
    if let Some(entry) = meta["entry"].as_str() {
        println!("entry={entry}");
    }
    write_meta_out(flags, &meta)
}

fn pack(flags: &Flags) -> Result<(), String> {
    let manifest_path = require(flags, "manifest")?;
    let out = require(flags, "out")?;
    let wasm_opt = flags.values.get("wasm");
    let (manifest_text, meta) = load_manifest_meta(&manifest_path)?;
    let kind = meta["execution"]["kind"]
        .as_str()
        .unwrap_or("wasm")
        .to_string();
    let entry_name = meta["entry"].as_str().map(|s| s.to_string());

    let mut bytes = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o644);
        write_entry(
            &mut writer,
            MANIFEST_FILE,
            manifest_text.as_bytes(),
            options,
        )?;
        match kind.as_str() {
            "wasm" => {
                let entry = entry_name.ok_or_else(|| {
                    "wasm 包 manifest 缺少 entry（如 entry = \"plugin.wasm\"）".to_string()
                })?;
                if entry == MANIFEST_FILE || entry.ends_with('/') {
                    return Err(format!("非法 entry: {entry}"));
                }
                let wasm_path = wasm_opt
                    .ok_or_else(|| "wasm 包必须提供 --wasm <component.wasm>".to_string())?;
                let wasm =
                    std::fs::read(wasm_path).map_err(|error| format!("读取 wasm 失败: {error}"))?;
                if wasm.len() < 4 || wasm[..4] != WASM_MAGIC {
                    return Err("--wasm 不是 WASM 二进制（缺 \\0asm magic）".into());
                }
                write_entry(&mut writer, &entry, &wasm, options)?;
            }
            "builtin" => {
                if wasm_opt.is_some() {
                    return Err("builtin 包是宿主预置实现，不携带 guest：请去掉 --wasm".into());
                }
            }
            _ => unreachable!("meta_from_manifest 已校验 kind"),
        }
        // --file <归档路径>=<源文件>：manifest 声明的 UI 入口等附加文件。
        for (_, source) in &flags.multi {
            let Some((archive_name, source_path)) = source.split_once('=') else {
                return Err(format!("--file 需要 <归档路径>=<源文件> 形式: {source}"));
            };
            let content = std::fs::read(source_path)
                .map_err(|error| format!("读取附加文件 {source_path} 失败: {error}"))?;
            write_entry(&mut writer, archive_name, &content, options)?;
        }
        writer
            .finish()
            .map_err(|error| format!("收尾 zip 失败: {error}"))?;
    }
    // Reject missing/invalid declared schemas before writing any artifact.
    verify_archive_bytes(&bytes)?;
    if let Some(parent) = PathBuf::from(&out).parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("创建目录失败: {error}"))?;
    }
    std::fs::write(&out, &bytes).map_err(|error| format!("写入 {out} 失败: {error}"))?;
    write_meta_out(flags, &meta)?;
    println!("sha256={:x}", Sha256::digest(&bytes));
    println!("size={}", bytes.len());
    Ok(())
}

fn verify(flags: &Flags) -> Result<(), String> {
    let archive_path = require(flags, "archive")?;
    let bytes = std::fs::read(&archive_path).map_err(|error| format!("读取归档失败: {error}"))?;
    let meta = verify_archive_bytes(&bytes)?;
    println!("id={}", meta["id"].as_str().unwrap_or_default());
    println!("version={}", meta["version"].as_str().unwrap_or_default());
    println!(
        "kind={}",
        meta["execution"]["kind"].as_str().unwrap_or("wasm")
    );
    println!("sha256={:x}", Sha256::digest(&bytes));
    println!("size={}", bytes.len());
    Ok(())
}

/// Build-side validation complements the real host's authoritative install
/// validation. It never disables or replaces that host validation.
fn verify_archive_bytes(bytes: &[u8]) -> Result<Value, String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes))
        .map_err(|error| format!("zip 中央目录校验失败: {error}"))?;
    let names: Vec<String> = archive.file_names().map(|name| name.to_string()).collect();
    let mut seen = std::collections::BTreeSet::new();
    for name in &names {
        validate_archive_path(name)?;
        if !seen.insert(name) {
            return Err(format!("Duplicate archive path: {name}"));
        }
    }
    if !names.iter().any(|name| name == MANIFEST_FILE) {
        return Err(format!("归档缺少 {MANIFEST_FILE}"));
    }
    let text = {
        let mut manifest_entry = archive
            .by_name(MANIFEST_FILE)
            .map_err(|error| format!("读取 {MANIFEST_FILE} 失败: {error}"))?;
        let mut text = String::new();
        manifest_entry
            .read_to_string(&mut text)
            .map_err(|error| format!("读取 {MANIFEST_FILE} 内容失败: {error}"))?;
        text
    };
    let meta = meta_from_manifest(&text)?;
    let kind = meta["execution"]["kind"]
        .as_str()
        .unwrap_or("wasm")
        .to_string();
    if kind == "wasm" {
        let entry = meta["entry"]
            .as_str()
            .ok_or_else(|| "wasm 包 manifest 缺少 entry".to_string())?;
        let mut entry_file = archive
            .by_name(entry)
            .map_err(|_| format!("归档缺少 entry {entry}"))?;
        if entry_file.size() < 4 {
            return Err("WASM entry 太小".into());
        }
        let mut magic = [0u8; 4];
        entry_file
            .read_exact(&mut magic)
            .map_err(|error| format!("读取 entry magic 失败: {error}"))?;
        if magic != WASM_MAGIC {
            return Err("entry 不是 WASM 二进制".into());
        }
    }
    let tools: Vec<ToolDeclaration> =
        serde_json::from_value(meta["tools"].clone()).map_err(|error| error.to_string())?;
    for tool in tools {
        for (path, input) in std::iter::once((&tool.input_schema, true))
            .chain(tool.output_schema.as_ref().map(|path| (path, false)))
        {
            let mut file = archive
                .by_name(path)
                .map_err(|_| format!("Declared tool schema is missing: {path}"))?;
            if file.is_dir() || file.size() > MAX_SCHEMA_BYTES as u64 {
                return Err(format!("Invalid tool schema size: {path}"));
            }
            let mut content = Vec::new();
            (&mut file)
                .take(MAX_SCHEMA_BYTES as u64 + 1)
                .read_to_end(&mut content)
                .map_err(|error| error.to_string())?;
            if content.len() > MAX_SCHEMA_BYTES {
                return Err(format!("Tool schema exceeds limit: {path}"));
            }
            validate_schema(&content, input)?;
        }
    }
    Ok(meta)
}

fn write_entry(
    writer: &mut zip::ZipWriter<std::io::Cursor<&mut Vec<u8>>>,
    name: &str,
    content: &[u8],
    options: zip::write::SimpleFileOptions,
) -> Result<(), String> {
    validate_archive_path(name)?;
    writer
        .start_file(name, options)
        .map_err(|error| format!("写入 {name} 失败: {error}"))?;
    writer
        .write_all(content)
        .map_err(|error| format!("写入 {name} 内容失败: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const BASE:&str="manifest_version=2\nid='com.example.tools'\nversion='1.0.0'\nname='Tools'\npermissions=['resource.read']\n[execution]\nkind='builtin'\nbuiltin_id='com.example.tools'\n";
    const TOOL:&str="\n[[tools]]\ntool_id='read'\naction='read'\ndescription='Read'\ninput_schema='schemas/read.json'\nrequired_scopes=['resource.read']\nrequired_permissions=['resource.read']\neffect='read'\n";
    #[test]
    fn retired_ai_permissions_and_builtin_cannot_be_repackaged() {
        for permission in ["ai.connect", " ai.connect ", "network.http", "unknown"] {
            let manifest = BASE.replace("resource.read", permission);
            assert!(meta_from_manifest(&manifest).is_err());
            let mut bytes = Vec::new();
            {
                let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
                write_entry(
                    &mut writer,
                    MANIFEST_FILE,
                    manifest.as_bytes(),
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
                writer.finish().unwrap();
            }
            assert!(verify_archive_bytes(&bytes).is_err());
        }
        assert!(meta_from_manifest(&BASE.replace(
            "permissions=['resource.read']",
            "permissions=['resource.read',1]"
        ))
        .is_err());
        assert!(meta_from_manifest(
            &BASE.replace("permissions=['resource.read']", "permissions='ai.connect'")
        )
        .is_err());
        assert!(meta_from_manifest(
            &BASE.replace("builtin_id='com.example.tools'", "builtin_id='gamer-ai'")
        )
        .is_err());
        assert!(meta_from_manifest(&BASE.replace(
            "permissions=['resource.read']",
            "permissions=[' resource.read ']"
        ))
        .is_ok());
    }
    fn bundle(schema: Option<&[u8]>) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
            let options = zip::write::SimpleFileOptions::default();
            write_entry(
                &mut writer,
                MANIFEST_FILE,
                format!("{BASE}{TOOL}").as_bytes(),
                options,
            )
            .unwrap();
            if let Some(schema) = schema {
                write_entry(&mut writer, "schemas/read.json", schema, options).unwrap();
            }
            writer.finish().unwrap();
        }
        bytes
    }
    #[test]
    fn explicit_tools_are_preserved_and_require_valid_scopes_and_permissions() {
        let meta = meta_from_manifest(&format!("{BASE}{TOOL}")).unwrap();
        assert_eq!(meta["tools"][0]["input_schema"], "schemas/read.json");
        for tool in [
            TOOL.replace("['resource.read']", "[]"),
            TOOL.replace("effect='read'", "effect='unknown'"),
            TOOL.replace("schemas/read.json", "../secret.json"),
            format!("{TOOL}\nunknown=true"),
            format!("{TOOL}{TOOL}"),
        ] {
            assert!(meta_from_manifest(&format!("{BASE}{tool}")).is_err());
        }
        assert!(meta_from_manifest(&format!(
            "{BASE}{}",
            TOOL.replace(
                "required_permissions=['resource.read']",
                "required_permissions=['resource.write']"
            )
        ))
        .is_err());
    }
    #[test]
    fn archive_requires_real_schema_files_and_offline_valid_schema() {
        assert!(verify_archive_bytes(&bundle(Some(
            br#"{"type":"object","properties":{},"additionalProperties":false}"#
        )))
        .is_ok());
        assert!(verify_archive_bytes(&bundle(None)).is_err());
        for schema in [
            b"not json".as_slice(),
            br#"{"type":"unknown"}"#,
            br#"{"type":"object","$ref":"https://example.invalid/schema"}"#,
            br#"{"type":"object","$id":"file:///secret.json"}"#,
        ] {
            assert!(verify_archive_bytes(&bundle(Some(schema))).is_err());
        }
    }
    #[test]
    fn write_schema_may_use_core_injected_operation_id() {
        assert!(validate_schema(
            br#"{"type":"object","properties":{},"additionalProperties":false}"#,
            true
        )
        .is_ok());
    }
}
