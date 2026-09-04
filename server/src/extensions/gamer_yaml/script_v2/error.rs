//! 结构化脚本错误（docs/reference/SCRIPT_EDITOR_CONTRACT.md §5.1 五元组）。
//!
//! 前端以 `{code, step_path, field}` 定位卡片与控件，`message` 仅展示；
//! `resource` 标识出错资源。顶层/整文件错误 `step_path` 为 `None`（序列化为 `""`）。

use serde::Serialize;

/// 错误码常量。取值对齐 CONTRACT §5.3 五域命名空间 + 阶段 0 fixture 固化的码；
/// 个别 §5.3 未列出但分层校验必需的码以 `// 契约缺口` 标注（见模块汇报）。
pub mod codes {
    // yaml / 根结构
    pub const YAML_SYNTAX_ERROR: &str = "yaml.syntax_error";
    pub const SCRIPT_ROOT_TYPE: &str = "script.root_type";
    pub const SCRIPT_TOP_LEVEL_UNKNOWN_KEY: &str = "script.top_level.unknown_key";
    // 契约缺口：§5.3 未列 config 域码，分层校验需要（config 子键白名单与取值域）
    pub const SCRIPT_CONFIG_UNKNOWN_KEY: &str = "script.config.unknown_key";
    pub const SCRIPT_CONFIG_INVALID: &str = "script.config.invalid";
    // 函数库记录（阶段 0 fixture 固化）
    pub const FUNC_RECORD_TYPE: &str = "func.record_type";
    pub const FUNC_RECORD_UNKNOWN_KEY: &str = "func.record_unknown_key";
    // 参数
    pub const PARAM_DECL_QUOTE_STYLE: &str = "param.decl.quote_style";
    pub const PARAM_DECL_FORMAT: &str = "param.decl.format";
    pub const PARAM_DECL_NAME_INVALID: &str = "param.decl.name_invalid";
    pub const PARAM_DECL_NAME_DUPLICATE: &str = "param.decl.name_duplicate";
    pub const PARAM_DEFAULT_EMPTY: &str = "param.default.empty";
    pub const PARAM_DEFAULT_INVALID: &str = "param.default.invalid";
    pub const PARAM_REF_UNKNOWN: &str = "param.ref.unknown";
    pub const PARAM_REF_TYPE_MISMATCH: &str = "param.ref.type_mismatch";
    pub const PARAM_ARGS_UNKNOWN: &str = "param.args.unknown";
    pub const PARAM_ARGS_MISSING_REQUIRED: &str = "param.args.missing_required";
    pub const PARAM_ARGS_TYPE_MISMATCH: &str = "param.args.type_mismatch";
    // 步骤
    pub const STEP_UNKNOWN_ACTION: &str = "step.unknown_action";
    pub const STEP_MULTI_ACTION: &str = "step.multi_action";
    pub const STEP_LIST_TYPE: &str = "step.list_type";
    pub const STEP_FIELD_MISSING: &str = "step.field.missing";
    pub const STEP_FIELD_TYPE_MISMATCH: &str = "step.field.type_mismatch";
    pub const STEP_FIELD_UNKNOWN: &str = "step.field.unknown";
    pub const STEP_MATCH_CANDIDATES_TYPE: &str = "step.match.candidates_type";
    pub const STEP_MATCH_CANDIDATE_DUPLICATE: &str = "step.match.candidate_duplicate";
    pub const STEP_MATCH_ELSE_IN_CANDIDATES: &str = "step.match.else_in_candidates";
    pub const STEP_IF_NON_BOOL_COND: &str = "step.if.non_bool_cond";
    pub const STEP_COLOR_DUPLICATE: &str = "step.color.duplicate";
    pub const STEP_COLOR_FORMAT: &str = "step.color.format";
    pub const STEP_COORD_RANGE: &str = "step.coord.range";
    pub const STEP_TIME_FORMAT: &str = "step.time.format";
    pub const STEP_WAIT_RANGE_INVALID: &str = "step.wait.range_invalid";
    pub const STEP_LOOP_EMPTY_STEPS: &str = "step.loop.empty_steps";
    pub const STEP_BREAK_OUTSIDE_LOOP: &str = "step.break.outside_loop";
    pub const STEP_RETURN_IN_SCRIPT: &str = "step.return.in_script";
    pub const STEP_NESTING_DEPTH: &str = "step.nesting.depth";
    // 契约缺口：§5.3 未列 find 主模板与 block 重复的码，按域命名规则补
    pub const STEP_FIND_BLOCK_DUPLICATE: &str = "step.find.block_duplicate";
    // 资源
    pub const RESOURCE_TMPL_NOT_FOUND: &str = "resource.tmpl.not_found";
    pub const RESOURCE_TMPL_AMBIGUOUS: &str = "resource.tmpl.ambiguous";
    pub const RESOURCE_SCRIPT_NOT_FOUND: &str = "resource.script.not_found";
    pub const RESOURCE_FUNC_NOT_FOUND: &str = "resource.func.not_found";
    pub const RESOURCE_IMPORT_INVALID: &str = "resource.import.invalid";
    // 引用
    pub const REF_CALL_PATH_TRAVERSAL: &str = "ref.call.path_traversal";
    pub const REF_CALL_SELF_CYCLE: &str = "ref.call.self_cycle";
    pub const REF_CALL_CROSS_CYCLE: &str = "ref.call.cross_cycle";
    pub const REF_CALL_DEPTH: &str = "ref.call.depth";
    pub const REF_FUNC_PATH_TRAVERSAL: &str = "ref.func.path_traversal";
    pub const REF_FUNC_SYNTAX: &str = "ref.func.syntax";
    // 契约缺口：§5.3 未列跨文件函数环的码，按域命名规则补
    pub const REF_FUNC_CYCLE: &str = "ref.func.cycle";
    // 运行时（静态深度预检借用）
    pub const RUNTIME_NESTING_LIMIT: &str = "runtime.nesting.limit";
}

/// 结构化脚本错误：`code + message + resource + step_path + field`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptError {
    /// 错误码（CONTRACT §5.3 命名空间）。
    pub code: String,
    /// 人类可读中文消息（仅展示，前端禁止解析文案定位）。
    pub message: String,
    /// 出错资源 ID（脚本资源 id / 函数文件短路径 / fixture 逻辑 ID）。
    pub resource: String,
    /// 定位路径（如 `steps[1].then[0]`、`login.params[0]`）；顶层/整文件错误为 `None`。
    pub step_path: Option<String>,
    /// 出错字段名；无具体字段时为 `None`。
    pub field: Option<String>,
}

impl ScriptError {
    pub fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        resource: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            resource: resource.into(),
            step_path: None,
            field: None,
        }
    }

    /// 附带定位路径与字段名。
    pub fn at(mut self, step_path: impl Into<String>, field: impl Into<String>) -> Self {
        self.step_path = Some(step_path.into());
        self.field = Some(field.into());
        self
    }

    /// 测试/展示用：`None` 归一为空串（CONTRACT §5.2 顶层错误 step_path = ""）。
    pub fn step_path_str(&self) -> &str {
        self.step_path.as_deref().unwrap_or("")
    }

    pub fn field_str(&self) -> &str {
        self.field.as_deref().unwrap_or("")
    }
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)?;
        let step_path = self.step_path_str();
        if !step_path.is_empty() {
            write!(f, " @ {step_path}")?;
        }
        let field = self.field_str();
        if !field.is_empty() {
            write!(f, ".{field}")?;
        }
        write!(f, " ({})", self.resource)
    }
}

impl Serialize for ScriptError {
    /// 序列化为 CONTRACT §5.1 形态；`None` 的 step_path/field 输出空串。
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = serializer.serialize_struct("ScriptError", 5)?;
        st.serialize_field("code", &self.code)?;
        st.serialize_field("message", &self.message)?;
        st.serialize_field("resource", &self.resource)?;
        st.serialize_field("step_path", self.step_path_str())?;
        st.serialize_field("field", self.field_str())?;
        st.end()
    }
}
