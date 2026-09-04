//! 语义层校验（plan §13.2 分层的 3~6 层）：类型化引用存在性、静态重复、
//! call/func 跨文件引用图（环 + 深度）、模板资源存在性。
//!
//! 结构层（顶层键/声明/步骤形态）在 loader.rs；两层通过 mod.rs 的
//! parse_script_file/parse_function_file 串联。

use std::collections::{HashMap, HashSet};

use super::error::codes;
use super::error::ScriptError;
use super::loader::{build_function_file, build_script_file, load, BuildCtx, FileKind};
use super::model::{
    ArgAssign, Cell, FunctionFile, ParamDecl, ParamType, ScriptFile, Step, TypedValue,
};
use super::params;

/// 静态深度上限：调用链（call/func 混合计）与步骤嵌套共用 32 层。
pub(crate) const MAX_DEPTH: usize = 32;

// ---------------------------------------------------------------------------
// 资源供给
// ---------------------------------------------------------------------------

/// 模板短名解析结果（同短名多个 `#` 后缀候选 = 歧义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateAvail {
    NotFound,
    Found,
    Ambiguous,
}

/// 分区内资源供给：loader/validator 只依赖该最小接口；阶段 2 由
/// scripts::ScriptStore 实现真实文件版（yaml/ func/ tmpl/）。
pub trait ResourceProvider {
    /// `yaml/` 下脚本是否存在（id 为 call 目标书写形式，如 `sub/inner.yaml`）。
    fn script_exists(&self, resource_id: &str) -> bool;
    /// 读取脚本源码。
    fn script_content(&self, resource_id: &str) -> Option<String>;
    /// 读取函数文件（短路径，如 `common`）；`None` = 文件不存在。
    fn function_file_content(&self, file_short: &str) -> Option<String>;
    /// 函数文件中是否存在该函数。
    fn function_exists(&self, file_short: &str, function: &str) -> bool;
    /// 模板短名在当前分区的可用性（唯一存在 / 缺失 / 歧义）。
    fn resolve_template(&self, short_name: &str) -> TemplateAvail;
}

/// 内存实现：测试与阶段 2 存储接入前的过渡共用。
#[derive(Debug, Default, Clone)]
pub struct InMemoryResources {
    scripts: HashMap<String, String>,
    function_files: HashMap<String, String>,
    templates: HashSet<String>,
}

impl InMemoryResources {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_script(&mut self, resource_id: impl Into<String>, content: impl Into<String>) {
        self.scripts.insert(resource_id.into(), content.into());
    }

    pub fn add_function_file(&mut self, file_short: impl Into<String>, content: impl Into<String>) {
        self.function_files
            .insert(file_short.into(), content.into());
    }

    pub fn add_template(&mut self, short_name: impl Into<String>) {
        self.templates.insert(short_name.into());
    }
}

impl ResourceProvider for InMemoryResources {
    fn script_exists(&self, resource_id: &str) -> bool {
        self.scripts.contains_key(resource_id)
    }

    fn script_content(&self, resource_id: &str) -> Option<String> {
        self.scripts.get(resource_id).cloned()
    }

    fn function_file_content(&self, file_short: &str) -> Option<String> {
        self.function_files.get(file_short).cloned()
    }

    fn function_exists(&self, file_short: &str, function: &str) -> bool {
        self.function_file_content(file_short)
            .and_then(|content| try_build_function_file(&content))
            .is_some_and(|f| f.find(function).is_some())
    }

    fn resolve_template(&self, short_name: &str) -> TemplateAvail {
        if self.templates.contains(short_name) {
            TemplateAvail::Found
        } else {
            TemplateAvail::NotFound
        }
    }
}

// ---------------------------------------------------------------------------
// 宽松构建（引用图遍历用：被引用资源自身的错误由其自己的校验负责）
// ---------------------------------------------------------------------------

pub(crate) fn try_build_script(content: &str) -> Option<ScriptFile> {
    let node = load(content).ok()?;
    let mut ctx = BuildCtx::new("", FileKind::Script);
    let file = build_script_file(&mut ctx, &node)?;
    if ctx.errors.is_empty() {
        Some(file)
    } else {
        None
    }
}

pub(crate) fn try_build_function_file(content: &str) -> Option<FunctionFile> {
    let node = load(content).ok()?;
    let mut ctx = BuildCtx::new("", FileKind::FunctionLibrary);
    let file = build_function_file(&mut ctx, &node)?;
    if ctx.errors.is_empty() {
        Some(file)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// 校验入口
// ---------------------------------------------------------------------------

pub(crate) fn validate_script_file(
    file: &ScriptFile,
    resource: &str,
    provider: &dyn ResourceProvider,
) -> Vec<ScriptError> {
    let mut v = Validator::new(resource, FileKind::Script, provider, &file.params);
    v.walk_steps(&file.steps, "steps", 1, 0);
    v.walk_graph(GraphEntry::Script(resource.to_string(), file));
    v.errors
}

pub(crate) fn validate_function_file(
    file: &FunctionFile,
    resource: &str,
    provider: &dyn ResourceProvider,
) -> Vec<ScriptError> {
    let mut errors = Vec::new();
    for func in &file.functions {
        let mut v = Validator::new(resource, FileKind::FunctionLibrary, provider, &func.params);
        v.walk_steps(&func.steps, &format!("{}.steps", func.name), 1, 0);
        errors.append(&mut v.errors);
    }
    // 跨文件 func/call 引用图以文件为节点，统一走一遍。
    let mut v = Validator::new(resource, FileKind::FunctionLibrary, provider, &[]);
    v.walk_graph(GraphEntry::Function(resource.to_string(), file));
    errors.append(&mut v.errors);
    errors
}

struct Validator<'a> {
    resource: String,
    kind: FileKind,
    provider: &'a dyn ResourceProvider,
    scope: &'a [ParamDecl],
    errors: Vec<ScriptError>,
}

impl<'a> Validator<'a> {
    fn new(
        resource: &str,
        kind: FileKind,
        provider: &'a dyn ResourceProvider,
        scope: &'a [ParamDecl],
    ) -> Self {
        Self {
            resource: resource.to_string(),
            kind,
            provider,
            scope,
            errors: Vec::new(),
        }
    }

    fn push(&mut self, code: &str, step_path: &str, field: &str, message: impl Into<String>) {
        self.errors.push(
            ScriptError::new(code, message, self.resource.clone())
                .at(step_path.to_string(), field.to_string()),
        );
    }

    fn push_in(
        &mut self,
        resource: &str,
        code: &str,
        step_path: &str,
        field: &str,
        message: impl Into<String>,
    ) {
        self.errors.push(
            ScriptError::new(code, message, resource.to_string())
                .at(step_path.to_string(), field.to_string()),
        );
    }

    // -- 步骤遍历 -----------------------------------------------------------

    fn walk_steps(&mut self, steps: &[Step], path: &str, depth: usize, loop_depth: usize) {
        if depth > MAX_DEPTH {
            // 每个超限容器只报一次，且不再下钻。
            self.push(
                codes::STEP_NESTING_DEPTH,
                path,
                last_segment(path),
                format!("步骤嵌套超过 {MAX_DEPTH} 层"),
            );
            return;
        }
        for (i, step) in steps.iter().enumerate() {
            let p = format!("{path}[{i}]");
            self.walk_step(step, &p, depth, loop_depth);
        }
    }

    fn walk_step(&mut self, step: &Step, path: &str, depth: usize, loop_depth: usize) {
        match step {
            Step::StrApp | Step::ClsApp | Step::Throw { .. } => {}
            Step::Break => {
                if loop_depth == 0 {
                    self.push(
                        codes::STEP_BREAK_OUTSIDE_LOOP,
                        path,
                        "",
                        "break 只能出现在 loop 子流程内",
                    );
                }
            }
            Step::Tap { at } => self.check_cell(at, ParamType::Coord, path, "at"),
            Step::Swipe { from, to, time } => {
                self.check_cell(from, ParamType::Coord, path, "from");
                self.check_cell(to, ParamType::Coord, path, "to");
                self.check_cell(time, ParamType::Time, path, "time");
            }
            Step::Key { key } => self.check_cell(key, ParamType::Key, path, "key"),
            Step::Text { value } => self.check_cell(value, ParamType::Text, path, "value"),
            Step::Log { message } => self.check_cell(message, ParamType::Text, path, "message"),
            Step::Wait {
                duration,
                duration_max,
            } => {
                self.check_cell(duration, ParamType::Time, path, "duration");
                if let Some(max) = duration_max {
                    self.check_cell(max, ParamType::Time, path, "duration_max");
                }
            }
            Step::Find {
                template,
                block,
                timeout,
                then,
                r#else,
                ..
            } => {
                self.check_cell(template, ParamType::Tmpl, path, "template");
                for (i, b) in block.iter().enumerate() {
                    self.check_cell(b, ParamType::Tmpl, &format!("{path}.block[{i}]"), "block");
                }
                // 静态重复：主模板与 block 重复（模板名大小写敏感）。
                if let Cell::Lit(TypedValue::Tmpl(main)) = template {
                    for b in block.iter() {
                        if let Cell::Lit(TypedValue::Tmpl(name)) = b {
                            if name == main {
                                self.push(
                                    codes::STEP_FIND_BLOCK_DUPLICATE,
                                    path,
                                    "block",
                                    format!("障碍模板 {name} 与主模板重复"),
                                );
                            }
                        }
                    }
                }
                if let Some(t) = timeout {
                    self.check_cell(t, ParamType::Time, path, "timeout");
                }
                self.walk_branch(then, path, "then", depth, loop_depth);
                self.walk_branch(r#else, path, "else", depth, loop_depth);
            }
            Step::Match {
                candidates,
                r#else,
                timeout,
            } => {
                self.check_cell_duplicates(candidates, path);
                for (i, c) in candidates.iter().enumerate() {
                    self.check_cell(
                        &c.template,
                        ParamType::Tmpl,
                        &format!("{path}.candidates[{i}]"),
                        "candidates",
                    );
                    self.walk_branch(
                        &c.steps,
                        &format!("{path}.candidates[{i}]"),
                        "steps",
                        depth,
                        loop_depth,
                    );
                }
                if let Some(t) = timeout {
                    self.check_cell(t, ParamType::Time, path, "timeout");
                }
                self.walk_branch(r#else, path, "else", depth, loop_depth);
            }
            Step::Check { template, .. } => {
                self.check_cell(template, ParamType::Tmpl, path, "template");
            }
            Step::Color { at, expect, r#else } => {
                self.check_cell(at, ParamType::Coord, path, "at");
                // 静态重复：颜色统一小写后比较（FF8800 与 ff8800 视为重复）。
                let mut seen: Vec<String> = Vec::new();
                for (i, e) in expect.iter().enumerate() {
                    let p = format!("{path}.expect[{i}]");
                    if let Cell::Lit(TypedValue::Color(c)) = &e.color {
                        let lower = c.to_ascii_lowercase();
                        if seen.contains(&lower) {
                            self.push(
                                codes::STEP_COLOR_DUPLICATE,
                                path,
                                "expect",
                                format!("颜色候选 {c} 重复（与 {lower} 视为同一颜色）"),
                            );
                        } else {
                            seen.push(lower);
                        }
                    }
                    self.check_cell(&e.color, ParamType::Color, &p, "expect");
                    self.walk_branch(&e.steps, &p, "steps", depth, loop_depth);
                }
                self.walk_branch(r#else, path, "else", depth, loop_depth);
            }
            Step::If { cond, then, r#else } => {
                self.check_cell(cond, ParamType::Bool, path, "cond");
                self.walk_branch(then, path, "then", depth, loop_depth);
                self.walk_branch(r#else, path, "else", depth, loop_depth);
            }
            Step::Loop { steps, .. } => {
                self.walk_branch(steps, path, "steps", depth, loop_depth + 1);
            }
            Step::Call { target, args } => {
                self.check_call(target, args, path);
            }
            Step::Func {
                target,
                args,
                then,
                r#else,
            } => {
                self.check_func(target, args, path);
                self.walk_branch(then, path, "then", depth, loop_depth);
                self.walk_branch(r#else, path, "else", depth, loop_depth);
            }
            Step::Return { value } => {
                // parse 层已拒绝脚本中的 return；函数文件内做引用类型校验。
                self.check_cell(value, ParamType::Bool, path, "value");
            }
        }
    }

    fn walk_branch(
        &mut self,
        steps: &[Step],
        path: &str,
        key: &str,
        depth: usize,
        loop_depth: usize,
    ) {
        if steps.is_empty() {
            return;
        }
        self.walk_steps(steps, &format!("{path}.{key}"), depth + 1, loop_depth);
    }

    // -- 引用 / 资源 ---------------------------------------------------------

    /// `$name` 引用存在性与类型；tmpl 字面量的分区内唯一存在性。
    fn check_cell(&mut self, cell: &Cell, expected: ParamType, path: &str, field: &str) {
        match cell {
            Cell::Ref(name) => self.check_ref(name, expected, path, field),
            Cell::Lit(TypedValue::Tmpl(name)) => {
                if expected == ParamType::Tmpl {
                    self.check_template(name, path, field);
                }
            }
            Cell::Lit(_) => {}
        }
    }

    fn check_ref(&mut self, name: &str, expected: ParamType, path: &str, field: &str) {
        let Some(decl) = self.scope.iter().find(|d| d.name == name) else {
            self.push(
                codes::PARAM_REF_UNKNOWN,
                path,
                field,
                format!("$name 引用 {name:?} 不在当前参数表中"),
            );
            return;
        };
        if decl.ty != expected {
            self.push(
                codes::PARAM_REF_TYPE_MISMATCH,
                path,
                field,
                format!(
                    "参数 {name} 的类型是 {}，字段需要 {}",
                    decl.ty.as_str(),
                    expected.as_str()
                ),
            );
        }
    }

    fn check_template(&mut self, name: &str, path: &str, field: &str) {
        match self.provider.resolve_template(name) {
            TemplateAvail::Found => {}
            TemplateAvail::Ambiguous => self.push(
                codes::RESOURCE_TMPL_AMBIGUOUS,
                path,
                field,
                format!("模板短名 {name} 在当前分区有多个 # 后缀候选，歧义"),
            ),
            TemplateAvail::NotFound => self.push(
                codes::RESOURCE_TMPL_NOT_FOUND,
                path,
                field,
                format!("模板短名 {name} 在当前分区不存在"),
            ),
        }
    }

    fn check_cell_duplicates(&mut self, candidates: &[super::model::MatchCandidate], path: &str) {
        let mut seen: Vec<&str> = Vec::new();
        for c in candidates {
            if let Cell::Lit(TypedValue::Tmpl(name)) = &c.template {
                if seen.contains(&name.as_str()) {
                    self.push(
                        codes::STEP_MATCH_CANDIDATE_DUPLICATE,
                        path,
                        "candidates",
                        format!("候选模板 {name} 重复"),
                    );
                } else {
                    seen.push(name);
                }
            }
        }
    }

    // -- call / func ---------------------------------------------------------

    fn check_call(&mut self, target: &str, args: &[ArgAssign], path: &str) {
        if bad_resource_path(target) {
            self.push(
                codes::REF_CALL_PATH_TRAVERSAL,
                path,
                "target",
                format!("call 目标 {target:?} 含 ..、绝对路径或反斜杠"),
            );
            return;
        }
        if normalize_id(target) == normalize_id(&self.resource) {
            self.push(
                codes::REF_CALL_SELF_CYCLE,
                path,
                "target",
                format!("call 目标 {target:?} 是脚本自身，形成调用环"),
            );
            return;
        }
        if !self.provider.script_exists(target) {
            self.push(
                codes::RESOURCE_SCRIPT_NOT_FOUND,
                path,
                "target",
                format!("call 目标脚本 {target:?} 不存在"),
            );
            return;
        }
        if let Some(content) = self.provider.script_content(target) {
            if let Some(tf) = try_build_script(&content) {
                self.check_args(&tf.params, args, path, target);
            }
        }
    }

    fn check_func(&mut self, target: &str, args: &[ArgAssign], path: &str) {
        if bad_resource_path(target) {
            self.push(
                codes::REF_FUNC_PATH_TRAVERSAL,
                path,
                "target",
                format!("函数路径 {target:?} 含 ..、绝对路径或反斜杠"),
            );
            return;
        }
        let Some((file_short, func_name)) = split_func_path(target) else {
            self.push(
                codes::REF_FUNC_SYNTAX,
                path,
                "target",
                format!("函数路径 {target:?} 必须是 <文件短路径>/<函数名>"),
            );
            return;
        };
        if !self.provider.function_exists(&file_short, &func_name) {
            self.push(
                codes::RESOURCE_FUNC_NOT_FOUND,
                path,
                "target",
                format!("函数 {target:?} 不存在（文件或函数名未找到）"),
            );
            return;
        }
        if let Some(content) = self.provider.function_file_content(&file_short) {
            if let Some(ff) = try_build_function_file(&content) {
                if let Some(decl) = ff.find(&func_name) {
                    self.check_args(&decl.params, args, path, target);
                }
            }
        }
    }

    /// args 键与目标声明一致：未知键 / 必填缺失 / 字面量按目标类型重定型。
    fn check_args(
        &mut self,
        decls: &[ParamDecl],
        args: &[ArgAssign],
        path: &str,
        target_label: &str,
    ) {
        for arg in args {
            let Some(decl) = decls.iter().find(|d| d.name == arg.name) else {
                self.push(
                    codes::PARAM_ARGS_UNKNOWN,
                    path,
                    "args",
                    format!("args 键 {:?} 不是目标 {target_label} 的参数", arg.name),
                );
                continue;
            };
            match &arg.value {
                Cell::Ref(ref_name) => self.check_ref(ref_name, decl.ty, path, "args"),
                Cell::Lit(v) => {
                    if coerce_literal(v, decl.ty).is_none() {
                        self.push(
                            codes::PARAM_ARGS_TYPE_MISMATCH,
                            path,
                            "args",
                            format!(
                                "args[{}] 的值 {v:?} 与目标参数类型 {} 不符",
                                arg.name,
                                decl.ty.as_str()
                            ),
                        );
                    }
                }
            }
        }
        for decl in decls {
            if decl.default.is_none() && !args.iter().any(|a| a.name == decl.name) {
                self.push(
                    codes::PARAM_ARGS_MISSING_REQUIRED,
                    path,
                    "args",
                    format!("目标 {target_label} 的必填参数 {} 未出现在 args", decl.name),
                );
            }
        }
    }

    // -- 静态引用图（环 + 深度） ----------------------------------------------

    fn walk_graph(&mut self, entry: GraphEntry<'_>) {
        let mut walker = GraphWalker {
            provider: self.provider,
            errors: &mut self.errors,
            visited: HashSet::new(),
        };
        match entry {
            GraphEntry::Script(id, file) => walker.visit(
                &normalize_id(&id),
                &id,
                GraphFile::Script(file),
                &mut Vec::new(),
            ),
            GraphEntry::Function(id, file) => walker.visit(
                &normalize_id(&id),
                &id,
                GraphFile::Function(file),
                &mut Vec::new(),
            ),
        }
    }
}

enum GraphEntry<'a> {
    Script(String, &'a ScriptFile),
    Function(String, &'a FunctionFile),
}

#[derive(Clone, Copy)]
enum GraphFile<'a> {
    Script(&'a ScriptFile),
    Function(&'a FunctionFile),
}

struct GraphWalker<'p, 'e> {
    provider: &'p dyn ResourceProvider,
    errors: &'e mut Vec<ScriptError>,
    visited: HashSet<String>,
}

impl<'p, 'e> GraphWalker<'p, 'e> {
    /// DFS：`stack` 为当前调用链（含自身，长度 = 深度）；环/超限在**闭边所在
    /// 资源**（发起该调用的步骤）上报错。
    fn visit<'a>(&mut self, key: &str, label: &str, file: GraphFile<'a>, stack: &mut Vec<String>) {
        if !stack.is_empty() && self.visited.contains(key) {
            return;
        }
        stack.push(key.to_string());
        let edges = collect_edges(label, file);
        for edge in edges {
            match edge.to {
                EdgeTarget::Script { raw } => {
                    let to_key = normalize_id(&raw);
                    if to_key == *key {
                        // 自引用已在逐步骤校验报 self_cycle，图上跳过。
                        continue;
                    }
                    if stack.contains(&to_key) {
                        self.errors.push(
                            ScriptError::new(
                                codes::REF_CALL_CROSS_CYCLE,
                                format!("call 目标 {raw:?} 回到调用链，形成跨文件调用环"),
                                label,
                            )
                            .at(edge.step_path.clone(), "target"),
                        );
                        continue;
                    }
                    if stack.len() + 1 > MAX_DEPTH {
                        self.errors.push(
                            ScriptError::new(
                                codes::REF_CALL_DEPTH,
                                format!("调用深度超过 {MAX_DEPTH} 层（{raw:?}）"),
                                label,
                            )
                            .at(edge.step_path.clone(), "target"),
                        );
                        continue;
                    }
                    if self.provider.script_exists(&raw) {
                        if let Some(content) = self.provider.script_content(&raw) {
                            if let Some(sf) = try_build_script(&content) {
                                self.visit(&to_key, &raw, GraphFile::Script(&sf), stack);
                                continue;
                            }
                        }
                    }
                    // 不存在/构建失败：资源错误已由逐步骤校验报告，图上跳过。
                }
                EdgeTarget::Function {
                    file: file_short,
                    raw,
                } => {
                    let to_key = normalize_id(&file_short);
                    if to_key == *key {
                        // 同文件函数递归不在静态图内（运行期由 32 层嵌套 guard 兜底）。
                        continue;
                    }
                    if stack.contains(&to_key) {
                        self.errors.push(
                            ScriptError::new(
                                codes::REF_FUNC_CYCLE,
                                format!("函数路径 {raw:?} 回到调用链，形成跨文件函数环"),
                                label,
                            )
                            .at(edge.step_path.clone(), "target"),
                        );
                        continue;
                    }
                    if stack.len() + 1 > MAX_DEPTH {
                        self.errors.push(
                            ScriptError::new(
                                codes::RUNTIME_NESTING_LIMIT,
                                format!("函数嵌套超过 {MAX_DEPTH} 层（{raw:?}）"),
                                label,
                            )
                            .at(edge.step_path.clone(), "target"),
                        );
                        continue;
                    }
                    if let Some(content) = self.provider.function_file_content(&file_short) {
                        if let Some(ff) = try_build_function_file(&content) {
                            self.visit(&to_key, &file_short, GraphFile::Function(&ff), stack);
                        }
                    }
                }
            }
        }
        stack.pop();
        self.visited.insert(key.to_string());
    }
}

enum EdgeTarget {
    Script { raw: String },
    Function { file: String, raw: String },
}

struct Edge {
    step_path: String,
    to: EdgeTarget,
}

/// 收集一个资源（脚本或函数文件）中全部 call/func 出边。
fn collect_edges(label: &str, file: GraphFile<'_>) -> Vec<Edge> {
    let mut edges = Vec::new();
    match file {
        GraphFile::Script(sf) => collect_from_steps(&sf.steps, "steps", &mut edges),
        GraphFile::Function(ff) => {
            for func in &ff.functions {
                collect_from_steps(&func.steps, &format!("{}.steps", func.name), &mut edges);
            }
        }
    }
    let _ = label;
    edges
}

fn collect_from_steps(steps: &[Step], path: &str, edges: &mut Vec<Edge>) {
    for (i, step) in steps.iter().enumerate() {
        let p = format!("{path}[{i}]");
        match step {
            Step::Call { target, .. } => edges.push(Edge {
                step_path: p.clone(),
                to: EdgeTarget::Script {
                    raw: target.clone(),
                },
            }),
            Step::Func { target, .. } => {
                if let Some((file_short, _)) = split_func_path(target) {
                    edges.push(Edge {
                        step_path: p.clone(),
                        to: EdgeTarget::Function {
                            file: file_short,
                            raw: target.clone(),
                        },
                    });
                }
            }
            _ => {}
        }
        // 递归分支。
        let branches: &[(&str, &Vec<Step>)] = match step {
            Step::Find { then, r#else, .. } => &[("then", then), ("else", r#else)],
            Step::Match {
                candidates, r#else, ..
            } => {
                for (i, c) in candidates.iter().enumerate() {
                    collect_from_steps(&c.steps, &format!("{p}.candidates[{i}].steps"), edges);
                }
                &[("else", r#else)]
            }
            Step::Color { expect, r#else, .. } => {
                for (i, e) in expect.iter().enumerate() {
                    collect_from_steps(&e.steps, &format!("{p}.expect[{i}].steps"), edges);
                }
                &[("else", r#else)]
            }
            Step::If { then, r#else, .. } => &[("then", then), ("else", r#else)],
            Step::Loop { steps, .. } => &[("steps", steps)],
            Step::Func { then, r#else, .. } => &[("then", then), ("else", r#else)],
            _ => &[],
        };
        for (key, sub) in branches {
            collect_from_steps(sub, &format!("{p}.{key}"), edges);
        }
    }
}

// ---------------------------------------------------------------------------
// 工具
// ---------------------------------------------------------------------------

/// 引用路径非法：`..` 段、绝对路径、反斜杠。
pub(crate) fn bad_resource_path(target: &str) -> bool {
    target.contains('\\') || target.starts_with('/') || target.split('/').any(|seg| seg == "..")
}

/// 函数路径 `文件短路径/函数名`：恰好一个 `/` 且两段非空。
pub(crate) fn split_func_path(target: &str) -> Option<(String, String)> {
    let (file, func) = target.split_once('/')?;
    if file.is_empty() || func.is_empty() || func.contains('/') {
        return None;
    }
    Some((file.to_string(), func.to_string()))
}

/// 资源 id 归一（去 .yaml 后缀），用于自引用/环比较。
pub(crate) fn normalize_id(id: &str) -> String {
    id.strip_suffix(".yaml").unwrap_or(id).to_string()
}

fn last_segment(path: &str) -> &str {
    path.rsplit('.').next().unwrap_or(path)
}

/// args 字面量按目标参数类型重定型；不可定型返回 `None`。
///
/// 装载层把非布尔/坐标标量统一存为 Text，此处按目标类型重新解析：
/// time 目标要求可解析为合法时长，color 目标要求 6 位十六进制，
/// key 目标要求在按键枚举内（或纯数字 keycode），
/// bool 目标只接受布尔字面量（字符串 "true" 非法，CONTRACT §3.3）。
pub(crate) fn coerce_literal(v: &TypedValue, target: ParamType) -> Option<TypedValue> {
    match (target, v) {
        (ParamType::Text, TypedValue::Text(s)) => Some(TypedValue::Text(s.clone())),
        (ParamType::Bool, TypedValue::Bool(b)) => Some(TypedValue::Bool(*b)),
        (ParamType::Coord, TypedValue::Coord(c)) => Some(TypedValue::Coord(*c)),
        (ParamType::Time, TypedValue::Time(s) | TypedValue::Text(s)) => {
            params::parse_time_ms(s).map(|_| TypedValue::Time(s.clone()))
        }
        (ParamType::Color, TypedValue::Color(s) | TypedValue::Text(s)) => {
            params::is_valid_color(s).then(|| TypedValue::Color(s.clone()))
        }
        (ParamType::Key, TypedValue::Key(s) | TypedValue::Text(s)) => {
            params::is_valid_key(s).then(|| TypedValue::Key(s.clone()))
        }
        (ParamType::Tmpl, TypedValue::Tmpl(s) | TypedValue::Text(s)) => {
            (!s.is_empty()).then(|| TypedValue::Tmpl(s.clone()))
        }
        _ => None,
    }
}
