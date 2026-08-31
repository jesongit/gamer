//! 显式语义校验（先于结构回退校验执行，保证每个违规项有专属错误码）。
//! 规则与 release/contracts/validate-manifest.mjs 的 semanticChecks 一致。

use std::collections::HashMap;

use serde_json::Value;

use crate::manifest::codes;
use crate::manifest::semver;
use crate::manifest::ManifestError;

pub const PRODUCT: &str = "gamebot";
pub const KNOWN_PLATFORMS: &[&str] = &["windows-x86_64"];
pub const KNOWN_CHANNELS: &[&str] = &["stable", "beta"];
pub const JAR_BINDING: &str = "application";

/// 上限值（与 manifest-v1.schema.json / validate-manifest.mjs 同步）。
pub struct Limits;
impl Limits {
    /// 单压缩包 2 GiB
    pub const MAX_ARTIFACT_BYTES: f64 = 2_147_483_648.0;
    /// 单文件 1 GiB
    pub const MAX_FILE_BYTES: f64 = 1_073_741_824.0;
    /// 平台内所有声明 size 之和 6 GiB
    pub const MAX_TOTAL_BYTES: f64 = 6_442_450_944.0;
}

pub struct Expectations<'a> {
    pub expect_current_version: Option<&'a str>,
    pub expect_channel: Option<&'a str>,
}

fn err(code: &'static str, detail: impl Into<String>) -> ManifestError {
    ManifestError {
        code: code.to_string(),
        detail: detail.into(),
    }
}

/// 在 serde_json::Value 上执行语义规则（类型不合法的字段交由结构回退校验拒绝）。
pub fn semantic_checks(manifest: &Value, exp: &Expectations<'_>) -> Vec<ManifestError> {
    let mut errors = Vec::new();

    // -- schema / product ----------------------------------------------------
    if let Some(v) = manifest.get("schema_version") {
        if v.as_i64() != Some(1) {
            errors.push(err(
                codes::UNKNOWN_SCHEMA_VERSION,
                format!("schema_version={v}；只定义了 1"),
            ));
        }
    }
    if let Some(v) = manifest.get("product") {
        if v.as_str() != Some(PRODUCT) {
            errors.push(err(
                codes::PRODUCT_MISMATCH,
                format!("product={v}；应为 {PRODUCT:?}"),
            ));
        }
    }

    // -- release 级规则 --------------------------------------------------------
    let empty = serde_json::Map::new();
    let release = manifest
        .get("release")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    for field in [
        "version",
        "minimum_launcher_version",
        "minimum_upgrade_version",
    ] {
        if let Some(Value::String(v)) = release.get(field) {
            if semver::parse(v).is_none() {
                errors.push(err(
                    codes::VERSION_NOT_SEMVER,
                    format!("release.{field}={v:?} 不是 SemVer 2.0.0"),
                ));
            }
        }
    }
    if let Some(v) = release.get("channel") {
        let known = v.as_str().is_some_and(|s| KNOWN_CHANNELS.contains(&s));
        if !known {
            errors.push(err(codes::CHANNEL_INVALID, format!("release.channel={v}")));
        }
    }
    if let Some(expect) = exp.expect_current_version {
        if let Some(Value::String(version)) = release.get("version") {
            if let Some(target) = semver::parse(version) {
                match semver::parse(expect) {
                    None => errors.push(err(
                        codes::VERSION_NOT_SEMVER,
                        format!("--expect-current-version {expect:?} 不是 SemVer"),
                    )),
                    Some(current) => {
                        if semver::is_lt(&target, &current) {
                            errors.push(err(
                                codes::VERSION_DOWNGRADE,
                                format!(
                                    "release.version {version} 低于当前安装 {expect}；拒绝版本降级（计划 §11.1）"
                                ),
                            ));
                        }
                    }
                }
            }
        }
    }
    if let Some(expect_channel) = exp.expect_channel {
        if let Some(Value::String(channel)) = release.get("channel") {
            if channel != expect_channel {
                errors.push(err(
                    codes::CHANNEL_MISMATCH,
                    format!("release.channel={channel:?} 与本轨道消费的 {expect_channel:?} 不一致"),
                ));
            }
        }
    }

    // -- 平台白名单 -------------------------------------------------------------
    let platforms = manifest.get("platforms").and_then(Value::as_object);
    if let Some(platforms) = platforms {
        if platforms.is_empty() {
            errors.push(err(codes::UNKNOWN_PLATFORM, "platforms 为空"));
        }
        for key in platforms.keys() {
            if !KNOWN_PLATFORMS.contains(&key.as_str()) {
                errors.push(err(
                    codes::UNKNOWN_PLATFORM,
                    format!("平台 {key:?} 不在白名单 {KNOWN_PLATFORMS:?}"),
                ));
            }
        }
    }

    // -- 每平台深检 --------------------------------------------------------------
    if let Some(platforms) = platforms {
        for (pname, platform) in platforms {
            let Some(platform) = platform.as_object() else {
                continue;
            };
            check_platform(pname, platform, exp, &mut errors);
        }
    }

    errors
}

/// 收集压缩包 artifact 的 sha256/size 声明。
fn push_artifact(
    at: String,
    artifact: Option<&Value>,
    hashes: &mut Vec<(String, String)>,
    sizes: &mut Vec<(String, f64, f64)>,
) {
    let Some(a) = artifact.and_then(Value::as_object) else {
        return;
    };
    if let Some(Value::String(h)) = a.get("sha256") {
        hashes.push((at.clone(), h.clone()));
    }
    if let Some(Value::Number(n)) = a.get("size") {
        if let Some(v) = n.as_f64() {
            sizes.push((at, v, Limits::MAX_ARTIFACT_BYTES));
        }
    }
}

fn check_platform(
    pname: &str,
    platform: &serde_json::Map<String, Value>,
    _exp: &Expectations<'_>,
    errors: &mut Vec<ManifestError>,
) {
    // jar 绑定（计划 §2：scrcpy-server.jar 与应用版本强绑定）
    let jar = platform
        .get("resources")
        .and_then(Value::as_object)
        .and_then(|r| r.get("scrcpy_server"))
        .and_then(Value::as_object);
    if let Some(jar) = jar {
        if let Some(binding) = jar.get("binding") {
            if binding.as_str() != Some(JAR_BINDING) {
                errors.push(err(
                    codes::JAR_BINDING_MISMATCH,
                    format!("{pname}.resources.scrcpy_server.binding={binding}；jar 必须为 \"{JAR_BINDING}\" 绑定"),
                ));
            }
        }
        if let Some(Value::String(path)) = jar.get("path") {
            if !path.starts_with("assets/") {
                errors.push(err(
                    codes::JAR_PATH_NOT_ASSETS,
                    format!("{pname}.resources.scrcpy_server.path 必须位于 assets/ 下"),
                ));
            }
        }
    }

    // 收集 hash / size
    let mut hashes: Vec<(String, String)> = Vec::new(); // (位置, 值)
    let mut sizes: Vec<(String, f64, f64)> = Vec::new(); // (位置, 值, 上限)
    push_artifact(
        format!("{pname}.app.artifact"),
        platform.get("app").and_then(|a| a.get("artifact")),
        &mut hashes,
        &mut sizes,
    );
    if let Some(Value::Array(components)) = platform.get("components") {
        for (ci, comp) in components.iter().enumerate() {
            let Some(comp) = comp.as_object() else {
                continue;
            };
            let cid = comp
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("#{ci}"));
            push_artifact(
                format!("{pname}.components[{ci}]({cid}).artifact"),
                comp.get("artifact"),
                &mut hashes,
                &mut sizes,
            );
            if let Some(Value::Array(files)) = comp.get("required_files") {
                for (fi, file) in files.iter().enumerate() {
                    let Some(file) = file.as_object() else {
                        continue;
                    };
                    let at = format!("{pname}.components[{ci}]({cid}).required_files[{fi}]");
                    if let Some(Value::String(h)) = file.get("sha256") {
                        hashes.push((at.clone(), h.clone()));
                    }
                    if let Some(Value::Number(n)) = file.get("size") {
                        if let Some(v) = n.as_f64() {
                            sizes.push((at, v, Limits::MAX_FILE_BYTES));
                        }
                    }
                }
            }
        }
    }
    if let Some(jar) = jar {
        if let Some(Value::String(h)) = jar.get("sha256") {
            hashes.push((format!("{pname}.resources.scrcpy_server.sha256"), h.clone()));
        }
    }

    // hash 规则：64 位小写 hex
    for (at, h) in &hashes {
        let is_mixed_hex = h.len() == 64 && h.bytes().all(|c| c.is_ascii_hexdigit());
        let has_upper = h.bytes().any(|c| c.is_ascii_uppercase());
        if is_mixed_hex && has_upper {
            errors.push(err(
                codes::SHA256_UPPERCASE,
                format!("{at}: hash 必须为小写"),
            ));
        } else {
            let is_lower_hex = h.len() == 64
                && h.bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c));
            if !is_lower_hex {
                errors.push(err(
                    codes::SHA256_WRONG_LENGTH,
                    format!("{at}: 应为 64 位小写 hex，实际长度 {}", h.len()),
                ));
            }
        }
    }

    // size 规则：>= 0 且受限
    let mut total = 0.0;
    for (at, value, cap) in &sizes {
        total += value;
        if *value < 0.0 {
            errors.push(err(codes::SIZE_NEGATIVE, format!("{at}: size={value}")));
        } else if *value > *cap {
            errors.push(err(
                codes::SIZE_OVERSIZED,
                format!("{at}: size={value} 超过上限 {cap}"),
            ));
        }
    }
    if total > Limits::MAX_TOTAL_BYTES {
        errors.push(err(
            codes::SIZE_OVERSIZED,
            format!(
                "{pname}: 声明 size 总和 {total} 超过上限 {}",
                Limits::MAX_TOTAL_BYTES
            ),
        ));
    }

    // 路径规则（单条 + 跨条目）
    let (entries, names) = collect_path_entries(pname, platform);
    for (at, path) in entries.iter().chain(names.iter()) {
        if let Some(code) = crate::manifest::pathsafe::check_single_path(path) {
            errors.push(err(code, format!("{at}: {path:?}")));
        }
    }
    // 安装树命名空间（entrypoint + required_files + resources）内：
    // 大小写不敏感碰撞与重复条目拒绝（Windows 大小写不敏感）
    let mut seen: HashMap<String, String> = HashMap::new();
    for (at, path) in &entries {
        let lower = path.to_lowercase();
        match seen.get(&lower) {
            None => {
                seen.insert(lower, path.clone());
            }
            Some(prev) if prev == path => {
                errors.push(err(
                    codes::PATH_DUPLICATE_ENTRY,
                    format!("{pname}: {path:?} 声明了两次（{at}）"),
                ));
            }
            Some(prev) => {
                errors.push(err(
                    codes::PATH_CASE_COLLISION,
                    format!("{pname}: {path:?} 与 {prev:?} 大小写不敏感碰撞（{at}）"),
                ));
            }
        }
    }
}

/// 路径条目：(位置标签, 路径)。
type PathEntry = (String, String);

/// 收集安装树路径条目（entries）与发行资产名（names，仅做单条检查、不参与碰撞）。
fn collect_path_entries(
    pname: &str,
    platform: &serde_json::Map<String, Value>,
) -> (Vec<PathEntry>, Vec<PathEntry>) {
    let mut entries = Vec::new();
    let mut names = Vec::new();
    if let Some(app) = platform.get("app").and_then(Value::as_object) {
        if let Some(Value::String(entrypoint)) = app.get("entrypoint") {
            entries.push((format!("{pname}.app.entrypoint"), entrypoint.clone()));
        }
        if let Some(Value::String(name)) = app.get("artifact").and_then(|a| a.get("name")) {
            names.push((format!("{pname}.app.artifact.name"), name.clone()));
        }
    }
    if let Some(Value::Array(components)) = platform.get("components") {
        for (ci, comp) in components.iter().enumerate() {
            let Some(comp) = comp.as_object() else {
                continue;
            };
            let cid = comp
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("#{ci}"));
            if let Some(Value::Array(files)) = comp.get("required_files") {
                for (fi, file) in files.iter().enumerate() {
                    if let Some(Value::String(path)) = file.as_object().and_then(|f| f.get("path"))
                    {
                        entries.push((
                            format!("{pname}.components[{ci}]({cid}).required_files[{fi}]"),
                            path.clone(),
                        ));
                    }
                }
            }
            if let Some(Value::String(name)) = comp.get("artifact").and_then(|a| a.get("name")) {
                names.push((
                    format!("{pname}.components[{ci}]({cid}).artifact.name"),
                    name.clone(),
                ));
            }
        }
    }
    if let Some(resources) = platform.get("resources").and_then(Value::as_object) {
        for (rid, res) in resources {
            if let Some(Value::String(path)) = res.as_object().and_then(|r| r.get("path")) {
                entries.push((format!("{pname}.resources.{rid}.path"), path.clone()));
            }
        }
    }
    (entries, names)
}
