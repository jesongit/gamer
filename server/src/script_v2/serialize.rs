//! AST → 规范 YAML 序列化（CONTRACT §3「规范 YAML」列 + fixture 规范形态）。
//!
//! 满足：`serialize(parse(fixture)) == fixture` 逐字节、
//! `parse(serialize(parse(x))) == parse(x)` 深等、二次序列化字节稳定。
//! 关键规范形态：match 候选为 indentless 序列（§4.1）、params 整条单引号、
//! color 候选键纯数字色值加单引号（§4.2）、text 字面量统一双引号、
//! wait 区间为 flow 序列、空分支/默认字段省略。

use super::model::{ArgAssign, Cell, FunctionFile, ScriptFile, Step, TypedValue};
use super::params;

pub fn serialize_script(file: &ScriptFile) -> String {
    let mut out = String::new();
    if !file.params.is_empty() {
        out.push_str("params:\n");
        for p in &file.params {
            out.push_str("  - ");
            out.push_str(&quote_single(&decl_string(p)));
            out.push('\n');
        }
    }
    if let Some(cfg) = &file.config {
        out.push_str("config:\n");
        out.push_str(&format!(
            "  interval: {}\n",
            params::fmt_duration(&cfg.interval)
        ));
        out.push_str(&format!(
            "  threshold: {}\n",
            params::fmt_num(cfg.threshold)
        ));
        out.push_str(&format!("  log_level: {}\n", cfg.log_level.as_str()));
    }
    if file.steps.is_empty() {
        out.push_str("steps: []\n");
    } else {
        out.push_str("steps:\n");
        write_steps(&mut out, &file.steps, 2);
    }
    out
}

pub fn serialize_function_file(file: &FunctionFile) -> String {
    let mut out = String::new();
    for (i, func) in file.functions.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&func.name);
        out.push_str(":\n");
        if !func.params.is_empty() {
            out.push_str("  params:\n");
            for p in &func.params {
                out.push_str("    - ");
                out.push_str(&quote_single(&decl_string(p)));
                out.push('\n');
            }
        }
        if func.steps.is_empty() {
            out.push_str("  steps: []\n");
        } else {
            out.push_str("  steps:\n");
            write_steps(&mut out, &func.steps, 4);
        }
    }
    out
}

/// `类型:变量名:备注[:默认值]`（第 4 段按契约规则 7 输出）。
fn decl_string(p: &super::model::ParamDecl) -> String {
    let mut s = format!("{}:{}:{}", p.ty.as_str(), p.name, p.remark);
    if let Some(v) = &p.default {
        s.push(':');
        s.push_str(&match v {
            TypedValue::Text(text) => format!("\"{}\"", params::escape_double_quoted(text)),
            TypedValue::Coord([x, y]) => {
                format!("[{}, {}]", params::fmt_num(*x), params::fmt_num(*y))
            }
            TypedValue::Bool(b) => b.to_string(),
            // color/time/key/tmpl 原样输出（CONTRACT §3.3 规则 7）。
            TypedValue::Color(c)
            | TypedValue::Time(c)
            | TypedValue::Key(c)
            | TypedValue::Tmpl(c) => c.clone(),
        });
    }
    s
}

// ---------------------------------------------------------------------------
// 步骤
// ---------------------------------------------------------------------------

fn write_steps(out: &mut String, steps: &[Step], indent: usize) {
    for step in steps {
        push_indent(out, indent);
        out.push_str("- ");
        write_step(out, step, indent);
    }
}

/// 分支键（then/else/steps 等）：与动作键同列（indent+2），子步骤再进 2。
fn write_branch(out: &mut String, key: &str, steps: &[Step], indent: usize) {
    if steps.is_empty() {
        return;
    }
    push_indent(out, indent + 2);
    out.push_str(key);
    out.push_str(":\n");
    write_steps(out, steps, indent + 4);
}

fn write_step(out: &mut String, step: &Step, indent: usize) {
    match step {
        Step::StrApp => out.push_str("str_app\n"),
        Step::ClsApp => out.push_str("cls_app\n"),
        Step::Tap { at } => {
            out.push_str(&format!("tap: {}\n", render_cell(at)));
        }
        Step::Swipe { from, to, time } => {
            out.push_str("swipe:\n");
            push_indent(out, indent + 4);
            out.push_str(&format!("fm: {}\n", render_cell(from)));
            push_indent(out, indent + 4);
            out.push_str(&format!("to: {}\n", render_cell(to)));
            push_indent(out, indent + 4);
            out.push_str(&format!("time: {}\n", render_cell(time)));
        }
        Step::Key { key } => out.push_str(&format!("key: {}\n", render_cell(key))),
        Step::Text { value } => out.push_str(&format!("text: {}\n", render_cell(value))),
        // log 消息按 plain 安全规则输出（fixture 规范形态：中文日志裸写）。
        Step::Log { message } => out.push_str(&format!("log: {}\n", render_log_message(message))),
        Step::Wait {
            duration,
            duration_max,
        } => match duration_max {
            None => out.push_str(&format!("wait: {}\n", render_cell(duration))),
            Some(max) => out.push_str(&format!(
                "wait: [{}, {}]\n",
                render_cell(duration),
                render_cell(max)
            )),
        },
        Step::Find {
            template,
            block,
            verify,
            timeout,
            then,
            r#else,
        } => {
            out.push_str(&format!("find: {}\n", render_cell(template)));
            if !block.is_empty() {
                push_indent(out, indent + 2);
                out.push_str("block:\n");
                for b in block {
                    push_indent(out, indent + 4);
                    out.push_str(&format!("- {}\n", render_cell(b)));
                }
            }
            if *verify {
                push_indent(out, indent + 2);
                out.push_str("verify: true\n");
            }
            if let Some(t) = timeout {
                push_indent(out, indent + 2);
                out.push_str(&format!("timeout: {}\n", render_cell(t)));
            }
            write_branch(out, "then", then, indent);
            write_branch(out, "else", r#else, indent);
        }
        Step::Match {
            candidates,
            r#else,
            timeout,
        } => {
            out.push_str("match:\n");
            let items: Vec<CandidateOut<'_>> = candidates
                .iter()
                .map(|c| CandidateOut {
                    key: render_cell(&c.template),
                    click: c.click,
                    steps: &c.steps,
                })
                .collect();
            write_candidates(out, &items, indent + 2);
            write_branch(out, "else", r#else, indent);
            if let Some(t) = timeout {
                push_indent(out, indent + 2);
                out.push_str(&format!("timeout: {}\n", render_cell(t)));
            }
        }
        Step::Check { template, r#throw } => {
            out.push_str(&format!("check: {}\n", render_cell(template)));
            push_indent(out, indent + 2);
            out.push_str(&format!("throw: {}\n", render_plain(r#throw)));
        }
        Step::Color { at, expect, r#else } => {
            out.push_str("color:\n");
            push_indent(out, indent + 4);
            out.push_str(&format!("at: {}\n", render_cell(at)));
            if !expect.is_empty() {
                push_indent(out, indent + 4);
                out.push_str("expect:\n");
                let items: Vec<CandidateOut<'_>> = expect
                    .iter()
                    .map(|e| CandidateOut {
                        key: render_color_key(&e.color),
                        click: e.click,
                        steps: &e.steps,
                    })
                    .collect();
                write_candidates(out, &items, indent + 6);
            }
            write_branch(out, "else", r#else, indent);
        }
        Step::If { cond, then, r#else } => {
            out.push_str(&format!("if: {}\n", render_cell(cond)));
            write_branch(out, "then", then, indent);
            write_branch(out, "else", r#else, indent);
        }
        Step::Loop { times, steps } => {
            out.push_str("loop:\n");
            if let Some(n) = times {
                push_indent(out, indent + 4);
                out.push_str(&format!("times: {n}\n"));
            }
            push_indent(out, indent + 4);
            if steps.is_empty() {
                out.push_str("steps: []\n");
            } else {
                out.push_str("steps:\n");
                write_steps(out, steps, indent + 6);
            }
        }
        Step::Call { target, args } => {
            out.push_str(&format!("call: {}\n", render_plain(target)));
            write_args(out, args, indent);
        }
        Step::Func {
            target,
            args,
            then,
            r#else,
        } => {
            out.push_str(&format!("func: {}\n", render_plain(target)));
            write_args(out, args, indent);
            write_branch(out, "then", then, indent);
            write_branch(out, "else", r#else, indent);
        }
        Step::Throw { message } => match message {
            None => out.push_str("throw\n"),
            Some(m) => out.push_str(&format!("throw: {}\n", render_plain(m))),
        },
        Step::Return { value } => out.push_str(&format!("return: {}\n", render_cell(value))),
    }
}

/// 候选列表项：`- 键:` + 分支步骤（键下无缩进序列，键列 = 列表项键列）。
/// `click: true` 时写映射形态 `{click: true, steps: [...]}`（steps 空则省略），
/// false 保持列表形态——规范不变式：列表 ⇔ 不点击，映射 ⇔ 点击。
/// 映射键比候选模板键深两级（YAML 映射值不能与键同列，序列才能同列）。
struct CandidateOut<'a> {
    key: String,
    click: bool,
    steps: &'a [Step],
}

fn write_candidates(out: &mut String, items: &[CandidateOut<'_>], dash_indent: usize) {
    for item in items {
        push_indent(out, dash_indent);
        out.push_str("- ");
        out.push_str(&item.key);
        out.push_str(":\n");
        if !item.click {
            if item.steps.is_empty() {
                push_indent(out, dash_indent + 2);
                out.push_str("[]\n");
            } else {
                write_steps(out, item.steps, dash_indent + 2);
            }
        } else {
            push_indent(out, dash_indent + 4);
            out.push_str("click: true\n");
            if !item.steps.is_empty() {
                push_indent(out, dash_indent + 4);
                out.push_str("steps:\n");
                write_steps(out, item.steps, dash_indent + 6);
            }
        }
    }
}

fn write_args(out: &mut String, args: &[ArgAssign], indent: usize) {
    if args.is_empty() {
        return;
    }
    push_indent(out, indent + 2);
    out.push_str("args:\n");
    for a in args {
        push_indent(out, indent + 4);
        out.push_str(&format!("{}: {}\n", render_plain(&a.name), render_arg(a)));
    }
}

/// args 实参渲染：引用原样；文本实参按源引号样式回写；其余按变体规则。
fn render_arg(a: &ArgAssign) -> String {
    match &a.value {
        Cell::Ref(name) => format!("${name}"),
        Cell::Lit(TypedValue::Text(s)) if a.quoted => {
            format!("\"{}\"", params::escape_double_quoted(s))
        }
        Cell::Lit(TypedValue::Text(s)) => render_plain(s),
        Cell::Lit(other) => render_typed(other),
    }
}

// ---------------------------------------------------------------------------
// 标量渲染
// ---------------------------------------------------------------------------

/// 取值单元格：$name 引用原样；字面量按变体（text 统一双引号，color 纯数字加单引号）。
fn render_cell(cell: &Cell) -> String {
    match cell {
        Cell::Ref(name) => format!("${name}"),
        Cell::Lit(v) => render_typed(v),
    }
}

fn render_typed(v: &TypedValue) -> String {
    match v {
        TypedValue::Tmpl(s) | TypedValue::Key(s) | TypedValue::Time(s) => render_plain(s),
        // 纯数字色值必须加引号防止被解析成数字丢前导零（CONTRACT §4.2）。
        TypedValue::Color(s) => {
            if s.chars().all(|c| c.is_ascii_digit()) {
                quote_single(s)
            } else {
                render_plain(s)
            }
        }
        TypedValue::Text(s) => format!("\"{}\"", params::escape_double_quoted(s)),
        TypedValue::Coord([x, y]) => {
            format!("[{}, {}]", params::fmt_num(*x), params::fmt_num(*y))
        }
        TypedValue::Bool(b) => b.to_string(),
    }
}

/// color 候选键：与 render_typed 的 Color 分支同规则。
fn render_color_key(cell: &Cell) -> String {
    render_cell(cell)
}

/// log 消息：引用原样；文本字面量按 plain 安全规则（不安全才退双引号）。
fn render_log_message(cell: &Cell) -> String {
    match cell {
        Cell::Ref(name) => format!("${name}"),
        Cell::Lit(TypedValue::Text(s)) => render_plain(s),
        Cell::Lit(other) => render_typed(other),
    }
}

/// 优先 plain 标量（不安全时退双引号）。
fn render_plain(s: &str) -> String {
    if plain_safe(s) {
        s.to_string()
    } else {
        format!("\"{}\"", params::escape_double_quoted(s))
    }
}

/// plain 标量安全性：指示符开头、`: `、` #`、首尾空白、控制字符、以及
/// 会被 YAML 1.2 core 解析成非字符串的词（数字/bool/null）一律视为不安全。
fn plain_safe(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let first = s.chars().next().unwrap();
    if "-?:,[]{}#&*!|>'\"%@`".contains(first) {
        return false;
    }
    if s.starts_with(' ') || s.starts_with('\t') || s.ends_with(' ') || s.ends_with(':') {
        return false;
    }
    if s.contains(": ")
        || s.contains(" #")
        || s.chars().any(|c| c == '\t' || c == '\n' || c == '\r')
    {
        return false;
    }
    // 数字 / bool / null 形态的字符串需引号（前端 js-yaml 会改变节点类型）。
    if s.parse::<f64>().is_ok() || matches!(s, "true" | "false" | "null" | "~") {
        return false;
    }
    true
}

fn quote_single(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn push_indent(out: &mut String, n: usize) {
    for _ in 0..n {
        out.push(' ');
    }
}
