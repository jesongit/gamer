//! 结构回退校验：用 serde 严格类型（`deny_unknown_fields`）+ 少量手写规则执行
//! `manifest-v1.schema.json` 的结构约束。语义校验已给出专属错误码的情况下不会走到这里。

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

/// release manifest v1 顶层（白名单外字段 = 结构错误）。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: i64,
    pub product: String,
    pub release: Release,
    pub platforms: BTreeMap<String, Platform>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    pub version: String,
    pub channel: String,
    pub published_at: String,
    pub minimum_launcher_version: String,
    pub minimum_upgrade_version: String,
    pub data_schema: i64,
    pub rollback_floor: i64,
    pub release_notes_url: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Platform {
    pub app: AppPackage,
    pub components: Vec<Component>,
    pub resources: Resources,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppPackage {
    pub artifact: Artifact,
    pub entrypoint: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Component {
    pub id: String,
    pub version: String,
    pub artifact: Artifact,
    pub required_files: Vec<RequiredFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub name: String,
    pub url: String,
    pub size: i64,
    pub sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequiredFile {
    pub path: String,
    pub size: i64,
    pub sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resources {
    pub scrcpy_server: ScrcpyServer,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScrcpyServer {
    pub version: String,
    pub path: String,
    pub sha256: String,
    pub binding: String,
}

const I32_MAX: i64 = 2_147_483_647;
const MAX_URL_LEN: usize = 2048;
const MAX_REL_PATH_LEN: usize = 512;

fn is_https_url(s: &str) -> bool {
    s.len() <= MAX_URL_LEN
        && s.starts_with("https://")
        && !s["https://".len()..].chars().any(char::is_whitespace)
}

fn is_rfc3339(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() < 20 {
        return false;
    }
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    // YYYY-MM-DD
    if !digits(0..4) || b[4] != b'-' || !digits(5..7) || b[7] != b'-' || !digits(8..10) {
        return false;
    }
    // [Tt] HH:MM:SS
    if !(b[10] == b'T' || b[10] == b't') {
        return false;
    }
    if !digits(11..13) || b[13] != b':' || !digits(14..16) || b[16] != b':' || !digits(17..19) {
        return false;
    }
    let mut i = 19;
    // 可选小数秒
    if b[i] == b'.' {
        i += 1;
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return false;
        }
    }
    // [Zz | ±HH:MM]
    match b.get(i) {
        Some(b'Z' | b'z') => i + 1 == b.len(),
        Some(b'+' | b'-') => {
            i += 1;
            b.len() == i + 5 && digits(i..i + 2) && b[i + 2] == b':' && digits(i + 3..i + 5)
        }
        _ => false,
    }
}

fn is_component_id(s: &str) -> bool {
    let b = s.as_bytes();
    if b.is_empty() || b.len() > 32 {
        return false;
    }
    if !b[0].is_ascii_lowercase() {
        return false;
    }
    b[1..]
        .iter()
        .all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

fn artifact_errors(at: &str, a: &Artifact, out: &mut Vec<String>) {
    if !is_https_url(&a.url) {
        out.push(format!("{at}.url: 必须为 https:// URL 且无空白"));
    }
    if a.name.len() > MAX_REL_PATH_LEN {
        out.push(format!("{at}.name: 超过 {MAX_REL_PATH_LEN} 字符上限"));
    }
}

/// 结构校验；返回的每条 detail 都映射为 `schema-invalid`。
pub fn structural_errors(value: &Value) -> Vec<String> {
    let m: Manifest = match serde_json::from_value(value.clone()) {
        Ok(m) => m,
        Err(e) => return vec![format!("结构校验失败: {e}")],
    };
    let mut out = Vec::new();

    if !(1..=I32_MAX).contains(&m.release.data_schema) {
        out.push(format!("release.data_schema 必须为 1..={I32_MAX} 的整数"));
    }
    if !(1..=I32_MAX).contains(&m.release.rollback_floor) {
        out.push(format!(
            "release.rollback_floor 必须为 1..={I32_MAX} 的整数"
        ));
    }
    if !is_rfc3339(&m.release.published_at) {
        out.push("release.published_at: 不是 RFC 3339 date-time".to_string());
    }
    if !is_https_url(&m.release.release_notes_url) {
        out.push("release.release_notes_url: 必须为 https:// URL".to_string());
    }

    if m.platforms.is_empty() {
        out.push("platforms 至少需要一个平台".to_string());
    }
    for (pname, platform) in &m.platforms {
        if platform.components.len() > 16 {
            out.push(format!("{pname}: components 超过 16 项上限"));
        }
        if platform.app.entrypoint.len() > MAX_REL_PATH_LEN {
            out.push(format!(
                "{pname}.app.entrypoint: 超过 {MAX_REL_PATH_LEN} 字符上限"
            ));
        }
        artifact_errors(
            &format!("{pname}.app.artifact"),
            &platform.app.artifact,
            &mut out,
        );
        for (ci, comp) in platform.components.iter().enumerate() {
            if !is_component_id(&comp.id) {
                out.push(format!(
                    "{pname}.components[{ci}].id: {:?} 不匹配 ^[a-z][a-z0-9-]{{0,31}}$",
                    comp.id
                ));
            }
            if comp.version.is_empty() || comp.version.len() > 64 {
                out.push(format!(
                    "{pname}.components[{}].version: 长度须在 1..=64",
                    ci
                ));
            }
            if comp.required_files.is_empty() {
                out.push(format!(
                    "{pname}.components[{ci}].required_files: 至少 1 项"
                ));
            }
            if comp.required_files.len() > 1024 {
                out.push(format!(
                    "{pname}.components[{ci}].required_files: 超过 1024 项上限"
                ));
            }
            for (fi, file) in comp.required_files.iter().enumerate() {
                if file.path.len() > MAX_REL_PATH_LEN {
                    out.push(format!(
                        "{pname}.components[{ci}].required_files[{fi}].path: 超过 {MAX_REL_PATH_LEN} 字符上限"
                    ));
                }
            }
            artifact_errors(
                &format!("{pname}.components[{ci}].artifact"),
                &comp.artifact,
                &mut out,
            );
        }
        let jar = &platform.resources.scrcpy_server;
        if jar.version.is_empty() || jar.version.len() > 32 {
            out.push(format!(
                "{pname}.resources.scrcpy_server.version: 长度须在 1..=32"
            ));
        }
        if jar.path.len() > MAX_REL_PATH_LEN {
            out.push(format!(
                "{pname}.resources.scrcpy_server.path: 超过 {MAX_REL_PATH_LEN} 字符上限"
            ));
        }
    }
    out
}
