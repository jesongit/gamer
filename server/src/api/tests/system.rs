use super::*;

#[tokio::test]
async fn readiness_is_public_structured_and_does_not_leak_paths() {
    let t = build_app(
        "ready",
        test_credential("admin123"),
        Default::default(),
    );
    let resp = send(&t.app, req("GET", "/health/ready", None, &[], None)).await;
    assert!(matches!(
        resp.status(),
        StatusCode::OK | StatusCode::SERVICE_UNAVAILABLE
    ));
    let status = resp.status();
    let body = json_body(resp).await;
    assert!(body["ready"].is_boolean());
    for name in ["data_dir", "sqlite", "scrcpy_server", "adb", "ffmpeg"] {
        assert!(body["checks"][name]["ok"].is_boolean(), "{name}");
    }
    assert_eq!(body["ready"], status == StatusCode::OK);
    assert!(!body
        .to_string()
        .contains(&t.dir.to_string_lossy().to_string()));
}

#[tokio::test]
async fn metrics_is_public_prometheus_text_with_low_cardinality() {
    let t = build_app(
        "metrics",
        test_credential("admin123"),
        Default::default(),
    );
    let resp = send(&t.app, req("GET", "/metrics", None, &[], None)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("text/plain; version=0.0.4; charset=utf-8")
    );
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(body.contains("gamer_sessions_active "));
    assert!(body.contains("gamer_runs_active "));
    assert!(body.contains("gamer_db_ready 1"));
    assert!(!body.contains(&t.dir.to_string_lossy().to_string()));
}

#[test]
fn control_parse_rejects_missing_and_invalid_fields() {
    // tap 缺坐标 / 越界 / 负值（NaN/Infinity 在 JSON 层即反序列化失败）
    assert!(parse_ctl(&ctl_req(r#"{"type":"tap"}"#)).is_err());
    assert!(
        parse_ctl(&ctl_req(r#"{"type":"tap","x":500}"#)).is_err(),
        "缺 y 拒绝"
    );
    assert!(parse_ctl(&ctl_req(r#"{"type":"tap","x":1e30,"y":0}"#)).is_err());
    assert!(parse_ctl(&ctl_req(r#"{"type":"tap","x":-1,"y":0}"#)).is_err());

    // swipe 缺坐标 / duration 非法
    assert!(parse_ctl(&ctl_req(r#"{"type":"swipe","x1":1,"y1":1,"x2":2}"#)).is_err());
    assert!(parse_ctl(&ctl_req(
        r#"{"type":"swipe","x1":1,"y1":1,"x2":2,"y2":2,"duration":999999999}"#
    ))
    .is_err());
    assert!(
        parse_ctl(&ctl_req(
            r#"{"type":"swipe","x1":1,"y1":1,"x2":2,"y2":2,"duration":300}"#
        ))
        .is_ok(),
        "合法 swipe 带时长放行"
    );

    // text 空 / 超 300 字节协议上限；多字节字符按字节计
    assert!(parse_ctl(&ctl_req(r#"{"type":"text"}"#)).is_err());
    assert!(parse_ctl(&ctl_req(r#"{"type":"text","text":""}"#)).is_err());
    let long = format!("{{\"type\":\"text\",\"text\":\"{}\"}}", "字".repeat(101)); // 303 字节
    assert!(parse_ctl(&ctl_req(&long)).is_err());
    let ok_len = format!("{{\"type\":\"text\",\"text\":\"{}\"}}", "a".repeat(299));
    assert!(parse_ctl(&ctl_req(&ok_len)).is_ok());

    // press keycode 0 与越界拒绝，合法值放行
    assert!(parse_ctl(&ctl_req(r#"{"type":"press"}"#)).is_err());
    assert!(parse_ctl(&ctl_req(r#"{"type":"press","keycode":0}"#)).is_err());
    assert!(parse_ctl(&ctl_req(r#"{"type":"press","keycode":1001}"#)).is_err());
    assert!(parse_ctl(&ctl_req(r#"{"type":"press","keycode":187}"#)).is_ok());

    // start_app 包名校验
    assert!(parse_ctl(&ctl_req(r#"{"type":"start_app"}"#)).is_err());
    assert!(parse_ctl(&ctl_req(r#"{"type":"start_app","app":"bad pkg!"}"#)).is_err());
    assert!(parse_ctl(&ctl_req(
        r#"{"type":"start_app","app":"+com.miHoYo.hkrpg"}"#
    ))
    .is_ok());
    assert!(
        parse_ctl(&ctl_req(r#"{"type":"start_app","app":"+bad/pkg"}"#)).is_err(),
        "+ 后非法包名拒绝"
    );
    assert!(
        parse_ctl(&ctl_req(r#"{"type":"start_app","app":"?崩坏星穹铁道"}"#)).is_ok(),
        "? 按名搜索透传"
    );
    assert!(parse_ctl(&ctl_req(r#"{"type":"start_app","app":"?"}"#)).is_err());
    assert!(parse_ctl(&ctl_req(r#"{"type":"start_app","app":"com.miHoYo.hkrpg"}"#)).is_ok());
    for injected in [
        "com.safe.app;id",
        "com.safe.app&&id",
        "com.safe.app$(id)",
        "com.safe.app`id`",
        "com.safe.app\nid",
        "com.safe.app --user 0",
        "+com.safe.app;id",
        "+--user",
    ] {
        let body = serde_json::json!({"type": "start_app", "app": injected}).to_string();
        assert!(
            parse_ctl(&ctl_req(&body)).is_err(),
            "可能进入 adb shell 包名拼接边界的注入载荷必须拒绝: {injected:?}"
        );
    }
    assert!(
        parse_ctl(&ctl_req(r#"{"type":"start_app","app":"?游戏; id"}"#)).is_ok(),
        "? 搜索名只经 scrcpy 二进制控制协议透传，不进入 adb shell 包名路径"
    );

    // clipboard 上限与空值
    assert!(parse_ctl(&ctl_req(r#"{"type":"clipboard","text":""}"#)).is_err());

    // 无参动作不受影响
    assert!(parse_ctl(&ctl_req(r#"{"type":"home"}"#)).is_ok());
    assert!(parse_ctl(&ctl_req(r#"{"type":"rotate"}"#)).is_ok());

    // 未知命令仍 400 文案
    match parse_ctl(&ctl_req(r#"{"type":"touch","action":"down"}"#)) {
        Err(e) => {
            assert_eq!(e.status(), StatusCode::BAD_REQUEST);
            assert_eq!(e.message(), "unknown command");
        }
        Ok(_) => panic!("touch 不属于 REST 控制命令"),
    }
}

#[tokio::test]
async fn api_error_maps_status_and_json_body() {
    let resp = ApiError::conflict("device_busy").into_response();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json, serde_json::json!({"error": "device_busy"}));
}

#[test]
fn log_limit_clamped_to_1_1000() {
    assert_eq!(clamp_log_limit(None), 200);
    assert_eq!(clamp_log_limit(Some(50)), 50);
    assert_eq!(clamp_log_limit(Some(0)), 1);
    assert_eq!(clamp_log_limit(Some(-100)), 1);
    assert_eq!(clamp_log_limit(Some(1001)), 1000);
    assert_eq!(clamp_log_limit(Some(1_000_000)), 1000);
}

#[test]
fn route_validation_rejects_ambiguous_device_configuration() {
    let valid = CreateDeviceReq {
        name: "demo".into(),
        kind: "redroid".into(),
        addr: Some("127.0.0.1:5555".into()),
        screen_mode: Some("virtual".into()),
        vd_res: Some("1920x1080".into()),
        vd_dpi: Some(420),
        pkg: Some("com.example.game".into()),
        fps: Some(60),
    };
    assert!(validate_device_req(&valid).is_ok());

    let mut invalid = valid;
    invalid.screen_mode = Some("unexpected".into());
    assert!(validate_device_req(&invalid).is_err());
    invalid.screen_mode = Some("virtual".into());
    invalid.vd_res = Some("1920".into());
    assert!(validate_device_req(&invalid).is_err());
    invalid.vd_res = Some("1x1080".into());
    assert!(validate_device_req(&invalid).is_err());
    invalid.vd_res = Some("1920x1080".into());
    invalid.fps = Some(121);
    assert!(validate_device_req(&invalid).is_err());
    invalid.fps = Some(60);
    invalid.name = "line\nfeed".into();
    assert!(validate_device_req(&invalid).is_err());
}

#[test]
fn session_affecting_change_only_detects_casting_fields() {
    let base = Device {
        id: "d1".into(),
        name: "挂机一号".into(),
        kind: "redroid".into(),
        addr: "127.0.0.1:5555".into(),
        screen_mode: ScreenMode::Virtual,
        vd_res: Some("1920x1080".into()),
        vd_dpi: Some(420),
        pkg: Some("com.example.game".into()),
        fps: Some(30),
        created_at: "2026-01-01 00:00:00".into(),
    };
    let mutate = |f: &dyn Fn(&mut Device)| {
        let mut d = base.clone();
        f(&mut d);
        d
    };

    // 非投屏字段（名称/包名）任意变化 → 不重建会话
    assert!(!session_affecting_change(
        &base,
        &mutate(&|d| d.name = "改名".into()),
        30
    ));
    assert!(!session_affecting_change(
        &base,
        &mutate(&|d| d.pkg = None),
        30
    ));
    assert!(!session_affecting_change(
        &base,
        &mutate(&|d| d.pkg = Some("com.other.app".into())),
        30
    ));

    // 写法差异但生效值相同（空串/None/默认值归一）→ 不重建会话
    assert!(!session_affecting_change(
        &base,
        &mutate(&|d| d.vd_res = Some(" 1920X1080 ".into())),
        30
    ));
    assert!(!session_affecting_change(
        &base,
        &mutate(&|d| d.vd_res = None),
        30
    ));
    // DPI None 与 0 同为"自动"
    let no_dpi = mutate(&|d| d.vd_dpi = None);
    assert!(!session_affecting_change(
        &no_dpi,
        &mutate(&|d| d.vd_dpi = Some(0)),
        30
    ));
    assert!(!session_affecting_change(
        &base,
        &mutate(&|d| d.fps = None),
        30
    ));

    // 投屏字段实质变化 → 重建会话
    assert!(session_affecting_change(
        &base,
        &mutate(&|d| d.screen_mode = ScreenMode::Mirror),
        30
    ));
    assert!(session_affecting_change(
        &base,
        &mutate(&|d| d.vd_res = Some("1280x720".into())),
        30
    ));
    assert!(session_affecting_change(
        &base,
        &mutate(&|d| d.vd_dpi = Some(320)),
        30
    ));
    assert!(session_affecting_change(
        &base,
        &mutate(&|d| d.fps = Some(60)),
        30
    ));
    assert!(session_affecting_change(
        &base,
        &mutate(&|d| d.addr = "192.168.1.9:5555".into()),
        30
    ));
    assert!(session_affecting_change(
        &base,
        &mutate(&|d| d.kind = "emu".into()),
        30
    ));

    // fps None 跟随全局配置：全局值不同则生效值不同 → 重建
    assert!(session_affecting_change(
        &base,
        &mutate(&|d| d.fps = None),
        60
    ));
}

#[test]
fn route_validation_rejects_path_like_template_names() {
    for name in [
        "",
        ".hidden.png",
        "..",
        "../escape.png",
        "a\\b.png",
        "a:b.png",
    ] {
        assert!(
            validate_template_name(name).is_err(),
            "{name:?} must be rejected"
        );
    }
    assert_eq!(
        validate_template_name("login#0.1_0.2_0.3_0.4.png").unwrap(),
        "login#0.1_0.2_0.3_0.4.png"
    );
    assert!(validate_template_name("截图 1.png").is_ok());
}

#[test]
fn route_validation_bounds_run_and_task_requests() {
    let task = SaveTaskReq {
        id: None,
        name: "daily".into(),
        cron: "*/5 * * * *".into(),
        script_id: "com.example.game/daily.yaml".into(),
        device_id: "device-1".into(),
        enabled: Some(true),
        args: None,
        reconfirm: false,
    };
    assert!(validate_task_req(&task).is_ok());
    let mut bad_task = task;
    bad_task.device_id.clear();
    assert!(validate_task_req(&bad_task).is_err());

    let run = RunReqArgs {
        device_id: "device-1".into(),
        start_index: Some(100_000),
        function: None,
        args: None,
    };
    assert!(validate_run_req(&run).is_ok());
    let bad_run = RunReqArgs {
        start_index: Some(100_001),
        ..run
    };
    assert!(validate_run_req(&bad_run).is_err());
}
