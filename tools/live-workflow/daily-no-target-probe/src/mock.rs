use crate::{native_funcs, syntax};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Mutex;
use yaml_interp::{EventSink, HostError, HostErrorKind, HostFunctions};

#[derive(Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Roles {
    pub add: String,
    pub cavern: String,
    pub storage: String,
    pub enter: String,
    pub storage_center: [f64; 2],
    pub enter_offset_pixels: [f64; 2],
    pub enter_template_pixels: [f64; 2],
    pub left_from: [f64; 2],
    pub left_to: [f64; 2],
    pub right_from: [f64; 2],
    pub right_to: [f64; 2],
}
impl Default for Roles {
    fn default() -> Self {
        Self {
            add: "daily_no_target_add_20261015.png".into(),
            cavern: "daily_no_target_cavern_20261015.png".into(),
            storage: "daily_no_target_storage_20261015.png".into(),
            enter: "daily_no_target_enter_20261015.png".into(),
            storage_center: [951.0 / 1920.0, 839.0 / 1080.0],
            enter_offset_pixels: [608.0, -23.0],
            enter_template_pixels: [52.0, 32.0],
            left_from: [0.2833333333, 0.7527777778],
            left_to: [0.3078125, 0.2703703704],
            right_from: [0.5682291667, 0.7620370370],
            right_to: [0.5885416667, 0.4092592593],
        }
    }
}
#[derive(Clone)]
pub struct Case {
    pub name: String,
    pub function: String,
    pub group: String,
    pub entry_clicks: usize,
    pub first: String,
    pub second_missing: bool,
    pub late_enter: bool,
    pub entry_missing: bool,
    pub enter_after_observations: usize,
    pub phone_after_observations: usize,
    pub add: bool,
    pub already_challenge: bool,
    pub left_after: usize,
    pub right_after: usize,
    pub missing_enter: bool,
    pub missing_challenge: bool,
    // The location text is captured first; the list may move before the next
    // button observation. Keep this independent of the script's search ROI.
    pub enter_drift_pixels: [f64; 2],
    pub expect_enter_rejected: bool,
    pub first_storage_offset_pixels: [f64; 2],
    pub storage_refresh_missing: bool,
}
impl Case {
    pub fn smoke() -> Self {
        Self {
            name: "smoke".into(),
            function: "".into(),
            group: "smoke".into(),
            entry_clicks: 0,
            first: "phone".into(),
            second_missing: false,
            late_enter: false,
            entry_missing: false,
            enter_after_observations: 0,
            phone_after_observations: 0,
            add: false,
            already_challenge: false,
            left_after: 2,
            right_after: 2,
            missing_enter: false,
            missing_challenge: false,
            enter_drift_pixels: [0.0, 0.0],
            expect_enter_rejected: false,
            first_storage_offset_pixels: [0.0, 0.0],
            storage_refresh_missing: false,
        }
    }
}
#[derive(Default, Serialize)]
pub struct State {
    pub now_ms: u64,
    pub frame: u64,
    pub calls: Vec<Value>,
    pub events: Vec<Value>,
    pub inputs: Vec<Value>,
    pub violations: Vec<String>,
    pub entry_clicks: usize,
    pub enter_remaining: usize,
    pub first_captures: usize,
    pub first_hit: Option<String>,
    pub first_hit_ms: Option<u64>,
    pub monthly_cleared: usize,
    pub second_confirmed: bool,
    pub second_started: bool,
    pub left_swipes: usize,
    pub right_swipes: usize,
    pub cavern_selected: bool,
    pub training_selected: bool,
    pub row_entered: bool,
    pub legacy_entered: bool,
    pub wrong_row: bool,
    pub battle_reached: bool,
    pub daily_reached: bool,
    pub storage_match_centers: Vec<[f64; 2]>,
    pub storage_match_times_ms: Vec<u64>,
}
pub struct MockHost {
    pub case: Case,
    pub roles: Roles,
    pub state: Mutex<State>,
}
fn error(message: impl Into<String>) -> HostError {
    HostError::new(HostErrorKind::Failed, message)
}
fn phone(template: &str) -> bool {
    template.contains("手机#009_036_043_094")
}
fn monthly(template: &str) -> bool {
    template.contains("supply_bottom")
}
fn click_enter(template: &str) -> bool {
    template.contains("daily_click_enter")
}
fn challenge(template: &str) -> bool {
    template.contains("daily_challenge_word")
}
fn same(template: &str, role: &str) -> bool {
    template == role || template.split('#').next() == role.strip_suffix(".png")
}
fn duration(value: &Value) -> u64 {
    value.as_u64().unwrap_or_else(|| {
        syntax::parse_duration_ms(value.as_str().unwrap_or("0ms")).unwrap() as u64
    })
}
fn point(value: &Value) -> Option<[f64; 2]> {
    if let Some(center) = value.get("center") {
        return point(center);
    }
    if let Some(values) = value.as_array() {
        return Some([values.first()?.as_f64()?, values.get(1)?.as_f64()?]);
    }
    Some([value["x"].as_f64()?, value["y"].as_f64()?])
}
fn close(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).abs() < 1e-8 && (a[1] - b[1]).abs() < 1e-8
}
fn inside(position: [f64; 2], region: &Value) -> bool {
    let Some(region) = region.as_array() else {
        return true;
    };
    let bounds: Vec<_> = region.iter().map(|v| v.as_f64().unwrap()).collect();
    position[0] >= bounds[0]
        && position[0] <= bounds[0] + bounds[2]
        && position[1] >= bounds[1]
        && position[1] <= bounds[1] + bounds[3]
}
// Production matching searches template top-left positions and requires the
// entire source-sized crop to fit. Point-only containment hides clipped text.
fn template_inside(center: [f64; 2], pixels: [f64; 2], region: &Value) -> bool {
    let Some(bounds) = region.as_array() else {
        return true;
    };
    let screen = [1920.0, 1080.0];
    let origin = [
        (bounds[0].as_f64().unwrap() * screen[0]).round(),
        (bounds[1].as_f64().unwrap() * screen[1]).round(),
    ];
    let size = [
        (bounds[2].as_f64().unwrap() * screen[0]).round(),
        (bounds[3].as_f64().unwrap() * screen[1]).round(),
    ];
    (0..2).all(|axis| {
        let start = (center[axis] * screen[axis] - pixels[axis] / 2.0).round();
        start >= origin[axis] && start + pixels[axis] <= origin[axis] + size[axis]
    })
}
impl MockHost {
    pub fn new(case: Case, roles: Roles) -> Self {
        let state = State {
            enter_remaining: case.entry_clicks,
            ..Default::default()
        };
        Self {
            case,
            roles,
            state: Mutex::new(state),
        }
    }
    fn matched(&self, template: &str, state: &State, center: [f64; 2]) -> Value {
        json!({"template":template,"center":{"x":center[0],"y":center[1]},"x":center[0],"y":center[1],"score":1.0,"_frame":{"mock_frame":state.frame},"_observed_at_ms":state.now_ms})
    }
    pub fn intended_enter(&self) -> [f64; 2] {
        [
            self.roles.storage_center[0]
                + (self.roles.enter_offset_pixels[0] + self.case.enter_drift_pixels[0]) / 1920.0,
            self.roles.storage_center[1]
                + (self.roles.enter_offset_pixels[1] + self.case.enter_drift_pixels[1]) / 1080.0,
        ]
    }
    fn visible(
        &self,
        template: &str,
        state: &mut State,
        region: &Value,
    ) -> Result<Option<[f64; 2]>, HostError> {
        let result = if phone(template) {
            (state.enter_remaining == 0
                && state.first_captures >= self.case.phone_after_observations
                && self.case.first != "never"
                && !(self.case.first == "monthly" && state.monthly_cleared == 0)
                && !(state.second_started && self.case.second_missing))
                .then_some([0.03, 0.06])
        } else if monthly(template) {
            let due = matches!(self.case.first.as_str(), "monthly" | "both")
                || (self.case.first == "late_monthly"
                    && state
                        .first_hit_ms
                        .is_some_and(|at| state.now_ms >= at + 3000));
            (state.enter_remaining == 0 && due && state.monthly_cleared == 0).then_some([0.5, 0.8])
        } else if click_enter(template) {
            (state.enter_remaining > 0
                && state.first_captures >= self.case.enter_after_observations)
                .then_some([0.5, 0.93])
        } else if template.contains("daily_enter_game") {
            (!self.case.entry_missing).then_some([0.8, 0.7])
        } else if template.contains("cloud_got_it") {
            None
        } else if challenge(template) {
            (!self.case.missing_challenge
                && (self.case.already_challenge || state.row_entered || state.legacy_entered))
                .then_some([0.84, 0.91])
        } else if template.contains("daily_power_anchor_20261023") {
            (!self.case.missing_challenge
                && (self.case.already_challenge || state.row_entered || state.legacy_entered))
                .then_some([0.84, 0.06])
        } else if [
            "daily_dungeon_echo_title_20261023.png",
            "daily_dungeon_cavern_title_20261023.png",
            "daily_dungeon_calyx_title_20261023.png",
            "daily_dungeon_stagnant_title_20261023.png",
            "daily_dungeon_planar_title_20261023.png",
        ]
        .contains(&template)
        {
            None // Numeric/title matching is covered by the saved-frame OCR probe.
        } else if same(template, &self.roles.add) {
            self.case.add.then_some([0.5, 0.8])
        } else if same(template, &self.roles.cavern) {
            (state.left_swipes >= self.case.left_after).then_some([0.3, 0.8])
        } else if same(template, &self.roles.storage) {
            if !state.cavern_selected || state.right_swipes < self.case.right_after {
                None
            } else if !state.storage_match_centers.is_empty() && self.case.storage_refresh_missing {
                None
            } else {
                let mut center = self.roles.storage_center;
                if state.storage_match_centers.is_empty() {
                    center[0] += self.case.first_storage_offset_pixels[0] / 1920.0;
                    center[1] += self.case.first_storage_offset_pixels[1] / 1080.0;
                }
                state.storage_match_centers.push(center);
                state.storage_match_times_ms.push(state.now_ms);
                Some(center)
            }
        } else if same(template, &self.roles.enter) {
            // Identical buttons above/below the intended row must remain
            // excluded even when its crop is missing. The old remote decoy
            // also catches accidental full-list search.
            let wanted = self.intended_enter();
            let decoys = [
                [0.805, 0.4],
                [wanted[0], wanted[1] - 135.0 / 1080.0],
                [wanted[0], wanted[1] + 135.0 / 1080.0],
            ];
            if let Some(decoy) = decoys.into_iter().find(|position| {
                template_inside(*position, self.roles.enter_template_pixels, region)
            }) {
                Some(decoy)
            } else if !self.case.missing_enter
                && template_inside(wanted, self.roles.enter_template_pixels, region)
            {
                Some(wanted)
            } else {
                None
            }
        } else if template.contains("daily_train_target") {
            (!self.case.add).then_some([0.3, 0.3])
        } else if template.contains("daily_enter_first_button") {
            (!self.case.add && state.training_selected).then_some([0.8, 0.52])
        } else {
            return Err(error(format!("UNCONFIGURED_TEMPLATE:{template}")));
        };
        Ok(result.filter(|position| inside(*position, region)))
    }
    fn tap(
        &self,
        template: &str,
        position: [f64; 2],
        state: &mut State,
        via: &str,
    ) -> Result<(), HostError> {
        state.inputs.push(json!({"kind":"tap","template":template,"position":position,"via":via,"at_ms":state.now_ms,"second_phase":state.second_started}));
        if click_enter(template) {
            if state.enter_remaining == 0 {
                return Err(error("attempted hidden click-enter"));
            }
            state.enter_remaining -= 1;
            state.entry_clicks += 1;
        } else if monthly(template) {
            state.monthly_cleared += 1;
        } else if same(template, &self.roles.cavern) {
            state.cavern_selected = true;
        } else if same(template, &self.roles.enter) {
            state.wrong_row = !close(position, self.intended_enter());
            state.row_entered = !state.wrong_row;
        } else if template.contains("daily_train_target") {
            state.training_selected = true;
        } else if template.contains("daily_enter_first_button") {
            state.legacy_entered = true;
        } else if !template.contains("daily_enter_game") && !template.contains("cloud_got_it") {
            return Err(error(format!("UNCONFIGURED_INPUT:{template}")));
        }
        Ok(())
    }
    fn wait(&self, args: &Value, state: &mut State) -> Result<Value, HostError> {
        let template = args["template"]
            .as_str()
            .ok_or_else(|| error("wait template missing"))?;
        let timeout = duration(&args["timeout"]);
        let interval = duration(&args["interval"]).max(50);
        let started = state.now_ms;
        if phone(template) && timeout > 0 {
            state.second_started = true;
            if self.case.late_enter {
                state.enter_remaining = 2;
            }
        }
        for _ in 0..10000 {
            state.frame += 1;
            let mut cleared = false;
            for obstacle in args["obstacles"].as_array().into_iter().flatten() {
                let obstacle = obstacle.as_str().unwrap();
                if let Some(position) = self.visible(obstacle, state, &Value::Null)? {
                    self.tap(obstacle, position, state, "wait_find.obstacle")?;
                    cleared = true;
                    break;
                }
            }
            if !cleared {
                if let Some(position) = self.visible(template, state, &args["region"])? {
                    let hit = self.matched(template, state, position);
                    if args["click"] == true {
                        self.tap(template, position, state, "wait_find.target")?;
                    }
                    if phone(template) && timeout > 0 {
                        state.second_confirmed = true;
                    }
                    return Ok(hit);
                }
            }
            if state.now_ms - started >= timeout {
                return Ok(Value::Null);
            }
            state.now_ms += interval.min(timeout - (state.now_ms - started));
        }
        Err(error("MOCK_WAIT_BOUND_EXCEEDED"))
    }
    fn normalized(name: &str, args: Value) -> Result<Value, HostError> {
        let Some(function) = native_funcs::native_function(name) else {
            return Ok(args);
        };
        let mut values = if let Some(object) = args.as_object() {
            object.clone()
        } else {
            let mut values = serde_json::Map::new();
            if let Some(first) = function.params.first() {
                values.insert(first.name.into(), args);
            }
            values
        };
        for parameter in &function.params {
            if !values.contains_key(parameter.name) {
                if let Some(value) = &parameter.default {
                    values.insert(parameter.name.into(), value.clone());
                } else if parameter.required {
                    return Err(error(format!(
                        "MISSING_NATIVE_ARG:{name}.{}",
                        parameter.name
                    )));
                }
            }
        }
        Ok(Value::Object(values))
    }
}
impl HostFunctions for MockHost {
    fn invoke(&self, name: &str, args: Value) -> Result<Value, HostError> {
        let args = Self::normalized(name, args)?;
        let mut state = self.state.lock().unwrap();
        // The authoritative interpreter owns its unchanged 100000-step budget.
        // This guard must not hide the candidate's long cloud-loading boundary.
        if state.calls.len() > 200000 {
            return Err(error("MOCK_CALL_BUDGET"));
        }
        let at_ms = state.now_ms;
        state
            .calls
            .push(json!({"function":name,"args":args,"at_ms":at_ms}));
        match name {
            "eq" | "ne" => {
                let equal = match (args["a"].as_f64(), args["b"].as_f64()) {
                    (Some(a), Some(b)) => a == b,
                    _ => args["a"] == args["b"],
                };
                Ok(json!(if name == "eq" { equal } else { !equal }))
            }
            "calculate" => {
                let a = args["a"].as_f64().unwrap();
                let b = args["b"].as_f64().unwrap();
                match args["op"].as_str() {
                    Some("sub") => Ok(json!(a - b)),
                    Some("add") => Ok(json!(a + b)),
                    _ => Err(error("UNCONFIGURED_CALCULATE")),
                }
            }
            "sleep" => {
                state.now_ms += duration(&args["duration"]);
                Ok(Value::Null)
            }
            "log" => Ok(Value::Null),
            "find" | "observe" => {
                state.frame += 1;
                let template = args["template"].as_str().unwrap();
                Ok(self
                    .visible(template, &mut state, &args["region"])?
                    .map(|position| self.matched(template, &state, position))
                    .unwrap_or(Value::Null))
            }
            "find_any" => {
                state.frame += 1;
                state.first_captures += 1;
                for (index, template) in args["templates"].as_array().unwrap().iter().enumerate() {
                    let template = template.as_str().unwrap();
                    if let Some(position) = self.visible(template, &mut state, &args["region"])? {
                        let mut value = self.matched(template, &state, position);
                        value["index"] = json!(index);
                        if state.first_hit.is_none() {
                            state.first_hit = Some(template.into());
                            state.first_hit_ms = Some(state.now_ms);
                        }
                        return Ok(value);
                    }
                }
                Ok(Value::Null)
            }
            "wait_find" => self.wait(&args, &mut state),
            "tap_template" => {
                let template = args["template"].as_str().unwrap();
                if let Some(position) = self.visible(template, &mut state, &args["region"])? {
                    let value = self.matched(template, &state, position);
                    self.tap(template, position, &mut state, "tap_template")?;
                    Ok(value)
                } else {
                    Ok(Value::Null)
                }
            }
            "tap" => {
                let value = &args["position"];
                let template = value["template"]
                    .as_str()
                    .ok_or_else(|| error("TAP_LOST_WHOLE_MATCH"))?;
                let position = point(value).ok_or_else(|| error("TAP_LOST_MATCH_CENTER"))?;
                self.tap(template, position, &mut state, "tap")?;
                Ok(Value::Null)
            }
            "swipe" => {
                let from = point(&args["from"]).unwrap();
                let to = point(&args["to"]).unwrap();
                let left = close(from, self.roles.left_from) && close(to, self.roles.left_to);
                let right = close(from, self.roles.right_from) && close(to, self.roles.right_to);
                if left {
                    state.left_swipes += 1;
                    if duration(&args["duration"]) != 580 {
                        state.violations.push("wrong left swipe duration".into());
                    }
                } else if right {
                    state.right_swipes += 1;
                    if duration(&args["duration"]) != 460 {
                        state.violations.push("wrong right swipe duration".into());
                    }
                } else {
                    state.violations.push(format!(
                        "swipe outside declared list coordinates: {from:?}->{to:?}"
                    ));
                    return Err(error("SWIPE_OUTSIDE_LIST"));
                }
                if right && !state.cavern_selected {
                    state
                        .violations
                        .push("right list scrolled before cavern selection".into());
                }
                state.inputs.push(json!({"kind":"swipe","list":if left{"left"}else{"right"},"from":from,"to":to,"at_ms":at_ms}));
                Ok(Value::Null)
            }
            "ocr" | "find_text" => {
                state.battle_reached = true;
                Err(error("PROBE_STOP:BATTLE_OCR"))
            }
            "daily_tasks" => {
                state.daily_reached = true;
                Err(error("PROBE_STOP:DAILY_TASKS"))
            }
            "fail" => Err(error(format!(
                "SCRIPT_FAIL:{}:{}",
                args["code"].as_str().unwrap_or(""),
                args["message"].as_str().unwrap_or("")
            ))),
            _ => Err(error(format!("UNCONFIGURED_NATIVE:{name}"))),
        }
    }
}
impl EventSink for MockHost {
    fn emit(&self, event: Value) {
        self.state.lock().unwrap().events.push(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_failure_center_is_inside_but_full_enter_crop_is_clipped() {
        let center = [1559.0 / 1920.0, 816.0 / 1080.0];
        let old = json!([0.75, 809.0 / 1080.0, 225.0 / 1920.0, 68.0 / 1080.0]);
        assert!(inside(center, &old));
        assert!(!template_inside(center, [52.0, 32.0], &old));

        let widened = json!([0.72, 776.0 / 1080.0, 0.17, 130.0 / 1080.0]);
        assert!(template_inside(center, [52.0, 32.0], &widened));
        assert!(!template_inside(
            [center[0], center[1] - 135.0 / 1080.0],
            [52.0, 32.0],
            &widened
        ));
    }

    #[test]
    fn exact_crop_edges_fit_but_one_pixel_clipping_does_not() {
        let region = json!([100.0 / 1920.0, 200.0 / 1080.0, 52.0 / 1920.0, 32.0 / 1080.0]);
        let center = [126.0 / 1920.0, 216.0 / 1080.0];
        assert!(template_inside(center, [52.0, 32.0], &region));
        for delta in [[-1.0, 0.0], [1.0, 0.0], [0.0, -1.0], [0.0, 1.0]] {
            let shifted = [center[0] + delta[0] / 1920.0, center[1] + delta[1] / 1080.0];
            assert!(inside(shifted, &region));
            assert!(!template_inside(shifted, [52.0, 32.0], &region));
        }
    }
}
