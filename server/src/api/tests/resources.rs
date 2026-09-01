use super::*;

// ---------- 阶段 1 资源 API：/api/functions CRUD、版本冲突、dry-run 导入 ----------
//
// 资源 id 含 `/`，URL 里整体 encodeURIComponent（%2F），与 scripts 路由同规则。

const FUNC_YAML: &str = "login:\n  steps:\n    - return: true\n";
const FUNC_YAML_V2: &str = "login:\n  steps:\n    - return: false\n";

#[tokio::test]
async fn malformed_control_payload_is_400_even_offline() {
    let t = build_app(
        "ctl400",
        test_credential("admin123"),
        Default::default(),
    );
    let ck = cookie_of(&login(&t.app).await);
    let sid = first_cookie_pair(&ck);
    // 设备不存在：先经过输入校验（400），轮不到会话检查（409）
    let resp = send(
        &t.app,
        req(
            "POST",
            "/api/devices/nope/control",
            None,
            &[
                (header::COOKIE.to_string(), sid),
                (header::CONTENT_TYPE.to_string(), JSON_CT.into()),
            ],
            Some(r#"{"type":"tap"}"#.into()),
        ),
    )
    .await;
    assert!(resp.status().is_client_error());
}

#[tokio::test]
async fn template_upload_rejects_byte_and_pixel_bombs_with_4xx() {
    let t = build_app(
        "tmpllimits",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    let headers = |cookie: String| {
        vec![
            (header::COOKIE.to_string(), cookie),
            (header::CONTENT_TYPE.to_string(), JSON_CT.into()),
        ]
    };

    let bomb_b64 = base64::engine::general_purpose::STANDARD.encode(pixel_bomb_png(30_000, 30_000));
    let body = serde_json::json!({
        "short_name": "bomb.png",
        "pkg": "com.test.app",
        "data_b64": bomb_b64,
    })
    .to_string();
    let resp = send(
        &t.app,
        req(
            "POST",
            "/api/templates",
            None,
            &headers(sid.clone()),
            Some(body),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // 构造超过原始模板字节上限的 base64，但保持请求体低于 16MiB 路由上限，
    // 断言 API 在解码前直接以 400 拒绝，不分配/解码图片。
    let too_large_b64 = "A".repeat((matcher::TEMPLATE_MAX_INPUT_BYTES / 3 + 2) * 4);
    let body = serde_json::json!({
        "short_name": "large.png",
        "pkg": "com.test.app",
        "data_b64": too_large_b64,
    })
    .to_string();
    let resp = send(
        &t.app,
        req("POST", "/api/templates", None, &headers(sid), Some(body)),
    )
    .await;
    assert!(resp.status().is_client_error());
}

#[tokio::test]
async fn zip_import_rejects_slip_duplicate_and_pixel_bomb_with_4xx() {
    let t = build_app(
        "ziplimits",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    let headers = |cookie: String| {
        vec![
            (header::COOKIE.to_string(), cookie),
            (header::CONTENT_TYPE.to_string(), "application/zip".into()),
        ]
    };
    let cases = [
        craft_zip(vec![("yaml/../escape.yaml", b"steps: []\n".to_vec())]),
        craft_zip(vec![("../escape.yaml", b"steps: []\n".to_vec())]),
        craft_zip(vec![("/absolute.yaml", b"steps: []\n".to_vec())]),
        craft_zip(vec![("yaml\\..\\escape.yaml", b"steps: []\n".to_vec())]),
        craft_zip(vec![
            ("yaml/one.yaml", b"steps: []\n".to_vec()),
            ("yaml/ONE.yaml", b"steps: []\n".to_vec()),
        ]),
        craft_zip(vec![("tmpl/bomb.png", pixel_bomb_png(30_000, 30_000))]),
    ];
    for zip_bytes in cases {
        let resp = send(
            &t.app,
            req_bytes(
                "POST",
                "/api/scripts/import?pkg=com.test.app&confirm=1",
                None,
                &headers(sid.clone()),
                zip_bytes,
            ),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn request_body_limits_reject_oversize_json_and_zip_with_413() {
    let t = build_app(
        "bodylimits",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    let json_headers = [
        (header::COOKIE.to_string(), sid.clone()),
        (header::CONTENT_TYPE.to_string(), JSON_CT.into()),
    ];
    let resp = send(
        &t.app,
        req_bytes(
            "POST",
            "/api/devices",
            None,
            &json_headers,
            vec![b'x'; BODY_LIMIT_JSON + 1],
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let zip_headers = [
        (header::COOKIE.to_string(), sid),
        (header::CONTENT_TYPE.to_string(), "application/zip".into()),
    ];
    let resp = send(
        &t.app,
        req_bytes(
            "POST",
            "/api/scripts/import?pkg=com.test.app",
            None,
            &zip_headers,
            vec![0u8; BODY_LIMIT_ZIP_IMPORT + 1],
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn functions_routes_require_auth() {
    let t = build_app(
        "fnauth",
        test_credential("admin123"),
        Default::default(),
    );
    let cases = [
        ("GET", "/api/functions?pkg=com.test.app", None),
        (
            "POST",
            "/api/functions",
            Some(r#"{"pkg":"p","name":"a","content":"x: {}\n"}"#),
        ),
        ("GET", "/api/functions/com.test.app%2Fa.yaml", None),
        (
            "PUT",
            "/api/functions/com.test.app%2Fa.yaml",
            Some(r#"{"content":"x: {}\n"}"#),
        ),
        ("DELETE", "/api/functions/com.test.app%2Fa.yaml", None),
    ];
    for (method, uri, body) in cases {
        let resp = send(
            &t.app,
            req(method, uri, None, &[], body.map(str::to_string)),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{method} {uri}");
    }
}

#[tokio::test]
async fn functions_crud_cycle_with_version_conflict() {
    let t = build_app(
        "fncrud",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));

    // create：缺扩展名自动补 .yaml，返回 id/file/version/函数名清单
    let body = serde_json::json!({"pkg": "com.test.app", "name": "common", "content": FUNC_YAML});
    let resp = send(
        &t.app,
        req(
            "POST",
            "/api/functions",
            None,
            &json_headers(sid.clone()),
            Some(body.to_string()),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let j = json_body(resp).await;
    assert_eq!(j["ok"], true);
    assert_eq!(j["id"], "com.test.app/common.yaml");
    assert_eq!(j["file"], "common");
    assert_eq!(j["functions"], serde_json::json!(["login"]));
    let v1 = j["version"].as_str().unwrap().to_string();
    assert_eq!(v1.len(), 12);

    // POST 只创建：同名函数库再次提交不得覆盖
    let resp = send(
        &t.app,
        req(
            "POST",
            "/api/functions",
            None,
            &json_headers(sid.clone()),
            Some(body.to_string()),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    // list：pkg 必填、返回文件短路径 + 函数名清单
    let resp = send(
        &t.app,
        req(
            "GET",
            "/api/functions?pkg=com.test.app",
            None,
            &json_headers(sid.clone()),
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let j = json_body(resp).await;
    assert_eq!(j[0]["id"], "com.test.app/common.yaml");
    assert_eq!(j[0]["file"], "common");
    assert_eq!(j[0]["version"], v1.as_str());
    assert_eq!(j[0]["functions"], serde_json::json!(["login"]));

    // get（%2F 编码 id）：内容往返一致
    let resp = send(
        &t.app,
        req(
            "GET",
            "/api/functions/com.test.app%2Fcommon.yaml",
            None,
            &json_headers(sid.clone()),
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let j = json_body(resp).await;
    assert_eq!(j["content"], FUNC_YAML);
    assert_eq!(j["pkg"], "com.test.app");

    // update 带 expected_version：成功并换新版本
    let body = serde_json::json!({"content": FUNC_YAML_V2, "expected_version": v1});
    let resp = send(
        &t.app,
        req(
            "PUT",
            "/api/functions/com.test.app%2Fcommon.yaml",
            None,
            &json_headers(sid.clone()),
            Some(body.to_string()),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let v2 = json_body(resp).await["version"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(v2, v1);

    // update 带过期版本 → 409 {code:"version_conflict", message, resource}
    let body = serde_json::json!({"content": FUNC_YAML, "expected_version": v1});
    let resp = send(
        &t.app,
        req(
            "PUT",
            "/api/functions/com.test.app%2Fcommon.yaml",
            None,
            &json_headers(sid.clone()),
            Some(body.to_string()),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let j = json_body(resp).await;
    assert_eq!(j["code"], "version_conflict");
    assert_eq!(j["resource"], "com.test.app/common.yaml");
    assert!(j["message"].is_string());

    // 缺少 expected_version → 409；显式 force:true 才允许跳过版本门禁
    let body = serde_json::json!({"content": FUNC_YAML});
    let resp = send(
        &t.app,
        req(
            "PUT",
            "/api/functions/com.test.app%2Fcommon.yaml",
            None,
            &json_headers(sid.clone()),
            Some(body.to_string()),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = serde_json::json!({"content": FUNC_YAML, "force": true});
    let resp = send(
        &t.app,
        req(
            "PUT",
            "/api/functions/com.test.app%2Fcommon.yaml",
            None,
            &json_headers(sid.clone()),
            Some(body.to_string()),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let v3 = json_body(resp).await["version"].as_str().unwrap().to_string();

    // 重命名也是更新：必须使用源文件当前版本
    let body = serde_json::json!({
        "name": "renamed",
        "content": FUNC_YAML_V2,
        "expected_version": v3
    });
    let resp = send(
        &t.app,
        req(
            "PUT",
            "/api/functions/com.test.app%2Fcommon.yaml",
            None,
            &json_headers(sid.clone()),
            Some(body.to_string()),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // delete → get 404 → delete 幂等失败 404
    let resp = send(
        &t.app,
        req(
            "DELETE",
            "/api/functions/com.test.app%2Frenamed.yaml",
            None,
            &json_headers(sid.clone()),
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = send(
        &t.app,
        req(
            "GET",
            "/api/functions/com.test.app%2Frenamed.yaml",
            None,
            &json_headers(sid.clone()),
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let resp = send(
        &t.app,
        req(
            "DELETE",
            "/api/functions/com.test.app%2Fcommon.yaml",
            None,
            &json_headers(sid),
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn functions_input_validation_and_missing_pkg() {
    let t = build_app(
        "fnvalid",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));

    // pkg 缺失/空 → 400（GET 与 POST）
    let resp = send(
        &t.app,
        req(
            "GET",
            "/api/functions",
            None,
            &json_headers(sid.clone()),
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = serde_json::json!({"pkg": "", "name": "a", "content": "x:\n  steps: []\n"});
    let resp = send(
        &t.app,
        req(
            "POST",
            "/api/functions",
            None,
            &json_headers(sid.clone()),
            Some(body.to_string()),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // 严格 loader：顶层键保留字 / 非法函数名 / YAML 语法错 / 子目录短路径
    let cases = [
        ("match:\n  steps: []\n", "保留字"),
        ("1abc:\n  steps: []\n", "只允许 unicode 字母"),
        ("带 空 格:\n  steps: []\n", "只允许 unicode 字母"),
        ("login: [unclosed", "YAML"),
        (
            "123:
  steps: []
",
            "不是字符串标量",
        ),
    ];
    for (content, marker) in cases {
        let body = serde_json::json!({"pkg": "com.test.app", "name": "bad", "content": content});
        let resp = send(
            &t.app,
            req(
                "POST",
                "/api/functions",
                None,
                &json_headers(sid.clone()),
                Some(body.to_string()),
            ),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{content}");
        let j = json_body(resp).await;
        assert_eq!(j["error"], "invalid_yaml", "{content}");
        assert!(
            j["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["message"].as_str().unwrap_or_default().contains(marker)),
            "{content}: {j}"
        );
    }
    let body =
        serde_json::json!({"pkg": "com.test.app", "name": "sub/common", "content": FUNC_YAML});
    let resp = send(
        &t.app,
        req(
            "POST",
            "/api/functions",
            None,
            &json_headers(sid.clone()),
            Some(body.to_string()),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // PUT / GET 不存在的函数文件 → 404
    for (method, uri, body) in [
        ("GET", "/api/functions/com.test.app%2Fnope.yaml", None),
        (
            "PUT",
            "/api/functions/com.test.app%2Fnope.yaml",
            Some(r#"{"content":"a:\n  steps: []\n"}"#),
        ),
    ] {
        let resp = send(
            &t.app,
            req(
                method,
                uri,
                None,
                &json_headers(sid.clone()),
                body.map(str::to_string),
            ),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{method} {uri}");
    }
}

#[tokio::test]
async fn functions_never_leak_into_script_sources() {
    let t = build_app(
        "fnisolation",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    // 同分区各建一个脚本与一个函数库文件
    let script =
        serde_json::json!({"pkg": "com.test.app", "name": "main.yaml", "content": "steps: []\n"});
    let resp = send(
        &t.app,
        req(
            "POST",
            "/api/scripts",
            None,
            &json_headers(sid.clone()),
            Some(script.to_string()),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let func =
        serde_json::json!({"pkg": "com.test.app", "name": "common.yaml", "content": FUNC_YAML});
    let resp = send(
        &t.app,
        req(
            "POST",
            "/api/functions",
            None,
            &json_headers(sid.clone()),
            Some(func.to_string()),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // 脚本列表只含 yaml/ 脚本，func 文件绝不混入
    let resp = send(
        &t.app,
        req(
            "GET",
            "/api/scripts",
            None,
            &json_headers(sid.clone()),
            None,
        ),
    )
    .await;
    let j = json_body(resp).await;
    assert_eq!(j.as_array().unwrap().len(), 1);
    assert_eq!(j[0]["name"], "main.yaml");

    // 函数 id 在脚本读取/运行接口一律 404（目录即类型，不做内容推断）
    let resp = send(
        &t.app,
        req(
            "GET",
            "/api/scripts/com.test.app%2Fcommon.yaml",
            None,
            &json_headers(sid.clone()),
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let resp = send(
        &t.app,
        req(
            "POST",
            "/api/scripts/com.test.app%2Fcommon.yaml/run",
            None,
            &json_headers(sid.clone()),
            Some(r#"{"device_id":"d1"}"#.into()),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(json_body(resp).await["error"], "脚本不存在");

    // 函数列表也只含 func/ 文件
    let resp = send(
        &t.app,
        req(
            "GET",
            "/api/functions?pkg=com.test.app",
            None,
            &json_headers(sid),
            None,
        ),
    )
    .await;
    let j = json_body(resp).await;
    assert_eq!(j.as_array().unwrap().len(), 1);
    assert_eq!(j[0]["file"], "common");
}

#[tokio::test]
async fn scripts_get_version_and_save_expected_version_conflict() {
    let t = build_app(
        "scriptvers",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));

    let resp = post_json(
            &t,
            &sid,
            "/api/scripts",
            serde_json::json!({"pkg": "com.test.app", "name": "main.yaml", "content": "steps:\n  - log: v1\n"}),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let v1 = json_body(resp).await["version"]
        .as_str()
        .unwrap()
        .to_string();

    // GET 单脚本返回内容与版本
    let resp = get_json(&t, &sid, "/api/scripts/com.test.app%2Fmain.yaml").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let j = json_body(resp).await;
    assert_eq!(j["version"], v1.as_str());
    assert!(j["content"].as_str().unwrap().contains("log: v1"));

    // POST 只创建：已有资源再次 POST → 409，且不接受更新字段
    let resp = post_json(
            &t,
            &sid,
            "/api/scripts",
            serde_json::json!({"pkg": "com.test.app", "name": "main.yaml", "content": "steps: []\n"}),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let j = json_body(resp).await;
    assert_eq!(j["error"], "资源已存在: com.test.app/main.yaml");

    // POST body 不再接受旧的 id/expected_version 更新形态
    let resp = post_json(
            &t,
            &sid,
            "/api/scripts",
            serde_json::json!({"pkg": "com.test.app", "name": "other.yaml", "content": "steps: []\n", "expected_version": v1}),
        )
        .await;
    assert!(resp.status().is_client_error());
}

#[tokio::test]
async fn import_dry_run_reports_then_confirm_writes() {
    let t = build_app(
        "dryrun",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    let z = craft_zip(vec![
        ("yaml/ok.yaml", b"steps: []\n".to_vec()),
        ("func/common.yaml", FUNC_YAML.as_bytes().to_vec()),
        ("tmpl/a.png", valid_template_png()),
    ]);

    // dry-run：三类资源报告、不落盘
    let resp = send(
        &t.app,
        req_bytes(
            "POST",
            "/api/scripts/import?pkg=com.test.app",
            None,
            &zip_headers(sid.clone()),
            z.clone(),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let j = json_body(resp).await;
    assert_eq!(j["scripts"]["add"], serde_json::json!(["yaml/ok.yaml"]));
    assert_eq!(
        j["functions"]["add"],
        serde_json::json!(["func/common.yaml"])
    );
    assert_eq!(j["templates"]["add"].as_array().unwrap().len(), 1);
    assert!(j["scripts"]["invalid"].as_array().unwrap().is_empty());
    assert!(j["functions"]["invalid"].as_array().unwrap().is_empty());
    assert!(!t.dir.join("com.test.app/yaml/ok.yaml").exists());

    // confirm：落盘
    let resp = send(
        &t.app,
        req_bytes(
            "POST",
            "/api/scripts/import?pkg=com.test.app&confirm=1",
            None,
            &zip_headers(sid.clone()),
            z,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(t.dir.join("com.test.app/yaml/ok.yaml").is_file());
    assert!(t.dir.join("com.test.app/func/common.yaml").is_file());

    // dry-run 报告 invalid（函数名保留字）；confirm 整体拒绝、合法条目不写入
    let bad = craft_zip(vec![
        ("yaml/ok.yaml", b"steps: []\n".to_vec()),
        ("func/bad.yaml", b"return:\n  steps: []\n".to_vec()),
    ]);
    let resp = send(
        &t.app,
        req_bytes(
            "POST",
            "/api/scripts/import?pkg=com.test.app",
            None,
            &zip_headers(sid.clone()),
            bad.clone(),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let j = json_body(resp).await;
    assert_eq!(j["functions"]["invalid"][0]["path"], "func/bad.yaml");
    assert!(j["functions"]["invalid"][0]["diagnostics"].is_array());
    let resp = send(
        &t.app,
        req_bytes(
            "POST",
            "/api/scripts/import?pkg=com.test.app&confirm=1",
            None,
            &zip_headers(sid.clone()),
            bad,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(!t.dir.join("com.test.app/func/bad.yaml").exists());
    // ok.yaml 上一轮 confirm 已存在，本轮整体拒绝不覆盖（mtime 校验过重，查内容即可）
    assert_eq!(
        std::fs::read_to_string(t.dir.join("com.test.app/yaml/ok.yaml")).unwrap(),
        "steps: []\n"
    );
}

#[tokio::test]
async fn export_import_roundtrip_via_api() {
    let t = build_app(
        "roundtrip",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    // 造齐三类资源
    let script = serde_json::json!({"pkg": "com.test.app", "name": "main.yaml", "content": "steps:\n  - log: x\n"});
    let resp = send(
        &t.app,
        req(
            "POST",
            "/api/scripts",
            None,
            &json_headers(sid.clone()),
            Some(script.to_string()),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let func = serde_json::json!({"pkg": "com.test.app", "name": "common", "content": FUNC_YAML});
    let resp = send(
        &t.app,
        req(
            "POST",
            "/api/functions",
            None,
            &json_headers(sid.clone()),
            Some(func.to_string()),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let tmpl = serde_json::json!({
        "pkg": "com.test.app",
        "short_name": "icon.png",
        "data_b64": base64::engine::general_purpose::STANDARD.encode(valid_template_png()),
    });
    let resp = send(
        &t.app,
        req(
            "POST",
            "/api/templates",
            None,
            &json_headers(sid.clone()),
            Some(tmpl.to_string()),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // 导出（zip 字节）→ 导入到另一分区
    let resp = send(
        &t.app,
        req(
            "GET",
            "/api/scripts/export?pkg=com.test.app",
            None,
            &json_headers(sid.clone()),
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let zip_bytes = axum::body::to_bytes(resp.into_body(), 32 * 1024 * 1024)
        .await
        .unwrap()
        .to_vec();
    let resp = send(
        &t.app,
        req_bytes(
            "POST",
            "/api/scripts/import?pkg=com.other.app&confirm=1",
            None,
            &zip_headers(sid.clone()),
            zip_bytes,
        ),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "导入失败: {:?}",
        json_body(resp).await
    );

    // 零差异：脚本/函数/模板三类资源逐项一致
    let resp = get_json(&t, &sid, "/api/scripts").await;
    let j = json_body(resp).await;
    let content_of = |pkg: &str| {
        j.as_array()
            .unwrap()
            .iter()
            .find(|s| s["package"] == pkg)
            .map(|s| s["content"].as_str().unwrap().to_string())
            .unwrap_or_default()
    };
    assert_eq!(content_of("com.test.app"), content_of("com.other.app"));
    assert_eq!(
        func_first(&t, &sid, "com.test.app").await,
        func_first(&t, &sid, "com.other.app").await
    );
    let resp = get_json(&t, &sid, "/api/templates?pkg=com.other.app").await;
    let j = json_body(resp).await;
    assert_eq!(j.as_array().unwrap().len(), 1);
    assert_eq!(j[0]["name"], "icon.png");
}

// ---------- 模板上传命名契约（plan §11.7：短名 + 搜索区域由服务端组合完整名）----------

#[test]
fn short_name_and_region_composition_units() {
    // 短名合法口径：unicode 字母数字（含中文）+ `-` `_` + `.png`；`#` 是区域分隔符必须拒绝
    assert!(validate_short_name("record_click_20260829_001.png").is_ok());
    assert!(validate_short_name("  a-b_C9.png  ").is_ok());
    assert!(validate_short_name("中文.png").is_ok());
    assert!(validate_short_name("委托界面_2.png").is_ok());
    assert!(validate_short_name("x.jpg").is_err());
    assert!(validate_short_name("bad name!.png").is_err());
    assert!(validate_short_name(".png").is_err());
    assert!(validate_short_name("委#托.png").is_err());
    // 区域 ×1000 三位整数；1.0 钳到 999；越界夹取；退化（x2<=x1 / y2<=y1）拒绝
    assert_eq!(
        compose_region_suffix([0.1, 0.2, 0.3, 0.4]).unwrap(),
        "100_200_300_400"
    );
    assert_eq!(
        compose_region_suffix([0.0, 0.0, 1.0, 1.0]).unwrap(),
        "000_000_999_999"
    );
    assert_eq!(
        compose_region_suffix([-1.0, -1.0, 2.0, 2.0]).unwrap(),
        "000_000_999_999"
    );
    assert!(compose_region_suffix([0.5, 0.5, 0.5, 0.9]).is_err());
    assert!(compose_region_suffix([0.1, 0.9, 0.3, 0.2]).is_err());
}

#[tokio::test]
async fn template_upload_short_name_composes_full_name_and_rejects_conflict() {
    let t = build_app(
        "tmplshort",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    let png = base64::engine::general_purpose::STANDARD.encode(valid_template_png());

    // 短名 + region → 服务端组合 `<短名去.png>#x1_y1_x2_y2.png`
    let resp = post_json(
        &t,
        &sid,
        "/api/templates",
        serde_json::json!({
            "pkg": "com.test.app",
            "short_name": "login_btn.png",
            "region": [0.1, 0.2, 0.3, 0.4],
            "data_b64": png,
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let j = json_body(resp).await;
    assert_eq!(j["ok"], true);
    assert_eq!(j["name"], "login_btn#100_200_300_400.png");

    // 磁盘文件与列表都呈现完整名（引擎 #后缀即搜索区域元数据）
    assert!(t
        .dir
        .join("com.test.app/tmpl/login_btn#100_200_300_400.png")
        .is_file());
    let resp = get_json(&t, &sid, "/api/templates?pkg=com.test.app").await;
    let j = json_body(resp).await;
    assert_eq!(j.as_array().unwrap().len(), 1);
    assert_eq!(j[0]["name"], "login_btn#100_200_300_400.png");

    // 同短名再传（不同区域）→ 409 冲突不覆盖（§11.7 冲突要求改名）；
    // 磁盘上仍是第一次的完整名
    let resp = post_json(
        &t,
        &sid,
        "/api/templates",
        serde_json::json!({
            "pkg": "com.test.app",
            "short_name": "login_btn.png",
            "region": [0.0, 0.0, 0.5, 0.5],
            "data_b64": png,
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert!(t
        .dir
        .join("com.test.app/tmpl/login_btn#100_200_300_400.png")
        .is_file());
    assert!(!t
        .dir
        .join("com.test.app/tmpl/login_btn#000_000_500_500.png")
        .is_file());

    // 明确勾选保留颜色时，完整文件名尾部追加 #1；旧请求省略该字段仍是灰度格式。
    let resp = post_json(
        &t,
        &sid,
        "/api/templates",
        serde_json::json!({
            "pkg": "com.test.app",
            "short_name": "color_btn.png",
            "region": [0.1, 0.2, 0.3, 0.4],
            "grayscale_only": false,
            "data_b64": png,
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["name"], "color_btn#100_200_300_400#1.png");
    assert!(t
        .dir
        .join("com.test.app/tmpl/color_btn#100_200_300_400#1.png")
        .is_file());

    // 非法短名 / 非法 region / 参数互斥与缺参 → 400
    for body in [
        serde_json::json!({"pkg": "com.test.app", "short_name": "bad name!.png", "data_b64": png}),
        serde_json::json!({"pkg": "com.test.app", "short_name": "x.jpg", "data_b64": png}),
        serde_json::json!({"pkg": "com.test.app", "short_name": "ok.png", "region": [0.5, 0.5, 0.5, 0.5], "data_b64": png}),
        serde_json::json!({"pkg": "com.test.app", "short_name": "ok.png", "name": "y.png", "data_b64": png}),
        serde_json::json!({"pkg": "com.test.app", "data_b64": png}),
    ] {
        let resp = post_json(&t, &sid, "/api/templates", body).await;
        assert!(resp.status().is_client_error());
    }

    // 短名无 region → 无 # 后缀普通名落盘；旧的 name/data 上传形态拒绝
    let resp = post_json(
        &t,
        &sid,
        "/api/templates",
        serde_json::json!({
            "pkg": "com.test.app",
            "short_name": "plain.png",
            "data_b64": png,
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["name"], "plain.png");
    let resp = post_json(
        &t,
        &sid,
        "/api/templates",
        serde_json::json!({
            "pkg": "com.test.app",
            "name": "plain.png",
            "data_b64": png,
        }),
    )
    .await;
    assert!(resp.status().is_client_error());
}
