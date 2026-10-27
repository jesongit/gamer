use crate::{
    mock::{Case, MockHost, Roles},
    syntax,
};
use serde_json::{json, Map, Value};

fn entry_cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for function in ["cloud_enter_game", "phone_resume_confirm"] {
        for (mode, clicks, second_missing, late_enter) in [
            ("phone", 0, false, false),
            ("phone", 1, false, false),
            ("phone", 3, false, false),
            ("monthly", 0, false, false),
            ("both", 0, false, false),
            ("monthly", 2, false, false),
            ("late_monthly", 0, false, false),
            ("phone", 0, false, true),
            ("never", 0, false, false),
            ("phone", 0, true, false),
        ] {
            let mut case = Case::smoke();
            case.function = function.into();
            case.group = "entry".into();
            case.name = format!(
                "{function}-{mode}-{clicks}-{}-{}",
                if second_missing {
                    "second-timeout"
                } else {
                    "ready"
                },
                if late_enter { "late-enter" } else { "normal" }
            );
            case.first = mode.into();
            case.entry_clicks = clicks;
            case.second_missing = second_missing;
            case.late_enter = late_enter;
            cases.push(case);
        }
    }
    let mut missing = Case::smoke();
    missing.name = "cloud-entry-missing-bounded-failure".into();
    missing.function = "cloud_enter_game".into();
    missing.group = "entry".into();
    missing.entry_missing = true;
    cases.push(missing);
    for enter_late in [false, true] {
        let mut delayed = Case::smoke();
        delayed.name = if enter_late {
            "cloud-click-enter-after-151-observations"
        } else {
            "cloud-phone-after-151-observations"
        }
        .into();
        delayed.function = "cloud_enter_game".into();
        delayed.group = "entry".into();
        delayed.entry_clicks = usize::from(enter_late);
        delayed.enter_after_observations = if enter_late { 152 } else { 0 };
        delayed.phone_after_observations = 152;
        cases.push(delayed);
    }
    cases
}
fn navigation_cases() -> Vec<(Case, Option<f64>)> {
    let mut cases = Vec::new();
    for (name, add, already, left, right, missing_enter, missing_challenge, y) in [
        (
            "configured-original-navigation",
            false,
            false,
            2,
            2,
            false,
            false,
            None,
        ),
        (
            "already-challenge-skips-both-navigation-paths",
            true,
            true,
            2,
            2,
            false,
            false,
            None,
        ),
        (
            "no-target-default-path",
            true,
            false,
            2,
            2,
            false,
            false,
            None,
        ),
        (
            "no-target-first-scroll-hit",
            true,
            false,
            1,
            1,
            false,
            false,
            None,
        ),
        (
            "no-target-last-scroll-hit",
            true,
            false,
            8,
            12,
            false,
            false,
            None,
        ),
        (
            "left-category-never-appears",
            true,
            false,
            usize::MAX,
            2,
            false,
            false,
            None,
        ),
        (
            "right-storage-never-appears",
            true,
            false,
            2,
            usize::MAX,
            false,
            false,
            None,
        ),
        (
            "category-after-bound-is-rejected",
            true,
            false,
            9,
            2,
            false,
            false,
            None,
        ),
        (
            "storage-after-bound-is-rejected",
            true,
            false,
            2,
            13,
            false,
            false,
            None,
        ),
        (
            "storage-enter-missing-decoy-must-not-click",
            true,
            false,
            2,
            2,
            true,
            false,
            None,
        ),
        (
            "dynamic-storage-row-upper",
            true,
            false,
            2,
            2,
            false,
            false,
            Some(0.65),
        ),
        (
            "dynamic-storage-row-lower",
            true,
            false,
            2,
            2,
            false,
            false,
            Some(0.85),
        ),
        (
            "default-navigation-common-challenge-timeout",
            true,
            false,
            2,
            2,
            false,
            true,
            None,
        ),
        (
            "configured-navigation-common-challenge-timeout",
            false,
            false,
            2,
            2,
            false,
            true,
            None,
        ),
    ] {
        let mut case = Case::smoke();
        case.name = name.into();
        case.group = "navigation".into();
        case.function = "drain_stamina".into();
        case.add = add;
        case.already_challenge = already;
        case.left_after = left;
        case.right_after = right;
        case.missing_enter = missing_enter;
        case.missing_challenge = missing_challenge;
        cases.push((case, y));
    }
    // Reproduce the real 2026-10-16 failure: the first storage observation was
    // y=861, but the settled label/button were y=839/816. The old ROI starts
    // at y=809, cutting nine pixels off the 32px button crop.
    for (name, first_storage_offset, enter_drift, rejected) in [
        (
            "real-failure-storage861-settles839-enter816",
            [0.0, 22.0],
            [0.0, 0.0],
            false,
        ),
        (
            "storage-refresh-after-upward-anchor-drift60",
            [0.0, -60.0],
            [0.0, 0.0],
            false,
        ),
        (
            "enter-button-inertia-up40-still-same-row",
            [0.0, 0.0],
            [0.0, -40.0],
            false,
        ),
        (
            "enter-button-inertia-down40-still-same-row",
            [0.0, 0.0],
            [0.0, 40.0],
            false,
        ),
        (
            "enter-button-column-left80",
            [0.0, 0.0],
            [-80.0, 0.0],
            false,
        ),
        (
            "enter-button-column-right100",
            [0.0, 0.0],
            [100.0, 0.0],
            false,
        ),
        (
            "enter-center-in-band-but-template-top-clipped",
            [0.0, 0.0],
            [0.0, -57.0],
            true,
        ),
        (
            "enter-center-in-band-but-template-bottom-clipped",
            [0.0, 0.0],
            [0.0, 63.0],
            true,
        ),
        (
            "enter-center-in-column-but-template-left-clipped",
            [0.0, 0.0],
            [-167.0, 0.0],
            true,
        ),
        (
            "enter-center-in-column-but-template-right-clipped",
            [0.0, 0.0],
            [139.0, 0.0],
            true,
        ),
    ] {
        let mut case = Case::smoke();
        case.name = name.into();
        case.group = "navigation".into();
        case.function = "drain_stamina".into();
        case.add = true;
        case.first_storage_offset_pixels = first_storage_offset;
        case.enter_drift_pixels = enter_drift;
        case.expect_enter_rejected = rejected;
        cases.push((case, None));
    }
    let mut lost = Case::smoke();
    lost.name = "storage-disappears-during-refresh-no-enter-click".into();
    lost.group = "navigation".into();
    lost.function = "drain_stamina".into();
    lost.add = true;
    lost.storage_refresh_missing = true;
    cases.push((lost, None));
    cases
}
pub fn run(script: &syntax::Script, roles: Roles) -> Vec<Value> {
    let mut cases: Vec<_> = entry_cases().into_iter().map(|case| (case, None)).collect();
    cases.extend(navigation_cases());
    cases
        .into_iter()
        .map(|(case, y)| {
            let mut roles = roles.clone();
            if let Some(y) = y {
                roles.storage_center[1] = y;
            }
            run_case(script, case, roles)
        })
        .collect()
}
fn run_case(script: &syntax::Script, case: Case, roles: Roles) -> Value {
    let Some((_, definition)) = script
        .functions
        .iter()
        .find(|(name, _)| name == &case.function)
    else {
        return json!({"name":case.name,"passed":false,"error":format!("missing candidate function {}",case.function)});
    };
    let vars = script.vars.iter().cloned().collect();
    let mut wire =
        syntax::build_function_program(&case.function, definition, &script.functions, vars, 0);
    // A deliberate, recorded native sentinel replaces only the next business
    // stage; all entry/navigation source and local helpers remain interpreted.
    wire["functions"]
        .as_object_mut()
        .unwrap()
        .remove("daily_tasks");
    let program: yaml_interp::Program = serde_json::from_value(wire).unwrap();
    let host = MockHost::new(case.clone(), roles);
    let outcome = yaml_interp::run(&program, &host, Some(&host));
    let error = outcome.as_ref().err().cloned();
    let state = host.state.lock().unwrap();
    let mut checks = Map::new();
    checks.insert(
        "no_unconfigured_calls_or_mock_budget".into(),
        json!(error.as_ref().is_none_or(|e| !e.contains("UNCONFIGURED_")
            && !e.contains("MOCK_")
            && !e.contains("TAP_LOST_"))),
    );
    checks.insert(
        "input_coordinates_valid".into(),
        json!(state.violations.is_empty()),
    );
    if case.group == "entry" {
        let phone_waits: Vec<_> = state
            .calls
            .iter()
            .filter(|call| {
                call["function"] == "wait_find"
                    && call["args"]["template"]
                        .as_str()
                        .is_some_and(|name| name.contains("手机#009_036_043_094"))
            })
            .collect();
        let first: Vec<_> = phone_waits
            .iter()
            .filter(|call| call["args"]["timeout"] == "0ms")
            .collect();
        let second: Vec<_> = phone_waits
            .iter()
            .filter(|call| call["args"]["timeout"] == "5m")
            .collect();
        let early_failure = case.entry_missing || case.first == "never";
        let expected_error = if case.entry_missing {
            Some("CLOUD_ENTRY_TIMEOUT")
        } else if case.first == "never" {
            Some(if case.function == "cloud_enter_game" {
                "PHONE_FIRST_WAIT_TIMEOUT"
            } else {
                "PHONE_FIRST_RESUME_TIMEOUT"
            })
        } else if case.second_missing {
            Some(if case.function == "cloud_enter_game" {
                "PHONE_SECOND_WAIT_TIMEOUT"
            } else {
                "PHONE_SECOND_RESUME_TIMEOUT"
            })
        } else if case.function == "phone_resume_confirm" {
            Some("PROBE_STOP:DAILY_TASKS")
        } else {
            None
        };
        checks.insert(
            "expected_terminal".into(),
            json!(match expected_error {
                Some(code) => error.as_ref().is_some_and(|error| error.contains(code)),
                None => outcome.is_ok(),
            }),
        );
        checks.insert(
            "no_standalone_click_enter_gate".into(),
            json!(!state.calls.iter().any(|call| matches!(
                call["function"].as_str(),
                Some("wait_find" | "tap_template")
            ) && call["args"]["template"]
                .as_str()
                .is_some_and(|template| template.contains("daily_click_enter")))),
        );
        checks.insert(
            "first_phase_monthly_wins_same_frame".into(),
            json!(
                case.entry_missing
                    || case.first == "never"
                    || state.first_hit.as_ref().is_some_and(|template| {
                        if matches!(case.first.as_str(), "monthly" | "both") {
                            template.contains("supply_bottom")
                        } else {
                            template.contains("手机#009_036_043_094")
                        }
                    })
            ),
        );
        checks.insert(
            "monthly_is_never_clicked_in_first_phase".into(),
            json!(state
                .inputs
                .iter()
                .filter(|input| input["template"]
                    .as_str()
                    .is_some_and(|t| t.contains("supply_bottom")))
                .all(|input| input["second_phase"] == true)),
        );
        checks.insert(
            "first_miss_only_has_roi_enter_obstacle".into(),
            json!(first.iter().all(|call| call["args"]["obstacles"]
                .as_array()
                .is_some_and(|obstacles| obstacles.len() == 1
                    && obstacles[0] == "daily_click_enter_obstacle_20261015#350_880_650_980.png"))),
        );
        checks.insert(
            "first_scan_monthly_then_phone".into(),
            json!(state
                .calls
                .iter()
                .filter(|call| call["function"] == "find_any")
                .all(|call| call["args"]["templates"].as_array().is_some_and(
                    |templates| templates.len() == 2
                        && templates[0]
                            .as_str()
                            .is_some_and(|t| t.contains("supply_bottom"))
                        && templates[1]
                            .as_str()
                            .is_some_and(|t| t.contains("手机#009_036_043_094"))
                ))),
        );
        let sleep3 = state
            .calls
            .iter()
            .filter(|call| call["function"] == "sleep" && call["args"]["duration"] == "3s")
            .count();
        checks.insert(
            "phone_alone_waits_exactly_three_seconds".into(),
            json!(
                sleep3
                    == usize::from(
                        !early_failure && !matches!(case.first.as_str(), "monthly" | "both")
                    )
            ),
        );
        checks.insert(
            "second_wait_contract".into(),
            json!(
                second.len() == usize::from(!early_failure)
                    && second.iter().all(|call| call["args"]["interval"] == "2s"
                        && call["args"]["click"] != true
                        && call["args"]["obstacles"]
                            .as_array()
                            .is_some_and(|items| items.len() == 2
                                && items[0]
                                    .as_str()
                                    .is_some_and(|t| t.contains("supply_bottom"))
                                && items[1]
                                    == "daily_click_enter_obstacle_20261015#350_880_650_980.png"))
            ),
        );
        checks.insert(
            "multiple_and_late_enter_clicks_are_handled".into(),
            json!(
                state.entry_clicks
                    == if case.entry_missing {
                        0
                    } else {
                        case.entry_clicks + if case.late_enter { 2 } else { 0 }
                    }
            ),
        );
        checks.insert(
            "monthly_or_late_monthly_cleared_in_second_wait".into(),
            json!(
                state.monthly_cleared
                    == usize::from(
                        !early_failure
                            && !case.second_missing
                            && matches!(case.first.as_str(), "monthly" | "both" | "late_monthly")
                    )
            ),
        );
        let first_limit = if case.function == "cloud_enter_game" {
            3600
        } else {
            150
        };
        checks.insert(
            "first_phase_bounded".into(),
            json!(
                state.first_captures <= first_limit
                    && (case.first != "never" || state.first_captures == first_limit)
            ),
        );
        checks.insert(
            "long_cloud_loading_advances_after_old_limit".into(),
            json!(
                case.phone_after_observations <= 150
                    || (state.first_captures > 150
                        && state.first_captures
                            == case.phone_after_observations + usize::from(case.entry_clicks > 0)
                        && state.second_confirmed)
            ),
        );
        checks.insert(
            "second_confirmation_required_before_continuation".into(),
            json!(
                state.second_confirmed == (!early_failure && !case.second_missing)
                    && (!state.daily_reached || state.second_confirmed)
            ),
        );
    } else {
        let left_failure = case.add && !case.already_challenge && case.left_after > 8;
        let right_failure =
            case.add && !case.already_challenge && !left_failure && case.right_after > 12;
        let refresh_failure = case.add
            && !case.already_challenge
            && !left_failure
            && !right_failure
            && case.storage_refresh_missing;
        let enter_failure = case.add
            && !case.already_challenge
            && !left_failure
            && !right_failure
            && !refresh_failure
            && (case.missing_enter || case.expect_enter_rejected);
        let expected = if left_failure {
            "DEFAULT_CAVERN_CATEGORY_NOT_FOUND"
        } else if right_failure || refresh_failure {
            "DEFAULT_CAVERN_STAGE_NOT_FOUND"
        } else if enter_failure {
            "DEFAULT_CAVERN_ENTER_NOT_FOUND"
        } else if case.missing_challenge {
            "CHALLENGE_PAGE_TIMEOUT"
        } else {
            "PROBE_STOP:BATTLE_OCR"
        };
        checks.insert(
            "expected_navigation_or_battle_boundary".into(),
            json!(error.as_ref().is_some_and(|error| error.contains(expected))),
        );
        checks.insert(
            "battle_only_after_valid_navigation".into(),
            json!(state.battle_reached == (expected == "PROBE_STOP:BATTLE_OCR")),
        );
        checks.insert("left_swipes_bounded".into(), json!(state.left_swipes <= 8));
        checks.insert(
            "right_swipes_bounded".into(),
            json!(state.right_swipes <= 12),
        );
        checks.insert("no_wrong_row_enter".into(), json!(!state.wrong_row));
        checks.insert(
            "storage_anchor_refreshed_after_list_settles".into(),
            json!(
                !case.add
                    || case.already_challenge
                    || left_failure
                    || right_failure
                    || refresh_failure
                    || (state.storage_match_centers.len() >= 2
                        && state.storage_match_centers.last() == Some(&host.roles.storage_center)
                        && state.storage_match_times_ms.last().unwrap()
                            - state.storage_match_times_ms.first().unwrap()
                            >= 600)
            ),
        );
        checks.insert(
            "clipped_template_never_clicked".into(),
            json!(!case.expect_enter_rejected || !state.row_entered),
        );
        checks.insert(
            "no_target_uses_only_default_navigation".into(),
            json!(
                !case.add
                    || case.already_challenge
                    || (!state.training_selected && !state.legacy_entered && state.left_swipes > 0)
            ),
        );
        checks.insert(
            "configured_target_keeps_original_navigation".into(),
            json!(
                case.add
                    || case.already_challenge
                    || (state.training_selected
                        && state.legacy_entered
                        && state.left_swipes == 0
                        && state.right_swipes == 0)
            ),
        );
        checks.insert(
            "already_challenge_does_not_navigate".into(),
            json!(
                !case.already_challenge
                    || (state.inputs.is_empty()
                        && !state.calls.iter().any(|call| call["function"] == "find"
                            && call["args"]["template"] == host.roles.add))
            ),
        );
        checks.insert(
            "list_boundaries_stop_before_later_actions".into(),
            json!(
                (!left_failure || (!state.cavern_selected && state.right_swipes == 0))
                    && (!right_failure || !state.row_entered)
                    && (!refresh_failure
                        || (!state.row_entered && state.storage_match_centers.len() == 1))
            ),
        );
        checks.insert(
            "shared_challenge_wait_preserved".into(),
            json!(
                case.already_challenge
                    || left_failure
                    || right_failure
                    || refresh_failure
                    || enter_failure
                    || state
                        .calls
                        .iter()
                        .any(|call| call["function"] == "wait_find"
                            && call["args"]["template"]
                                .as_str()
                                .is_some_and(|name| name.contains("daily_challenge_word"))
                            && call["args"]["timeout"] == "20s"
                            && call["args"]["region"] == json!([0.80, 0.889, 0.077, 0.045]))
            ),
        );
        checks.insert(
            "power_ocr_boundary_preserved".into(),
            json!(
                !state.battle_reached
                    || state
                        .calls
                        .last()
                        .is_some_and(|call| (call["function"] == "ocr"
                            && call["args"]["region"] == json!([0.83, 0.04, 0.09, 0.045]))
                            || (call["function"] == "find_text"
                                && call["args"]["text"] == "/"
                                && call["args"]["whitelist"] == "0123456789/"
                                && state.calls.iter().any(|anchor| anchor["function"] == "find"
                                    && anchor["args"]["template"]
                                        == "daily_power_anchor_20261023.png"
                                    && anchor["args"]["region"]
                                        == json!([0.82, 0.025, 0.045, 0.065]))))
            ),
        );
    }
    let passed = checks.values().all(|value| value == true);
    json!({"name":case.name,"function":case.function,"group":case.group,"passed":passed,"checks":checks,"outcome":error.unwrap_or_else(||"success".into()),"virtual_ms":state.now_ms,"entry_clicks":state.entry_clicks,"first_observations":state.first_captures,"monthly_clicks":state.monthly_cleared,"left_swipes":state.left_swipes,"right_swipes":state.right_swipes,"battle_boundary_reached":state.battle_reached,"storage_match_centers":state.storage_match_centers,"storage_match_times_ms":state.storage_match_times_ms,"enter_center":host.intended_enter(),"enter_template_pixels":host.roles.enter_template_pixels,"enter_drift_pixels":case.enter_drift_pixels,"expected_enter_rejected":case.expect_enter_rejected,"calls":state.calls,"mock_inputs":state.inputs,"violations":state.violations,"last_events":state.events.iter().rev().take(20).collect::<Vec<_>>()})
}
