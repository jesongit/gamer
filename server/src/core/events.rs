//! Runtime event boundary shared by runners and output adapters.

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};

use super::DeviceId;

/// Wire-neutral runtime event payload.  The device scope stays outside the
/// browser-facing event shape and is consumed by the selected sink adapter.
///
/// P12.6（ADR-YAML-03 运行可视化事件 wire 契约）：在 v2 引擎的投屏标记事件
/// （tap/swipe/hit/miss，设备像素坐标）之外新增运行结构事件——run/step/call/
/// vision/budget。这些事件由 YAML v3 运行时发射（WASM guest 经私有
/// `__event` capability 通道、宿主侧在 vision/input capability 完成后补发），
/// 前端据此驱动运行事件 feed 与步骤高亮；事件一律不携带帧图像数据。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "ev", rename_all = "snake_case")]
pub enum RuntimeEventKind {
    Tap {
        x: u32,
        y: u32,
    },
    Swipe {
        x1: u32,
        y1: u32,
        x2: u32,
        y2: u32,
    },
    Hit {
        tpl: String,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        score: f32,
    },
    Miss {
        tpl: String,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
    },
    /// 运行开始（guest 进入 program / 原生解释器进入）。
    RunStart,
    /// 运行结束；失败时带错误文本（原样透传机器可读码）。
    RunEnd {
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// 进入 surface step（path 语法 `steps[0].then[1]`，与前端编辑器寻址一致）。
    StepStart {
        path: String,
        desc: String,
    },
    /// surface step 完成 / 失败。
    StepEnd {
        path: String,
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// 进入 call 目标（depth = guest/解释器本地调用深度）。
    CallStart {
        target: String,
        depth: u32,
    },
    /// 单次模板匹配结果（模板名 / 相对坐标，无帧数据）。
    Vision {
        template: String,
        found: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        score: Option<f32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        center: Option<[f64; 2]>,
    },
    /// 预算终止（kind = STEP_BUDGET_EXCEEDED / CALL_DEPTH_EXCEEDED / CANCELLED）。
    Budget {
        kind: String,
    },
}

#[derive(Clone, Debug)]
pub struct RuntimeEvent {
    pub device_id: DeviceId,
    pub kind: RuntimeEventKind,
}

impl RuntimeEvent {
    pub fn new(device_id: DeviceId, kind: RuntimeEventKind) -> Self {
        Self { device_id, kind }
    }
}

/// Thin output seam.  Adapters choose whether to send to WebRTC, logs, a
/// websocket, or nowhere; the runner does not know the destination registry.
pub trait EventSink: Send + Sync + 'static {
    fn emit(&self, event: RuntimeEvent) -> BoxFuture<'_, anyhow::Result<()>>;
}

/// 测试 / 无事件订阅装配用的空事件汇（engine 测试 Rig 兜底）。
#[allow(dead_code)]
#[derive(Debug, Default)]
pub struct NullEventSink;

impl EventSink for NullEventSink {
    fn emit(&self, _event: RuntimeEvent) -> BoxFuture<'_, anyhow::Result<()>> {
        Box::pin(async { Ok(()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::FutureExt;

    #[test]
    fn runtime_event_keeps_device_scope_out_of_wire_kind() {
        let event = RuntimeEvent::new(
            DeviceId::new("d1").unwrap(),
            RuntimeEventKind::Tap { x: 1, y: 2 },
        );
        assert_eq!(
            serde_json::to_value(&event.kind).unwrap(),
            serde_json::json!({
                "ev": "tap",
                "x": 1,
                "y": 2,
            })
        );
        NullEventSink.emit(event).now_or_never().unwrap().unwrap();
    }

    /// P12.6 wire 契约锁定：运行结构事件沿用 `{"type":"se","ev":...}` 信封
    /// （type 由 ViewerEventSink 补），step 事件省略 None 字段。
    #[test]
    fn run_structure_events_keep_contract_wire_shape() {
        assert_eq!(
            serde_json::to_value(RuntimeEventKind::RunStart).unwrap(),
            serde_json::json!({ "ev": "run_start" })
        );
        assert_eq!(
            serde_json::to_value(RuntimeEventKind::StepStart {
                path: "steps[0].then[1]".into(),
                desc: "tap 0.5,0.3".into(),
            })
            .unwrap(),
            serde_json::json!({
                "ev": "step_start",
                "path": "steps[0].then[1]",
                "desc": "tap 0.5,0.3",
            })
        );
        assert_eq!(
            serde_json::to_value(RuntimeEventKind::StepEnd {
                path: "steps[2]".into(),
                ok: false,
                error: Some("FIND_TIMEOUT: login".into()),
            })
            .unwrap(),
            serde_json::json!({
                "ev": "step_end",
                "path": "steps[2]",
                "ok": false,
                "error": "FIND_TIMEOUT: login",
            })
        );
        assert_eq!(
            serde_json::to_value(RuntimeEventKind::StepEnd {
                path: "steps[2]".into(),
                ok: true,
                error: None,
            })
            .unwrap(),
            serde_json::json!({ "ev": "step_end", "path": "steps[2]", "ok": true })
        );
    }
}
