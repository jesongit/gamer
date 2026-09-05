//! 定时任务参数快照与签名门禁（plan §12.3 / 契约 §4.3–4.5 / P12.3 参数桥）。
//!
//! 任务保存的是**完整类型化 args 快照**（七类 TypedValue 的 JSON 形态，与 run
//! API args 同构）+ 保存时脚本的 psig1 参数签名。调度/立即运行前过本模块的门禁：
//!
//! - 脚本缺失 / 非 `version: 3` / 解析失败 → 明确失败（同口径，不空跑）；
//! - 签名不一致（脚本参数声明/默认值变化）→ 参数过期，明确失败，等待重新确认；
//! - 门禁通过 → 快照整体作为 StartRequest.args 传入（快照是全量，天然不静默
//!   继承新默认值）。
//!
//! 参数唯一来源是 v3 `Program.params`（YAML v2 已删除，无回落）。签名
//! [`v3_param_signature`] 产出 psig1 wire 形态（等价声明逐字节稳定，存量任务
//! 行的 stored 签名继续可比对）。
//!
//! 日志约束：运行链路只记录参数签名与参数名列表，**绝不记录参数值**（text
//! 参数防泄露）；日志侧展示签名用 [`signature_short_code`] 短码。

use crate::extensions::gamer_yaml::error::{codes, diagnostics_from_vnext, ScriptError};
use crate::extensions::gamer_yaml::params::{
    coord_in_range, fmt_num, is_valid_color, is_valid_key, parse_time_ms, unescape_double_quoted,
};
use crate::extensions::gamer_yaml::run_target::{BoundEntryArgs, RunTarget, TypedValue};
use crate::resources::ResourceKind as RK;
use crate::resources::ResourceStore;

/// 签名门禁失败的机器可读原因（依赖缺失/参数过期的细分信号）。
pub const REASON_SIGNATURE_MISMATCH: &str = "signature_mismatch";

/// 任务参数门禁结果：签名 + 从已存快照重建的全量类型化覆盖。
pub struct TaskArgs {
    pub signature: String,
    /// 参数名列表（按脚本声明顺序；日志用，不含值）。
    pub names: Vec<String>,
    /// 全量覆盖（每个声明参数都有值）→ StartRequest.args。
    pub overrides: Vec<(String, TypedValue)>,
}

/// 门禁失败：调度与「立即运行」共用同一口径（明确失败，绝不空跑）。
#[derive(Debug, Clone)]
pub enum GateError {
    /// 脚本不存在（调度路径与既有"脚本不存在"失败语义一致）。
    ScriptMissing,
    /// 脚本读取/解析失败（携带结构化诊断，与保存期 400 同源）。
    ScriptInvalid(Vec<ScriptError>),
    /// 签名不一致：stored = 保存时签名，current = 脚本当前声明签名。
    SignatureMismatch { stored: String, current: String },
}

impl GateError {
    /// 409 body 的 `reason`（仅签名过期走 409）。
    pub fn reason(&self) -> &'static str {
        match self {
            GateError::SignatureMismatch { .. } => REASON_SIGNATURE_MISMATCH,
            GateError::ScriptMissing | GateError::ScriptInvalid(_) => "",
        }
    }

    /// 人类可读中文消息（409 body / 任务结果 / 摘要日志共用；不含参数值）。
    pub fn message(&self) -> String {
        match self {
            GateError::ScriptMissing => "脚本不存在".to_string(),
            GateError::ScriptInvalid(errors) => format!(
                "脚本解析失败（{} 项）：{}",
                errors.len(),
                errors
                    .iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("；")
            ),
            GateError::SignatureMismatch { .. } => {
                "脚本参数声明已变化，任务参数过期，请重新确认任务参数".to_string()
            }
        }
    }
}

/// 读取失败 → 结构化诊断（统一口径）。
fn read_failed(error: anyhow::Error, resource: &str) -> Vec<ScriptError> {
    vec![ScriptError::new(
        codes::YAML_SYNTAX_ERROR,
        format!("读取脚本失败: {error:#}"),
        resource,
    )]
}

/// 完整任务门禁：载入脚本当前签名 →（旧数据带签名时）与存储签名比对 → 从
/// payload 的 args 重建全量类型化覆盖。返回 [`TaskArgs`] 供 StartRequest 使用。
///
/// P11.1（ADR-12）：`stored_signature` 为 None（新 Task 模型保存的 payload
/// 不携带 psig1 快照签名）时跳过过期门禁，按当前声明重绑参数。
pub fn gate_task(
    scripts: &ResourceStore,
    script_id: &str,
    args: &serde_json::Value,
    stored_signature: Option<&str>,
) -> Result<TaskArgs, GateError> {
    match scripts.get_text(RK::Scripts, script_id) {
        Ok(Some(_)) => {}
        Ok(None) => return Err(GateError::ScriptMissing),
        Err(error) => {
            return Err(GateError::ScriptInvalid(read_failed(error, script_id)))
        }
    }
    let decls = probe_v3_script_decls(scripts, script_id).map_err(GateError::ScriptInvalid)?;
    let current = v3_param_signature(&decls);
    if let Some(stored) = stored_signature {
        if stored != current {
            return Err(GateError::SignatureMismatch {
                stored: stored.to_string(),
                current,
            });
        }
    }
    let overrides = rebind_v3_snapshot(&decls, args, script_id).map_err(GateError::ScriptInvalid)?;
    Ok(TaskArgs {
        signature: current,
        names: decls.iter().map(|d| d.name.clone()).collect(),
        overrides,
    })
}

/// 签名短码（日志展示用）：FNV-1a 64 高 32 位的 8 位十六进制。签名串本身
/// 含默认值，日志允许记录签名，但短码足够比对且更省行。
pub fn signature_short_code(signature: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in signature.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:08x}", (hash >> 32) as u32)
}

// ===========================================================================
// v3 参数桥（P12.3 / 契约 §7）：version:3 源的参数声明、psig1 签名与绑定
// ===========================================================================
//
// 脚本顶层经 `yaml_vnext::load` 取 `Program.params`；函数库 bare-map 无
// version 键（v3-ness 由步语法承载），经 `yaml_vnext::parse_function_library`
// 严格解析后取目标函数的 params。声明统一规范化为 [`V3ParamDecl`]：
// - 签名：[`v3_param_signature`] 产出 `psig1|ty,name,required,canon` wire
//   形态（等价声明逐字节一致，存量任务 stored 签名继续可比对）；
// - 绑定：显式实参按声明类型收纳为 v3 执行链的七类 [`TypedValue`]
//   （数值类暂以文本形态过线，见 `coerce_v3_arg`）。

/// v3 参数声明（脚本 `Program.params` 与函数库解析共用的规范化视图）。
#[derive(Debug, Clone, PartialEq)]
pub struct V3ParamDecl {
    pub name: String,
    /// 类型名：七类（tmpl/coord/color/time/key/text/bool）或 v3 扩展名
    /// （string/template/boolean/int/integer/number/value）。
    pub ty: String,
    /// 备注描述（透出到参数 schema description）。
    pub remark: String,
    /// JSON 形态默认值；None = 必填。字符串形态声明的默认值保持原串
    /// （与 `yaml_vnext::parse_params` 一致），消费侧按类型规整。
    pub default: Option<serde_json::Value>,
}

/// v3 声明可识别的类型全集（七类 + v3 别名/扩展）。
const V3_KNOWN_TYPES: &[&str] = &[
    "tmpl", "template", "coord", "color", "time", "key", "text", "string", "bool", "boolean",
    "int", "integer", "number", "value",
];

pub(crate) fn is_known_v3_type(ty: &str) -> bool {
    V3_KNOWN_TYPES.contains(&ty.trim())
}

/// `yaml_vnext::Program.params` → 规范化 v3 声明。
pub(crate) fn v3_decls_from_program(
    program: &crate::extensions::gamer_yaml::yaml_vnext::Program,
) -> Vec<V3ParamDecl> {
    program
        .params
        .iter()
        .map(|decl| V3ParamDecl {
            name: decl.name.clone(),
            ty: decl.ty.trim().to_string(),
            remark: decl.remark.clone().unwrap_or_default(),
            default: decl.default.as_ref().map(v3_value_to_plain_json),
        })
        .collect()
}

/// `yaml_vnext::Value` → 无标签 JSON（serde 序列化是 `{"type","value"}` wire
/// 形态，签名/绑定/表单默认值都消费普通 JSON）。
fn v3_value_to_plain_json(value: &crate::extensions::gamer_yaml::yaml_vnext::Value) -> serde_json::Value {
    use crate::extensions::gamer_yaml::yaml_vnext::Value as V;
    match value {
        V::Null => serde_json::Value::Null,
        V::Bool(b) => serde_json::Value::Bool(*b),
        V::Int(i) => serde_json::json!(i),
        V::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        V::String(s) | V::Color(s) => serde_json::Value::String(s.clone()),
        V::Duration(ms) => serde_json::json!(ms),
        V::Coordinate([x, y]) => serde_json::json!([x, y]),
        V::List(items) => {
            serde_json::Value::Array(items.iter().map(v3_value_to_plain_json).collect())
        }
        V::Map(entries) => serde_json::Value::Object(
            entries
                .iter()
                .map(|(key, value)| (key.clone(), v3_value_to_plain_json(value)))
                .collect(),
        ),
        V::Handle { kind, id } => serde_json::json!({ "kind": kind, "id": id }),
    }
}

/// 读取脚本并解析 v3 参数声明。脚本缺失 → `resource.script.not_found`；
/// 非 `version: 3` 源 → 版本门禁诊断（`yaml.v3.version`）；v3 解析失败 →
/// 保留 `yaml.v3.*` 码的结构化诊断。
pub(crate) fn probe_v3_script_decls(
    scripts: &ResourceStore,
    script_id: &str,
) -> Result<Vec<V3ParamDecl>, Vec<ScriptError>> {
    let entry = match scripts.get_text(RK::Scripts, script_id) {
        Ok(Some(entry)) => entry,
        Ok(None) => {
            return Err(vec![ScriptError::new(
                codes::RESOURCE_SCRIPT_NOT_FOUND,
                format!("脚本 {script_id:?} 不存在"),
                script_id,
            )])
        }
        Err(error) => return Err(read_failed(error, script_id)),
    };
    if !crate::extensions::gamer_yaml::yaml_vnext::is_v3_source(&entry.content) {
        return Err(vec![ScriptError::new(
            codes::VERSION_UNSUPPORTED,
            "不支持的 YAML 版本——当前只支持 version: 3 脚本（YAML v2 已移除）",
            script_id,
        )]);
    }
    let program = crate::extensions::gamer_yaml::yaml_vnext::load(&entry.content)
        .map_err(|diagnostics| diagnostics_from_vnext(&diagnostics, script_id))?;
    Ok(v3_decls_from_program(&program))
}

/// 解析 v3 函数库并取目标函数的参数声明。`function` 为 None 时取文件内
/// 第一个函数（与入口/描述器语义一致）。文件缺失 → `resource.func.not_found`；
/// 目标函数不存在 → 同码；库解析失败 → `yaml.v3.*` 结构化诊断。
pub(crate) fn probe_v3_function_decls(
    scripts: &ResourceStore,
    pkg: &str,
    file: &str,
    function: Option<&str>,
) -> Result<Vec<V3ParamDecl>, Vec<ScriptError>> {
    use crate::extensions::gamer_yaml::yaml_vnext;
    let rel = format!("{pkg}/{file}.yaml");
    let entry = match scripts.get_text(RK::Functions, &rel) {
        Ok(Some(entry)) => entry,
        Ok(None) => {
            return Err(vec![ScriptError::new(
                codes::RESOURCE_FUNC_NOT_FOUND,
                format!("函数文件 {rel} 不存在"),
                file.to_string(),
            )])
        }
        Err(error) => return Err(read_failed(error, &rel)),
    };
    let library = yaml_vnext::parse_function_library(&entry.content)
        .map_err(|diagnostics| diagnostics_from_vnext(&diagnostics, file))?;
    let declaration = match function {
        Some(name) => library
            .iter()
            .find(|decl| decl.name == name.trim())
            .ok_or_else(|| {
                vec![ScriptError::new(
                    codes::RESOURCE_FUNC_NOT_FOUND,
                    format!("函数 {file}/{name} 不存在（函数文件中无该函数名）"),
                    file.to_string(),
                )]
            })?,
        // 缺省 = 文件内第一个函数
        None => library.first().ok_or_else(|| {
            vec![ScriptError::new(
                codes::RESOURCE_FUNC_NOT_FOUND,
                format!("函数文件 {file} 未定义任何函数"),
                file.to_string(),
            )]
        })?,
    };
    Ok(declaration
        .params
        .iter()
        .map(|decl| V3ParamDecl {
            name: decl.name.clone(),
            ty: decl.ty.trim().to_string(),
            remark: decl.remark.clone().unwrap_or_default(),
            default: decl.default.as_ref().map(v3_value_to_plain_json),
        })
        .collect())
}

/// v3 声明 → psig1 签名（`psig1|ty,name,required,canon|…`）。等价声明
/// 逐字节一致——存量 Task 行的 stored 签名对迁移后的脚本继续可比对；
/// v3 扩展类型名原样入签。
pub fn v3_param_signature(decls: &[V3ParamDecl]) -> String {
    let entries: Vec<String> = decls.iter().map(canonical_v3_entry).collect();
    format!("psig1|{}", entries.join("|"))
}

fn canonical_v3_entry(decl: &V3ParamDecl) -> String {
    let (required, canon) = match &decl.default {
        None => ("1", String::new()),
        Some(value) => ("0", canonical_v3_default(&decl.ty, value)),
    };
    format!("{},{},{},{}", decl.ty.trim(), decl.name, required, canon)
}

/// v3 默认值的规范串（time 小写且 min→m、color 小写、key 大写、text 剥引号
/// 并转义分隔符、coord 去空白、数值最短形）。
fn canonical_v3_default(ty: &str, value: &serde_json::Value) -> String {
    let ty = ty.trim();
    match value {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => fmt_num(n.as_f64().unwrap_or_default()),
        serde_json::Value::String(raw) => canonical_v3_string_default(ty, raw),
        serde_json::Value::Array(items) if ty == "coord" && items.len() == 2 => format!(
            "[{},{}]",
            fmt_num(items[0].as_f64().unwrap_or_default()),
            fmt_num(items[1].as_f64().unwrap_or_default())
        ),
        other => other.to_string(),
    }
}

fn canonical_v3_string_default(ty: &str, raw: &str) -> String {
    match ty {
        "bool" | "boolean" => raw.to_ascii_lowercase(),
        "color" => raw.to_ascii_lowercase(),
        "key" => raw.to_ascii_uppercase(),
        "time" => {
            let lower = raw.to_ascii_lowercase();
            match lower.strip_suffix("min") {
                Some(num) => format!("{num}m"),
                None => lower,
            }
        }
        "coord" => parse_coord_string(raw)
            .map(|[x, y]| format!("[{},{}]", fmt_num(x), fmt_num(y)))
            .unwrap_or_else(|| raw.to_string()),
        // 字符串形态声明的 text 默认值带双引号包裹时先剥离反转义，再按规则
        // 转义签名分隔符。
        "text" | "string" => strip_quoted_text(raw)
            .replace('\\', "\\\\")
            .replace(',', "\\,")
            .replace('|', "\\|"),
        "int" | "integer" | "number" => raw
            .trim()
            .parse::<f64>()
            .map(fmt_num)
            .unwrap_or_else(|_| raw.to_string()),
        _ => raw.to_string(),
    }
}

/// 字符串形态 text 默认值的外层双引号剥离与反转义（非包裹形态原样）。
fn strip_quoted_text(raw: &str) -> String {
    if raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"') {
        unescape_double_quoted(&raw[1..raw.len() - 1]).unwrap_or_else(|| raw.to_string())
    } else {
        raw.to_string()
    }
}

/// 字符串形态 coord 默认值 `[x, y]` → 分量（失败 None）。
fn parse_coord_string(raw: &str) -> Option<[f64; 2]> {
    let inner = raw.trim().strip_prefix('[')?.strip_suffix(']')?;
    let mut nums = Vec::new();
    for part in inner.split(',') {
        nums.push(part.trim().parse::<f64>().ok()?);
    }
    if nums.len() != 2 {
        return None;
    }
    Some([nums[0], nums[1]])
}

/// v3 参数默认值 → JSON 原生形态（表单预填/签名/展示消费）：字符串形态声明
/// 的默认值按类型规整（剥引号、数值化、coord 数组化），其余原样。
pub(crate) fn normalize_v3_default_json(ty: &str, value: &serde_json::Value) -> serde_json::Value {
    let ty = ty.trim();
    match value {
        serde_json::Value::String(raw) => match ty {
            "text" | "string" => serde_json::Value::String(strip_quoted_text(raw)),
            "bool" | "boolean" => match raw.trim() {
                "true" => serde_json::Value::Bool(true),
                "false" => serde_json::Value::Bool(false),
                _ => value.clone(),
            },
            "int" | "integer" => raw
                .trim()
                .parse::<i64>()
                .map(serde_json::Value::from)
                .unwrap_or_else(|_| value.clone()),
            "number" => raw
                .trim()
                .parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .map(serde_json::Value::Number)
                .unwrap_or_else(|| value.clone()),
            "coord" => parse_coord_string(raw)
                .map(|[x, y]| {
                    serde_json::Value::Array(vec![
                        serde_json::json!(x),
                        serde_json::json!(y),
                    ])
                })
                .unwrap_or_else(|| value.clone()),
            _ => value.clone(),
        },
        _ => value.clone(),
    }
}

/// v3 显式实参（JSON）→ 七类 TypedValue（执行链 wire）。
///
/// 已知限制：TypedValue wire 无数值变体——int/number 显式实参暂以文本形态
/// 过线（默认值不受影响：v3 运行只传稀疏覆盖，缺省参数由 guest 按声明默认
/// 值取类型化值）。wire 补数值变体前，数值实参在 `$x` 用作 loop.times 等
/// 强类型位置会退化为字符串。
pub(crate) fn coerce_v3_arg(ty: &str, value: &serde_json::Value) -> Option<TypedValue> {
    match ty.trim() {
        "text" | "string" => value.as_str().map(|s| TypedValue::Text(s.to_string())),
        "bool" | "boolean" => match value {
            serde_json::Value::Bool(b) => Some(TypedValue::Bool(*b)),
            serde_json::Value::String(s) => match s.as_str() {
                "true" => Some(TypedValue::Bool(true)),
                "false" => Some(TypedValue::Bool(false)),
                _ => None,
            },
            _ => None,
        },
        "time" => value
            .as_str()
            .filter(|s| parse_time_ms(s).is_some())
            .map(|s| TypedValue::Time(s.to_string())),
        "tmpl" | "template" => value
            .as_str()
            .filter(|s| !s.is_empty())
            .map(|s| TypedValue::Tmpl(s.to_string())),
        "color" => value
            .as_str()
            .filter(|s| is_valid_color(s))
            .map(|s| TypedValue::Color(s.to_string())),
        "coord" => {
            let items = value.as_array()?;
            if items.len() != 2 {
                return None;
            }
            let x = items[0].as_f64()?;
            let y = items[1].as_f64()?;
            (coord_in_range(x) && coord_in_range(y)).then_some(TypedValue::Coord([x, y]))
        }
        "key" => value
            .as_str()
            .filter(|s| is_valid_key(s))
            .map(|s| TypedValue::Key(s.to_string())),
        "int" | "integer" => {
            let n = match value {
                serde_json::Value::Number(n) => n.as_i64()?,
                serde_json::Value::String(s) => s.trim().parse::<i64>().ok()?,
                _ => return None,
            };
            Some(TypedValue::Text(n.to_string()))
        }
        "number" => {
            let n = match value {
                serde_json::Value::Number(n) => n.as_f64()?,
                serde_json::Value::String(s) => s.trim().parse::<f64>().ok()?,
                _ => return None,
            };
            Some(TypedValue::Text(fmt_num(n)))
        }
        "value" => match value {
            serde_json::Value::Bool(b) => Some(TypedValue::Bool(*b)),
            serde_json::Value::Number(n) => Some(TypedValue::Text(fmt_num(n.as_f64()?))),
            serde_json::Value::String(s) => Some(TypedValue::Text(s.clone())),
            serde_json::Value::Array(items)
                if items.len() == 2
                    && items.iter().all(|item| item.as_f64().is_some())
                    && items
                        .iter()
                        .all(|item| coord_in_range(item.as_f64().unwrap_or_default())) =>
            {
                Some(TypedValue::Coord([
                    items[0].as_f64().unwrap_or_default(),
                    items[1].as_f64().unwrap_or_default(),
                ]))
            }
            _ => None,
        },
        _ => None,
    }
}

/// 手动运行（POST /api/runs）入口参数解析（v3-only）。
///
/// 绑定即前置校验：未知键 / 类型不符 / 缺必填 → 结构化诊断（400 invalid_args）。
pub fn resolve_manual_entry_args(
    scripts: &ResourceStore,
    target: &RunTarget,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<BoundEntryArgs, Vec<ScriptError>> {
    match target {
        RunTarget::Script { script_id, .. } => {
            let decls = probe_v3_script_decls(scripts, script_id)?;
            bind_v3_manual_args(&decls, args, script_id)
        }
        RunTarget::Function {
            pkg,
            file,
            function,
            ..
        } => {
            let decls = probe_v3_function_decls(scripts, pkg, file, function.as_deref())?;
            let label = match function {
                Some(name) => format!("{file}/{name}"),
                None => file.clone(),
            };
            bind_v3_manual_args(&decls, args, &label)
        }
    }
}

/// v3 手动运行绑定（严格）：未知键 / 类型不符 / 缺必填全部报错；覆盖只含
/// 显式实参（缺省参数由 guest 按声明默认值补类型化值）；`resolved` 为
/// 「声明默认 → 显式覆盖」合并的全量 JSON 视图（响应 resolved_args）。
pub(crate) fn bind_v3_manual_args(
    decls: &[V3ParamDecl],
    args: &serde_json::Map<String, serde_json::Value>,
    resource: &str,
) -> Result<BoundEntryArgs, Vec<ScriptError>> {
    let mut errors = Vec::new();
    let mut overrides = Vec::new();
    let mut resolved = serde_json::Map::new();
    for (name, _) in args {
        if !decls.iter().any(|d| &d.name == name) {
            errors.push(
                ScriptError::new(
                    codes::PARAM_ARGS_UNKNOWN,
                    format!("args 键 {name:?} 不是目标参数"),
                    resource,
                )
                .at(format!("args.{name}"), "args"),
            );
        }
    }
    for decl in decls {
        if !is_known_v3_type(&decl.ty) {
            errors.push(
                ScriptError::new(
                    codes::PARAM_DECL_FORMAT,
                    format!(
                        "参数 {} 声明了未知类型 {:?}（可用：tmpl/coord/color/time/key/text/bool/string/int/number/value）",
                        decl.name, decl.ty
                    ),
                    resource,
                )
                .at(format!("params.{}", decl.name), "params"),
            );
            continue;
        }
        match args.get(&decl.name) {
            Some(value) => match coerce_v3_arg(&decl.ty, value) {
                Some(v) => overrides.push((decl.name.clone(), v)),
                None => errors.push(
                    ScriptError::new(
                        codes::PARAM_ARGS_TYPE_MISMATCH,
                        format!(
                            "参数 {} 的实参 {value} 与声明类型 {} 不符",
                            decl.name, decl.ty
                        ),
                        resource,
                    )
                    .at(format!("args.{}", decl.name), "args"),
                ),
            },
            None if decl.default.is_none() => {
                errors.push(
                    ScriptError::new(
                        codes::PARAM_ARGS_MISSING_REQUIRED,
                        format!("必填参数 {} 未提供", decl.name),
                        resource,
                    )
                    .at("args", "args"),
                );
            }
            None => {}
        }
        if let Some(value) = args.get(&decl.name) {
            resolved.insert(decl.name.clone(), value.clone());
        } else if let Some(default) = &decl.default {
            resolved.insert(
                decl.name.clone(),
                normalize_v3_default_json(&decl.ty, default),
            );
        }
    }
    if errors.is_empty() {
        Ok(BoundEntryArgs {
            overrides,
            resolved: serde_json::Value::Object(resolved),
        })
    } else {
        Err(errors)
    }
}

/// v3 任务快照重绑（宽松）：存活参数保留原值、新增参数取声明默认值、已删
/// 参数静默丢弃；缺必填/类型不符报结构化诊断。
pub(crate) fn rebind_v3_snapshot(
    decls: &[V3ParamDecl],
    args: &serde_json::Value,
    resource: &str,
) -> Result<Vec<(String, TypedValue)>, Vec<ScriptError>> {
    let stored: serde_json::Map<String, serde_json::Value> = match args.as_object() {
        Some(map) => map.clone(),
        None => {
            return Err(vec![ScriptError::new(
                codes::PARAM_ARGS_TYPE_MISMATCH,
                "任务参数快照必须是 JSON 对象",
                resource,
            )
            .at("args", "args")])
        }
    };
    let mut overrides = Vec::new();
    let mut errors = Vec::new();
    for decl in decls {
        if let Some(value) = stored.get(&decl.name) {
            match coerce_v3_arg(&decl.ty, value) {
                Some(v) => overrides.push((decl.name.clone(), v)),
                None => errors.push(
                    ScriptError::new(
                        codes::PARAM_ARGS_TYPE_MISMATCH,
                        format!("任务参数快照中的参数 {} 类型无效", decl.name),
                        resource,
                    )
                    .at(format!("args.{}", decl.name), "args"),
                ),
            }
        } else if let Some(default) = &decl.default {
            let normalized = normalize_v3_default_json(&decl.ty, default);
            match coerce_v3_arg(&decl.ty, &normalized) {
                Some(v) => overrides.push((decl.name.clone(), v)),
                None => errors.push(
                    ScriptError::new(
                        codes::PARAM_DEFAULT_INVALID,
                        format!(
                            "参数 {} 的默认值 {default} 无法按类型 {} 解析",
                            decl.name, decl.ty
                        ),
                        resource,
                    )
                    .at(format!("params.{}", decl.name), "params"),
                ),
            }
        } else {
            errors.push(
                ScriptError::new(
                    codes::PARAM_ARGS_MISSING_REQUIRED,
                    format!("必填参数 {} 未提供", decl.name),
                    resource,
                )
                .at("args", "args"),
            );
        }
    }
    if errors.is_empty() {
        Ok(overrides)
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    /// v3 脚本（含默认值参数）供门禁/重绑路径使用。
    const V3_SCRIPT: &str = "\
version: 3
params:
  - 'bool:enable:是否启用:true'
  - 'time:timeout:最长等待:30s'
  - 'text:message:提示文本:\"hello\"'
  - 'coord:pos:位置:[0.5, 0.5]'
steps:
  - log: 'ok'
";

    /// 含必填参数的 v3 脚本供必填缺失路径使用。
    const V3_SCRIPT_REQUIRED: &str = "\
version: 3
params:
  - 'text:secret:密文'
steps:
  - log: $secret
";

    fn script_dir(tag: &str) -> (Config, std::path::PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("gamer-task-params-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = Config {
            data_dir: dir.clone(),
            ..Default::default()
        };
        (cfg, dir)
    }

    fn write_script(cfg: &Config, name: &str, content: &str) {
        let dir = cfg.data_dir.join("com.test.app").join("scripts");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), content).unwrap();
    }

    fn write_function(cfg: &Config, name: &str, content: &str) {
        let dir = cfg.data_dir.join("com.test.app").join("functions");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), content).unwrap();
    }

    fn v3_script_decls_in_memory(source: &str) -> Vec<V3ParamDecl> {
        let program = crate::extensions::gamer_yaml::yaml_vnext::load(source)
            .unwrap_or_else(|e| panic!("fixture parse failed: {e:?}"));
        v3_decls_from_program(&program)
    }

    #[test]
    fn short_code_is_stable_and_distinguishes_signatures() {
        let a = signature_short_code("psig1|bool,enable,0,true");
        let b = signature_short_code("psig1|bool,enable,0,false");
        assert_eq!(
            a,
            signature_short_code("psig1|bool,enable,0,true"),
            "确定性"
        );
        assert_ne!(a, b);
        assert_eq!(a.len(), 8);
    }

    #[test]
    fn rebind_keeps_surviving_values_and_defaults_new_params() {
        let decls = v3_script_decls_in_memory(V3_SCRIPT);
        let snapshot = serde_json::json!({
            "enable": false,
            "timeout": "1m",
            "message": "TOP-SECRET",
            "pos": [0.1, 0.2],
        });
        // 声明不变：全部保留原值
        let bound = rebind_v3_snapshot(&decls, &snapshot, "t").unwrap();
        let map: std::collections::HashMap<String, TypedValue> = bound.into_iter().collect();
        assert_eq!(map["enable"], TypedValue::Bool(false));
        assert_eq!(map["timeout"], TypedValue::Time("1m".into()));
        assert_eq!(map["message"], TypedValue::Text("TOP-SECRET".into()));
        assert_eq!(map["pos"], TypedValue::Coord([0.1, 0.2]));
    }

    #[test]
    fn rebind_fills_default_for_new_param_and_drops_removed() {
        // 新脚本删除了 timeout，新增带默认值的 color
        let decls = v3_script_decls_in_memory(
            "\
version: 3
params:
  - 'bool:enable:是否启用:true'
  - 'text:message:提示文本:\"hello\"'
  - 'coord:pos:位置:[0.5, 0.5]'
  - 'color:target:目标颜色:123456'
steps:
  - log: 'ok'
",
        );
        let old_snapshot = serde_json::json!({
            "enable": false,
            "timeout": "1m",
            "message": "keep",
            "pos": [0.1, 0.2],
            "ghost": "已删参数",
        });
        let bound = rebind_v3_snapshot(&decls, &old_snapshot, "t").unwrap();
        let map: std::collections::HashMap<String, TypedValue> = bound.into_iter().collect();
        assert_eq!(map["enable"], TypedValue::Bool(false), "存活参数保留原值");
        assert_eq!(map["message"], TypedValue::Text("keep".into()));
        assert_eq!(
            map["target"],
            TypedValue::Color("123456".into()),
            "新增参数取当前默认值"
        );
        assert!(!map.contains_key("timeout"), "已删参数不出现");
        assert!(!map.contains_key("ghost"));
    }

    #[test]
    fn rebind_missing_required_reports_structured_diagnostic() {
        let decls = v3_script_decls_in_memory(V3_SCRIPT_REQUIRED);
        let err = rebind_v3_snapshot(&decls, &serde_json::json!({}), "t").unwrap_err();
        assert!(
            err.iter()
                .any(|e| e.code == codes::PARAM_ARGS_MISSING_REQUIRED),
            "必填缺失必须有结构化诊断: {err:?}"
        );
    }

    #[tokio::test]
    async fn gate_task_passes_with_matching_signature_and_rebuilds_overrides() {
        let (cfg, dir) = script_dir("gate-ok");
        write_script(&cfg, "daily.yaml", V3_SCRIPT);
        let scripts = std::sync::Arc::new(ResourceStore::open(&cfg).unwrap());
        let decls = probe_v3_script_decls(&scripts, "com.test.app/daily.yaml").unwrap();
        let signature = v3_param_signature(&decls);
        // 快照 = 完整覆盖（含覆盖值），并带 psig1 签名做过期门禁
        let snapshot = serde_json::json!({
            "enable": false,
            "timeout": "45s",
            "message": "SECRET-VALUE",
            "pos": [0.25, 0.75],
        });
        let gate = gate_task(
            &scripts,
            "com.test.app/daily.yaml",
            &snapshot,
            Some(&signature),
        )
        .unwrap();
        assert_eq!(gate.signature, signature);
        assert_eq!(
            gate.names,
            decls.iter().map(|d| d.name.clone()).collect::<Vec<_>>()
        );
        let map: std::collections::HashMap<String, TypedValue> =
            gate.overrides.into_iter().collect();
        assert_eq!(map.len(), 4, "快照是全量覆盖");
        assert_eq!(map["timeout"], TypedValue::Time("45s".into()));
        drop(scripts);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn gate_task_detects_stale_signature_and_rejects_invalid_snapshots() {
        let (cfg, dir) = script_dir("gate-stale");
        write_script(&cfg, "daily.yaml", V3_SCRIPT);
        let scripts = std::sync::Arc::new(ResourceStore::open(&cfg).unwrap());
        let (_, signature) = {
            let decls = probe_v3_script_decls(&scripts, "com.test.app/daily.yaml").unwrap();
            ((), v3_param_signature(&decls))
        };
        // 签名不一致 → 过期
        let stale = gate_task(
            &scripts,
            "com.test.app/daily.yaml",
            &serde_json::json!({}),
            Some("psig1|old"),
        );
        match stale {
            Err(GateError::SignatureMismatch { stored, current }) => {
                assert_eq!(stored, "psig1|old");
                assert!(current.starts_with("psig1|"));
            }
            other => panic!("expected stale, got {:?}", other.is_ok()),
        }
        // 空快照与非法 JSON 都必须拒绝，不得按默认值兜底。
        for args in ["", "null", "not-json"] {
            let args: serde_json::Value =
                serde_json::from_str(args).unwrap_or(serde_json::Value::Null);
            match gate_task(&scripts, "com.test.app/daily.yaml", &args, Some(&signature)) {
                Err(GateError::ScriptInvalid(diags)) => assert!(diags.iter().any(|diag| {
                    diag.code == codes::PARAM_ARGS_TYPE_MISMATCH
                })),
                other => panic!("expected invalid snapshot, got {:?}", other.is_ok()),
            }
        }
        // 新 Task 模型保存的 payload 无 param_signature → 跳过过期门禁、重绑生效
        let untyped = gate_task(
            &scripts,
            "com.test.app/daily.yaml",
            &serde_json::json!({"timeout": "10s"}),
            None,
        )
        .unwrap();
        let map: std::collections::HashMap<String, TypedValue> =
            untyped.overrides.into_iter().collect();
        assert_eq!(map["timeout"], TypedValue::Time("10s".into()));
        assert_eq!(map["enable"], TypedValue::Bool(true), "新参数取当前默认值");
        drop(scripts);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn gate_task_missing_broken_and_v2_scripts_are_distinct_failures() {
        let (cfg, dir) = script_dir("gate-script");
        write_script(&cfg, "broken.yaml", "version: 3\nparams: []\n");
        // v2 形态存量脚本：明确版本错误（v3-only 门禁，无 fallback）
        write_script(&cfg, "legacy.yaml", "params: []\nsteps: []\n");
        let scripts = std::sync::Arc::new(ResourceStore::open(&cfg).unwrap());
        match probe_v3_script_decls(&scripts, "com.test.app/missing.yaml") {
            Err(diags) => assert!(diags
                .iter()
                .any(|d| d.code == codes::RESOURCE_SCRIPT_NOT_FOUND)),
            other => panic!("expected missing, got {:?}", other.is_ok()),
        }
        match probe_v3_script_decls(&scripts, "com.test.app/broken.yaml") {
            Err(diags) => assert!(diags.iter().any(|d| d.code == "yaml.v3.steps.missing")),
            other => panic!("expected invalid, got {:?}", other.is_ok()),
        }
        match gate_task(&scripts, "com.test.app/legacy.yaml", &serde_json::json!({}), None) {
            Err(GateError::ScriptInvalid(diags)) => assert!(
                diags.iter().any(|d| d.code == codes::VERSION_UNSUPPORTED),
                "v2 形态必须报版本门禁错误: {diags:?}"
            ),
            other => panic!("expected version gate, got {:?}", other.is_ok()),
        }
        drop(scripts);
        std::fs::remove_dir_all(dir).unwrap();
    }

    // ---------------------------------------------------------------------
    // psig1 签名稳定性（存量任务行 wire 兼容）
    // ---------------------------------------------------------------------

    /// 七类声明的规范形态（签名 wire 逐字节锚定）。
    const V3_TWIN: &str = "\
version: 3
params:
  - 'bool:enable:是否启用:true'
  - 'time:timeout:最长等待:30s'
  - 'text:message:提示文本:\"hello\"'
  - 'coord:pos:位置:[0.5, 0.5]'
  - 'color:target:目标颜色:ABCDEF'
  - 'tmpl:tpl:模板:reward'
steps:
  - log: ok
";

    /// v3 扩展类型（int/string 映射形态 + 必填），签名稳定性与过期门禁夹具。
    const V3_MIXED: &str = "\
version: 3
params:
  - 'int:count:次数:3'
  - name: mode
    type: string
    default: auto
  - 'text:secret:密文'
steps:
  - log: $secret
";

    #[test]
    fn v3_signature_is_stable_and_byte_anchored() {
        let decls = v3_script_decls_in_memory(V3_TWIN);
        assert_eq!(
            v3_param_signature(&decls),
            "psig1|bool,enable,0,true|time,timeout,0,30s|text,message,0,hello|coord,pos,0,[0.5,0.5]|color,target,0,abcdef|tmpl,tpl,0,reward",
            "psig1 wire 形态逐字节锚定（存量任务行兼容）"
        );
        let decls = v3_script_decls_in_memory(V3_MIXED);
        let signature = v3_param_signature(&decls);
        assert_eq!(signature, "psig1|int,count,0,3|string,mode,0,auto|text,secret,1,");
        // 类型/默认值变化必须改变签名（过期门禁的判定基础）
        let changed = v3_script_decls_in_memory(
            "version: 3\nparams:\n  - 'int:count:次数:4'\n  - name: mode\n    type: string\n    default: auto\n  - 'text:secret:密文'\nsteps:\n  - log: ok\n",
        );
        assert_ne!(signature, v3_param_signature(&changed));
        // 映射形态的 v3 声明经 roundtrip 保持稳定
        assert_eq!(signature, v3_param_signature(&v3_script_decls_in_memory(V3_MIXED)));
    }

    #[tokio::test]
    async fn gate_task_v3_accepts_matching_signature_and_rebinds_snapshot() {
        let (cfg, dir) = script_dir("gate-v3");
        write_script(&cfg, "v3daily.yaml", V3_MIXED);
        let scripts = std::sync::Arc::new(ResourceStore::open(&cfg).unwrap());
        // 无 stored 签名：按当前声明重绑（新 Task 模型保存路径）
        let first = gate_task(
            &scripts,
            "com.test.app/v3daily.yaml",
            &serde_json::json!({"count": "5", "secret": "s0"}),
            None,
        )
        .unwrap();
        // 带 psig1 的旧快照走签名门禁：一致 → 通过；不一致 → 409 语义
        let snapshot = serde_json::json!({"count": 7, "mode": "manual", "secret": "v"});
        let gate = gate_task(&scripts, "com.test.app/v3daily.yaml", &snapshot, Some(&first.signature))
            .unwrap_or_else(|e| panic!("matching signature must pass: {e:?}"));
        assert_eq!(first.signature, gate.signature);
        let map: std::collections::HashMap<String, TypedValue> =
            gate.overrides.into_iter().collect();
        assert_eq!(map["count"], TypedValue::Text("7".into()));
        assert_eq!(map["mode"], TypedValue::Text("manual".into()));
        assert_eq!(map["secret"], TypedValue::Text("v".into()));
        // 旧签名对 v3 脚本 → 过期（明确失败，不空跑）
        match gate_task(
            &scripts,
            "com.test.app/v3daily.yaml",
            &snapshot,
            Some("psig1|old"),
        ) {
            Err(GateError::SignatureMismatch { current, .. }) => {
                assert_eq!(current, first.signature);
            }
            other => panic!("expected stale, got {:?}", other.is_ok()),
        }
        drop(scripts);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn gate_task_v3_missing_required_and_bad_types_are_structured() {
        let (cfg, dir) = script_dir("gate-v3-strict");
        write_script(&cfg, "v3req.yaml", V3_MIXED);
        let scripts = std::sync::Arc::new(ResourceStore::open(&cfg).unwrap());
        // 空快照：必填 secret 缺失（默认值参数不受影响）
        match gate_task(&scripts, "com.test.app/v3req.yaml", &serde_json::json!({}), None) {
            Err(GateError::ScriptInvalid(diags)) => assert!(diags.iter().any(|d| d.code
                == codes::PARAM_ARGS_MISSING_REQUIRED)),
            other => panic!("expected missing required, got {:?}", other.is_ok()),
        }
        // 快照必须是 JSON 对象
        match gate_task(
            &scripts,
            "com.test.app/v3req.yaml",
            &serde_json::Value::Null,
            None,
        ) {
            Err(GateError::ScriptInvalid(diags)) => assert!(diags.iter().any(|d| d.code
                == codes::PARAM_ARGS_TYPE_MISMATCH)),
            other => panic!("expected type mismatch, got {:?}", other.is_ok()),
        }
        // 类型不符：count 非数值
        match gate_task(
            &scripts,
            "com.test.app/v3req.yaml",
            &serde_json::json!({"count": "abc", "secret": "v"}),
            None,
        ) {
            Err(GateError::ScriptInvalid(diags)) => assert!(diags.iter().any(|d| d.code
                == codes::PARAM_ARGS_TYPE_MISMATCH)),
            other => panic!("expected type mismatch, got {:?}", other.is_ok()),
        }
        // v3 语法坏源：诊断保留 yaml.v3.* 码
        write_script(&cfg, "v3broken.yaml", "version: 3\nparams: []\n");
        match gate_task(&scripts, "com.test.app/v3broken.yaml", &serde_json::json!({}), None) {
            Err(GateError::ScriptInvalid(diags)) => {
                assert!(diags.iter().any(|d| d.code == "yaml.v3.steps.missing"))
            }
            other => panic!("expected invalid v3, got {:?}", other.is_ok()),
        }
        drop(scripts);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn v3_function_library_probe_resolves_target_function_params() {
        let (cfg, dir) = script_dir("v3-func");
        write_function(
            &cfg,
            "lib.yaml",
            "greet:\n  params:\n    - 'text:who:称呼:\"玩家\"'\n    - 'int:times:次数:2'\n  steps:\n    - log: $who\nfarewell:\n  steps:\n    - log: bye\n",
        );
        let scripts = std::sync::Arc::new(ResourceStore::open(&cfg).unwrap());
        let decls = probe_v3_function_decls(&scripts, "com.test.app", "lib", Some("greet"))
            .expect("probe ok");
        assert_eq!(decls.len(), 2);
        assert_eq!(decls[0].name, "who");
        assert_eq!(
            decls[0].default,
            Some(serde_json::Value::String("\"玩家\"".into()))
        );
        assert_eq!(decls[1].ty, "int");
        // 手动绑定：显式 + 默认合并视图
        let mut args = serde_json::Map::new();
        args.insert("times".into(), serde_json::json!(3));
        let bound = bind_v3_manual_args(&decls, &args, "lib/greet").unwrap();
        assert_eq!(
            bound.resolved["who"],
            serde_json::Value::String("玩家".into()),
            "默认值按类型规整进 resolved 视图"
        );
        assert_eq!(bound.resolved["times"], serde_json::json!(3));
        assert_eq!(bound.overrides.len(), 1, "只显式实参进覆盖");
        // 显式函数名缺失 → 结构化 not_found
        match probe_v3_function_decls(&scripts, "com.test.app", "lib", Some("nope")) {
            Err(diags) => assert!(diags
                .iter()
                .any(|d| d.code == codes::RESOURCE_FUNC_NOT_FOUND)),
            other => panic!("expected func not_found, got {:?}", other.is_ok()),
        }
        // 缺省 = 第一个函数
        let first = probe_v3_function_decls(&scripts, "com.test.app", "lib", None).unwrap();
        assert_eq!(first[0].name, "who");
        drop(scripts);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn bind_v3_manual_args_rejects_unknown_keys_types_and_missing_required() {
        let decls = v3_script_decls_in_memory(V3_MIXED);
        // 未知键
        let mut args = serde_json::Map::new();
        args.insert("ghost".into(), serde_json::json!(1));
        args.insert("secret".into(), serde_json::json!("v"));
        let err = bind_v3_manual_args(&decls, &args, "t").unwrap_err();
        assert!(
            err.iter()
                .any(|e| e.code == codes::PARAM_ARGS_UNKNOWN)
        );
        // 类型不符
        let mut args = serde_json::Map::new();
        args.insert("count".into(), serde_json::json!(true));
        args.insert("secret".into(), serde_json::json!("v"));
        let err = bind_v3_manual_args(&decls, &args, "t").unwrap_err();
        assert!(err.iter().any(|e| e.code == codes::PARAM_ARGS_TYPE_MISMATCH
            && e.step_path_str() == "args.count"));
        // 缺必填
        let args = serde_json::Map::new();
        let err = bind_v3_manual_args(&decls, &args, "t").unwrap_err();
        assert!(err
            .iter()
            .any(|e| e.code == codes::PARAM_ARGS_MISSING_REQUIRED));
    }
}
