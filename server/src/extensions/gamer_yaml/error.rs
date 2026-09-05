//! 结构化脚本诊断载体（五元组 `{code, message, resource, step_path, field}`）。
//!
//! gamer.yaml 扩展侧 REST 错误的统一形态：前端以 `{code, step_path, field}`
//! 定位卡片与控件，`message` 仅展示；`resource` 标识出错资源。顶层/整文件
//! 错误 `step_path` 为 `None`（序列化为 `""`）。
//!
//! v3 源面解析（[`yaml_vnext`]）自产 `Diagnostic{code, path, message}`；保存
//! 边界直接透传该形态，运行/参数绑定边界则经本类型承载（见
//! [`crate::extensions::gamer_yaml::task_params`]）。

use serde::Serialize;

/// 错误码常量（现役集合：v3 版本门禁 + 参数声明/绑定 + 资源缺失）。
pub mod codes {
    /// 读取脚本/函数文件失败（IO 层）。
    pub const YAML_SYNTAX_ERROR: &str = "yaml.syntax_error";
    /// 非 `version: 3` 源（v2 已删除，无 fallback；与 yaml_vnext 版本门禁同码）。
    pub const VERSION_UNSUPPORTED: &str = "yaml.v3.version";
    /// 参数声明格式（type:name[:remark[:default]] 四段式 / 未知类型）。
    pub const PARAM_DECL_FORMAT: &str = "param.decl.format";
    /// 参数默认值无法按声明类型解析。
    pub const PARAM_DEFAULT_INVALID: &str = "param.default.invalid";
    /// args 键不是目标参数。
    pub const PARAM_ARGS_UNKNOWN: &str = "param.args.unknown";
    /// 必填参数未提供。
    pub const PARAM_ARGS_MISSING_REQUIRED: &str = "param.args.missing_required";
    /// 实参/快照值与声明类型不符。
    pub const PARAM_ARGS_TYPE_MISMATCH: &str = "param.args.type_mismatch";
    /// 分区内脚本不存在。
    pub const RESOURCE_SCRIPT_NOT_FOUND: &str = "resource.script.not_found";
    /// 函数库/目标函数不存在。
    pub const RESOURCE_FUNC_NOT_FOUND: &str = "resource.func.not_found";
}

/// 结构化脚本诊断：`code + message + resource + step_path + field`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptError {
    /// 错误码。
    pub code: String,
    /// 人类可读中文消息（仅展示，前端禁止解析文案定位）。
    pub message: String,
    /// 出错资源 ID（脚本资源 id / 函数文件短路径）。
    pub resource: String,
    /// 定位路径（如 `steps[1].then[0]`）；顶层/整文件错误为 `None`。
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

    /// 测试/展示用：`None` 归一为空串（顶层错误 step_path = ""）。
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
    /// 序列化为五元组形态；`None` 的 step_path/field 输出空串。
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

/// [`yaml_vnext::Diagnostic`] → 本载体（保留 `yaml.v3.*` 码；`path` →
/// `step_path`，resource 由调用方按入口资源标注）。
pub(crate) fn diagnostics_from_vnext(
    diagnostics: &[crate::extensions::gamer_yaml::yaml_vnext::Diagnostic],
    resource: &str,
) -> Vec<ScriptError> {
    diagnostics
        .iter()
        .map(|diagnostic| {
            ScriptError::new(
                diagnostic.code.clone(),
                diagnostic.message.clone(),
                resource,
            )
            .at(diagnostic.path.clone(), "")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 五元组 wire 形态锁定（前端 400 诊断回填契约）。
    #[test]
    fn serializes_to_contract_wire_shape() {
        let error = ScriptError::new("param.args.missing_required", "必填参数 x 未提供", "a/b.yaml")
            .at("args", "args");
        let json = serde_json::to_value(&error).unwrap();
        assert_eq!(json["code"], "param.args.missing_required");
        assert_eq!(json["step_path"], "args");
        assert_eq!(json["field"], "args");
        assert_eq!(json["resource"], "a/b.yaml");
        let top = serde_json::to_value(ScriptError::new("yaml.v3.version", "v", "r")).unwrap();
        assert_eq!(top["step_path"], "");
        assert_eq!(top["field"], "");
    }
}
