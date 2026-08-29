//! script_v2：脚本新语法（2026-08 冻结契约，docs/SCRIPT_EDITOR_CONTRACT.md）
//! 的严格装载、AST 与语义校验。
//!
//! 阶段 2 执行引擎（engine.rs）与运行 API 消费本模块：
//! 1. [`loader`]：saphyr-parser 事件级装载（ScalarStyle + Span）→ 节点树 →
//!    结构层 AST 构建（顶层键白名单、参数声明、步骤结构/字段互斥）；
//! 2. [`validate`]：语义层（$name 引用、args 绑定一致性、模板存在性、
//!    静态重复、call/func 引用图环与深度）；
//! 3. [`params`]：参数声明解析、类型化字面量、args 绑定（引擎与 API 共用）。
//!
//! serialize / param_signature / loader span 等由测试与阶段 4/5（编辑器保存、
//! 任务快照签名）消费，非测试构建暂不引用。

#![allow(dead_code)]

pub mod error;
pub mod loader;
pub mod model;
pub mod params;
pub mod serialize;
pub mod validate;

#[cfg(test)]
mod fixtures_tests;
#[cfg(test)]
mod tests;

// 以下再导出为引擎 / API / 测试消费的公共面（bin crate 下部分条目仅测试
// 或外部阶段使用，未在本 crate 非测试代码内引用）。
#[allow(unused_imports)]
pub use error::ScriptError;
#[allow(unused_imports)]
pub use loader::FileKind;
#[allow(unused_imports)]
pub use model::param_signature;
#[allow(unused_imports)]
pub use model::{
    ArgAssign, ArgsRef, Cell, ColorBranch, FunctionDecl, FunctionFile, LogLevel, MatchCandidate,
    ParamDecl, ParamType, ScriptConfig, ScriptFile, Step, TypedValue,
};
#[allow(unused_imports)]
pub use serialize::{serialize_function_file, serialize_script};
#[allow(unused_imports)]
pub use validate::{InMemoryResources, ResourceProvider, TemplateAvail};

/// 严格装载一个可执行脚本（yaml/）：解析 + 结构校验 + 语义校验。
///
/// `resource` 为脚本资源 id（call 自引用比较用，如 `daily/login.yaml` 或
/// 测试用逻辑 ID）；校验失败返回全部结构化错误（带定位）。
pub fn parse_script_file(
    content: &str,
    resource: &str,
    provider: &dyn ResourceProvider,
) -> Result<ScriptFile, Vec<ScriptError>> {
    let node = loader::load(content).map_err(|message| vec![syntax_error(&message, resource)])?;
    let mut ctx = loader::BuildCtx::new(resource, loader::FileKind::Script);
    let file = loader::build_script_file(&mut ctx, &node);
    // 结构层错误（含嵌套项内累积的错误）非空即整体失败。
    if !ctx.errors.is_empty() {
        return Err(ctx.errors);
    }
    let Some(file) = file else {
        return Err(ctx.errors);
    };
    let errors = validate::validate_script_file(&file, resource, provider);
    if errors.is_empty() {
        Ok(file)
    } else {
        Err(errors)
    }
}

/// 严格装载一个函数库文件（func/）：顶层键 = 函数名，记录只允许 params/steps。
pub fn parse_function_file(
    content: &str,
    resource: &str,
    provider: &dyn ResourceProvider,
) -> Result<FunctionFile, Vec<ScriptError>> {
    let node = loader::load(content).map_err(|message| vec![syntax_error(&message, resource)])?;
    let mut ctx = loader::BuildCtx::new(resource, loader::FileKind::FunctionLibrary);
    let file = loader::build_function_file(&mut ctx, &node);
    if !ctx.errors.is_empty() {
        return Err(ctx.errors);
    }
    let Some(file) = file else {
        return Err(ctx.errors);
    };
    let errors = validate::validate_function_file(&file, resource, provider);
    if errors.is_empty() {
        Ok(file)
    } else {
        Err(errors)
    }
}

fn syntax_error(message: &str, resource: &str) -> ScriptError {
    ScriptError::new(error::codes::YAML_SYNTAX_ERROR, message, resource).at("", "yaml")
}
