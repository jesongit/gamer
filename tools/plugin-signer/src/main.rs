//! Dev 插件市场签名工具（tools/build-plugins.ps1 的后端）。
//!
//! 仅用于本地/开发市场：私钥存放于 tools/plugin-signing/，公钥以
//! `<key_id>.pem` 形式提交，并由 server（src/extensions/signature.rs）内嵌为
//! 内置信任锚。生产部署必须换用独立 keypair 并通过 plugin-trust 目录注入。
//!
//! 子命令：
//! - keygen  --out <key 文件> [--key-id gamer-dev-1] [--pem-out <pem 文件>]
//!     生成 32 字节 hex 私钥；同时打印/写出 SPKI PEM 公钥。
//! - pack    --manifest <toml> --wasm <component.wasm> --key <key 文件>
//!           --key-id <id> --out <gplugin> [--file <归档路径>=<源文件>]...
//!     打包 .gplugin（zip：manifest.toml + plugin.wasm + 附加文件 +
//!     signature.sig，签名覆盖 manifest.toml 原始字节）。
//! - registry-proof --key <key 文件> --key-id <id> --id <插件 id>
//!           --version <版本> --download-url <url> --sha256 <hex>
//!     产出 Registry proof（base64(JSON)），前端安装官方插件时经
//!     x-gamer-registry-proof 头提交，服务端验签后放行。

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};
use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

const SIG_MAGIC: &str = "gamebot-gplugin-sig-1";
const REGISTRY_CLAIM_MAGIC: &str = "gamebot-gplugin-registry-entry-1";
const SIGNATURE_FILE: &str = "signature.sig";
const MANIFEST_FILE: &str = "manifest.toml";

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
        return Err("用法: plugin-signer <keygen|pack|registry-proof> ...".into());
    };
    match command.as_str() {
        "keygen" => keygen(&flags(&args[1..])?),
        "pack" => pack(&flags(&args[1..])?),
        "registry-proof" => registry_proof(&flags(&args[1..])?),
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

fn load_key(path: &str) -> Result<SigningKey, String> {
    let hex = std::fs::read_to_string(path).map_err(|error| format!("读取私钥失败: {error}"))?;
    let hex = hex.trim();
    let mut bytes = [0u8; 32];
    for index in 0..32 {
        bytes[index] = u8::from_str_radix(
            hex.get(index * 2..index * 2 + 2)
                .ok_or_else(|| "私钥长度必须是 64 位 hex".to_string())?,
            16,
        )
        .map_err(|error| format!("私钥不是合法 hex: {error}"))?;
    }
    Ok(SigningKey::from_bytes(&bytes))
}

/// SPKI DER 包装（302a300506032b6570032100 + 32 字节公钥）→ PEM。
fn public_key_pem(verifying: &ed25519_dalek::VerifyingKey) -> String {
    let mut der = vec![0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00];
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

fn pack(flags: &Flags) -> Result<(), String> {
    let manifest_path = require(flags, "manifest")?;
    let wasm_path = require(flags, "wasm")?;
    let key_path = require(flags, "key")?;
    let key_id = require(flags, "key-id")?;
    let out = require(flags, "out")?;
    let manifest = std::fs::read(&manifest_path)
        .map_err(|error| format!("读取 manifest 失败: {error}"))?;
    let wasm =
        std::fs::read(&wasm_path).map_err(|error| format!("读取 wasm 失败: {error}"))?;
    let signing = load_key(&key_path)?;

    let signature = signing.sign(&manifest);
    let sig_file = format!("{SIG_MAGIC} {key_id}\n{}\n", B64.encode(signature.to_bytes()));

    let mut bytes = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o644);
        write_entry(&mut writer, MANIFEST_FILE, &manifest, options)?;
        write_entry(&mut writer, "plugin.wasm", &wasm, options)?;
        // --file <归档路径>=<源文件>：manifest 声明的 UI 入口等附加文件。
        for (_, source) in &flags.multi {
            let Some((archive_name, source_path)) = source.split_once('=') else {
                return Err(format!("--file 需要 <归档路径>=<源文件> 形式: {source}"));
            };
            let content = std::fs::read(source_path)
                .map_err(|error| format!("读取附加文件 {source_path} 失败: {error}"))?;
            write_entry(&mut writer, archive_name, &content, options)?;
        }
        write_entry(&mut writer, SIGNATURE_FILE, sig_file.as_bytes(), options)?;
        writer.finish().map_err(|error| format!("收尾 zip 失败: {error}"))?;
    }
    if let Some(parent) = PathBuf::from(&out).parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("创建目录失败: {error}"))?;
    }
    std::fs::write(&out, &bytes).map_err(|error| format!("写入 {out} 失败: {error}"))?;
    println!("sha256={:x}", Sha256::digest(&bytes));
    println!("size={}", bytes.len());
    Ok(())
}

fn registry_proof(flags: &Flags) -> Result<(), String> {
    let key_path = require(flags, "key")?;
    let key_id = require(flags, "key-id")?;
    let id = require(flags, "id")?;
    let version = require(flags, "version")?;
    let download_url = require(flags, "download-url")?;
    let sha256 = require(flags, "sha256")?;
    let signing = load_key(&key_path)?;

    let claim = format!(
        "{REGISTRY_CLAIM_MAGIC}\nid={id}\nversion={version}\ndownload_url={download_url}\nsha256={sha256}\n"
    );
    let signature = signing.sign(claim.as_bytes());
    let proof = serde_json::json!({
        "id": id,
        "version": version,
        "download_url": download_url,
        "sha256": sha256,
        "key_id": key_id,
        "signature": B64.encode(signature.to_bytes()),
    });
    let mut stdout = std::io::stdout();
    stdout
        .write_all(B64.encode(proof.to_string()).as_bytes())
        .map_err(|error| format!("输出 proof 失败: {error}"))?;
    println!();
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
