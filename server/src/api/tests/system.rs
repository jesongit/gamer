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
async fn system_info_is_protected_structured_and_does_not_leak_paths() {
    let t = build_app(
        "system-info",
        test_credential("admin123"),
        Default::default(),
    );

    let unauthenticated = send(&t.app, req("GET", "/api/system/info", None, &[], None)).await;
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(json_body(unauthenticated).await["error"], "unauthorized");

    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    let response = send(
        &t.app,
        req(
            "GET",
            "/api/system/info",
            None,
            &[(header::COOKIE.to_string(), sid)],
            None,
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    // 契约六组顶层字段（system-api-v1 §2）；原型自有字段 readiness/timezone/
    // schema_version 已随契约化移除
    for group in [
        "app",
        "deployment",
        "schema",
        "dependencies",
        "capabilities",
        "startup",
    ] {
        assert!(body[group].is_object(), "missing contract group {group}");
    }
    assert!(body.get("readiness").is_none(), "readiness 以 /health/ready 为准");
    assert!(body.get("timezone").is_none());
    assert!(body.get("schema_version").is_none());
    // app 组接 build_info（SYS-001）
    assert_eq!(body["app"]["version"], env!("CARGO_PKG_VERSION"));
    for field in ["version", "commit", "built_at", "channel", "target"] {
        assert!(body["app"][field].is_string(), "app.{field}");
    }
    // 依赖三组件 × 四字段（fixture 冻结结构）
    for dep in ["adb", "ffmpeg", "scrcpy"] {
        for field in ["status", "version", "source", "binding"] {
            assert!(
                body["dependencies"][dep].get(field).is_some(),
                "dependencies.{dep}.{field}"
            );
        }
    }
    assert!(body["capabilities"]["check"].is_boolean());
    assert!(body["startup"]["boot_id"].is_string());
    assert!(!body
        .to_string()
        .contains(&t.dir.to_string_lossy().to_string()));
}

#[tokio::test]
async fn shutdown_state_endpoint_tracks_coordinator_anonymously() {
    let t = build_app(
        "shutdown-state",
        test_credential("admin123"),
        Default::default(),
    );

    // 匿名可查（OPS-002 轻量端点），初始 running
    let resp = send(&t.app, req("GET", "/health/shutdown", None, &[], None)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["state"], "running");
    assert_eq!(body["drained"], false);

    // POST /api/shutdown（需登录）触发协调器 → 状态推进到 finished
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    let resp = send(
        &t.app,
        req(
            "POST",
            "/api/shutdown",
            None,
            &[(header::COOKIE.to_string(), sid)],
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = send(&t.app, req("GET", "/health/shutdown", None, &[], None)).await;
    let body = json_body(resp).await;
    assert_eq!(body["state"], "finished");
    assert_eq!(body["drained"], true);
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
        app: crate::core::AppContext::from_legacy_package("device-1", "com.example.game").unwrap(),
        runner: RunnerSpecDto {
            runner_id: "gamer.yaml".into(),
            entrypoint: "com.example.game/daily.yaml".into(),
            payload: serde_json::json!({}),
        },
        schedule: TaskSchedule::new("cron", serde_json::json!({"expression": "*/5 * * * *"}))
            .unwrap(),
        enabled: Some(true),
        preset_id: None,
    };
    // 未注册 provider（空 registry）保存放行：未来扩展可先存任务、后装 provider
    assert!(build_task(&ScheduleRegistry::new(), "t1".into(), task.clone(), None).is_ok());
    let mut bad_task = task;
    bad_task.name.clear();
    assert!(build_task(&ScheduleRegistry::new(), "t1".into(), bad_task, None).is_err());

}
