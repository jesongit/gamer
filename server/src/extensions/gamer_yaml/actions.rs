//! gamer.yaml 的**版本化公开动作清单**（计划 §10.1 跨插件能力契约）。
//!
//! 视频工作台（gamer.video）等兄弟插件对 YAML 栈的一切制作能力调用只经
//! `POST /api/extensions/gamer.yaml/call` 的 native_call_action 缝（[`extensions/mod.rs`]）
//! 走本清单声明过的动作；**禁止**直接写 gamer.yaml 私有目录或自行解析 YAML。
//! 清单是唯一契约面：动作名 / 版本 / 参数 / 调用方上下文要求集中在此声明，
//! 实现与清单由下方测试双向锁死（清单里有的必有实现或显式映射，实现必在清单）。
//!
//! 三种 surface：
//! - [`ActionSurface::Native`]：本缝内原生实现（按调用执行，无常驻实例）；
//! - [`ActionSurface::Rest`]：复用既有 Core REST 端点（不重复实现，`mapping`
//!   写明端点映射关系；调用方直接打 REST，不走本缝）；
//! - [`ActionSurface::Frontend`]：纯前端导航/装配契约（无服务端往返）。
//!
//! 调用方上下文要求（`caller` / `context`）是**声明性契约**：REST 通路经用户
//! 会话鉴权，进程内没有可验证的调用方身份，因此 `caller` 记录本动作的预期
//! 调用方（预留未来缝上加 caller 校验的形状），`context` 列出调用必须携带的
//! 上下文字段（Package Context / 帧身份 / 校准元数据），缺失即结构化拒绝。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::video_draft::{self, AUTOMATION_CREATE_DRAFT};
use super::yaml_extension::YAML_EXTENSION_ID;
use crate::extensions::{ExtensionError, ExtensionResult};
use crate::resources::{PackageStore, SaveBinaryValidation, SaveValidation};

/// 模板创建动作（§10.2）：从视频确定帧裁出的 PNG + 相对搜索区域 → 存为
/// 当前 Package 的 gamer.yaml 模板（灰度归一化经资源字节钩子自动生效）。
pub(crate) const TEMPLATE_CREATE_FROM_FRAME: &str = "template.create_from_frame";
/// 草稿保存动作（§10.3）：v3 草稿文本 → 存为当前 Package 的 automations 脚本
///（保存经 v3 校验钩子，非法源结构化拒绝）。
pub(crate) const AUTOMATION_SAVE_DRAFT: &str = "automation.save_draft";
/// 模板离线测试：与 `POST /api/capabilities/vision/test`（media_id+pts_us/frame_index
/// 离线寻址）同能力 —— **复用 REST 不重复实现**；清单内只登记映射关系。
pub(crate) const VISION_TEST_TEMPLATE: &str = "vision.test_template";
/// 草稿保存成功后打开/定位 YAML 编辑器：纯前端导航契约（video 面板 →
/// gamer.yaml:automation 面板 + 编辑器加载），无服务端往返。
pub(crate) const AUTOMATION_OPEN_EDITOR: &str = "automation.open_editor";

/// 动作 surface（见模块注释）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActionSurface {
    /// 本缝内原生实现（`dispatch` 有对应分支）。
    Native,
    /// 复用既有 REST 端点（`mapping` 给出映射）。
    Rest,
    /// 纯前端契约（调用方自行解析，无服务端调用）。
    Frontend,
}

/// 公开动作清单条目（唯一契约声明点）。
pub(crate) struct PublicAction {
    /// 动作名（wire 形态 `domain.verb`）。
    pub name: &'static str,
    /// 契约版本（破坏性参数变更必须升版本并保留旧版退出路径）。
    pub version: u32,
    pub surface: ActionSurface,
    /// 一句话语义。
    pub summary: &'static str,
    /// 预期调用方插件 id（声明性；见模块注释）。
    pub caller: &'static str,
    /// 调用必须携带的上下文字段名（values 内）。
    pub context: &'static [&'static str],
    /// values 参数名（不含 context）。
    pub params: &'static [&'static str],
    /// Rest surface 的端点映射说明；其余 surface 为空。
    pub mapping: &'static str,
}

/// gamer.yaml 公开动作清单（唯一声明点；顺序即文档顺序）。
pub(crate) const PUBLIC_ACTIONS: &[PublicAction] = &[
    PublicAction {
        name: TEMPLATE_CREATE_FROM_FRAME,
        version: 1,
        surface: ActionSurface::Native,
        summary: "从视频确定帧裁剪 PNG 创建模板（服务端灰度归一化 + 短名/区域命名规则）",
        caller: "gamer.video",
        context: &["package_id", "frame", "calibration"],
        params: &[
            "name",
            "png_base64",
            "region",
            "preserve_color?",
            "overwrite?",
        ],
        mapping: "",
    },
    PublicAction {
        name: VISION_TEST_TEMPLATE,
        version: 1,
        surface: ActionSurface::Rest,
        summary:
            "模板匹配测试（在线 device_id / 离线 media_id+pts_us|frame_index 互斥），响应附帧身份",
        caller: "gamer.video",
        context: &["pkg", "plugin", "name"],
        params: &[
            "device_id|media_id",
            "pts_us?|frame_index?",
            "threshold?",
            "region?",
        ],
        mapping:
            "POST /api/capabilities/vision/test（Core vision 能力位；离线帧身份随附 frame 字段）",
    },
    PublicAction {
        name: AUTOMATION_CREATE_DRAFT,
        version: 1,
        surface: ActionSurface::Native,
        summary: "录制操作事件 → YAML v3 草稿文本（不落盘不执行；不可映射事件进诊断）",
        caller: "gamer.video",
        context: &["recording_id"],
        params: &["event_ids?", "comments?"],
        mapping: "",
    },
    PublicAction {
        name: AUTOMATION_SAVE_DRAFT,
        version: 1,
        surface: ActionSurface::Native,
        summary: "v3 草稿文本保存为当前 Package 的 automations 脚本（保存边界 v3 校验）",
        caller: "gamer.video",
        context: &["package_id"],
        params: &["name", "yaml", "overwrite?"],
        mapping: "",
    },
    PublicAction {
        name: AUTOMATION_OPEN_EDITOR,
        version: 1,
        surface: ActionSurface::Frontend,
        summary: "打开/定位 YAML 编辑器到刚保存的脚本（自动化面板 + 载入编辑态）",
        caller: "gamer.video",
        context: &["package_id", "script_id"],
        params: &[],
        mapping:
            "前端契约：切到 gamer.yaml:automation 面板并编辑 script_id（automationEditorBridge）",
    },
];

/// native_call_action 缝的 gamer.yaml 侧分发入口（extensions/mod.rs 调用）：
/// 命中清单内 Native 动作 → 应答；其余返回 `None` 交回通用路径。
pub(crate) fn native_call_action(
    extension_id: &str,
    action: &str,
    values: &Value,
    data_dir: &Path,
) -> Option<ExtensionResult<Value>> {
    if extension_id != YAML_EXTENSION_ID {
        return None;
    }
    let result = match action {
        AUTOMATION_CREATE_DRAFT => video_draft::create_draft(values, data_dir),
        AUTOMATION_SAVE_DRAFT => save_draft(values, data_dir),
        TEMPLATE_CREATE_FROM_FRAME => create_template_from_frame(values, data_dir),
        _ => return None,
    };
    Some(result)
}

// ---------------------------------------------------------------------------
// 共用助手
// ---------------------------------------------------------------------------

/// 动作内自建 PackageStore（与组合根同源 data_dir）。PackageStore 无进程内
/// 缓存（原子文件读写），实例间一致；gamer.yaml 的内容钩子（v3 校验 / 模板
/// 灰度归一化）按 plugin-id 注册在本实例上，保证动作写路径与 REST 写路径
/// 走同一套校验/归一化。
fn store_for(data_dir: &Path) -> Result<PackageStore, ExtensionError> {
    let store = PackageStore::open(&crate::config::Config {
        data_dir: data_dir.to_path_buf(),
        ..Default::default()
    })
    .map_err(|error| ExtensionError::Runtime(format!("打开 PackageStore 失败: {error:#}")))?;
    super::resources::register_resource_handlers(&store);
    Ok(store)
}

fn rejected(message: impl Into<String>) -> ExtensionError {
    ExtensionError::CallRejected(message.into())
}

/// 取非空字符串字段。
fn str_field(values: &Value, key: &str) -> Result<String, ExtensionError> {
    let raw = values
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if raw.is_empty() {
        return Err(rejected(format!("{key} 不能为空")));
    }
    Ok(raw.to_string())
}

/// 脚本/模板名的安全相对路径校验：拒绝空段、`.`/`..`、路径分隔符混入与
/// 超 255 字节段（与 PackageStore 的 traversal 防护双保险，动作层先给
/// 结构化拒绝而非底层错误）。
fn validate_relative_name(name: &str, field: &str) -> Result<String, ExtensionError> {
    let name = name.trim().trim_matches('/');
    if name.is_empty() {
        return Err(rejected(format!("{field} 不能为空")));
    }
    for segment in name.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(rejected(format!("{field} 含非法路径段: {segment:?}")));
        }
        if segment.len() > 255 {
            return Err(rejected(format!(
                "{field} 路径段超过 255 字节: {segment:?}"
            )));
        }
    }
    Ok(name.to_string())
}

// ---------------------------------------------------------------------------
// automation.save_draft
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveDraftRequest {
    package_id: String,
    name: String,
    yaml: String,
    #[serde(default)]
    overwrite: bool,
}

/// `automation.save_draft`：草稿文本 → `automations/<name>.yaml`（v3 保存钩子
/// 强制校验；非 v3 结构化拒绝）。`overwrite=false` 且目标已存在 → 名字冲突。
fn save_draft(values: &Value, data_dir: &Path) -> ExtensionResult<Value> {
    let request: SaveDraftRequest = serde_json::from_value(values.clone()).map_err(|error| {
        rejected(format!(
            "automation.save_draft 入参无效（需要 package_id/name/yaml）: {error}"
        ))
    })?;
    let package_id = request.package_id.trim().to_string();
    if package_id.is_empty() {
        return Err(rejected("package_id（Package Context）不能为空"));
    }
    let raw_name = request.name.trim();
    let with_ext = if raw_name.to_ascii_lowercase().ends_with(".yaml")
        || raw_name.to_ascii_lowercase().ends_with(".yml")
    {
        raw_name.to_string()
    } else {
        format!("{raw_name}.yaml")
    };
    let rel = validate_relative_name(&with_ext, "name")?;
    let path = format!("automations/{rel}");
    let store = store_for(data_dir)?;
    // 保存边界 v3 校验（与 REST PUT 同一钩子）；诊断原样回传（结构化数组）。
    store
        .validate_save(SaveValidation {
            package: &package_id,
            plugin: YAML_EXTENSION_ID,
            path: &path,
            content: &request.yaml,
            store: &store,
        })
        .map_err(|diagnostics| {
            rejected(format!(
                "草稿未通过 YAML v3 校验: {}",
                serde_json::to_string(&diagnostics).unwrap_or_default()
            ))
        })?;
    let exists = store
        .read_text(&package_id, YAML_EXTENSION_ID, &path)
        .map_err(|error| rejected(format!("读取目标脚本失败: {error:#}")))?
        .is_some();
    if exists && !request.overwrite {
        return Err(rejected(format!(
            "自动化脚本已存在: {package_id}/{path}（确认覆盖请带 overwrite:true）"
        )));
    }
    let entry = store
        .write_text(
            &package_id,
            YAML_EXTENSION_ID,
            &path,
            &request.yaml,
            None,
            request.overwrite,
        )
        .map_err(|error| rejected(format!("保存脚本失败: {error:#}")))?;
    Ok(json!({
        "action": AUTOMATION_SAVE_DRAFT,
        "version": 1,
        "id": format!("{package_id}/{with_ext}"),
        "path": entry.path,
        "package_id": package_id,
        "content_version": entry.meta.get("version").cloned().unwrap_or(Value::Null),
    }))
}

// ---------------------------------------------------------------------------
// template.create_from_frame
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateTemplateRequest {
    package_id: String,
    /// 模板短名（区域/颜色后缀由服务端按既有命名规则追加）。
    name: String,
    /// 帧 PNG（base64；允许彩色，落盘前统一灰度归一化）。
    png_base64: String,
    /// 相对搜索区域 [x1, y1, x2, y2]（0..=1，x2>x1 / y2>y1；进模板文件名）。
    region: [f64; 4],
    #[serde(default)]
    preserve_color: bool,
    #[serde(default)]
    overwrite: bool,
    /// 帧身份（来源追溯；不做存在性强校验——模板制作允许素材事后被清理）。
    frame: FrameIdentity,
    /// 记录创建时的校准元数据（版本 + 参考分辨率 + 旋转）。
    calibration: CalibrationMeta,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FrameIdentity {
    media_id: String,
    #[serde(default)]
    frame_index: Option<u64>,
    #[serde(default)]
    pts_us: Option<u64>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CalibrationMeta {
    version: u32,
    #[serde(default)]
    reference_size: Option<[u32; 2]>,
    #[serde(default)]
    rotation: Option<u32>,
}

/// 与前端 `composeTemplateName`（web/src/api.js）逐字同规则的文件名组装：
/// 短名 + `#x1_y1_x2_y2`（相对坐标 ×1000 取整、3 位补零、上限 999）+ 可选
/// `#1` 颜色标记 + `.png`。命名规则归 gamer.yaml（调用方不自行拼名）。
fn compose_template_name(short_name: &str, region: &[f64; 4], preserve_color: bool) -> String {
    let raw = short_name.trim();
    let lower = raw.to_ascii_lowercase();
    let stem = if lower.ends_with(".png") {
        &raw[..raw.len() - 4]
    } else {
        raw
    };
    let to_int3 = |v: &f64| {
        let scaled = (v * 1000.0).round();
        let clamped = scaled.clamp(0.0, 999.0) as i64;
        format!("{clamped:03}")
    };
    let mut name = format!(
        "{}#{}_{}_{}_{}",
        stem,
        to_int3(&region[0]),
        to_int3(&region[1]),
        to_int3(&region[2]),
        to_int3(&region[3])
    );
    if preserve_color {
        name.push_str("#1");
    }
    format!("{name}.png")
}

/// 相对区域合法性（与前端 parseTplRegion 同口径：0..=1 且 x2>x1 / y2>y1）。
fn validate_region(region: &[f64; 4]) -> Result<(), ExtensionError> {
    let ok = region
        .iter()
        .all(|v| v.is_finite() && *v >= 0.0 && *v <= 1.0)
        && region[2] > region[0]
        && region[3] > region[1];
    if ok {
        Ok(())
    } else {
        Err(rejected(
            "region 必须是 [x1,y1,x2,y2] 相对坐标（0..=1）且 x2>x1、y2>y1",
        ))
    }
}

/// 短名冲突检测：与当前包内既有模板按短名（去 `#区域`/`#1`）比较。
fn find_short_name_conflict(
    store: &PackageStore,
    package_id: &str,
    short_name: &str,
) -> anyhow::Result<Option<String>> {
    let existing = store.list(package_id, YAML_EXTENSION_ID, "templates")?;
    let wanted = super::resources::template_short_name(short_name).to_ascii_lowercase();
    for entry in existing {
        let name = entry.path.strip_prefix("templates/").unwrap_or(&entry.path);
        if super::resources::template_short_name(name).to_ascii_lowercase() == wanted {
            return Ok(Some(name.to_string()));
        }
    }
    Ok(None)
}

/// `template.create_from_frame`：确定帧裁剪 PNG → gamer.yaml templates/
///（灰度归一化字节钩子 + 冲突检测 + 命名规则），响应携带帧身份与校准元数据。
fn create_template_from_frame(values: &Value, data_dir: &Path) -> ExtensionResult<Value> {
    let request: CreateTemplateRequest = serde_json::from_value(values.clone()).map_err(|error| {
        rejected(format!(
            "template.create_from_frame 入参无效（需要 package_id/name/png_base64/region/frame/calibration）: {error}"
        ))
    })?;
    let package_id = request.package_id.trim().to_string();
    if package_id.is_empty() {
        return Err(rejected("package_id（Package Context）不能为空"));
    }
    let short_name = validate_relative_name(&request.name, "name")?;
    if request.frame.media_id.trim().is_empty() {
        return Err(rejected("frame.media_id 不能为空（帧身份必填）"));
    }
    if request.calibration.version == 0 {
        return Err(rejected("calibration.version 必须 ≥ 1"));
    }
    validate_region(&request.region)?;
    let full_name = compose_template_name(&short_name, &request.region, request.preserve_color);
    let png_base64 = request.png_base64.trim();
    let png = base64_decode(png_base64).ok_or_else(|| rejected("png_base64 不是合法 base64"))?;
    if png.is_empty() {
        return Err(rejected("png_base64 解码后为空"));
    }

    let store = store_for(data_dir)?;
    if !request.overwrite {
        if let Some(existing) = find_short_name_conflict(&store, &package_id, &full_name)
            .map_err(|error| rejected(format!("检查模板重名失败: {error:#}")))?
        {
            return Err(rejected(format!(
                "模板短名冲突: {existing}（确认覆盖请带 overwrite:true）"
            )));
        }
    }
    // 字节钩子：8-bit 灰度 PNG 归一化（与 REST PUT 模板同一路径）。
    let normalized = store
        .validate_save_binary(SaveBinaryValidation {
            package: &package_id,
            plugin: YAML_EXTENSION_ID,
            path: &format!("templates/{full_name}"),
            bytes: &png,
            store: &store,
        })
        .map_err(|diagnostics| {
            rejected(format!(
                "模板图片未通过校验: {}",
                serde_json::to_string(&diagnostics).unwrap_or_default()
            ))
        })?;
    let entry = store
        .write_binary(
            &package_id,
            YAML_EXTENSION_ID,
            &format!("templates/{full_name}"),
            &normalized,
            None,
            request.overwrite,
        )
        .map_err(|error| rejected(format!("保存模板失败: {error:#}")))?;
    Ok(json!({
        "action": TEMPLATE_CREATE_FROM_FRAME,
        "version": 1,
        "name": full_name,
        "short_name": super::resources::template_short_name(&full_name),
        "path": entry.path,
        "package_id": package_id,
        "size": entry.size,
        "region": request.region,
        "frame": {
            "media_id": request.frame.media_id.trim(),
            "frame_index": request.frame.frame_index,
            "pts_us": request.frame.pts_us,
        },
        "calibration": {
            "version": request.calibration.version,
            "reference_size": request.calibration.reference_size,
            "rotation": request.calibration.rotation,
        },
    }))
}

/// 标准 base64 解码（无外部依赖；接受无 padding 输入）。
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    const INVALID: u8 = 0xFF;
    fn table() -> [u8; 256] {
        let mut t = [INVALID; 256];
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        for (i, &c) in alphabet.iter().enumerate() {
            t[c as usize] = i as u8;
        }
        t
    }
    let table = table();
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for &byte in input.as_bytes() {
        match byte {
            b'\r' | b'\n' | b' ' | b'\t' => continue,
            b'=' => break,
            c => {
                let value = table[c as usize];
                if value == INVALID {
                    return None;
                }
                buffer = (buffer << 6) | value as u32;
                bits += 6;
                if bits >= 8 {
                    bits -= 8;
                    out.push((buffer >> bits) as u8);
                }
            }
        }
    }
    Some(out)
}

/// 测试与诊断辅助：脚本资源落盘路径（相对插件根）。
#[cfg(test)]
pub(crate) fn automation_path_of(name: &str) -> PathBuf {
    PathBuf::from("automations").join(name)
}

/// base64 编码（测试夹具用）。
#[cfg(test)]
pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

// ---------------------------------------------------------------------------
// 清单 ↔ 实现 双向锁（§10.1：清单是唯一契约面）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::gamer_yaml::YAML_EXTENSION_ID;

    /// 清单内每个 Native 动作都有分发分支；每个分发分支都在清单内且为 Native。
    #[test]
    fn catalog_and_dispatch_are_bidirectionally_locked() {
        let native_in_catalog: Vec<&str> = PUBLIC_ACTIONS
            .iter()
            .filter(|action| action.surface == ActionSurface::Native)
            .map(|action| action.name)
            .collect();
        let dispatch_branches = [
            AUTOMATION_CREATE_DRAFT,
            AUTOMATION_SAVE_DRAFT,
            TEMPLATE_CREATE_FROM_FRAME,
        ];
        for name in &native_in_catalog {
            assert!(
                dispatch_branches.contains(name),
                "清单声明 Native 但无分发分支: {name}"
            );
        }
        for name in &dispatch_branches {
            assert!(
                native_in_catalog.contains(name),
                "分发分支未在清单声明: {name}"
            );
        }
        // Rest surface 必须带映射说明；Frontend 必须带上下文契约。
        for action in PUBLIC_ACTIONS {
            match action.surface {
                ActionSurface::Rest => {
                    assert!(!action.mapping.is_empty(), "{} 缺 REST 映射", action.name);
                }
                ActionSurface::Frontend => {
                    assert!(
                        !action.context.is_empty(),
                        "{} 缺前端上下文契约",
                        action.name
                    );
                }
                ActionSurface::Native => {}
            }
        }
        // 版本契约：全部 ≥ 1 且动作名唯一。
        let mut names: Vec<_> = PUBLIC_ACTIONS.iter().map(|a| a.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), PUBLIC_ACTIONS.len(), "动作名必须唯一");
        assert!(PUBLIC_ACTIONS.iter().all(|a| a.version >= 1));
    }

    /// 分发缝只应答 gamer.yaml 清单内动作；非清单动作返回 None。
    #[test]
    fn dispatch_gates_by_catalog() {
        let data_dir = std::env::temp_dir();
        assert!(native_call_action(
            "gamer.video",
            TEMPLATE_CREATE_FROM_FRAME,
            &json!({}),
            &data_dir
        )
        .is_none());
        assert!(native_call_action(
            YAML_EXTENSION_ID,
            "vision.test_template",
            &json!({}),
            &data_dir
        )
        .is_none());
        assert!(
            native_call_action(YAML_EXTENSION_ID, "unknown.action", &json!({}), &data_dir)
                .is_none()
        );
        // 清单内动作 → Some（入参缺失给结构化 CallRejected，不 panic）
        let error = native_call_action(
            YAML_EXTENSION_ID,
            AUTOMATION_SAVE_DRAFT,
            &json!({}),
            &data_dir,
        )
        .expect("save_draft 必须由本缝应答")
        .unwrap_err();
        assert!(error.to_string().contains("入参无效"), "{error}");
    }

    fn temp_store(tag: &str) -> (PackageStore, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().unwrap();
        let store = PackageStore::open(&crate::config::Config {
            data_dir: dir.path().to_path_buf(),
            ..Default::default()
        })
        .unwrap();
        store
            .create_package(crate::resources::PackageInput {
                id: tag.into(),
                ..Default::default()
            })
            .unwrap();
        (store, dir)
    }

    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba([200, 30, 40, 255]));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(image)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }

    fn create_values(package: &str, png: &[u8], region: [f64; 4]) -> Value {
        json!({
            "package_id": package,
            "name": "开始按钮",
            "png_base64": base64_encode(png),
            "region": region,
            "frame": { "media_id": "m-1", "frame_index": 12, "pts_us": 400_000 },
            "calibration": { "version": 1, "reference_size": [1280, 720], "rotation": 0 },
        })
    }

    /// 快乐路径：彩色 PNG 落盘为 8-bit 灰度、文件名按区域规则组装、响应携带
    /// 帧身份与校准元数据。
    #[test]
    fn create_template_from_frame_normalizes_and_records_provenance() {
        let (store, dir) = temp_store("tpl-ok");
        drop(store);
        let values = create_values("tpl-ok", &png_bytes(8, 6), [0.1, 0.2, 0.3, 0.4]);
        let result = native_call_action(
            YAML_EXTENSION_ID,
            TEMPLATE_CREATE_FROM_FRAME,
            &values,
            dir.path(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(result["name"], "开始按钮#100_200_300_400.png");
        assert_eq!(result["short_name"], "开始按钮.png");
        assert_eq!(result["path"], "templates/开始按钮#100_200_300_400.png");
        assert_eq!(result["frame"]["media_id"], "m-1");
        assert_eq!(result["frame"]["frame_index"], 12);
        assert_eq!(result["calibration"]["version"], 1);
        let on_disk = std::fs::read(
            dir.path()
                .join("packages/tpl-ok/plugins/gamer.yaml/templates/开始按钮#100_200_300_400.png"),
        )
        .unwrap();
        assert_eq!(
            image::load_from_memory(&on_disk).unwrap().color(),
            image::ColorType::L8,
            "落盘必须已经灰度归一化"
        );
    }

    /// 短名冲突：overwrite=false 结构化拒绝；overwrite=true 覆盖成功。
    #[test]
    fn create_template_name_conflict_requires_overwrite() {
        let (_store, dir) = temp_store("tpl-conflict");
        let values = create_values("tpl-conflict", &png_bytes(8, 6), [0.0, 0.0, 0.5, 0.5]);
        native_call_action(
            YAML_EXTENSION_ID,
            TEMPLATE_CREATE_FROM_FRAME,
            &values,
            dir.path(),
        )
        .unwrap()
        .unwrap();
        // 同短名不同区域 → 冲突
        let mut other = values.clone();
        other["region"] = json!([0.2, 0.2, 0.8, 0.8]);
        let error = native_call_action(
            YAML_EXTENSION_ID,
            TEMPLATE_CREATE_FROM_FRAME,
            &other,
            dir.path(),
        )
        .unwrap()
        .unwrap_err();
        assert!(error.to_string().contains("短名冲突"), "{error}");
        // overwrite → 成功（旧文件被替换）
        other["overwrite"] = json!(true);
        let result = native_call_action(
            YAML_EXTENSION_ID,
            TEMPLATE_CREATE_FROM_FRAME,
            &other,
            dir.path(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(result["name"], "开始按钮#200_200_800_800.png");
        let conflict_dir = dir
            .path()
            .join("packages/tpl-conflict/plugins/gamer.yaml/templates");
        assert!(
            conflict_dir.join("开始按钮#000_000_500_500.png").exists(),
            "不同区域名是独立文件"
        );
        assert!(conflict_dir.join("开始按钮#200_200_800_800.png").exists());
    }

    /// 非法入参：坏区域 / 非 PNG 字节 / 非法路径段 → 结构化 CallRejected。
    #[test]
    fn create_template_rejects_bad_region_png_and_path() {
        let (_store, dir) = temp_store("tpl-bad");
        let mut values = create_values("tpl-bad", &png_bytes(8, 6), [0.5, 0.2, 0.3, 0.4]);
        let error = native_call_action(
            YAML_EXTENSION_ID,
            TEMPLATE_CREATE_FROM_FRAME,
            &values,
            dir.path(),
        )
        .unwrap()
        .unwrap_err();
        assert!(error.to_string().contains("region"), "{error}");

        values["region"] = json!([0.0, 0.0, 0.5, 0.5]);
        values["png_base64"] = json!("not-a-png!!!");
        let error = native_call_action(
            YAML_EXTENSION_ID,
            TEMPLATE_CREATE_FROM_FRAME,
            &values.clone(),
            dir.path(),
        )
        .unwrap()
        .unwrap_err();
        assert!(error.to_string().contains("base64"), "{error}");

        values["png_base64"] = json!(base64_encode(b"definitely not an image"));
        let error = native_call_action(
            YAML_EXTENSION_ID,
            TEMPLATE_CREATE_FROM_FRAME,
            &values,
            dir.path(),
        )
        .unwrap()
        .unwrap_err();
        assert!(error.to_string().contains("模板图片未通过校验"), "{error}");

        let mut evil = create_values("tpl-bad", &png_bytes(8, 6), [0.0, 0.0, 0.5, 0.5]);
        evil["name"] = json!("../escape");
        let error = native_call_action(
            YAML_EXTENSION_ID,
            TEMPLATE_CREATE_FROM_FRAME,
            &evil,
            dir.path(),
        )
        .unwrap()
        .unwrap_err();
        assert!(error.to_string().contains("非法路径段"), "{error}");

        let mut zero_cal = create_values("tpl-bad", &png_bytes(8, 6), [0.0, 0.0, 0.5, 0.5]);
        zero_cal["calibration"] = json!({ "version": 0 });
        let error = native_call_action(
            YAML_EXTENSION_ID,
            TEMPLATE_CREATE_FROM_FRAME,
            &zero_cal,
            dir.path(),
        )
        .unwrap()
        .unwrap_err();
        assert!(error.to_string().contains("calibration.version"), "{error}");
    }

    /// 命名规则与前端 composeTemplateName 同口径（×1000 / 3 位补零 / #1 颜色）。
    #[test]
    fn template_name_composition_matches_frontend_rule() {
        assert_eq!(
            compose_template_name("btn.png", &[0.1, 0.22, 0.333, 0.444], false),
            "btn#100_220_333_444.png"
        );
        assert_eq!(
            compose_template_name("彩色", &[0.0, 0.0, 1.0, 1.0], true),
            "彩色#000_000_999_999#1.png"
        );
        // ×1000 上限 999（1.0 → "999"，与前端 Math.min(999, …) 同口径）
        assert_eq!(
            compose_template_name("x", &[0.5, 0.5, 0.5005, 1.0], false),
            "x#500_500_500_999.png"
        );
    }

    /// save_draft：v3 直存、非 v3 拒绝、重名需 overwrite、覆盖成功、id 组装。
    #[test]
    fn save_draft_validates_v3_and_enforces_name_conflict() {
        let (_store, dir) = temp_store("draft-ok");
        let v3 = "version: 3\nsteps:\n  - log: 草稿\n";
        let values = json!({ "package_id": "draft-ok", "name": "daily", "yaml": v3 });
        let result = native_call_action(
            YAML_EXTENSION_ID,
            AUTOMATION_SAVE_DRAFT,
            &values,
            dir.path(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            result["id"], "draft-ok/daily.yaml",
            "脚本资源 id 含扩展名（与 listScripts/getScript 同形）"
        );
        assert_eq!(result["path"], "automations/daily.yaml");
        let on_disk = dir
            .path()
            .join("packages/draft-ok/plugins/gamer.yaml/automations/daily.yaml");
        assert_eq!(std::fs::read_to_string(&on_disk).unwrap(), v3);

        // 重名：无 overwrite 拒绝；带 overwrite 覆盖
        let error = native_call_action(
            YAML_EXTENSION_ID,
            AUTOMATION_SAVE_DRAFT,
            &values,
            dir.path(),
        )
        .unwrap()
        .unwrap_err();
        assert!(error.to_string().contains("已存在"), "{error}");
        let mut overwrite = values.clone();
        overwrite["overwrite"] = json!(true);
        overwrite["yaml"] = json!("version: 3\nsteps:\n  - log: v2\n");
        native_call_action(
            YAML_EXTENSION_ID,
            AUTOMATION_SAVE_DRAFT,
            &overwrite,
            dir.path(),
        )
        .unwrap()
        .unwrap();
        assert!(std::fs::read_to_string(&on_disk).unwrap().contains("v2"));

        // 非 v3 → v3 版本门禁诊断
        let mut legacy = values.clone();
        legacy["name"] = json!("legacy");
        legacy["yaml"] = json!("steps: []\n");
        let error = native_call_action(
            YAML_EXTENSION_ID,
            AUTOMATION_SAVE_DRAFT,
            &legacy,
            dir.path(),
        )
        .unwrap()
        .unwrap_err();
        assert!(error.to_string().contains("yaml.v3.version"), "{error}");

        // 非法路径段
        let mut evil = values.clone();
        evil["name"] = json!("a/../b");
        let error = native_call_action(YAML_EXTENSION_ID, AUTOMATION_SAVE_DRAFT, &evil, dir.path())
            .unwrap()
            .unwrap_err();
        assert!(error.to_string().contains("非法路径段"), "{error}");
        // 缺 Package Context
        let error = native_call_action(
            YAML_EXTENSION_ID,
            AUTOMATION_SAVE_DRAFT,
            &json!({ "name": "x", "yaml": v3 }),
            dir.path(),
        )
        .unwrap()
        .unwrap_err();
        assert!(error.to_string().contains("package_id"), "{error}");
    }

    /// base64 往返（无外部依赖实现的最小自证）。
    #[test]
    fn base64_round_trip() {
        for len in 0..40usize {
            let bytes: Vec<u8> = (0..len as u8).map(|i| i.wrapping_mul(37)).collect();
            assert_eq!(
                base64_decode(&base64_encode(&bytes)).unwrap(),
                bytes,
                "len={len}"
            );
        }
        // 标准 alphabet 与 padding
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_decode("Zm9vYg==").unwrap(), b"foob");
        assert!(base64_decode("!!!!").is_none());
    }

    /// 事件注释注入测试夹具共享：保证 BTreeMap 序稳定（按 event id 排序注入）。
    #[test]
    fn comments_map_is_ordered() {
        let mut comments: BTreeMap<String, String> = BTreeMap::new();
        comments.insert("b".into(), "后".into());
        comments.insert("a".into(), "前".into());
        let keys: Vec<_> = comments.keys().cloned().collect();
        assert_eq!(keys, ["a", "b"]);
    }
}
