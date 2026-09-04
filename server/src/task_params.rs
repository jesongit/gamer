//! 定时任务参数快照与签名门禁（plan §12.3 / CONTRACT §4.3–4.5）。
//!
//! 任务保存的是**完整类型化 args 快照**（七类 TypedValue 的 JSON 形态，与 run
//! API args 同构）+ 保存时脚本的 psig1 参数签名。调度/立即运行前过本模块的门禁：
//!
//! - 脚本缺失 / 解析失败 → 明确失败（同口径，不空跑）；
//! - 签名不一致（脚本参数声明/默认值变化）→ 参数过期，明确失败，等待重新确认；
//! - 门禁通过 → 快照整体作为 StartRequest.args 传入（快照是全量，天然不静默
//!   继承新默认值）。
//!
//! 日志约束：运行链路只记录参数签名与参数名列表，**绝不记录参数值**（text
//! 参数防泄露）；日志侧展示签名用 [`signature_short_code`] 短码。

use crate::script_v2::model::param_signature;
use crate::script_v2::params::{merge_args, parse_json_arg};
use crate::script_v2::{ParamDecl, ScriptError, TypedValue};
use crate::scripts::ScriptStore;
use crate::store::Task;

/// API 409 冲突错误码（签名不一致的重确认信号）。
/// CONTRACT 错误码表未列此码（契约缺口：snake_case 与 §5.2 dot 命名空间不一致，
/// 以任务约定为准），前端按 `code + reason` 消费。
pub const CODE_SIGNATURE_CONFLICT: &str = "param_signature_conflict";

/// 签名门禁失败的机器可读原因（409 body 的 `reason` 字段）。
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

/// 载入脚本当前参数声明并复算 psig1 签名。
/// 脚本缺失 / 读取失败 / 严格解析失败分别映射到 [`GateError`]。
pub fn probe_script_signature(
    scripts: &ScriptStore,
    script_id: &str,
) -> Result<(Vec<ParamDecl>, String), GateError> {
    match scripts.get(script_id) {
        Ok(Some(_)) => {}
        Ok(None) => return Err(GateError::ScriptMissing),
        Err(e) => {
            return Err(GateError::ScriptInvalid(vec![ScriptError::new(
                crate::script_v2::error::codes::YAML_SYNTAX_ERROR,
                format!("读取脚本失败: {e:#}"),
                script_id,
            )]))
        }
    }
    let target = crate::engine::RunTarget::Script {
        script_id: script_id.to_string(),
        start_index: 0,
    };
    let (decls, _label) = crate::engine::load_entry_param_decls(scripts, &target)
        .map_err(GateError::ScriptInvalid)?;
    let signature = param_signature(&decls);
    Ok((decls, signature))
}

/// 完整任务门禁：载入脚本当前签名 → 与存储签名比对 → 从已存快照 JSON 重建
/// 全量类型化覆盖。返回 [`TaskArgs`] 供 StartRequest 使用。
pub fn gate_task(scripts: &ScriptStore, task: &Task) -> Result<TaskArgs, GateError> {
    let (decls, current) = probe_script_signature(scripts, &task.script_id)?;
    if task.param_signature != current {
        return Err(GateError::SignatureMismatch {
            stored: task.param_signature.clone(),
            current,
        });
    }
    let overrides = rebind_snapshot(&decls, &task.args_json, &task.script_id)
        .map_err(GateError::ScriptInvalid)?;
    Ok(TaskArgs {
        signature: current,
        names: decls.iter().map(|d| d.name.clone()).collect(),
        overrides,
    })
}

/// 把已存快照 JSON 重新绑定到当前声明（门禁通过后的运行传参、以及「重新确认
/// 不带 args」路径共用）：
/// - 仍存在的参数保留原快照值；
/// - 新增参数取声明默认值；
/// - 已删除的参数静默丢弃（签名门禁先行，正常路径不会出现）。
///
/// 绑定后缺失必填参数 → 结构化诊断（param.args.missing_required）。
pub fn rebind_snapshot(
    decls: &[ParamDecl],
    args_json: &str,
    resource: &str,
) -> Result<Vec<(String, TypedValue)>, Vec<ScriptError>> {
    use crate::script_v2::error::codes;
    let stored: serde_json::Map<String, serde_json::Value> = match serde_json::from_str(args_json) {
        Ok(serde_json::Value::Object(map)) if !args_json.trim().is_empty() => map,
        _ => {
            return Err(vec![ScriptError::new(
                codes::PARAM_ARGS_TYPE_MISMATCH,
                "任务参数快照必须是非空 JSON 对象",
                resource,
            )
            .at("args", "args")])
        }
    };
    let mut overrides = Vec::new();
    let mut errors = Vec::new();
    for decl in decls {
        if let Some(value) = stored.get(&decl.name) {
            if let Some(v) = parse_json_arg(decl.ty, value) {
                overrides.push((decl.name.clone(), v));
            } else {
                errors.push(
                    ScriptError::new(
                        codes::PARAM_ARGS_TYPE_MISMATCH,
                        format!("任务参数快照中的参数 {} 类型无效", decl.name),
                        resource,
                    )
                    .at(format!("args.{}", decl.name), "args"),
                );
            }
        }
    }
    match merge_args(decls, overrides, resource) {
        Ok(bound) if errors.is_empty() => Ok(bound),
        Ok(_) => Err(errors),
        Err(mut diagnostics) => {
            errors.append(&mut diagnostics);
            Err(errors)
        }
    }
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

/// 类型化绑定对 → JSON 对象（快照存储形态；TypedValue 的 JSON 形态与 run API
/// args 同构：bool=布尔、coord=[x,y]、其余五类=字符串）。
pub fn typed_pairs_to_json(pairs: Vec<(String, TypedValue)>) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (name, value) in pairs {
        map.insert(name, serde_json::to_value(&value).unwrap_or_default());
    }
    serde_json::Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::script_v2::{parse_script_file, validate::InMemoryResources};

    const SCRIPT: &str = "\
params:
  - 'bool:enable:是否启用:true'
  - 'time:timeout:最长等待:30s'
  - 'text:message:提示文本:\"hello\"'
  - 'coord:pos:位置:[0.5, 0.5]'
steps:
  - log: 'ok'
";

    /// v12 形态脚本（含必填参数）供必填缺失路径使用。
    const SCRIPT_REQUIRED: &str = "\
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

    fn parse(content: &str) -> Vec<ParamDecl> {
        let provider = InMemoryResources::default();
        parse_script_file(content, "test.yaml", &provider)
            .unwrap_or_else(|e| panic!("fixture parse failed: {e:?}"))
            .params
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
        let old_decls = parse(SCRIPT);
        let old_snapshot = serde_json::json!({
            "enable": false,
            "timeout": "1m",
            "message": "TOP-SECRET",
            "pos": [0.1, 0.2],
        })
        .to_string();
        // 声明不变：全部保留原值
        let bound = rebind_snapshot(&old_decls, &old_snapshot, "t").unwrap();
        let map: std::collections::HashMap<String, TypedValue> = bound.into_iter().collect();
        assert_eq!(map["enable"], TypedValue::Bool(false));
        assert_eq!(map["timeout"], TypedValue::Time("1m".into()));
        assert_eq!(map["message"], TypedValue::Text("TOP-SECRET".into()));
        assert_eq!(map["pos"], TypedValue::Coord([0.1, 0.2]));
    }

    #[test]
    fn rebind_fills_default_for_new_param_and_drops_removed() {
        // 新脚本删除了 timeout，新增带默认值的 color
        let new_decls = parse(
            "\
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
        })
        .to_string();
        let bound = rebind_snapshot(&new_decls, &old_snapshot, "t").unwrap();
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
        let decls = parse(SCRIPT_REQUIRED);
        let err = rebind_snapshot(&decls, "{}", "t").unwrap_err();
        assert!(
            err.iter()
                .any(|e| e.code == crate::script_v2::error::codes::PARAM_ARGS_MISSING_REQUIRED),
            "必填缺失必须有结构化诊断: {err:?}"
        );
    }

    #[tokio::test]
    async fn gate_task_passes_with_matching_signature_and_rebuilds_overrides() {
        let (cfg, dir) = script_dir("gate-ok");
        write_script(&cfg, "daily.yaml", SCRIPT);
        let scripts = std::sync::Arc::new(ScriptStore::open(&cfg).unwrap());
        let (decls, signature) =
            probe_script_signature(&scripts, "com.test.app/daily.yaml").unwrap();
        // 快照 = 完整覆盖（含覆盖值）
        let snapshot = serde_json::json!({
            "enable": false,
            "timeout": "45s",
            "message": "SECRET-VALUE",
            "pos": [0.25, 0.75],
        });
        let task = Task {
            id: "t1".into(),
            name: "T".into(),
            cron: "0 * * * * *".into(),
            script_id: "com.test.app/daily.yaml".into(),
            device_id: "dev".into(),
            enabled: true,
            last_result: None,
            last_run_at: None,
            created_at: String::new(),
            args_json: snapshot.to_string(),
            param_signature: signature.clone(),
        };
        let gate = gate_task(&scripts, &task).unwrap();
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
        write_script(&cfg, "daily.yaml", SCRIPT);
        let scripts = std::sync::Arc::new(ScriptStore::open(&cfg).unwrap());
        let (_, signature) = probe_script_signature(&scripts, "com.test.app/daily.yaml").unwrap();
        let mk = |args_json: &str, sig: &str| Task {
            id: "t1".into(),
            name: "T".into(),
            cron: "0 * * * * *".into(),
            script_id: "com.test.app/daily.yaml".into(),
            device_id: "dev".into(),
            enabled: true,
            last_result: None,
            last_run_at: None,
            created_at: String::new(),
            args_json: args_json.into(),
            param_signature: sig.into(),
        };
        // 签名不一致 → 过期
        let stale = mk("{}", "psig1|old");
        match gate_task(&scripts, &stale) {
            Err(GateError::SignatureMismatch { stored, current }) => {
                assert_eq!(stored, "psig1|old");
                assert!(current.starts_with("psig1|"));
            }
            other => panic!("expected stale, got {:?}", other.is_ok()),
        }
        // 空快照与非法 JSON 都必须拒绝，不得按默认值兜底。
        for args_json in ["", "null", "not-json"] {
            match gate_task(&scripts, &mk(args_json, &signature)) {
                Err(GateError::ScriptInvalid(diags)) => assert!(diags.iter().any(|diag| {
                    diag.code == crate::script_v2::error::codes::PARAM_ARGS_TYPE_MISMATCH
                })),
                other => panic!("expected invalid snapshot, got {:?}", other.is_ok()),
            }
        }
        drop(scripts);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn gate_task_missing_and_broken_scripts_are_distinct_failures() {
        let (cfg, dir) = script_dir("gate-script");
        write_script(
            &cfg,
            "broken.yaml",
            "params:\n  - '不是合法声明'\nsteps: []\n",
        );
        let scripts = std::sync::Arc::new(ScriptStore::open(&cfg).unwrap());
        match probe_script_signature(&scripts, "com.test.app/missing.yaml") {
            Err(GateError::ScriptMissing) => {}
            other => panic!("expected missing, got {:?}", other.is_ok()),
        }
        match probe_script_signature(&scripts, "com.test.app/broken.yaml") {
            Err(GateError::ScriptInvalid(diags)) => assert!(!diags.is_empty()),
            other => panic!("expected invalid, got {:?}", other.is_ok()),
        }
        drop(scripts);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
