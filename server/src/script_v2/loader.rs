//! saphyr-parser 事件级装载：源码 → 带样式/Span 节点树 → 严格 AST。
//!
//! 为什么不用 serde_yaml：0.9 反序列化后标量书写样式彻底丢失，无法校验
//! params「整条单引号」契约（CONTRACT §2 选型结论）。事件级 API 同时携带
//! `ScalarStyle` 与 `Span`（错误定位），且对 match 紧凑缩进（indentless
//! sequence）解析正确。
//!
//! 本文件同时承载「结构层」构建：顶层键白名单、参数声明、步骤结构与字段
//! 互斥/类型检查；语义层（引用存在性、资源、调用环）在 validate.rs。

use saphyr_parser::{Event, Parser, ScalarStyle, Span};

use super::error::codes;
use super::error::ScriptError;
use super::model::{
    ArgAssign, Cell, ColorBranch, FunctionDecl, FunctionFile, LogLevel, MatchCandidate, ParamDecl,
    ScriptFile, Step, TypedValue,
};
use super::params;

/// 文件种类：决定顶层结构、return 合法性与校验范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Script,
    FunctionLibrary,
}

// ---------------------------------------------------------------------------
// 带样式节点树
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) struct Node {
    pub span: Span,
    pub kind: NodeKind,
}

#[derive(Debug, Clone)]
pub(crate) enum NodeKind {
    Scalar { raw: String, style: ScalarStyle },
    Seq(Vec<Node>),
    Map(Vec<MapEntry>),
}

#[derive(Debug, Clone)]
pub(crate) struct MapEntry {
    pub key: String,
    pub key_span: Span,
    pub value: Node,
}

impl Node {
    pub fn as_scalar(&self) -> Option<(&str, ScalarStyle)> {
        match &self.kind {
            NodeKind::Scalar { raw, style } => Some((raw, *style)),
            _ => None,
        }
    }

    pub fn as_seq(&self) -> Option<&[Node]> {
        match &self.kind {
            NodeKind::Seq(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_map(&self) -> Option<&[MapEntry]> {
        match &self.kind {
            NodeKind::Map(entries) => Some(entries),
            _ => None,
        }
    }

    /// 人类可读位置（错误消息内嵌）。
    pub fn loc(&self) -> String {
        format!("行 {} 列 {}", self.span.start.line(), self.span.start.col())
    }
}

enum Frame {
    Seq(Vec<Node>),
    Map {
        entries: Vec<MapEntry>,
        pending_key: Option<(String, Span)>,
    },
}

/// 硬装载错误（语法层）：统一映射为 `yaml.syntax_error`。
pub(crate) fn load(source: &str) -> Result<Node, String> {
    let mut stack: Vec<Frame> = Vec::new();
    let mut root: Option<Node> = None;

    for item in Parser::new_from_str(source) {
        let (ev, span) = item.map_err(|e| format!("YAML 解析失败: {e}"))?;
        match ev {
            Event::StreamStart
            | Event::StreamEnd
            | Event::DocumentStart(_)
            | Event::DocumentEnd
            | Event::Nothing => {}
            Event::Scalar(value, style, _anchor, _tag) => {
                let raw = value.into_owned();
                let is_key = matches!(
                    stack.last(),
                    Some(Frame::Map {
                        pending_key: None,
                        ..
                    })
                );
                if is_key {
                    match stack.last_mut() {
                        Some(Frame::Map { pending_key, .. }) => {
                            *pending_key = Some((raw, span));
                        }
                        _ => unreachable!(),
                    }
                } else {
                    attach(
                        &mut stack,
                        &mut root,
                        Node {
                            span,
                            kind: NodeKind::Scalar { raw, style },
                        },
                    )?;
                }
            }
            Event::SequenceStart(_anchor, _tag) => stack.push(Frame::Seq(Vec::new())),
            Event::SequenceEnd => {
                let frame = stack.pop().ok_or("序列结束不匹配")?;
                let items = match frame {
                    Frame::Seq(items) => items,
                    Frame::Map { .. } => return Err("序列结束遇到映射".into()),
                };
                attach(
                    &mut stack,
                    &mut root,
                    Node {
                        span,
                        kind: NodeKind::Seq(items),
                    },
                )?;
            }
            Event::MappingStart(_anchor, _tag) => stack.push(Frame::Map {
                entries: Vec::new(),
                pending_key: None,
            }),
            Event::MappingEnd => {
                let frame = stack.pop().ok_or("映射结束不匹配")?;
                let (entries, pending) = match frame {
                    Frame::Map {
                        entries,
                        pending_key,
                    } => (entries, pending_key),
                    Frame::Seq(_) => return Err("映射结束遇到序列".into()),
                };
                if pending.is_some() {
                    return Err("映射以悬空键结束".into());
                }
                attach(
                    &mut stack,
                    &mut root,
                    Node {
                        span,
                        kind: NodeKind::Map(entries),
                    },
                )?;
            }
            Event::Alias(_) => return Err("锚点/别名不在 script_v2 契约内".into()),
        }
    }

    root.ok_or_else(|| "空文档".to_string())
}

fn attach(stack: &mut [Frame], root: &mut Option<Node>, node: Node) -> Result<(), String> {
    match stack.last_mut() {
        Some(Frame::Seq(items)) => {
            items.push(node);
            Ok(())
        }
        Some(Frame::Map {
            entries,
            pending_key,
        }) => {
            let (key, key_span) = pending_key
                .take()
                .ok_or_else(|| "映射值缺少键（复键或结构异常）".to_string())?;
            if entries.iter().any(|e| e.key == key) {
                return Err(format!(
                    "重复键 {key:?}（YAML 映射键必须唯一）@ {}",
                    key_loc(&key_span)
                ));
            }
            entries.push(MapEntry {
                key,
                key_span,
                value: node,
            });
            Ok(())
        }
        None => {
            if root.is_some() {
                return Err("出现多余文档（单文档契约）".into());
            }
            *root = Some(node);
            Ok(())
        }
    }
}

fn key_loc(span: &Span) -> String {
    format!("行 {} 列 {}", span.start.line(), span.start.col())
}

// ---------------------------------------------------------------------------
// 结构层构建（错误累积，不早退）
// ---------------------------------------------------------------------------

/// 旧语法顶层键（CONTRACT §3.2：出现即报 legacy_format，给前端迁移引导提示位）。
const LEGACY_TOP_KEYS: &[&str] = &[
    "func",
    "name",
    "action_wait",
    "default_threshold",
    "package",
    "until",
    "cond",
];

/// 步骤动作键（十七类）。
pub(crate) const ACTION_KEYS: &[&str] = &[
    "str_app", "cls_app", "tap", "swipe", "key", "text", "log", "wait", "find", "match", "color",
    "if", "loop", "call", "func", "throw", "return",
];

pub(crate) struct BuildCtx {
    pub resource: String,
    pub kind: FileKind,
    pub errors: Vec<ScriptError>,
}

impl BuildCtx {
    pub fn new(resource: &str, kind: FileKind) -> Self {
        Self {
            resource: resource.to_string(),
            kind,
            errors: Vec::new(),
        }
    }

    fn push(&mut self, code: &str, step_path: &str, field: &str, message: impl Into<String>) {
        self.errors.push(
            ScriptError::new(code, message, self.resource.clone())
                .at(step_path.to_string(), field.to_string()),
        );
    }
}

/// 脚本顶层：params / config / steps。顶层键破坏时不再下钻（避免错误风暴）。
pub(crate) fn build_script_file(ctx: &mut BuildCtx, root: &Node) -> Option<ScriptFile> {
    let entries = match &root.kind {
        NodeKind::Map(entries) => entries,
        _ => {
            ctx.push(codes::SCRIPT_ROOT_TYPE, "", "yaml", "根节点必须是映射");
            return None;
        }
    };
    let mut top_bad = false;
    let mut params_node = None;
    let mut config_node = None;
    let mut steps_node = None;
    for e in entries {
        if LEGACY_TOP_KEYS.contains(&e.key.as_str()) {
            ctx.push(
                codes::SCRIPT_TOP_LEVEL_LEGACY,
                "",
                &e.key,
                format!(
                    "顶层键 {:?} 属于旧语法（旧 func:/name:/action_wait 等不再支持），请迁移到新语法",
                    e.key
                ),
            );
            top_bad = true;
        } else if !matches!(e.key.as_str(), "params" | "config" | "steps") {
            ctx.push(
                codes::SCRIPT_TOP_LEVEL_UNKNOWN_KEY,
                "",
                &e.key,
                format!("未知顶层键 {:?}，只允许 params/config/steps", e.key),
            );
            top_bad = true;
        } else {
            match e.key.as_str() {
                "params" => params_node = Some(&e.value),
                "config" => config_node = Some(&e.value),
                "steps" => steps_node = Some(&e.value),
                _ => unreachable!(),
            }
        }
    }
    if top_bad {
        return None;
    }
    let params_decls = params_node
        .map(|n| build_params(ctx, n, "params"))
        .unwrap_or_default();
    let config = config_node.and_then(|n| build_config(ctx, n));
    let Some(steps_node) = steps_node else {
        ctx.push(
            codes::STEP_FIELD_MISSING,
            "",
            "steps",
            "脚本缺少 steps（必需，可为空列表但不可省略）",
        );
        return None;
    };
    let steps = build_steps(ctx, steps_node, "steps");
    Some(ScriptFile {
        params: params_decls,
        config,
        steps,
    })
}

/// 函数库顶层：每个键 = 函数名，记录只允许 params/steps，保持书写顺序。
pub(crate) fn build_function_file(ctx: &mut BuildCtx, root: &Node) -> Option<FunctionFile> {
    let entries = match &root.kind {
        NodeKind::Map(entries) => entries,
        _ => {
            ctx.push(codes::SCRIPT_ROOT_TYPE, "", "yaml", "根节点必须是映射");
            return None;
        }
    };
    let mut functions = Vec::new();
    for e in entries {
        let name = &e.key;
        let Some(record) = e.value.as_map() else {
            ctx.push(
                codes::FUNC_RECORD_TYPE,
                name,
                "",
                format!("函数 {name} 的记录必须是映射（只允许 params/steps 两个记录键）"),
            );
            continue;
        };
        let mut params_node = None;
        let mut steps_node = None;
        for r in record {
            match r.key.as_str() {
                "params" => params_node = Some(&r.value),
                "steps" => steps_node = Some(&r.value),
                other => ctx.push(
                    codes::FUNC_RECORD_UNKNOWN_KEY,
                    name,
                    other,
                    format!("函数 {name} 记录含非法键 {other:?}，只允许 params/steps"),
                ),
            }
        }
        let fn_params = params_node
            .map(|n| build_params(ctx, n, &format!("{name}.params")))
            .unwrap_or_default();
        let Some(steps_node) = steps_node else {
            ctx.push(
                codes::STEP_FIELD_MISSING,
                name,
                "steps",
                format!("函数 {name} 缺少 steps（必需，可为空列表但不可省略）"),
            );
            continue;
        };
        let steps = build_steps(ctx, steps_node, &format!("{name}.steps"));
        functions.push(FunctionDecl {
            name: name.clone(),
            params: fn_params,
            steps,
        });
    }
    Some(FunctionFile { functions })
}

fn build_params(ctx: &mut BuildCtx, node: &Node, base_path: &str) -> Vec<ParamDecl> {
    let Some(items) = node.as_seq() else {
        ctx.push(
            codes::PARAM_DECL_FORMAT,
            base_path,
            "params",
            "params 必须是列表",
        );
        return Vec::new();
    };
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let path = format!("{base_path}[{i}]");
        let Some((raw, style)) = item.as_scalar() else {
            ctx.push(
                codes::PARAM_DECL_FORMAT,
                &path,
                "declaration",
                "params 项必须是标量（整条单引号）",
            );
            continue;
        };
        // 「整条单引号」契约：无引号 plain 等其他样式一律拒绝——样式丢失后无法
        // 与普通字符串区分，且备注/默认值中的特殊字符有歧义（CONTRACT §3.3）。
        if style != ScalarStyle::SingleQuoted {
            ctx.push(
                codes::PARAM_DECL_QUOTE_STYLE,
                &path,
                "style",
                format!(
                    "params 项必须整条用单引号包裹，如 'bool:enable:开关:true'（当前为{}）",
                    scalar_style_name(style)
                ),
            );
            continue;
        }
        match params::parse_param_decl(raw) {
            Ok(decl) => {
                if seen.contains(&decl.name) {
                    ctx.push(
                        codes::PARAM_DECL_NAME_DUPLICATE,
                        &path,
                        "name",
                        format!("参数名 {:?} 在同一参数表内重复", decl.name),
                    );
                } else {
                    seen.push(decl.name.clone());
                }
                out.push(decl);
            }
            Err(e) => ctx.push(e.code, &path, e.field, e.message),
        }
    }
    out
}

fn scalar_style_name(style: ScalarStyle) -> &'static str {
    match style {
        ScalarStyle::Plain => "无引号 plain",
        ScalarStyle::SingleQuoted => "单引号",
        ScalarStyle::DoubleQuoted => "双引号",
        ScalarStyle::Literal => "块字面量 |",
        ScalarStyle::Folded => "块折叠 >",
    }
}

fn build_config(ctx: &mut BuildCtx, node: &Node) -> Option<super::model::ScriptConfig> {
    use super::model::ScriptConfig;
    let entries = match node.as_map() {
        Some(entries) => entries,
        None => {
            ctx.push(
                codes::SCRIPT_CONFIG_INVALID,
                "config",
                "config",
                "config 必须是映射",
            );
            return None;
        }
    };
    let mut interval: Option<std::time::Duration> = None;
    let mut threshold: Option<f64> = None;
    let mut log_level: Option<LogLevel> = None;
    for e in entries {
        let field = e.key.as_str();
        let raw = e.value.as_scalar().map(|(raw, _)| raw);
        match field {
            "interval" => match raw.and_then(params::parse_time_duration) {
                Some(d) => interval = Some(d),
                None => ctx.push(
                    codes::SCRIPT_CONFIG_INVALID,
                    "config",
                    "interval",
                    "config.interval 必须是带单位时间（ms/s/m/min/h/d）且 > 0",
                ),
            },
            "threshold" => match raw.and_then(|r| r.parse::<f64>().ok()) {
                Some(x) if (0.0..=1.0).contains(&x) => threshold = Some(x),
                _ => ctx.push(
                    codes::SCRIPT_CONFIG_INVALID,
                    "config",
                    "threshold",
                    "config.threshold 必须是 0~1 的数字",
                ),
            },
            "log_level" => match raw.and_then(LogLevel::parse) {
                Some(l) => log_level = Some(l),
                None => ctx.push(
                    codes::SCRIPT_CONFIG_INVALID,
                    "config",
                    "log_level",
                    "config.log_level 必须是 debug/info/warn/error",
                ),
            },
            other => ctx.push(
                codes::SCRIPT_CONFIG_UNKNOWN_KEY,
                "config",
                other,
                format!("未知 config 键 {other:?}，只允许 interval/threshold/log_level"),
            ),
        }
    }
    let (Some(interval), Some(threshold), Some(log_level)) = (interval, threshold, log_level)
    else {
        // 缺键（或上面已报错）——config 要么整体省略，要么三键齐全。
        for (missing, name) in [
            (interval.is_none(), "interval"),
            (threshold.is_none(), "threshold"),
            (log_level.is_none(), "log_level"),
        ] {
            if missing {
                ctx.push(
                    codes::SCRIPT_CONFIG_INVALID,
                    "config",
                    name,
                    format!("config 缺少 {name}（config 要么整体省略，要么三键齐全）"),
                );
            }
        }
        return None;
    };
    Some(ScriptConfig {
        interval,
        threshold,
        log_level,
    })
}

fn build_steps(ctx: &mut BuildCtx, node: &Node, path: &str) -> Vec<Step> {
    let Some(items) = node.as_seq() else {
        ctx.push(
            codes::STEP_LIST_TYPE,
            path,
            last_segment(path),
            "步骤必须是列表",
        );
        return Vec::new();
    };
    items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| build_step(ctx, item, &format!("{path}[{i}]")))
        .collect()
}

fn last_segment(path: &str) -> &str {
    path.rsplit('.').next().unwrap_or(path)
}

/// 字段位置期望的字面量类型（决定 Cell 字面量变体与 $name 引用类型校验）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Exp {
    Coord,
    Time,
    Key,
    Text,
    Tmpl,
    Color,
    Bool,
}

impl Exp {
    pub fn param_type(self) -> super::model::ParamType {
        use super::model::ParamType;
        match self {
            Exp::Coord => ParamType::Coord,
            Exp::Time => ParamType::Time,
            Exp::Key => ParamType::Key,
            Exp::Text => ParamType::Text,
            Exp::Tmpl => ParamType::Tmpl,
            Exp::Color => ParamType::Color,
            Exp::Bool => ParamType::Bool,
        }
    }
}

fn exp_name(exp: Exp) -> &'static str {
    match exp {
        Exp::Coord => " [x, y] 坐标",
        Exp::Time => "时间字面量",
        Exp::Key => "按键名",
        Exp::Text => "文本",
        Exp::Tmpl => "模板短名",
        Exp::Color => "颜色",
        Exp::Bool => "布尔值",
    }
}

fn build_cell(ctx: &mut BuildCtx, node: &Node, path: &str, field: &str, exp: Exp) -> Option<Cell> {
    match &node.kind {
        NodeKind::Scalar { raw, style } => {
            if let Some(name) = raw.strip_prefix('$').filter(|n| !n.is_empty()) {
                return Some(Cell::Ref(name.to_string()));
            }
            let lit = match exp {
                Exp::Bool => {
                    if *style == ScalarStyle::Plain && matches!(raw.as_str(), "true" | "false") {
                        TypedValue::Bool(raw == "true")
                    } else {
                        bad_cell(ctx, path, field, exp, raw);
                        return None;
                    }
                }
                Exp::Coord => {
                    bad_cell(ctx, path, field, exp, raw);
                    return None;
                }
                Exp::Time => match params::parse_time_ms(raw) {
                    Some(_) => TypedValue::Time(raw.clone()),
                    None => {
                        ctx.push(
                            codes::STEP_TIME_FORMAT,
                            path,
                            field,
                            format!("时间 {raw:?} 必须带单位（ms/s/m/min/h/d）且 > 0"),
                        );
                        return None;
                    }
                },
                Exp::Key => {
                    if !params::is_valid_key(raw) {
                        ctx.push(
                            codes::STEP_FIELD_TYPE_MISMATCH,
                            path,
                            field,
                            params::invalid_key_reason(raw),
                        );
                        return None;
                    }
                    TypedValue::Key(raw.clone())
                }
                Exp::Text => TypedValue::Text(raw.clone()),
                Exp::Tmpl => {
                    if raw.is_empty() {
                        bad_cell(ctx, path, field, exp, raw);
                        return None;
                    }
                    TypedValue::Tmpl(raw.clone())
                }
                Exp::Color => {
                    if !params::is_valid_color(raw) {
                        ctx.push(
                            codes::STEP_COLOR_FORMAT,
                            path,
                            field,
                            format!("颜色 {raw:?} 不是 6 位十六进制"),
                        );
                        return None;
                    }
                    TypedValue::Color(raw.clone())
                }
            };
            Some(Cell::Lit(lit))
        }
        NodeKind::Seq(items) => {
            if exp != Exp::Coord {
                bad_cell(ctx, path, field, exp, "序列");
                return None;
            }
            if items.len() != 2 {
                ctx.push(
                    codes::STEP_FIELD_TYPE_MISMATCH,
                    path,
                    field,
                    "坐标必须是 [x, y] 两个数字",
                );
                return None;
            }
            let mut nums = [0.0f64; 2];
            for (idx, it) in items.iter().enumerate() {
                match it.as_scalar().and_then(|(raw, _)| raw.parse::<f64>().ok()) {
                    Some(x) => nums[idx] = x,
                    None => {
                        ctx.push(
                            codes::STEP_FIELD_TYPE_MISMATCH,
                            path,
                            field,
                            "坐标分量必须是数字",
                        );
                        return None;
                    }
                }
            }
            if !params::coord_in_range(nums[0]) || !params::coord_in_range(nums[1]) {
                ctx.push(
                    codes::STEP_COORD_RANGE,
                    path,
                    field,
                    format!(
                        "坐标 [{}, {}] 超出 0~1 相对坐标范围",
                        params::fmt_num(nums[0]),
                        params::fmt_num(nums[1])
                    ),
                );
                return None;
            }
            Some(Cell::Lit(TypedValue::Coord(nums)))
        }
        NodeKind::Map(_) => {
            bad_cell(ctx, path, field, exp, "映射");
            None
        }
    }
}

fn bad_cell(ctx: &mut BuildCtx, path: &str, field: &str, exp: Exp, got: &str) {
    // if 条件的非布尔字面量有专属错误码，其余位置用通用字段类型错误。
    let (code, expect) = if exp == Exp::Bool {
        (codes::STEP_IF_NON_BOOL_COND, "true/false 或 $bool 引用")
    } else {
        (codes::STEP_FIELD_TYPE_MISMATCH, exp_name(exp))
    };
    ctx.push(
        code,
        path,
        field,
        format!("字段 {field} 需要{expect}，得到 {got:?}"),
    );
}

fn build_bool_field(ctx: &mut BuildCtx, node: &Node, path: &str, field_name: &str) -> Option<bool> {
    match node.as_scalar() {
        Some(("true", ScalarStyle::Plain)) => Some(true),
        Some(("false", ScalarStyle::Plain)) => Some(false),
        _ => {
            ctx.push(
                codes::STEP_FIELD_TYPE_MISMATCH,
                path,
                field_name,
                format!("{field_name} 必须是布尔字面量 true/false"),
            );
            None
        }
    }
}

fn build_step(ctx: &mut BuildCtx, item: &Node, path: &str) -> Option<Step> {
    match &item.kind {
        NodeKind::Scalar { raw, .. } => match raw.as_str() {
            "str_app" => Some(Step::StrApp),
            "cls_app" => Some(Step::ClsApp),
            "throw" => Some(Step::Throw { message: None }),
            other => {
                ctx.push(
                    codes::STEP_UNKNOWN_ACTION,
                    path,
                    "",
                    format!("裸标量步骤只能是 str_app/cls_app/throw，得到 {other:?}"),
                );
                None
            }
        },
        NodeKind::Seq(_) => {
            ctx.push(codes::STEP_FIELD_TYPE_MISMATCH, path, "", "步骤不能是列表");
            None
        }
        NodeKind::Map(entries) => build_map_step(ctx, entries, path),
    }
}

fn build_map_step(ctx: &mut BuildCtx, entries: &[MapEntry], path: &str) -> Option<Step> {
    let mut action: Option<(&str, &Node)> = None;
    let mut action_count = 0usize;
    for e in entries {
        if ACTION_KEYS.contains(&e.key.as_str()) {
            action_count += 1;
            action = Some((e.key.as_str(), &e.value));
        }
    }
    if action_count > 1 {
        let second = entries
            .iter()
            .filter(|e| ACTION_KEYS.contains(&e.key.as_str()))
            .nth(1)
            .map(|e| e.key.clone())
            .unwrap_or_default();
        ctx.push(
            codes::STEP_MULTI_ACTION,
            path,
            &second,
            format!("一个步骤只能有一个动作键，发现第二个动作键 {second:?}"),
        );
        return None;
    }
    let Some((action, value)) = action else {
        let keys: Vec<&str> = entries.iter().map(|e| e.key.as_str()).collect();
        ctx.push(
            codes::STEP_UNKNOWN_ACTION,
            path,
            "",
            format!("步骤缺少动作键（十七类之一），现有键 {keys:?}"),
        );
        return None;
    };
    // 兄弟键分区：已知字段 / 未知字段（未知字段报错并忽略）。
    let known: &[&str] = match action {
        "find" => &["block", "verify", "timeout", "then", "else"],
        "match" => &["else", "timeout"],
        "color" => &["else"],
        "if" => &["then", "else"],
        "call" => &["args"],
        "func" => &["args", "then", "else"],
        _ => &[],
    };
    for e in entries {
        if ACTION_KEYS.contains(&e.key.as_str()) || known.contains(&e.key.as_str()) {
            continue;
        }
        ctx.push(
            codes::STEP_FIELD_UNKNOWN,
            path,
            &e.key,
            format!(
                "动作 {action} 不支持字段 {:?}（允许：{}）",
                e.key,
                known.join("/")
            ),
        );
    }
    match action {
        "str_app" | "cls_app" => {
            ctx.push(
                codes::STEP_FIELD_TYPE_MISMATCH,
                path,
                "",
                format!("{action} 必须裸写（- {action}），不接受值"),
            );
            None
        }
        "tap" => {
            let at = build_cell(ctx, value, path, "at", Exp::Coord)?;
            Some(Step::Tap { at })
        }
        "swipe" => {
            let Some(m) = value.as_map() else {
                ctx.push(
                    codes::STEP_FIELD_TYPE_MISMATCH,
                    path,
                    "swipe",
                    "swipe 值必须是映射（fm/to/time）",
                );
                return None;
            };
            let get = |k: &str| m.iter().find(|e| e.key == k).map(|e| &e.value);
            for e in m {
                if !matches!(e.key.as_str(), "fm" | "to" | "time") {
                    ctx.push(
                        codes::STEP_FIELD_UNKNOWN,
                        path,
                        &e.key,
                        format!("swipe 不支持字段 {:?}（允许：fm/to/time）", e.key),
                    );
                }
            }
            let (Some(fm), Some(to), Some(time)) = (get("fm"), get("to"), get("time")) else {
                for req in ["fm", "to", "time"] {
                    if get(req).is_none() {
                        ctx.push(
                            codes::STEP_FIELD_MISSING,
                            path,
                            req,
                            format!("swipe 缺少必需字段 {req}"),
                        );
                    }
                }
                return None;
            };
            let from = build_cell(ctx, fm, path, "from", Exp::Coord)?;
            let to = build_cell(ctx, to, path, "to", Exp::Coord)?;
            let time = build_cell(ctx, time, path, "time", Exp::Time)?;
            Some(Step::Swipe { from, to, time })
        }
        "key" => {
            let key = build_cell(ctx, value, path, "key", Exp::Key)?;
            Some(Step::Key { key })
        }
        "text" => {
            let v = build_cell(ctx, value, path, "value", Exp::Text)?;
            Some(Step::Text { value: v })
        }
        "log" => {
            let message = build_cell(ctx, value, path, "message", Exp::Text)?;
            Some(Step::Log { message })
        }
        "wait" => {
            let (duration, duration_max) = match &value.kind {
                NodeKind::Scalar { .. } => {
                    let d = build_cell(ctx, value, path, "duration", Exp::Time)?;
                    (d, None)
                }
                NodeKind::Seq(items) if items.len() == 2 => {
                    let a = build_cell(ctx, &items[0], path, "duration", Exp::Time)?;
                    let b = build_cell(ctx, &items[1], path, "duration_max", Exp::Time)?;
                    // 随机区间起点不得大于终点。
                    if let (Cell::Lit(TypedValue::Time(a)), Cell::Lit(TypedValue::Time(b))) =
                        (&a, &b)
                    {
                        if let (Some(x), Some(y)) =
                            (params::parse_time_ms(a), params::parse_time_ms(b))
                        {
                            if x > y {
                                ctx.push(
                                    codes::STEP_WAIT_RANGE_INVALID,
                                    path,
                                    "duration",
                                    format!("wait 随机区间起点 {a} 大于终点 {b}"),
                                );
                                return None;
                            }
                        }
                    }
                    (a, Some(b))
                }
                _ => {
                    ctx.push(
                        codes::STEP_FIELD_TYPE_MISMATCH,
                        path,
                        "duration",
                        "wait 值必须是时长或 [起, 止] 区间",
                    );
                    return None;
                }
            };
            Some(Step::Wait {
                duration,
                duration_max,
            })
        }
        "find" => {
            let template = build_cell(ctx, value, path, "template", Exp::Tmpl)?;
            let block = match lookup(entries, "block") {
                Some(n) => match n.as_seq() {
                    Some(items) => items
                        .iter()
                        .enumerate()
                        .filter_map(|(i, it)| {
                            build_cell(ctx, it, &format!("{path}.block[{i}]"), "block", Exp::Tmpl)
                        })
                        .collect(),
                    None => {
                        ctx.push(
                            codes::STEP_FIELD_TYPE_MISMATCH,
                            path,
                            "block",
                            "find.block 必须是模板列表",
                        );
                        Vec::new()
                    }
                },
                None => Vec::new(),
            };
            let verify = match lookup(entries, "verify") {
                Some(n) => build_bool_field(ctx, n, path, "verify")?,
                None => false,
            };
            let timeout = match lookup(entries, "timeout") {
                Some(n) => Some(build_cell(ctx, n, path, "timeout", Exp::Time)?),
                None => None,
            };
            let then = branch_steps(ctx, entries, path, "then");
            let r#else = branch_steps(ctx, entries, path, "else");
            Some(Step::Find {
                template,
                block,
                verify,
                timeout,
                then,
                r#else,
            })
        }
        "match" => build_match_step(ctx, value, entries, path),
        "color" => build_color_step(ctx, value, entries, path),
        "if" => {
            let cond = build_cell(ctx, value, path, "cond", Exp::Bool)?;
            let then = branch_steps(ctx, entries, path, "then");
            let r#else = branch_steps(ctx, entries, path, "else");
            Some(Step::If { cond, then, r#else })
        }
        "loop" => {
            let Some(m) = value.as_map() else {
                ctx.push(
                    codes::STEP_FIELD_TYPE_MISMATCH,
                    path,
                    "loop",
                    "loop 值必须是映射（times/steps）",
                );
                return None;
            };
            for e in m {
                if !matches!(e.key.as_str(), "times" | "steps") {
                    ctx.push(
                        codes::STEP_FIELD_UNKNOWN,
                        path,
                        &e.key,
                        format!("loop 不支持字段 {:?}（允许：times/steps）", e.key),
                    );
                }
            }
            let times = match m.iter().find(|e| e.key == "times").map(|e| &e.value) {
                Some(n) => match n.as_scalar() {
                    Some((raw, ScalarStyle::Plain)) => match raw.parse::<u64>() {
                        Ok(x) => Some(x),
                        Err(_) => {
                            ctx.push(
                                codes::STEP_FIELD_TYPE_MISMATCH,
                                path,
                                "times",
                                format!("loop.times {raw:?} 不是非负整数"),
                            );
                            return None;
                        }
                    },
                    _ => {
                        ctx.push(
                            codes::STEP_FIELD_TYPE_MISMATCH,
                            path,
                            "times",
                            "loop.times 必须是非负整数字面量",
                        );
                        return None;
                    }
                },
                None => None,
            };
            let Some(steps_node) = m.iter().find(|e| e.key == "steps").map(|e| &e.value) else {
                ctx.push(codes::STEP_FIELD_MISSING, path, "steps", "loop 缺少 steps");
                return None;
            };
            if steps_node.as_seq().is_some_and(|s| s.is_empty()) {
                ctx.push(
                    codes::STEP_LOOP_EMPTY_STEPS,
                    path,
                    "steps",
                    "loop 子流程为空",
                );
                return None;
            }
            let steps = build_steps(ctx, steps_node, &format!("{path}.steps"));
            Some(Step::Loop { times, steps })
        }
        "call" => {
            let target = scalar_target(ctx, value, path, "call")?;
            let args = build_args(ctx, lookup(entries, "args"), path);
            Some(Step::Call { target, args })
        }
        "func" => {
            let target = scalar_target(ctx, value, path, "func")?;
            let args = build_args(ctx, lookup(entries, "args"), path);
            let then = branch_steps(ctx, entries, path, "then");
            let r#else = branch_steps(ctx, entries, path, "else");
            Some(Step::Func {
                target,
                args,
                then,
                r#else,
            })
        }
        "throw" => {
            let message = match value.as_scalar() {
                Some(("", _)) => None,
                Some((raw, _)) => Some(raw.to_string()),
                None => {
                    ctx.push(
                        codes::STEP_FIELD_TYPE_MISMATCH,
                        path,
                        "message",
                        "throw 值必须是字符串或裸写",
                    );
                    return None;
                }
            };
            Some(Step::Throw { message })
        }
        "return" => {
            if ctx.kind == FileKind::Script {
                ctx.push(
                    codes::STEP_RETURN_IN_SCRIPT,
                    path,
                    "",
                    "return 只能出现在函数文件（func/）中",
                );
                return None;
            }
            let v = build_cell(ctx, value, path, "value", Exp::Bool)?;
            Some(Step::Return { value: v })
        }
        other => {
            ctx.push(
                codes::STEP_UNKNOWN_ACTION,
                path,
                "",
                format!("未知动作 {other:?}"),
            );
            None
        }
    }
}

/// match 紧凑缩进候选（CONTRACT §4.1）：候选列表是 match 键下的无缩进序列，
/// else/timeout 是步骤兄弟键，绝不接受 `- else:` / `- timeout:` 写进候选列表。
fn build_match_step(
    ctx: &mut BuildCtx,
    value: &Node,
    entries: &[MapEntry],
    path: &str,
) -> Option<Step> {
    let Some(cands) = value.as_seq() else {
        ctx.push(
            codes::STEP_MATCH_CANDIDATES_TYPE,
            path,
            "candidates",
            "match 值必须是候选列表（紧凑缩进：候选序列与 match 键同列）",
        );
        return None;
    };
    let mut candidates = Vec::new();
    for (i, c) in cands.iter().enumerate() {
        let steps_path = format!("{path}.candidates[{i}].steps");
        let Some(m) = c.as_map() else {
            ctx.push(
                codes::STEP_FIELD_TYPE_MISMATCH,
                path,
                "candidates",
                format!("候选 {i} 必须是单键映射 模板: [分支步骤]"),
            );
            continue;
        };
        if let Some(e) = m
            .iter()
            .find(|e| matches!(e.key.as_str(), "else" | "timeout"))
        {
            ctx.push(
                codes::STEP_MATCH_ELSE_IN_CANDIDATES,
                path,
                "candidates",
                format!(
                    "{:?} 写进了候选列表；else/timeout 必须是 match 步骤的兄弟键（@ {}）",
                    e.key,
                    e.key_loc_display()
                ),
            );
            continue;
        }
        if m.len() != 1 {
            ctx.push(
                codes::STEP_FIELD_TYPE_MISMATCH,
                path,
                "candidates",
                format!("候选 {i} 必须是单键映射，得到 {} 个键", m.len()),
            );
            continue;
        }
        let entry = &m[0];
        // 候选模板键：$name 引用或模板短名字符串。
        let template = if let Some(name) = entry.key.strip_prefix('$').filter(|n| !n.is_empty()) {
            Cell::Ref(name.to_string())
        } else if entry.key.is_empty() {
            ctx.push(
                codes::STEP_FIELD_TYPE_MISMATCH,
                path,
                "candidates",
                "候选模板名不能为空",
            );
            continue;
        } else {
            Cell::Lit(TypedValue::Tmpl(entry.key.clone()))
        };
        let steps = build_steps(ctx, &entry.value, &steps_path);
        candidates.push(MatchCandidate { template, steps });
    }
    let timeout = match lookup(entries, "timeout") {
        Some(n) => Some(build_cell(ctx, n, path, "timeout", Exp::Time)?),
        None => None,
    };
    let r#else = branch_steps(ctx, entries, path, "else");
    Some(Step::Match {
        candidates,
        r#else,
        timeout,
    })
}

/// color 候选（CONTRACT §4.2）：expect 为有序列表（每项单键映射 颜色: [分支步骤]），
/// 不用颜色做整个映射的键（JS 端整数形键会丢顺序）；else 是步骤兄弟键。
fn build_color_step(
    ctx: &mut BuildCtx,
    value: &Node,
    entries: &[MapEntry],
    path: &str,
) -> Option<Step> {
    let Some(m) = value.as_map() else {
        ctx.push(
            codes::STEP_FIELD_TYPE_MISMATCH,
            path,
            "color",
            "color 值必须是映射（at/expect）",
        );
        return None;
    };
    for e in m {
        if !matches!(e.key.as_str(), "at" | "expect") {
            ctx.push(
                codes::STEP_FIELD_UNKNOWN,
                path,
                &e.key,
                format!("color 不支持字段 {:?}（else 是步骤兄弟键）", e.key),
            );
        }
    }
    let Some(at_node) = m.iter().find(|e| e.key == "at").map(|e| &e.value) else {
        ctx.push(codes::STEP_FIELD_MISSING, path, "at", "color 缺少 at");
        return None;
    };
    let Some(expect_node) = m.iter().find(|e| e.key == "expect").map(|e| &e.value) else {
        ctx.push(
            codes::STEP_FIELD_MISSING,
            path,
            "expect",
            "color 缺少 expect（有序颜色候选列表）",
        );
        return None;
    };
    let at = build_cell(ctx, at_node, path, "at", Exp::Coord)?;
    let Some(items) = expect_node.as_seq() else {
        ctx.push(
            codes::STEP_FIELD_TYPE_MISMATCH,
            path,
            "expect",
            "color.expect 必须是有序列表（每项单键映射 颜色: [分支步骤]，不用颜色做映射键）",
        );
        return None;
    };
    let mut expect = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let steps_path = format!("{path}.expect[{i}].steps");
        let Some(em) = item.as_map() else {
            ctx.push(
                codes::STEP_FIELD_TYPE_MISMATCH,
                path,
                "expect",
                format!("expect 候选 {i} 必须是单键映射"),
            );
            continue;
        };
        if em.len() != 1 {
            ctx.push(
                codes::STEP_FIELD_TYPE_MISMATCH,
                path,
                "expect",
                format!("expect 候选 {i} 必须是单键映射，得到 {} 个键", em.len()),
            );
            continue;
        }
        let entry = &em[0];
        let color = if let Some(name) = entry.key.strip_prefix('$').filter(|n| !n.is_empty()) {
            Cell::Ref(name.to_string())
        } else if !super::params::is_valid_color(&entry.key) {
            ctx.push(
                codes::STEP_COLOR_FORMAT,
                path,
                "expect",
                format!(
                    "颜色 {:?} 不是 6 位十六进制（@ {}）",
                    entry.key,
                    entry.key_loc_display()
                ),
            );
            continue;
        } else {
            Cell::Lit(TypedValue::Color(entry.key.clone()))
        };
        let steps = build_steps(ctx, &entry.value, &steps_path);
        expect.push(ColorBranch { color, steps });
    }
    let r#else = branch_steps(ctx, entries, path, "else");
    Some(Step::Color { at, expect, r#else })
}

/// 兄弟键查找。
fn lookup<'a>(entries: &'a [MapEntry], name: &str) -> Option<&'a Node> {
    entries.iter().find(|e| e.key == name).map(|e| &e.value)
}

/// 兄弟分支键（then/else/steps）→ 子步骤列表（缺省为空）。
fn branch_steps(ctx: &mut BuildCtx, entries: &[MapEntry], path: &str, key: &str) -> Vec<Step> {
    match lookup(entries, key) {
        Some(n) => build_steps(ctx, n, &format!("{path}.{key}")),
        None => Vec::new(),
    }
}

fn scalar_target(ctx: &mut BuildCtx, value: &Node, path: &str, action: &str) -> Option<String> {
    match value.as_scalar() {
        Some((raw, _)) if !raw.is_empty() => Some(raw.to_string()),
        _ => {
            ctx.push(
                codes::STEP_FIELD_TYPE_MISMATCH,
                path,
                "target",
                format!("{action} 目标必须是非空字符串"),
            );
            None
        }
    }
}

fn build_args(ctx: &mut BuildCtx, node: Option<&Node>, path: &str) -> Vec<ArgAssign> {
    let Some(node) = node else {
        return Vec::new();
    };
    let Some(entries) = node.as_map() else {
        ctx.push(
            codes::STEP_FIELD_TYPE_MISMATCH,
            path,
            "args",
            "args 必须是具名映射",
        );
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in entries {
        // 实参单元格：$name 引用 / true|false / [x, y] / 其余标量按文本；
        // 标量的引号样式随 ArgAssign.quoted 保留，供规范序列化原样回写。
        let (cell, quoted) = match &e.value.kind {
            NodeKind::Scalar { raw, style } => {
                if let Some(name) = raw.strip_prefix('$').filter(|n| !n.is_empty()) {
                    (Cell::Ref(name.to_string()), false)
                } else if *style == ScalarStyle::Plain && matches!(raw.as_str(), "true" | "false") {
                    (Cell::Lit(TypedValue::Bool(raw == "true")), false)
                } else if raw.is_empty() {
                    (Cell::Lit(TypedValue::Text(String::new())), false)
                } else {
                    (
                        Cell::Lit(TypedValue::Text(raw.clone())),
                        *style != ScalarStyle::Plain,
                    )
                }
            }
            NodeKind::Seq(items) if items.len() == 2 => {
                let mut nums = [0.0f64; 2];
                let mut ok = true;
                for (idx, it) in items.iter().enumerate() {
                    match it.as_scalar().and_then(|(raw, _)| raw.parse::<f64>().ok()) {
                        Some(x) => nums[idx] = x,
                        None => {
                            ctx.push(
                                codes::STEP_FIELD_TYPE_MISMATCH,
                                path,
                                "args",
                                format!("args[{}] 坐标分量非法", e.key),
                            );
                            ok = false;
                            break;
                        }
                    }
                }
                if !ok {
                    continue;
                }
                if !params::coord_in_range(nums[0]) || !params::coord_in_range(nums[1]) {
                    ctx.push(
                        codes::STEP_COORD_RANGE,
                        path,
                        "args",
                        format!("args[{}] 坐标超出 0~1", e.key),
                    );
                    continue;
                }
                (Cell::Lit(TypedValue::Coord(nums)), false)
            }
            NodeKind::Seq(_) | NodeKind::Map(_) => {
                ctx.push(
                    codes::STEP_FIELD_TYPE_MISMATCH,
                    path,
                    "args",
                    format!("args[{}] 必须是标量或 [x, y]", e.key),
                );
                continue;
            }
        };
        out.push(ArgAssign {
            name: e.key.clone(),
            value: cell,
            quoted,
        });
    }
    out
}

impl MapEntry {
    fn key_loc_display(&self) -> String {
        format!(
            "行 {} 列 {}",
            self.key_span.start.line(),
            self.key_span.start.col()
        )
    }
}
