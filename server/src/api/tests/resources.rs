use super::*;

// ---------- 通用资源 API（P11.6）：/api/apps/:app/resources/:kind ----------
//
// 文本 kind（scripts/functions/keymaps/presets）走 JSON；字节 kind
// （templates/resources）走原始字节 + ?name=。资源 id 含 `/`，URL 里整体
// encodeURIComponent（%2F）。`app = "-"` 为跨分区通配。

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
    let png_headers = |cookie: String| {
        vec![
            (header::COOKIE.to_string(), cookie),
            (header::CONTENT_TYPE.to_string(), "image/png".into()),
        ]
    };

    // 像素炸弹：base64 体积小但解码尺寸超限 → 400
    let bomb = pixel_bomb_png(30_000, 30_000);
    let resp = send(
        &t.app,
        req_bytes(
            "POST",
            "/api/apps/com.test.app/resources/templates?name=bomb.png",
            None,
            &png_headers(sid.clone()),
            bomb,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // 超过原始模板字节上限：API 在解码前直接以 4xx 拒绝，不分配/解码图片
    let too_large = vec![b'A'; (matcher::TEMPLATE_MAX_INPUT_BYTES / 3 + 2) * 4];
    let resp = send(
        &t.app,
        req_bytes(
            "POST",
            "/api/apps/com.test.app/resources/templates?name=large.png",
            None,
            &png_headers(sid),
            too_large,
        ),
    )
    .await;
    assert!(resp.status().is_client_error());
}

#[tokio::test]
async fn request_body_limits_reject_oversize_json_and_package_install_with_413() {
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

    // App Package 安装 body 上限对齐包归档解压总量预算（BODY_LIMIT_PACKAGE_INSTALL）
    let zip_headers = [
        (header::COOKIE.to_string(), sid),
        (header::CONTENT_TYPE.to_string(), "application/zip".into()),
    ];
    let resp = send(
        &t.app,
        req_bytes(
            "POST",
            "/api/app-packages/install",
            None,
            &zip_headers,
            vec![0u8; BODY_LIMIT_PACKAGE_INSTALL + 1],
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
        ("GET", "/api/apps/com.test.app/resources/functions", None),
        (
            "POST",
            "/api/apps/com.test.app/resources/functions",
            Some(r#"{"name":"a","content":"x: {}\n"}"#),
        ),
        (
            "GET",
            "/api/apps/com.test.app/resources/functions/com.test.app%2Fa.yaml",
            None,
        ),
        (
            "PUT",
            "/api/apps/com.test.app/resources/functions/com.test.app%2Fa.yaml",
            Some(r#"{"content":"x: {}\n"}"#),
        ),
        (
            "DELETE",
            "/api/apps/com.test.app/resources/functions/com.test.app%2Fa.yaml",
            None,
        ),
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
    let body = serde_json::json!({"name": "common", "content": FUNC_YAML});
    let resp = send(
        &t.app,
        req(
            "POST",
            "/api/apps/com.test.app/resources/functions",
            None,
            &json_headers(sid.clone()),
            Some(body.to_string()),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let j = json_body(resp).await;
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
            "/api/apps/com.test.app/resources/functions",
            None,
            &json_headers(sid.clone()),
            Some(
                serde_json::json!({"name": "common", "content": FUNC_YAML}).to_string(),
            ),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    // list：返回文件短路径 + 函数名清单
    let resp = get_json(&t, &sid, "/api/apps/com.test.app/resources/functions").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let j = json_body(resp).await;
    assert_eq!(j[0]["id"], "com.test.app/common.yaml");
    assert_eq!(j[0]["file"], "common");
    assert_eq!(j[0]["version"], v1.as_str());
    assert_eq!(j[0]["functions"], serde_json::json!(["login"]));

    // get：内容往返一致
    let resp = get_json(
        &t,
        &sid,
        "/api/apps/com.test.app/resources/functions/com.test.app%2Fcommon.yaml",
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
            "/api/apps/com.test.app/resources/functions/com.test.app%2Fcommon.yaml",
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
            "/api/apps/com.test.app/resources/functions/com.test.app%2Fcommon.yaml",
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
            "/api/apps/com.test.app/resources/functions/com.test.app%2Fcommon.yaml",
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
            "/api/apps/com.test.app/resources/functions/com.test.app%2Fcommon.yaml",
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
            "/api/apps/com.test.app/resources/functions/com.test.app%2Fcommon.yaml",
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
            "/api/apps/com.test.app/resources/functions/com.test.app%2Frenamed.yaml",
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
            "/api/apps/com.test.app/resources/functions/com.test.app%2Frenamed.yaml",
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
            "/api/apps/com.test.app/resources/functions/com.test.app%2Fcommon.yaml",
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

    // 非法分区名（路径段）→ 400
    let resp = get_json(&t, &sid, "/api/apps/bad%2Fpkg/resources/functions").await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let resp = send(
        &t.app,
        req(
            "POST",
            "/api/apps/bad%2Fpkg/resources/functions",
            None,
            &json_headers(sid.clone()),
            Some(r#"{"name":"a","content":"x:\n  steps: []\n"}"#.into()),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // 严格 loader：顶层键保留字 / 非法函数名 / YAML 语法错 / 非字符串标量
    let cases = [
        ("match:\n  steps: []\n", "保留字"),
        ("1abc:\n  steps: []\n", "只允许 unicode 字母"),
        ("带 空 格:\n  steps: []\n", "只允许 unicode 字母"),
        ("login: [unclosed", "flow"),
        (
            "123:
  steps: []
",
            "不是字符串标量",
        ),
    ];
    for (content, marker) in cases {
        let body = serde_json::json!({"name": "bad", "content": content});
        let resp = send(
            &t.app,
            req(
                "POST",
                "/api/apps/com.test.app/resources/functions",
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
    // P12.5：子目录短路径放开（function:common/lib/fn 形态）——嵌套保存 201 且可读回
    let body = serde_json::json!({"name": "sub/common", "content": FUNC_YAML});
    let resp = send(
        &t.app,
        req(
            "POST",
            "/api/apps/com.test.app/resources/functions",
            None,
            &json_headers(sid.clone()),
            Some(body.to_string()),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = send(
        &t.app,
        req(
            "GET",
            "/api/apps/com.test.app/resources/functions/com.test.app%2Fsub%2Fcommon.yaml",
            None,
            &json_headers(sid.clone()),
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // PUT / GET 不存在的函数文件 → 404
    for (method, uri, body) in [
        (
            "GET",
            "/api/apps/com.test.app/resources/functions/com.test.app%2Fnope.yaml",
            None,
        ),
        (
            "PUT",
            "/api/apps/com.test.app/resources/functions/com.test.app%2Fnope.yaml",
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
    let resp = post_json(
        &t,
        &sid,
        "/api/apps/com.test.app/resources/scripts",
        serde_json::json!({"name": "main.yaml", "content": "version: 3\nsteps: []\n"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = post_json(
        &t,
        &sid,
        "/api/apps/com.test.app/resources/functions",
        serde_json::json!({"name": "common.yaml", "content": FUNC_YAML}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);

    // 脚本列表只含 scripts/ 脚本，functions/ 文件绝不混入
    let resp = get_json(&t, &sid, "/api/apps/com.test.app/resources/scripts").await;
    let j = json_body(resp).await;
    assert_eq!(j.as_array().unwrap().len(), 1);
    assert_eq!(j[0]["name"], "main.yaml");

    // 函数 id 在脚本读取/运行接口一律不可见（目录即类型，不做内容推断）
    let resp = get_json(
        &t,
        &sid,
        "/api/apps/com.test.app/resources/scripts/com.test.app%2Fcommon.yaml",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let resp = post_json(
        &t,
        &sid,
        "/api/runs",
        serde_json::json!({
            "runner_id": "gamer.yaml",
            "entrypoint": "com.test.app/common.yaml",
            "device_id": "d1",
            "payload": {},
        }),
    )
    .await;
    assert!(resp.status().is_client_error());
    assert_eq!(json_body(resp).await["error"], "not_found");

    // 函数列表也只含 functions/ 文件
    let resp = get_json(&t, &sid, "/api/apps/com.test.app/resources/functions").await;
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
        "/api/apps/com.test.app/resources/scripts",
        serde_json::json!({"name": "main.yaml", "content": "version: 3\nsteps:\n  - log: v1\n"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let v1 = json_body(resp).await["version"]
        .as_str()
        .unwrap()
        .to_string();

    // GET 单脚本返回内容与版本
    let resp = get_json(
        &t,
        &sid,
        "/api/apps/com.test.app/resources/scripts/com.test.app%2Fmain.yaml",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let j = json_body(resp).await;
    assert_eq!(j["version"], v1.as_str());
    assert!(j["content"].as_str().unwrap().contains("log: v1"));

    // POST 只创建：已有资源再次 POST → 409
    let resp = post_json(
        &t,
        &sid,
        "/api/apps/com.test.app/resources/scripts",
        serde_json::json!({"name": "main.yaml", "content": "version: 3\nsteps: []\n"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let j = json_body(resp).await;
    assert_eq!(j["error"], "资源已存在: com.test.app/main.yaml");

    // POST body 拒绝旧的 id/expected_version 更新形态（deny_unknown_fields）
    let resp = post_json(
        &t,
        &sid,
        "/api/apps/com.test.app/resources/scripts",
        serde_json::json!({"name": "other.yaml", "content": "version: 3\nsteps: []\n", "expected_version": v1}),
    )
    .await;
    assert!(resp.status().is_client_error());
}

// ---------- 模板上传（通用资源 API：客户端组合完整名，原始字节 body）----------

#[test]
fn short_name_and_region_composition_units() {
    // 短名合法口径：unicode 字母数字（含中文）+ `-` `_` + `.png`；`#` 是区域分隔符必须拒绝
    assert!(validate_short_name("button_20260829_001.png").is_ok());
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

fn png_headers(cookie: &str) -> Vec<(String, String)> {
    vec![
        (header::COOKIE.to_string(), cookie.to_string()),
        (
            header::CONTENT_TYPE.to_string(),
            "image/png".to_string(),
        ),
    ]
}

#[tokio::test]
async fn template_upload_short_name_composes_full_name_and_rejects_conflict() {
    let t = build_app(
        "tmplshort",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    let png = valid_template_png();
    // 客户端组合完整名（与前端 defaultTemplateName 同编码）：短名 + region 后缀
    let compose = |short: &str, region: Option<[f64; 4]>, color: bool| {
        let base = short.trim_end_matches(".png");
        let suffix = region.map(|r| compose_region_suffix(r).unwrap());
        let mut name = String::from(base);
        if let Some(suffix) = suffix {
            name.push('#');
            name.push_str(&suffix);
        }
        if color {
            name.push_str("#1");
        }
        name.push_str(".png");
        name
    };

    // 短名 + region → `login_btn#100_200_300_400.png`
    let name = compose("login_btn.png", Some([0.1, 0.2, 0.3, 0.4]), false);
    let resp = send(
        &t.app,
        req_bytes(
            "POST",
            &format!(
                "/api/apps/com.test.app/resources/templates?name={}",
                urlencoding_encode(&name)
            ),
            None,
            &png_headers(&sid),
            png.clone(),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED, "{}", text_or_empty(resp).await);
    let j = json_body(resp).await;
    assert_eq!(j["ok"], true);
    assert_eq!(j["name"], "login_btn#100_200_300_400.png");

    // 磁盘文件与列表都呈现完整名（引擎 #后缀即搜索区域元数据）
    assert!(t
        .dir
        .join("com.test.app/templates/login_btn#100_200_300_400.png")
        .is_file());
    let resp = get_json(&t, &sid, "/api/apps/com.test.app/resources/templates").await;
    let j = json_body(resp).await;
    assert_eq!(j.as_array().unwrap().len(), 1);
    assert_eq!(j[0]["name"], "login_btn#100_200_300_400.png");

    // 同短名再传（不同区域）→ 409 冲突不覆盖（§11.7 冲突要求改名）；
    // 磁盘上仍是第一次的完整名
    let name = compose("login_btn.png", Some([0.0, 0.0, 0.5, 0.5]), false);
    let resp = send(
        &t.app,
        req_bytes(
            "POST",
            &format!(
                "/api/apps/com.test.app/resources/templates?name={}",
                urlencoding_encode(&name)
            ),
            None,
            &png_headers(&sid),
            png.clone(),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert!(t
        .dir
        .join("com.test.app/templates/login_btn#100_200_300_400.png")
        .is_file());
    assert!(!t
        .dir
        .join("com.test.app/templates/login_btn#000_000_500_500.png")
        .is_file());

    // 明确勾选保留颜色时，完整文件名尾部追加 #1；省略该标记仍是灰度格式。
    let name = compose("color_btn.png", Some([0.1, 0.2, 0.3, 0.4]), true);
    let resp = send(
        &t.app,
        req_bytes(
            "POST",
            &format!(
                "/api/apps/com.test.app/resources/templates?name={}",
                urlencoding_encode(&name)
            ),
            None,
            &png_headers(&sid),
            png.clone(),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert_eq!(
        json_body(resp).await["name"],
        "color_btn#100_200_300_400#1.png"
    );
    assert!(t
        .dir
        .join("com.test.app/templates/color_btn#100_200_300_400#1.png")
        .is_file());

    // 非法模板名 → 400；缺 name 参数 → 400
    for query in [
        format!("name={}&pkg=com.test.app", urlencoding_encode("bad name!.png")),
        format!("name={}&pkg=com.test.app", urlencoding_encode("a/b.png")),
        String::from("pkg=com.test.app"),
    ] {
        let resp = send(
            &t.app,
            req_bytes(
                "POST",
                &format!("/api/apps/com.test.app/resources/templates?{query}"),
                None,
                &png_headers(&sid),
                png.clone(),
            ),
        )
        .await;
        assert!(
            resp.status().is_client_error(),
            "{query}: {}",
            resp.status()
        );
    }

    // 短名无 region → 无 # 后缀普通名落盘
    let resp = send(
        &t.app,
        req_bytes(
            "POST",
            &format!(
                "/api/apps/com.test.app/resources/templates?name={}",
                urlencoding_encode("plain.png")
            ),
            None,
            &png_headers(&sid),
            png,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert_eq!(json_body(resp).await["name"], "plain.png");
}

/// 极简 percent-encode（测试用：模板名只含 #/_/字母数字/点）
fn urlencoding_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

async fn text_or_empty(mut resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(std::mem::take(resp.body_mut()), 64 * 1024)
        .await
        .unwrap_or_default();
    String::from_utf8_lossy(&bytes).to_string()
}
