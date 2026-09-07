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
//! legacy 子命令（默认链不调用；仅为存量签名包/应急保留）：
//!
//! - `keygen`：`--out <key 文件> [--key-id gamer-dev-1] [--pem-out <pem 文件>]`。
//! - `sign`：`--manifest <toml> --wasm <component.wasm> --key <key 文件>
//!   --key-id <id> --out <gplugin> [--file ...]...`；旧版「打包 + Ed25519
//!   manifest 签名」一体路径。registry-proof 子命令已随 Registry proof 机制
//!   一并删除。

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{Read as _, Write as _};
use std::path::PathBuf;
use std::process::ExitCode;

const SIG_MAGIC: &str = "gamebot-gplugin-sig-1";
const MANIFEST_FILE: &str = "manifest.toml";
const WASM_MAGIC: [u8; 4] = [0x00, b'a', b's', b'm'];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("plugin-signer: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let Some(command) = args.first() else {
        return Err("用法: plugin-signer <pack|inspect|verify|keygen|sign> ...".into());
    };
    match command.as_str() {
        "pack" => pack(&flags(&args[1..])?),
        "inspect" => inspect(&flags(&args[1..])?),
        "verify" => verify(&flags(&args[1..])?),
        "keygen" => keygen(&flags(&args[1..])?),
        "sign" => sign(&flags(&args[1..])?),
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
    let entry = optional_string("entry").or_else(|| {
        execution
            .and_then(|t| t.get("entry"))
            .and_then(|value| value.as_str())
            .map(|s| s.to_string())
    });

    let permissions: Vec<String> = table
        .get("permissions")
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|value| value.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
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

    Ok(json!({
        "manifest_version": table.get("manifest_version").and_then(|v| v.as_integer()),
        "id": id,
        "version": version,
        "name": name,
        "description": optional_string("description"),
        "publisher": optional_string("publisher"),
        "entry": entry,
        "execution": { "kind": kind, "builtin_id": builtin_id },
        "permissions": permissions,
        "host_api": host_api,
        "ui": { "contributions": contributions },
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
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes))
        .map_err(|error| format!("zip 中央目录校验失败: {error}"))?;
    let names: Vec<String> = archive.file_names().map(|name| name.to_string()).collect();
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
    println!("id={}", meta["id"].as_str().unwrap_or_default());
    println!("version={}", meta["version"].as_str().unwrap_or_default());
    println!("kind={kind}");
    println!("sha256={:x}", Sha256::digest(&bytes));
    println!("size={}", bytes.len());
    Ok(())
}

// ---------- legacy：keygen / sign（默认链不调用） ----------

fn load_key(path: &str) -> Result<SigningKey, String> {
    let hex = std::fs::read_to_string(path).map_err(|error| format!("读取私钥失败: {error}"))?;
    let hex = hex.trim();
    let mut bytes = [0u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        let pair = hex
            .get(index * 2..index * 2 + 2)
            .ok_or_else(|| "私钥长度必须是 64 位 hex".to_string())?;
        *byte =
            u8::from_str_radix(pair, 16).map_err(|error| format!("私钥不是合法 hex: {error}"))?;
    }
    Ok(SigningKey::from_bytes(&bytes))
}

/// SPKI DER 包装（302a300506032b6570032100 + 32 字节公钥）→ PEM。
fn public_key_pem(verifying: &ed25519_dalek::VerifyingKey) -> String {
    let mut der = vec![
        0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
    ];
    der.extend_from_slice(verifying.as_bytes());
    let body = B64.encode(der);
    let mut pem = String::from("-----BEGIN PUBLIC KEY-----\n");
    for chunk in body.as_bytes().chunks(64) {
        pem.push_str(std::str::from_utf8(chunk).expect("base64 是 ASCII"));
        pem.push('\n');
    }
    pem.push_str("-----END PUBLIC KEY-----\n");
    pem
}

fn keygen(flags: &Flags) -> Result<(), String> {
    let out = require(flags, "out")?;
    let key_id = flags
        .values
        .get("key-id")
        .cloned()
        .unwrap_or_else(|| "gamer-dev-1".into());
    let mut secret = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut secret);
    let signing = SigningKey::from_bytes(&secret);
    if let Some(parent) = PathBuf::from(&out).parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("创建目录失败: {error}"))?;
    }
    std::fs::write(&out, hex(&signing.to_bytes()))
        .map_err(|error| format!("写入私钥失败: {error}"))?;
    let pem = public_key_pem(&signing.verifying_key());
    if let Some(pem_out) = flags.values.get("pem-out") {
        std::fs::write(pem_out, &pem).map_err(|error| format!("写入公钥 PEM 失败: {error}"))?;
    }
    println!("key_id={key_id}");
    print!("{pem}");
    Ok(())
}

fn sign(flags: &Flags) -> Result<(), String> {
    let manifest_path = require(flags, "manifest")?;
    let wasm_path = require(flags, "wasm")?;
    let key_path = require(flags, "key")?;
    let key_id = require(flags, "key-id")?;
    let out = require(flags, "out")?;
    let manifest =
        std::fs::read(&manifest_path).map_err(|error| format!("读取 manifest 失败: {error}"))?;
    let wasm = std::fs::read(&wasm_path).map_err(|error| format!("读取 wasm 失败: {error}"))?;
    let signing = load_key(&key_path)?;

    let signature = signing.sign(&manifest);
    let sig_file = format!(
        "{SIG_MAGIC} {key_id}\n{}\n",
        B64.encode(signature.to_bytes())
    );

    let mut bytes = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o644);
        write_entry(&mut writer, MANIFEST_FILE, &manifest, options)?;
        write_entry(&mut writer, "plugin.wasm", &wasm, options)?;
        for (_, source) in &flags.multi {
            let Some((archive_name, source_path)) = source.split_once('=') else {
                return Err(format!("--file 需要 <归档路径>=<源文件> 形式: {source}"));
            };
            let content = std::fs::read(source_path)
                .map_err(|error| format!("读取附加文件 {source_path} 失败: {error}"))?;
            write_entry(&mut writer, archive_name, &content, options)?;
        }
        write_entry(&mut writer, "signature.sig", sig_file.as_bytes(), options)?;
        writer
            .finish()
            .map_err(|error| format!("收尾 zip 失败: {error}"))?;
    }
    if let Some(parent) = PathBuf::from(&out).parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("创建目录失败: {error}"))?;
    }
    std::fs::write(&out, &bytes).map_err(|error| format!("写入 {out} 失败: {error}"))?;
    println!("sha256={:x}", Sha256::digest(&bytes));
    println!("size={}", bytes.len());
    Ok(())
}

fn write_entry(
    writer: &mut zip::ZipWriter<std::io::Cursor<&mut Vec<u8>>>,
    name: &str,
    content: &[u8],
    options: zip::write::SimpleFileOptions,
) -> Result<(), String> {
    writer
        .start_file(name, options)
        .map_err(|error| format!("写入 {name} 失败: {error}"))?;
    writer
        .write_all(content)
        .map_err(|error| format!("写入 {name} 内容失败: {error}"))?;
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
