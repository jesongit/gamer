//! 运行请求描述（runner 私有 wire，非 DSL 语义）。
//!
//! [`RunTarget`] / [`RunSpec`] / [`TypedValue`] / [`BoundEntryArgs`] 描述
//! 「跑什么、带什么参数」：手动运行、函数测试与定时任务共用同一形态。
//! 序列化形状是 RunManager payload 的持久化 wire（存量任务行的
//! `payload.target` / `WireTypedValue` args 依赖它），字段与判别值保持稳定。

use serde::{Deserialize, Serialize};

use crate::core::RunContext;

// ---------------------------------------------------------------------------
// 运行目标与请求
// ---------------------------------------------------------------------------

/// 统一运行目标：手动运行 / 从步骤运行 / 函数测试 / 定时任务。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunTarget {
    /// 可执行脚本（scripts/）。`start_index` = 顶层步骤序号（0=从头）。
    Script {
        script_id: String,
        start_index: usize,
    },
    /// 函数测试（functions/）。`file` = 文件短路径；`function` = 函数名
    /// （None = 文件第一个函数，由入口/API 解析）；`start_index` = 函数体内
    /// 顶层步骤序号。函数不伪装成脚本 ID 进入选择器。
    Function {
        pkg: String,
        file: String,
        function: Option<String>,
        start_index: usize,
    },
}

impl RunTarget {
    /// 运行分区（应用包名）：模板/脚本/函数解析域，也是 str_app/cls_app 包名。
    pub fn pkg(&self) -> &str {
        match self {
            RunTarget::Script { script_id, .. } => script_id.split('/').next().unwrap_or_default(),
            RunTarget::Function { pkg, .. } => pkg,
        }
    }

    /// 展示标签（RunRecord.script_id；busy 弹窗 / 运行日志落库共用）。
    pub fn label(&self) -> String {
        match self {
            RunTarget::Script { script_id, .. } => script_id.clone(),
            RunTarget::Function {
                pkg,
                file,
                function,
                ..
            } => match function {
                Some(f) => format!("{pkg}/{file}.yaml#{f}"),
                None => format!("{pkg}/{file}.yaml"),
            },
        }
    }
}

impl Serialize for RunTarget {
    /// wire JSON 形态（存量任务 payload 兼容）。
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        match self {
            RunTarget::Script {
                script_id,
                start_index,
            } => {
                let mut s = serializer.serialize_struct("RunTarget", 3)?;
                s.serialize_field("type", "script")?;
                s.serialize_field("script_id", script_id)?;
                s.serialize_field("start_index", start_index)?;
                s.end()
            }
            RunTarget::Function {
                pkg,
                file,
                function,
                start_index,
            } => {
                let mut s = serializer.serialize_struct("RunTarget", 5)?;
                s.serialize_field("type", "function")?;
                s.serialize_field("pkg", pkg)?;
                s.serialize_field("file", file)?;
                s.serialize_field("function", function)?;
                s.serialize_field("start_index", start_index)?;
                s.end()
            }
        }
    }
}

/// 一次执行的完整规格（RunManager StartRequest → 执行器的直通车）。
#[derive(Debug, Clone)]
pub struct RunSpec {
    pub context: RunContext,
    pub target: RunTarget,
    /// 稀疏类型化参数覆盖（入口绑定产出；缺省参数由 v3 guest 按声明默认值取值）。
    pub args: Vec<(String, TypedValue)>,
}

/// 类型化参数值（七类 wire）：任务参数快照与手动运行实参的统一形态。
#[derive(Debug, Clone, PartialEq)]
pub enum TypedValue {
    /// 模板短名（如 `account.png`）。
    Tmpl(String),
    /// 0~1 相对坐标 [x, y]。
    Coord([f64; 2]),
    /// 6 位十六进制颜色（无 #，保留书写大小写；比较时统一小写）。
    Color(String),
    /// 时间书写串（带单位，>0，如 "800ms"；数值保持书写形式）。
    Time(String),
    /// 按键名（如 "ESC"）。
    Key(String),
    /// 文本。
    Text(String),
    Bool(bool),
}

/// 稀疏 JSON args 的解析与绑定结果：
/// - `overrides`：稀疏类型化覆盖（进 [`RunSpec`]，运行开始时按当前声明重绑定）；
/// - `resolved`：声明默认值 → 覆盖 合并后的全量绑定视图（API 响应
///   `resolved_args`，展示本次运行实际生效的参数值）。
#[derive(Debug, Clone)]
pub struct BoundEntryArgs {
    pub overrides: Vec<(String, TypedValue)>,
    pub resolved: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// payload wire 往返：`{"type":"script",...}` 形态被存量任务行依赖。
    #[test]
    fn run_target_wire_roundtrip() {
        let script = RunTarget::Script {
            script_id: "com.a/daily.yaml".into(),
            start_index: 2,
        };
        let json = serde_json::to_value(&script).unwrap();
        assert_eq!(json["type"], "script");
        assert_eq!(json["start_index"], 2);
        assert_eq!(serde_json::from_value::<RunTarget>(json).unwrap(), script);

        let function = RunTarget::Function {
            pkg: "com.a".into(),
            file: "lib".into(),
            function: Some("greet".into()),
            start_index: 0,
        };
        let json = serde_json::to_value(&function).unwrap();
        assert_eq!(json["type"], "function");
        assert_eq!(json["function"], "greet");
        assert_eq!(
            serde_json::from_value::<RunTarget>(json).unwrap(),
            function
        );
        assert_eq!(function.label(), "com.a/lib.yaml#greet");
        assert_eq!(script.label(), "com.a/daily.yaml");
    }
}
