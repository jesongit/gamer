//! 运行请求描述（runner 私有 wire，非 DSL 语义）。
//!
//! [`RunTarget`] / [`RunSpec`] 描述「跑什么、带什么参数」：手动运行、函数
//! 测试与定时任务共用同一形态。序列化形状是 RunManager payload 的持久化
//! wire（任务行的 `payload.target` 依赖它）。V1 起参数覆盖为原始 JSON 对象
//! （类型绑定统一在执行边界按当前 Schema 完成，旧 v3 七类 TypedValue wire
//! 已删除——开发阶段不兼容旧任务快照）。

use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value};

use crate::core::RunContext;

// ---------------------------------------------------------------------------
// 运行目标与请求
// ---------------------------------------------------------------------------

/// 统一运行目标：手动运行 / 从步骤运行 / 函数测试 / 定时任务。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunTarget {
    /// 可执行脚本（automations/，`_function*.yaml` 函数库不可作此目标）。
    /// `start_index` = 顶层步骤序号（0=从头）。
    Script {
        script_id: String,
        start_index: usize,
    },
    /// 函数测试（简化计划 Phase 1：统一命名空间，纯函数名寻址）。函数从当前
    /// Package 全部 `_function*.yaml` 组合出的注册表按名解析——定义文件可移动/
    /// 拆分，不影响寻址；`start_index` = 函数体内顶层步骤序号。
    Function {
        pkg: String,
        function: String,
        start_index: usize,
    },
}

impl RunTarget {
    /// 运行目标所属 Package id（资源解析域：模板/脚本/函数定位与调用归属）。
    /// 与 Android 包名（launch/stop_app 缺省目标，AppContext.android_package）
    /// 是两个命名空间，不互相推导。
    pub fn pkg(&self) -> &str {
        match self {
            RunTarget::Script { script_id, .. } => script_id.split('/').next().unwrap_or_default(),
            RunTarget::Function { pkg, .. } => pkg,
        }
    }

    /// 「从此运行」的顶层步序号。
    pub fn start_index(&self) -> usize {
        match self {
            RunTarget::Script { start_index, .. } | RunTarget::Function { start_index, .. } => {
                *start_index
            }
        }
    }

    /// 展示标签（RunRecord.script_id；busy 弹窗 / 运行日志落库共用）。
    pub fn label(&self) -> String {
        match self {
            RunTarget::Script { script_id, .. } => script_id.clone(),
            RunTarget::Function { pkg, function, .. } => format!("{pkg}#{function}"),
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
                function,
                start_index,
            } => {
                let mut s = serializer.serialize_struct("RunTarget", 4)?;
                s.serialize_field("type", "function")?;
                s.serialize_field("pkg", pkg)?;
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
    /// 稀疏原始参数覆盖（执行边界按当前 Schema 绑定）。
    pub args: JsonMap<String, Value>,
    /// 未知实参处理：true（手动运行）报 `param.args.unknown`；false（任务
    /// 快照重绑）静默丢弃——存活值保留、新参数取默认值（计划 Phase 4.2）。
    pub strict_args: bool,
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
        assert_eq!(script.start_index(), 2);

        let function = RunTarget::Function {
            pkg: "com.a".into(),
            function: "greet".into(),
            start_index: 0,
        };
        let json = serde_json::to_value(&function).unwrap();
        assert_eq!(json["type"], "function");
        assert_eq!(json["function"], "greet");
        assert!(json.get("file").is_none(), "函数目标不携带定义文件段");
        assert_eq!(serde_json::from_value::<RunTarget>(json).unwrap(), function);
        assert_eq!(function.label(), "com.a#greet");
        assert_eq!(function.start_index(), 0);
        assert_eq!(script.label(), "com.a/daily.yaml");
    }
}
