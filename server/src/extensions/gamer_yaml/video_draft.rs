//! gamer.yaml 的视频草稿动作（视频工作台 V1，实施合同 §5 / 计划 §5.4）。
//!
//! `action = "automation.create_draft"`（经现有 `POST /api/extensions/:id/call`
//! 通路，不新增 REST 路由）：把录制会话的操作事件（`crate::recording`
//! 的 [`crate::recording::InputEventRecord`]）映射为 **YAML v3 草稿文本**。
//!
//! 映射口径（V1）：
//! - `tap` → `- tap: [相对坐标]`（device-display 像素按事件自带 `display_size`
//!   归一为 0~1，4 位小数）；`swipe` → `from/to + duration`（录制不记录滑动
//!   时长 → 建议值 300ms 并注释标注）；`key` → 命名键或数字字符串（词表见
//!   `yaml_extension.rs::key_code`，保持同步）；`wait` → `wait`（录制等待）。
//! - `text` **不映射**（V1 脱敏只记录长度，无内容可还原）→ 进诊断；未知
//!   kind / payload 损坏 / display_size 损坏 → 进诊断。**不丢弃、不猜测**
//!   （计划 §5.4）。
//! - 相邻动作间隔 → 建议性 `wait` 步骤（标注建议值；≥500ms 才生成，更小的
//!   间隔是人类操作节奏，逐条生成只会污染草稿）。
//!
//! 草稿只是文本返回（`{"yaml", "diagnostics"}`），**不落盘、不执行、不建
//! 任务**；生成物必须通过 [`super::yaml_vnext`] 的 parse（下方测试自证）。
//! 录制数据绝不自动创建定时任务 / 启动 Runner（合同 §5）。

use std::path::Path;

use serde_json::{json, Value};

use crate::extensions::{ExtensionError, ExtensionResult};
use crate::recording::InputEventRecord;

/// gamer.yaml 的视频草稿生成动作名（合同 §5 钉死；前端 api.js 同词表）。
/// Phase 7 起动作分发收口在 [`super::actions`] 的版本化公开动作清单，
/// 本模块只保留草稿动作本体（清单内 Native 动作的实现）。
pub(crate) const AUTOMATION_CREATE_DRAFT: &str = "automation.create_draft";

/// 建议等待的最小间隔（毫秒）：低于该值的事件间隔不生成 wait 步骤。
const SUGGESTED_WAIT_MIN_MS: u64 = 500;
/// 录制未记录滑动时长的建议值（毫秒）。
const SUGGESTED_SWIPE_DURATION_MS: u64 = 300;

/// call 入参（合同 §5 + Phase 7 §10.3 扩展）：
/// `{"recording_id":"...","event_ids":["..."]?,"comments":{"<event_id>":"注释"}?}`。
/// `comments`（§10.3 事件注释）：按事件 id 提供的注释文本会作为该步骤上方的
/// YAML 注释行渲染（未知 id 的注释忽略——草稿只渲染已选事件）。
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DraftRequest {
    recording_id: String,
    #[serde(default)]
    event_ids: Option<Vec<String>>,
    #[serde(default)]
    comments: Option<std::collections::BTreeMap<String, String>>,
}

pub(crate) fn create_draft(values: &Value, data_dir: &Path) -> ExtensionResult<Value> {
    let request: DraftRequest = serde_json::from_value(values.clone()).map_err(|error| {
        ExtensionError::CallRejected(format!(
            "automation.create_draft 入参无效（需要 recording_id + 可选 event_ids/comments）: {error}"
        ))
    })?;
    let recording_id = request.recording_id.trim();
    if recording_id.is_empty() {
        return Err(ExtensionError::CallRejected(
            "automation.create_draft 缺少 recording_id".into(),
        ));
    }
    let events = load_events(data_dir, recording_id)?;
    Ok(build_draft(
        recording_id,
        request.event_ids.as_deref(),
        request.comments.as_ref(),
        &events,
    ))
}

/// 读取录制会话事件（合同 §4：D1 消费 `RecordingService::events`）。
///
/// Core 录制服务是进程级单例（`OnceLock`，首次装配定根），其唯一消费的
/// 配置是 `Config.data_dir`（`data_dir.join("media")`）；`data_dir` 由
/// `ExtensionService` 的 store 根传入（组合根 `for_data_root(cfg.data_dir)`
/// 同源），因此此处装配得到的实例与设备层/REST 先行装配的实例必然同一。
fn load_events(data_dir: &Path, recording_id: &str) -> ExtensionResult<Vec<InputEventRecord>> {
    let cfg = crate::config::Config {
        data_dir: data_dir.to_path_buf(),
        ..Default::default()
    };
    crate::recording::service(&cfg)
        .events(&crate::recording::RecordingId(recording_id.to_string()))
        .map_err(|error| ExtensionError::CallRejected(format!("读取录制事件失败: {error:#}")))
}

/// 纯函数草稿装配（load 之后的全部逻辑；测试直供事件夹具）。
///
/// Phase 7（§10.3）：`comments` = 事件 id → 注释文本（渲染为该步骤上方注释行）；
/// 响应新增 `source` 回查信息（录制会话 id + 选中事件的 kind/时间轴/映射结果），
/// 调用方（视频工作台草稿 JSON）据此保留「事件来源/时间/视频帧引用」。
pub(crate) fn build_draft(
    recording_id: &str,
    event_ids: Option<&[String]>,
    comments: Option<&std::collections::BTreeMap<String, String>>,
    events: &[InputEventRecord],
) -> Value {
    let (selected, selection_diagnostics) = select_events(event_ids, events);
    let (step_lines, mapping_diagnostics) = render_steps(&selected, comments);
    let mut document = vec![
        "version: 3".to_string(),
        format!("# 草稿：由录制会话 {recording_id} 的操作事件生成（gamer.yaml automation.create_draft）。"),
        "# 等待与滑动时长均为建议值；运行前需人工补充模板判断、状态等待、分支与异常恢复，".to_string(),
        "# 再保存到当前 Package 的 automations/。草稿只返回文本，不会被自动执行。".to_string(),
    ];
    document.push(if step_lines.is_empty() {
        "steps: []".to_string()
    } else {
        "steps:".to_string()
    });
    document.extend(step_lines);
    let mut diagnostics = selection_diagnostics;
    diagnostics.extend(mapping_diagnostics.clone());
    // 回查信息：选中事件的来源/时间轴/映射结果（调用方写进草稿 JSON，供从
    // 草稿回跳录像帧；事件 → 视频帧的对齐由调用方经分段 base_pts_us 完成）。
    let selected_ids: std::collections::HashSet<&str> = selected
        .iter()
        .map(|event| event.event_id.as_str())
        .collect();
    let source_events: Vec<Value> = events
        .iter()
        .map(|event| {
            json!({
                "event_id": event.event_id,
                "kind": event.kind,
                "timeline_us": event.timeline_us,
                "time_domain": event.time_domain,
                "source": event.source,
                "selected": selected_ids.contains(event.event_id.as_str()),
                "mapped": mapping_diagnostics
                    .iter()
                    .all(|d| d["event_id"].as_str() != Some(event.event_id.as_str())),
            })
        })
        .collect();
    json!({
        "yaml": document.join("\n") + "\n",
        "diagnostics": diagnostics,
        "source": {
            "recording_id": recording_id,
            "events": source_events,
        },
    })
}

/// 事件选择：缺省（None 或空表）= 全部事件（时间轴升序）；给定 id 列表 =
/// 按**请求顺序**输出（工作台允许选择/重排，计划 §5.4）；未知 id 进诊断。
fn select_events<'a>(
    event_ids: Option<&[String]>,
    events: &'a [InputEventRecord],
) -> (Vec<&'a InputEventRecord>, Vec<Value>) {
    let Some(ids) = event_ids.filter(|ids| !ids.is_empty()) else {
        return (events.iter().collect(), Vec::new());
    };
    let mut index = std::collections::HashMap::new();
    for event in events {
        index.entry(event.event_id.clone()).or_insert(event);
    }
    let mut selected = Vec::with_capacity(ids.len());
    let mut diagnostics = Vec::new();
    for id in ids {
        match index.get(id) {
            Some(event) => selected.push(*event),
            None => diagnostics.push(json!({
                "event_id": id,
                "reason": "录制会话中不存在该事件 id",
            })),
        }
    }
    (selected, diagnostics)
}

/// 事件 → 步骤行（含建议间隔与事件注释）；第二返回值为逐事件诊断。
/// 注释（§10.3）：`comments` 给出该事件的注释文本时，在事件步骤（或其建议
/// 等待）上方渲染 `# 注释` 行；换行压成空格防注释逃逸。
fn render_steps(
    events: &[&InputEventRecord],
    comments: Option<&std::collections::BTreeMap<String, String>>,
) -> (Vec<String>, Vec<Value>) {
    let mut lines = Vec::new();
    let mut diagnostics = Vec::new();
    // 上一个已映射事件的「结束时刻」（V1 事件只有起始时间轴；wait 用
    // 起始+时长近似，其余用起始时刻近似——只会低估间隔，不会多等）。
    let mut prev_end_us: Option<u64> = None;
    for event in events {
        match map_event(event) {
            Ok((mut step_lines, end_us)) => {
                if let Some(gap) = suggested_wait(prev_end_us, event.timeline_us) {
                    lines.push(format!("  # 建议等待 {gap}ms（事件间隔推导）"));
                    lines.push(format!("  - wait: {gap}ms # 建议值"));
                }
                if let Some(comment) = comments
                    .and_then(|map| map.get(&event.event_id))
                    .map(|text| text.replace(['\r', '\n'], " "))
                    .filter(|text| !text.trim().is_empty())
                {
                    lines.push(format!("  # {comment}"));
                }
                lines.append(&mut step_lines);
                prev_end_us = Some(end_us);
            }
            Err(reason) => {
                diagnostics.push(json!({ "event_id": event.event_id, "reason": reason }));
            }
        }
    }
    (lines, diagnostics)
}

/// 映射单个事件：`Ok((步骤行, 结束时刻))` 或 `Err(诊断原因)`。
fn map_event(event: &InputEventRecord) -> Result<(Vec<String>, u64), String> {
    match event.kind.as_str() {
        "tap" => {
            let (x, y) = payload_point(&event.payload).ok_or("tap 事件 payload 缺少数值 x/y")?;
            let (rx, ry) = relative_pair(x, y, event)?;
            Ok((
                vec![format!(
                    "  - tap: [{}, {}] # {} tap",
                    format_coord(rx),
                    format_coord(ry),
                    event.event_id
                )],
                event.timeline_us,
            ))
        }
        "swipe" => {
            let (x, y, x2, y2) =
                payload_swipe(&event.payload).ok_or("swipe 事件 payload 缺少数值 x/y/x2/y2")?;
            let (fx, fy) = relative_pair(x, y, event)?;
            let (tx, ty) = relative_pair(x2, y2, event)?;
            Ok((
                vec![
                    format!("  - swipe: # {} swipe", event.event_id),
                    format!(
                        "      from: [{}, {}]",
                        format_coord(fx),
                        format_coord(fy)
                    ),
                    format!("      to: [{}, {}]", format_coord(tx), format_coord(ty)),
                    format!(
                        "      duration: {SUGGESTED_SWIPE_DURATION_MS}ms # 建议值（录制未记录滑动时长）"
                    ),
                ],
                event.timeline_us,
            ))
        }
        "key" => {
            let code =
                payload_number(&event.payload, "code").ok_or("key 事件 payload 缺少数值 code")?;
            Ok((
                vec![format!(
                    "  - key: {} # {} key",
                    key_token(code),
                    event.event_id
                )],
                event.timeline_us,
            ))
        }
        "wait" => {
            let duration_us = payload_number(&event.payload, "duration_us")
                .ok_or("wait 事件 payload 缺少数值 duration_us")?;
            let ms = round_ms(duration_us);
            Ok((
                vec![format!(
                    "  - wait: {ms}ms # {} 建议等待（录制等待）",
                    event.event_id
                )],
                event.timeline_us.saturating_add(duration_us),
            ))
        }
        // V1 脱敏无内容：文本输入无法还原为 text 步骤，进诊断不猜测。
        "text" => Err(
            "文本输入在录制中默认脱敏（只记录长度），无法还原 text 步骤；请在草稿中手动补充"
                .to_string(),
        ),
        other => Err(format!(
            "无法映射的事件类型 {other:?}（录制词表：tap/swipe/key/text/wait）"
        )),
    }
}

/// payload 取非负整数字段；缺失/非整数 → None（进诊断不猜测）。
fn payload_number(payload: &Value, key: &str) -> Option<u64> {
    payload.get(key)?.as_u64()
}

/// tap 坐标 `{x, y}`。
fn payload_point(payload: &Value) -> Option<(u64, u64)> {
    Some((payload_number(payload, "x")?, payload_number(payload, "y")?))
}

/// swipe 坐标 `{x, y, x2, y2}`（device-display 像素，合同 §2.1）。
fn payload_swipe(payload: &Value) -> Option<(u64, u64, u64, u64)> {
    let (x, y) = payload_point(payload)?;
    let (x2, y2) = payload_point_at(payload, "x2", "y2")?;
    Some((x, y, x2, y2))
}

fn payload_point_at(payload: &Value, x_key: &str, y_key: &str) -> Option<(u64, u64)> {
    Some((
        payload_number(payload, x_key)?,
        payload_number(payload, y_key)?,
    ))
}

/// (x, y) + 事件 display_size → 相对坐标 (0..=1)。宽或高为 0 = 损坏元数据。
fn relative_pair(x: u64, y: u64, event: &InputEventRecord) -> Result<(f64, f64), String> {
    let (width, height) = (event.display_size.width, event.display_size.height);
    if width == 0 || height == 0 {
        return Err("display_size 无效（宽或高为 0），无法换算相对坐标".to_string());
    }
    Ok((
        ((x as f64) / (width as f64)).clamp(0.0, 1.0),
        ((y as f64) / (height as f64)).clamp(0.0, 1.0),
    ))
}

/// 相对坐标文本：4 位小数舍入，去掉多余尾零（0.5 不写 0.5000）。
fn format_coord(value: f64) -> String {
    let rounded = (value * 10_000.0).round() / 10_000.0;
    format!("{rounded}")
}

/// 间隔 → 建议等待毫秒数（四舍五入；低于阈值返回 None）。
fn suggested_wait(prev_end_us: Option<u64>, timeline_us: u64) -> Option<u64> {
    let gap_us = timeline_us.saturating_sub(prev_end_us?);
    let ms = (gap_us + 500) / 1000;
    (ms >= SUGGESTED_WAIT_MIN_MS).then_some(ms)
}

/// 微秒 → 毫秒（四舍五入）。
fn round_ms(duration_us: u64) -> u64 {
    (duration_us + 500) / 1000
}

/// 录制 keycode → v3 key 词元。命名集合与 `yaml_extension.rs::key_code`
/// （运行时接受表）保持同步；未命名 Android keycode 输出**数字字符串**
/// （运行时按 `value.parse::<u32>()` 接受；必须加引号防止 YAML 解析成整数——
/// 运行时要求 key 是字符串/数字字符串）。
fn key_token(code: u64) -> String {
    match code {
        3 => "HOME".to_string(),
        4 => "BACK".to_string(),
        24 => "VOL_UP".to_string(),
        25 => "VOL_DOWN".to_string(),
        61 => "TAB".to_string(),
        62 => "SPACE".to_string(),
        66 => "ENTER".to_string(),
        67 => "BACKSPACE".to_string(),
        82 => "MENU".to_string(),
        111 => "ESC".to_string(),
        187 => "APP_SWITCH".to_string(),
        other => format!("\"{other}\""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::gamer_yaml::actions::native_call_action;
    use crate::extensions::gamer_yaml::YAML_EXTENSION_ID;
    use crate::recording::DisplaySize;
    use serde_json::json;

    fn event(id: &str, kind: &str, timeline_us: u64, payload: Value) -> InputEventRecord {
        InputEventRecord {
            schema_version: 1,
            event_id: id.to_string(),
            operation_id: format!("op-{id}"),
            session_id: "rec-test".to_string(),
            source: "manual".to_string(),
            kind: kind.to_string(),
            timeline_us,
            time_domain: "recording".to_string(),
            coordinate_space: "device-display".to_string(),
            display_size: DisplaySize {
                width: 1920,
                height: 1080,
            },
            payload,
            status: "accepted".to_string(),
        }
    }

    fn diagnostics_of(result: &Value) -> Vec<(String, String)> {
        result["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| {
                (
                    d["event_id"].as_str().unwrap().to_string(),
                    d["reason"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    }

    /// 全词表夹具：tap →（1.2s 间隔）swipe → key（命名）→ key（未命名码）
    /// → wait。生成物必须通过 v3 parse + lowering，且映射/建议值正确。
    #[test]
    fn draft_maps_tap_swipe_key_and_passes_v3_parse() {
        let events = vec![
            event("evt-1", "tap", 1_000_000, json!({"x": 820, "y": 460})),
            event(
                "evt-2",
                "swipe",
                2_200_000,
                json!({"x": 100, "y": 800, "x2": 500, "y2": 200}),
            ),
            event("evt-3", "key", 2_400_000, json!({"code": 3})),
            event("evt-4", "key", 2_600_000, json!({"code": 1234})),
            event(
                "evt-5",
                "wait",
                3_000_000,
                json!({"duration_us": 1_500_000}),
            ),
        ];
        let result = build_draft("rec-test", None, None, &events);
        assert!(diagnostics_of(&result).is_empty(), "{result}");
        let yaml = result["yaml"].as_str().unwrap();
        // 生成物必须通过 v3 parse + lowering（自证；否则草稿不可保存/运行）。
        super::super::yaml_vnext::load(yaml).expect("草稿必须通过 v3 parse");
        assert!(yaml.starts_with("version: 3\n"), "{yaml}");
        assert!(
            yaml.contains("- tap: [0.4271, 0.4259] # evt-1 tap"),
            "{yaml}"
        );
        // tap → swipe 间隔 1.2s → 建议等待
        assert!(yaml.contains("# 建议等待 1200ms（事件间隔推导）"), "{yaml}");
        assert!(yaml.contains("  - wait: 1200ms # 建议值"), "{yaml}");
        // swipe：相对坐标 + 建议时长
        assert!(yaml.contains("  - swipe: # evt-2 swipe"), "{yaml}");
        assert!(yaml.contains("      from: [0.0521, 0.7407]"), "{yaml}");
        assert!(yaml.contains("      to: [0.2604, 0.1852]"), "{yaml}");
        assert!(
            yaml.contains("      duration: 300ms # 建议值（录制未记录滑动时长）"),
            "{yaml}"
        );
        // 命名键 / 未命名 Android keycode（数字字符串）
        assert!(yaml.contains("  - key: HOME # evt-3 key"), "{yaml}");
        assert!(yaml.contains("  - key: \"1234\" # evt-4 key"), "{yaml}");
        // 录制等待事件 → wait（建议值口径）
        assert!(
            yaml.contains("  - wait: 1500ms # evt-5 建议等待（录制等待）"),
            "{yaml}"
        );
        assert_eq!(yaml.matches("- wait:").count(), 2, "{yaml}");
    }

    /// text（脱敏）/ 未知 kind / 损坏 payload → 全部进诊断，不丢弃不猜测；
    /// 其余事件照常映射，草稿仍可解析。
    #[test]
    fn unmappable_events_become_diagnostics_without_guessing() {
        let events = vec![
            event("evt-1", "tap", 0, json!({"x": 960, "y": 540})),
            event("evt-2", "text", 500_000, json!({"length": 19})),
            event("evt-3", "pinch", 600_000, json!({"scale": 2})),
            event("evt-4", "tap", 700_000, json!({"x": "broken"})),
            event("evt-5", "swipe", 800_000, json!({"x": 1, "y": 2})),
            event("evt-6", "key", 900_000, json!({})),
        ];
        let result = build_draft("rec-test", None, None, &events);
        let diagnostics = diagnostics_of(&result);
        assert_eq!(diagnostics.len(), 5, "{result}");
        assert_eq!(diagnostics[0].0, "evt-2");
        assert!(diagnostics[0].1.contains("脱敏"), "{}", diagnostics[0].1);
        assert_eq!(diagnostics[1].0, "evt-3");
        assert!(
            diagnostics[1].1.contains("无法映射"),
            "{}",
            diagnostics[1].1
        );
        assert_eq!(diagnostics[2].0, "evt-4");
        assert_eq!(diagnostics[3].0, "evt-5");
        assert_eq!(diagnostics[4].0, "evt-6");
        // 唯一成功映射的 tap 仍在；evt-1 → evt-4 之间的间隔按最后一个**已
        // 映射**事件（evt-1）计算，未映射事件不产生步骤也不推时间轴。
        let yaml = result["yaml"].as_str().unwrap();
        super::super::yaml_vnext::load(yaml).unwrap();
        assert!(yaml.contains("- tap: [0.5, 0.5] # evt-1 tap"), "{yaml}");
        assert!(!yaml.contains(" - text"), "text 不产生步骤: {yaml}");
        assert!(!yaml.contains(" - pinch"), "未知 kind 不产生步骤: {yaml}");
        // 未映射事件不推进 prev_end：无 ≥500ms 的已映射间隔 → 无建议等待。
        assert_eq!(yaml.matches("# 建议值").count(), 0, "{yaml}");
    }

    /// display_size 损坏（宽为 0）→ 坐标事件进诊断，不猜测分辨率。
    #[test]
    fn zero_display_size_is_reported_not_guessed() {
        let mut broken = event("evt-1", "tap", 0, json!({"x": 10, "y": 10}));
        broken.display_size = DisplaySize {
            width: 0,
            height: 0,
        };
        let result = build_draft("rec-test", None, None, &[broken]);
        assert_eq!(
            result["yaml"].as_str().unwrap(),
            "version: 3\n# 草稿：由录制会话 rec-test 的操作事件生成（gamer.yaml automation.create_draft）。\n# 等待与滑动时长均为建议值；运行前需人工补充模板判断、状态等待、分支与异常恢复，\n# 再保存到当前 Package 的 automations/。草稿只返回文本，不会被自动执行。\nsteps: []\n"
        );
        assert_eq!(diagnostics_of(&result).len(), 1);
    }

    /// 事件选择：缺省 = 全部（时间轴升序）；给定 id = 请求顺序（重排）；
    /// 未知 id 进诊断；空表 = 全部。
    #[test]
    fn event_ids_select_reorder_and_report_missing() {
        let events = vec![
            event("evt-a", "tap", 0, json!({"x": 192, "y": 108})),
            event("evt-b", "tap", 600_000, json!({"x": 960, "y": 540})),
            event("evt-c", "tap", 1_200_000, json!({"x": 384, "y": 216})),
        ];
        // 缺省 = 全部
        let all = build_draft("rec-test", None, None, &events);
        assert_eq!(all["yaml"].as_str().unwrap().matches(" - tap:").count(), 3);
        // 重排：b → a；缺失 id 进诊断
        let reordered = build_draft(
            "rec-test",
            Some(&[
                "evt-b".to_string(),
                "evt-a".to_string(),
                "evt-x".to_string(),
            ]),
            None,
            &events,
        );
        let yaml = reordered["yaml"].as_str().unwrap();
        super::super::yaml_vnext::load(yaml).unwrap();
        let first_tap = yaml.find("- tap: [").unwrap();
        assert!(
            yaml[first_tap..].contains("evt-b tap"),
            "重排后首个 tap 必须是 evt-b: {yaml}"
        );
        assert_eq!(yaml.matches(" - tap:").count(), 2, "{yaml}");
        let diagnostics = diagnostics_of(&reordered);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].0, "evt-x");
        assert!(diagnostics[0].1.contains("不存在"), "{}", diagnostics[0].1);
        // 显式空表 = 全部事件
        let empty: Vec<String> = Vec::new();
        let all_again = build_draft("rec-test", Some(&empty), None, &events);
        assert_eq!(
            all_again["yaml"]
                .as_str()
                .unwrap()
                .matches(" - tap:")
                .count(),
            3
        );
    }

    /// 坐标换算口径：4 位小数、clamp 到 0~1、边缘值稳定（不猜测越界坐标）。
    #[test]
    fn coordinates_normalize_and_clamp() {
        let overflow = DisplaySize {
            width: 100,
            height: 100,
        };
        let mut off_screen = event("evt-c", "tap", 0, json!({"x": 5000, "y": 5000}));
        off_screen.display_size = overflow;
        let events = vec![
            event("evt-a", "tap", 0, json!({"x": 0, "y": 0})),
            event("evt-b", "tap", 0, json!({"x": 1_000_000, "y": 1_000_000})),
            off_screen,
        ];
        let result = build_draft(
            "rec-test",
            Some(&["evt-a".into(), "evt-b".into(), "evt-c".into()]),
            None,
            &events,
        );
        let yaml = result["yaml"].as_str().unwrap();
        super::super::yaml_vnext::load(yaml).unwrap();
        assert!(yaml.contains("- tap: [0, 0] # evt-a tap"), "{yaml}");
        // 越界坐标钳制到 1（录像观察点不会产生，草稿侧防御性处理）
        assert!(yaml.contains("- tap: [1, 1] # evt-b tap"), "{yaml}");
        assert!(yaml.contains("- tap: [1, 1] # evt-c tap"), "{yaml}");
    }

    /// 小间隔（<500ms）不生成建议等待——人类操作节奏不污染草稿。
    #[test]
    fn small_gaps_do_not_emit_suggested_waits() {
        let events = vec![
            event("evt-a", "tap", 0, json!({"x": 10, "y": 10})),
            event("evt-b", "tap", 499_000, json!({"x": 20, "y": 20})),
            event("evt-c", "tap", 500_000, json!({"x": 30, "y": 30})),
        ];
        let result = build_draft("rec-test", None, None, &events);
        let yaml = result["yaml"].as_str().unwrap();
        super::super::yaml_vnext::load(yaml).unwrap();
        assert_eq!(yaml.matches("# 建议值").count(), 0, "{yaml}");
        assert_eq!(yaml.matches(" - tap:").count(), 3, "{yaml}");
    }

    /// 分发缝只应答本扩展的 automation.create_draft；入参校验失败 →
    /// CallRejected（REST 400 语义）。分发入口在 actions.rs（§10.1 清单）。
    #[test]
    fn native_call_action_gates_by_extension_and_action() {
        let data_dir = std::env::temp_dir();
        let values = json!({"recording_id": "whatever"});
        // 其他扩展 id / 其他动作 → None（交回通用 call 路径）
        assert!(
            native_call_action("gamer.video", AUTOMATION_CREATE_DRAFT, &values, &data_dir)
                .is_none()
        );
        assert!(
            native_call_action(YAML_EXTENSION_ID, "other.action", &values, &data_dir).is_none()
        );
        // 本扩展本动作 → Some（进入 create_draft；该 recording 不存在 →
        // CallRejected；注意进程级录制服务单例被首个测试定根，此处不断言
        // 具体根目录，只断言错误语义）
        let result = native_call_action(
            YAML_EXTENSION_ID,
            AUTOMATION_CREATE_DRAFT,
            &values,
            &data_dir,
        )
        .expect("gamer.yaml automation.create_draft 必须由本缝应答");
        let error = result.unwrap_err().to_string();
        assert!(
            error.contains("读取录制事件失败") || error.contains("recording"),
            "{error}"
        );
        // 缺 recording_id → CallRejected（先于任何 IO）
        let missing = native_call_action(
            YAML_EXTENSION_ID,
            AUTOMATION_CREATE_DRAFT,
            &json!({}),
            &data_dir,
        )
        .unwrap()
        .unwrap_err()
        .to_string();
        assert!(missing.contains("recording_id"), "{missing}");
        // 未知顶层字段 → CallRejected（deny_unknown_fields）
        let extra = native_call_action(
            YAML_EXTENSION_ID,
            AUTOMATION_CREATE_DRAFT,
            &json!({"recording_id": "x", "auto_run": true}),
            &data_dir,
        )
        .unwrap()
        .unwrap_err()
        .to_string();
        assert!(extra.contains("入参无效"), "{extra}");
    }

    /// Phase 7 §10.3：事件注释渲染为步骤上方注释行（换行压平防注释逃逸）；
    /// 未知 id 的注释忽略；生成物仍通过 v3 parse。
    #[test]
    fn event_comments_render_above_steps_and_pass_v3_parse() {
        let events = vec![
            event("evt-a", "tap", 0, json!({"x": 10, "y": 10})),
            event("evt-b", "tap", 100_000, json!({"x": 20, "y": 20})),
            event("evt-c", "tap", 200_000, json!({"x": 30, "y": 30})),
        ];
        let mut comments = std::collections::BTreeMap::new();
        comments.insert(
            "evt-b".to_string(),
            "打开背包后
点第一格"
                .to_string(),
        );
        comments.insert("evt-ghost".to_string(), "不该出现".to_string());
        let result = build_draft("rec-test", None, Some(&comments), &events);
        let yaml = result["yaml"].as_str().unwrap();
        super::super::yaml_vnext::load(yaml).expect("带注释草稿必须通过 v3 parse");
        assert!(
            yaml.contains(
                "  # 打开背包后 点第一格
  - tap: [0.0104, 0.0185] # evt-b tap"
            ),
            "{yaml}"
        );
        assert!(!yaml.contains("不该出现"), "未知 id 的注释必须忽略: {yaml}");
        // 空白注释不渲染
        let mut blank = std::collections::BTreeMap::new();
        blank.insert("evt-a".to_string(), "   ".to_string());
        let clean = build_draft("rec-test", None, Some(&blank), &events);
        assert!(
            !clean["yaml"].as_str().unwrap().contains("  #   "),
            "{clean}"
        );
    }

    /// Phase 7 §10.3：source 回查信息 = 录制会话 id + 全部事件的 kind/时间轴/
    /// 来源/选中/映射标记；选中且映射成功的事件 selected=true、mapped=true。
    #[test]
    fn source_block_records_provenance_for_backtrace() {
        let events = vec![
            event("evt-ok", "tap", 1_000, json!({"x": 5, "y": 5})),
            event("evt-text", "text", 2_000, json!({"length": 3})),
        ];
        let result = build_draft(
            "rec-77",
            Some(&["evt-ok".to_string(), "evt-text".to_string()]),
            None,
            &events,
        );
        let source = &result["source"];
        assert_eq!(source["recording_id"], "rec-77");
        let rows = source["events"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["event_id"], "evt-ok");
        assert_eq!(rows[0]["timeline_us"], 1_000);
        assert_eq!(rows[0]["kind"], "tap");
        assert_eq!(rows[0]["time_domain"], "recording");
        assert_eq!(rows[0]["selected"], true);
        assert_eq!(rows[0]["mapped"], true);
        assert_eq!(rows[1]["selected"], true);
        assert_eq!(rows[1]["mapped"], false, "text 进诊断 → mapped=false");
        // 未选事件：selected=false 但仍在回查表（草稿 JSON 可回查全部来源）
        let partial = build_draft("rec-77", Some(&["evt-ok".to_string()]), None, &events);
        let rows = partial["source"]["events"].as_array().unwrap();
        assert_eq!(rows[1]["selected"], false);
    }
}
