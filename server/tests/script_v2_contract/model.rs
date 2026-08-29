//! 节点树 → 拟议前端 Model JSON（阶段 0 golden 断言用）。
//!
//! 这不是阶段 2 的正式 codec：只覆盖 fixture 用到的形态，用于把 golden JSON
//! 断言落到可执行代码上。字段命名与 docs/SCRIPT_EDITOR_CONTRACT.md 第 3 节
//! 五方对照表的「前端 Model / API JSON」列一致。阶段 2 实现正式 AST 时替换本文件。

use crate::yaml_loader::Node;
use serde_json::{json, Map, Value};

/// 步骤动作键（CONTRACT.md Step 联合类型的 17 个成员）。
pub const ACTION_KEYS: &[&str] = &[
    "str_app", "cls_app", "tap", "swipe", "key", "text", "log", "wait", "find", "match", "color",
    "if", "loop", "call", "func", "throw", "return",
];

const PARAM_TYPES: &[&str] = &["tmpl", "coord", "color", "time", "key", "text", "bool"];

/// ScriptModel：{params, config, steps}。
pub fn build_script_model(root: &Node) -> Result<Value, String> {
    let entries = root.as_map().ok_or("script 根节点必须是映射")?;
    let mut params: Vec<Value> = Vec::new();
    let mut config = Value::Null;
    let mut steps: Vec<Value> = Vec::new();
    for (key, value) in entries {
        match key.as_str() {
            "params" => params = build_params(value)?,
            "config" => config = build_config(value)?,
            "steps" => steps = build_steps(value, "steps")?,
            other => return Err(format!("未知顶层键 {other}")),
        }
    }
    Ok(json!({ "params": params, "config": config, "steps": steps }))
}

/// FunctionLibraryModel：{file, functions}，函数记录只允许 params/steps。
pub fn build_function_library_model(root: &Node, file_short: &str) -> Result<Value, String> {
    let entries = root.as_map().ok_or("函数库根节点必须是映射")?;
    let mut functions = Vec::new();
    for (name, record) in entries {
        let record_map = record
            .as_map()
            .ok_or_else(|| format!("函数 {name} 的记录必须是映射"))?;
        let mut params: Vec<Value> = Vec::new();
        let mut steps: Vec<Value> = Vec::new();
        for (key, value) in record_map {
            match key.as_str() {
                "params" => params = build_params(value)?,
                "steps" => steps = build_steps(value, &format!("{name}.steps"))?,
                other => return Err(format!("函数 {name} 含非法记录键 {other}")),
            }
        }
        functions.push(json!({ "name": name, "params": params, "steps": steps }));
    }
    Ok(json!({ "file": file_short, "functions": functions }))
}

fn build_params(node: &Node) -> Result<Vec<Value>, String> {
    let items = node.as_seq().ok_or("params 必须是列表")?;
    let mut out = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let raw = item
            .as_str()
            .ok_or_else(|| format!("params[{i}] 必须是单引号标量"))?;
        out.push(parse_param_decl(raw).map_err(|e| format!("params[{i}]: {e}"))?);
    }
    Ok(out)
}

/// `类型:变量名:备注[:默认值]` → ParamDecl JSON。default 为 null 表示必填。
pub fn parse_param_decl(raw: &str) -> Result<Value, String> {
    let parts: Vec<&str> = raw.splitn(4, ':').collect();
    if parts.len() < 3 {
        return Err(format!("声明 {raw:?} 不是 类型:变量名:备注[:默认值] 四段式"));
    }
    let (ty, name, remark) = (parts[0], parts[1], parts[2]);
    if ty.is_empty() || name.is_empty() || remark.is_empty() {
        return Err(format!("声明 {raw:?} 的类型/变量名/备注不能为空"));
    }
    if !PARAM_TYPES.contains(&ty) {
        return Err(format!("未知参数类型 {ty:?}"));
    }
    let default = match parts.len() {
        3 => Value::Null,
        4 => typed_default(ty, parts[3])?,
        _ => unreachable!(),
    };
    Ok(json!({ "type": ty, "name": name, "remark": remark, "default": default }))
}

/// 默认值尾串按声明类型解析为类型化字面量。空尾串非法（不等价于没有默认值）。
pub fn typed_default(ty: &str, tail: &str) -> Result<Value, String> {
    if tail.is_empty() {
        return Err("空默认值（第四段尾串为空），应省略默认值或写成 \"\"".into());
    }
    match ty {
        "bool" => match tail {
            "true" => Ok(json!(true)),
            "false" => Ok(json!(false)),
            other => Err(format!("bool 默认值必须是 true/false，得到 {other:?}")),
        },
        "coord" => {
            let inner = tail.trim();
            let inner = inner.strip_prefix('[').ok_or_else(|| format!("coord 默认值必须是 [x, y]，得到 {tail:?}"))?;
            let inner = inner.strip_suffix(']').ok_or_else(|| format!("coord 默认值必须是 [x, y]，得到 {tail:?}"))?;
            let mut nums = Vec::new();
            for part in inner.split(',') {
                let x: f64 = part
                    .trim()
                    .parse()
                    .map_err(|_| format!("coord 默认值含非法数字 {part:?}"))?;
                nums.push(x);
            }
            if nums.len() != 2 {
                return Err(format!("coord 默认值必须是两个数字，得到 {tail:?}"));
            }
            Ok(json!(nums))
        }
        "color" => {
            if tail.len() != 6 || !tail.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(format!("color 默认值必须是 6 位十六进制，得到 {tail:?}"));
            }
            Ok(json!(tail))
        }
        "time" => {
            verify_time(tail)?;
            Ok(json!(tail))
        }
        "key" | "tmpl" => Ok(json!(tail)),
        // text 默认值可用双引号明确空格/特殊字符；外层双引号剥离后为实际值。
        "text" => {
            if tail.starts_with('"') && tail.ends_with('"') && tail.len() >= 2 {
                Ok(json!(tail[1..tail.len() - 1].to_string()))
            } else {
                Ok(json!(tail))
            }
        }
        other => Err(format!("未知参数类型 {other}")),
    }
}

/// 时间字面量：数字 + 单位（ms/s/m/min/h/d），必须带单位且大于 0。
pub fn verify_time(raw: &str) -> Result<(), String> {
    let lower = raw.to_ascii_lowercase();
    let units = ["min", "ms", "s", "m", "h", "d"];
    for unit in units {
        if let Some(num) = lower.strip_suffix(unit) {
            let x: f64 = num
                .parse()
                .map_err(|_| format!("时间 {raw:?} 的数值部分非法"))?;
            if x <= 0.0 {
                return Err(format!("时间 {raw:?} 必须大于 0"));
            }
            return Ok(());
        }
    }
    Err(format!("时间 {raw:?} 必须带单位（ms/s/m/min/h/d）"))
}

fn build_config(node: &Node) -> Result<Value, String> {
    let entries = node.as_map().ok_or("config 必须是映射")?;
    let mut out = Map::new();
    for (k, v) in entries {
        match k.as_str() {
            "interval" => out.insert(
                "interval".into(),
                json!(v.as_str().ok_or("config.interval 必须是字符串")?),
            ),
            "log_level" => out.insert(
                "log_level".into(),
                json!(v.as_str().ok_or("config.log_level 必须是字符串")?),
            ),
            "threshold" => {
                let raw = v.as_str().ok_or("config.threshold 必须是数字")?;
                let x: f64 = raw.parse().map_err(|_| "config.threshold 不是数字")?;
                out.insert("threshold".into(), json!(x))
            }
            other => return Err(format!("未知 config 键 {other}")),
        };
    }
    Ok(Value::Object(out))
}

fn build_steps(node: &Node, path: &str) -> Result<Vec<Value>, String> {
    let items = node.as_seq().ok_or_else(|| format!("{path} 必须是列表"))?;
    let mut out = Vec::new();
    for (i, item) in items.iter().enumerate() {
        out.push(build_step(item, &format!("{path}[{i}]"))?);
    }
    Ok(out)
}

#[derive(Clone, Copy, PartialEq)]
enum FieldType {
    Bool,
    Coord,
    /// tmpl/color/time/key/text 共用：字面量是字符串。
    Str,
}

/// 取值单元格 Cell：$name → {"ref": name}；字面量 → {"lit": 类型化值}。
fn cell(node: &Node, ty: FieldType, ctx: &str) -> Result<Value, String> {
    match node {
        Node::Scalar { raw, .. } => {
            if let Some(name) = raw.strip_prefix('$') {
                return Ok(json!({ "ref": name }));
            }
            let lit = match ty {
                FieldType::Bool => match raw.as_str() {
                    "true" => json!(true),
                    "false" => json!(false),
                    other => return Err(format!("{ctx}: 布尔字段只能是 true/false，得到 {other:?}")),
                },
                FieldType::Coord => {
                    return Err(format!("{ctx}: 坐标字面量必须是 [x, y] 序列，得到 {raw:?}"))
                }
                FieldType::Str => json!(raw),
            };
            Ok(json!({ "lit": lit }))
        }
        Node::Seq(items) => {
            if ty != FieldType::Coord {
                return Err(format!("{ctx}: 该字段不接受序列字面量"));
            }
            if items.len() != 2 {
                return Err(format!("{ctx}: 坐标必须是两个数字"));
            }
            let mut nums = Vec::new();
            for it in items {
                let raw = it
                    .as_str()
                    .ok_or_else(|| format!("{ctx}: 坐标分量必须是数字"))?;
                let x: f64 = raw
                    .parse()
                    .map_err(|_| format!("{ctx}: 坐标分量 {raw:?} 不是数字"))?;
                nums.push(x);
            }
            Ok(json!({ "lit": nums }))
        }
        Node::Map(_) => Err(format!("{ctx}: 字段不接受映射")),
    }
}

/// 候选键位置的单元格（match 候选模板 / color 候选颜色）：$name → ref，否则 lit 字符串。
fn cell_from_key(raw: &str) -> Value {
    match raw.strip_prefix('$') {
        Some(name) => json!({ "ref": name }),
        None => json!({ "lit": raw }),
    }
}

/// args 实参单元格：$name → ref；字面量宽松定型（true/false → 布尔，其余 → 字符串）。
/// 与目标参数类型的精确绑定需要目标声明，归阶段 2。
fn arg_cell(node: &Node, ctx: &str) -> Result<Value, String> {
    match node {
        Node::Scalar { raw, .. } => {
            if let Some(name) = raw.strip_prefix('$') {
                Ok(json!({ "ref": name }))
            } else if raw == "true" {
                Ok(json!({ "lit": true }))
            } else if raw == "false" {
                Ok(json!({ "lit": false }))
            } else {
                Ok(json!({ "lit": raw }))
            }
        }
        _ => Err(format!("{ctx}: args 实参必须是标量")),
    }
}

fn build_step(item: &Node, path: &str) -> Result<Value, String> {
    match item {
        Node::Scalar { raw, .. } => match raw.as_str() {
            "str_app" => Ok(json!({ "kind": "str_app" })),
            "cls_app" => Ok(json!({ "kind": "cls_app" })),
            "throw" => Ok(json!({ "kind": "throw", "message": null })),
            other => Err(format!(
                "{path}: 裸标量步骤只能是 str_app/cls_app/throw，得到 {other:?}"
            )),
        },
        Node::Map(entries) => {
            let mut action: Option<(&str, &Node)> = None;
            let mut fields: Vec<(&str, &Node)> = Vec::new();
            for (k, v) in entries {
                if ACTION_KEYS.contains(&k.as_str()) {
                    if action.is_some() {
                        return Err(format!("{path}: 一个步骤只能有一个动作键"));
                    }
                    action = Some((k.as_str(), v));
                } else {
                    fields.push((k.as_str(), v));
                }
            }
            let (action, value) = action.ok_or_else(|| format!("{path}: 步骤缺少动作键"))?;
            let then_else = |fields: &[(&str, &Node)], key: &str| -> Result<Vec<Value>, String> {
                match fields.iter().find(|(k, _)| *k == key) {
                    Some((_, v)) => build_steps(v, &format!("{path}.{key}")),
                    None => Ok(Vec::new()),
                }
            };
            match action {
                "tap" => Ok(json!({ "kind": "tap", "at": cell(value, FieldType::Coord, path)? })),
                "swipe" => {
                    let m = value.as_map().ok_or_else(|| format!("{path}: swipe 值必须是映射"))?;
                    let get = |k: &str| m.iter().find(|(key, _)| key == k).map(|(_, v)| v);
                    let from = cell(get("fm").ok_or_else(|| format!("{path}: swipe 缺少 fm"))?, FieldType::Coord, path)?;
                    let to = cell(get("to").ok_or_else(|| format!("{path}: swipe 缺少 to"))?, FieldType::Coord, path)?;
                    let time = cell(get("time").ok_or_else(|| format!("{path}: swipe 缺少 time"))?, FieldType::Str, path)?;
                    Ok(json!({ "kind": "swipe", "from": from, "to": to, "time": time }))
                }
                "key" => Ok(json!({ "kind": "key", "key": cell(value, FieldType::Str, path)? })),
                "text" => Ok(json!({ "kind": "text", "value": cell(value, FieldType::Str, path)? })),
                "log" => Ok(json!({ "kind": "log", "message": cell(value, FieldType::Str, path)? })),
                "wait" => match value {
                    Node::Scalar { .. } => Ok(json!({
                        "kind": "wait",
                        "duration": cell(value, FieldType::Str, path)?,
                        "duration_max": null,
                    })),
                    Node::Seq(items) if items.len() == 2 => Ok(json!({
                        "kind": "wait",
                        "duration": cell(&items[0], FieldType::Str, path)?,
                        "duration_max": cell(&items[1], FieldType::Str, path)?,
                    })),
                    _ => Err(format!("{path}: wait 值必须是时长或 [起, 止] 区间")),
                },
                "find" => {
                    let template = cell(value, FieldType::Str, path)?;
                    let block = match fields.iter().find(|(k, _)| *k == "block") {
                        Some((_, v)) => {
                            let items = v.as_seq().ok_or_else(|| format!("{path}: find.block 必须是模板列表"))?;
                            let mut cells = Vec::new();
                            for (i, it) in items.iter().enumerate() {
                                cells.push(cell(it, FieldType::Str, &format!("{path}.block[{i}]"))?);
                            }
                            cells
                        }
                        None => Vec::new(),
                    };
                    let verify = match fields.iter().find(|(k, _)| *k == "verify") {
                        Some((_, v)) => match v.as_str() {
                            Some("true") => true,
                            Some("false") => false,
                            other => return Err(format!("{path}: find.verify 必须是布尔，得到 {other:?}")),
                        },
                        None => false,
                    };
                    let timeout = match fields.iter().find(|(k, _)| *k == "timeout") {
                        Some((_, v)) => cell(v, FieldType::Str, path)?,
                        None => Value::Null,
                    };
                    let then = then_else(&fields, "then")?;
                    let els = then_else(&fields, "else")?;
                    Ok(json!({
                        "kind": "find", "template": template, "block": block,
                        "verify": verify, "timeout": timeout, "then": then, "else": els,
                    }))
                }
                "match" => {
                    let cand_seq = value
                        .as_seq()
                        .ok_or_else(|| format!("{path}: match 值必须是候选列表（紧凑缩进）"))?;
                    let mut candidates = Vec::new();
                    for (i, c) in cand_seq.iter().enumerate() {
                        let m = c
                            .as_map()
                            .ok_or_else(|| format!("{path}.candidates[{i}]: 候选必须是单键映射"))?;
                        if m.len() != 1 {
                            return Err(format!("{path}.candidates[{i}]: 候选必须是单键映射"));
                        }
                        let (key, steps_node) = &m[0];
                        candidates.push(json!({
                            "template": cell_from_key(key),
                            "steps": build_steps(steps_node, &format!("{path}.candidates[{i}].steps"))?,
                        }));
                    }
                    let els = then_else(&fields, "else")?;
                    let timeout = match fields.iter().find(|(k, _)| *k == "timeout") {
                        Some((_, v)) => cell(v, FieldType::Str, path)?,
                        None => Value::Null,
                    };
                    Ok(json!({
                        "kind": "match", "candidates": candidates,
                        "else": els, "timeout": timeout,
                    }))
                }
                "color" => {
                    let m = value.as_map().ok_or_else(|| format!("{path}: color 值必须是映射"))?;
                    let get = |k: &str| m.iter().find(|(key, _)| key == k).map(|(_, v)| v);
                    let at = cell(get("at").ok_or_else(|| format!("{path}: color 缺少 at"))?, FieldType::Coord, path)?;
                    // expect 是有序列表（每项单键映射 颜色→步骤），不用颜色做映射键：
                    // 纯数字色键在 YAML 对象化后会被 JS/解析端按整数键重排（js-yaml 实测），
                    // 列表形态在所有解析端保序，且与 match 候选同构。
                    let expect_node = get("expect").ok_or_else(|| format!("{path}: color 缺少 expect"))?;
                    let expect_seq = expect_node
                        .as_seq()
                        .ok_or_else(|| format!("{path}: color.expect 必须是 颜色:步骤 候选列表"))?;
                    let mut expect = Vec::new();
                    for (i, item) in expect_seq.iter().enumerate() {
                        let m = item
                            .as_map()
                            .ok_or_else(|| format!("{path}.expect[{i}]: 候选必须是单键映射"))?;
                        if m.len() != 1 {
                            return Err(format!("{path}.expect[{i}]: 候选必须是单键映射"));
                        }
                        let (color_key, steps_node) = &m[0];
                        expect.push(json!({
                            "color": cell_from_key(color_key),
                            "steps": build_steps(steps_node, &format!("{path}.expect[{i}].steps"))?,
                        }));
                    }
                    let els = then_else(&fields, "else")?;
                    Ok(json!({ "kind": "color", "at": at, "expect": expect, "else": els }))
                }
                "if" => {
                    let cond = cell(value, FieldType::Bool, path)?;
                    let then = then_else(&fields, "then")?;
                    let els = then_else(&fields, "else")?;
                    Ok(json!({ "kind": "if", "cond": cond, "then": then, "else": els }))
                }
                "loop" => {
                    let m = value.as_map().ok_or_else(|| format!("{path}: loop 值必须是映射"))?;
                    let times = match m.iter().find(|(k, _)| k == "times") {
                        Some((_, v)) => {
                            let raw = v.as_str().ok_or_else(|| format!("{path}: loop.times 必须是数字"))?;
                            json!(raw.parse::<u64>().map_err(|_| format!("{path}: loop.times {raw:?} 不是非负整数"))?)
                        }
                        None => Value::Null,
                    };
                    let steps_node = m
                        .iter()
                        .find(|(k, _)| k == "steps")
                        .map(|(_, v)| v)
                        .ok_or_else(|| format!("{path}: loop 缺少 steps"))?;
                    let steps = build_steps(steps_node, &format!("{path}.steps"))?;
                    Ok(json!({ "kind": "loop", "times": times, "steps": steps }))
                }
                "call" => {
                    let target = value
                        .as_str()
                        .ok_or_else(|| format!("{path}: call 目标必须是字符串"))?
                        .to_string();
                    let args = build_args(fields.iter().find(|(k, _)| *k == "args").map(|(_, v)| *v), path)?;
                    Ok(json!({ "kind": "call", "target": target, "args": args }))
                }
                "func" => {
                    let target = value
                        .as_str()
                        .ok_or_else(|| format!("{path}: func 目标必须是字符串"))?
                        .to_string();
                    let args = build_args(fields.iter().find(|(k, _)| *k == "args").map(|(_, v)| *v), path)?;
                    let then = then_else(&fields, "then")?;
                    let els = then_else(&fields, "else")?;
                    Ok(json!({ "kind": "func", "target": target, "args": args, "then": then, "else": els }))
                }
                "throw" => {
                    let raw = value
                        .as_str()
                        .ok_or_else(|| format!("{path}: throw 值必须是字符串或裸写"))?;
                    let message = if raw.is_empty() { Value::Null } else { json!(raw) };
                    Ok(json!({ "kind": "throw", "message": message }))
                }
                "return" => Ok(json!({ "kind": "return", "value": cell(value, FieldType::Bool, path)? })),
                other => Err(format!("{path}: 未知动作 {other}")),
            }
        }
        Node::Seq(_) => Err(format!("{path}: 步骤不能是列表")),
    }
}

fn build_args(node: Option<&Node>, path: &str) -> Result<Value, String> {
    let Some(node) = node else { return Ok(json!({})) };
    let entries = node
        .as_map()
        .ok_or_else(|| format!("{path}.args 必须是映射"))?;
    let mut out = Map::new();
    for (k, v) in entries {
        out.insert(k.clone(), arg_cell(v, &format!("{path}.args.{k}"))?);
    }
    Ok(Value::Object(out))
}

/// 任务参数签名（psig1）：按声明顺序覆盖类型/名称/必填性/默认值的规范化串。
/// 算法冻结在 docs/SCRIPT_EDITOR_CONTRACT.md 第 4.5 节。
pub fn param_signature(model: &Value) -> String {
    let params = model["params"].as_array().expect("params 数组");
    let mut entries = Vec::new();
    for p in params {
        let ty = p["type"].as_str().expect("type");
        let name = p["name"].as_str().expect("name");
        let default = &p["default"];
        let (required, canon) = if default.is_null() {
            ("1", String::new())
        } else {
            ("0", canonical_default(ty, default))
        };
        entries.push(format!("{ty},{name},{required},{canon}"));
    }
    format!("psig1|{}", entries.join("|"))
}

fn canonical_default(ty: &str, default: &Value) -> String {
    match ty {
        "bool" => default.as_bool().expect("bool").to_string(),
        "coord" => {
            let a = default.as_array().expect("coord 数组");
            format!(
                "[{},{}]",
                fmt_num(a[0].as_f64().expect("x")),
                fmt_num(a[1].as_f64().expect("y"))
            )
        }
        "color" => default.as_str().expect("color").to_ascii_lowercase(),
        "key" => default.as_str().expect("key").to_ascii_uppercase(),
        "time" => canonical_time(default.as_str().expect("time")),
        "text" => escape_signature(default.as_str().expect("text")),
        "tmpl" => default.as_str().expect("tmpl").to_string(),
        other => unreachable!("未知参数类型 {other}"),
    }
}

fn fmt_num(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 1e15 {
        format!("{}", x as i64)
    } else {
        format!("{x}")
    }
}

/// time 规范形：小写、min 归一为 m、数值保持书写形式。
fn canonical_time(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    match lower.strip_suffix("min") {
        Some(num) => format!("{num}m"),
        None => lower,
    }
}

/// text 规范形：`\` `,` `|` 三字符反斜杠转义（签名以 , 与 | 作分隔）。
fn escape_signature(s: &str) -> String {
    s.replace('\\', "\\\\").replace(',', "\\,").replace('|', "\\|")
}
