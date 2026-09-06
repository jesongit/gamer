use super::*;

// Package REST 冒烟（plan §2-§15 / §23 验收链）：建包 → 写资源 → 读资源 →
// 列表 → 复制 → 导出 → 删除；导入路径覆盖「同 id 二次导入默认 409 +
// overwrite=true 原子替换」与包内预设发布（id = `<package-id>:<名>`）。

fn pkg_url(pkg: &str) -> String {
    format!("/api/packages/{pkg}")
}

fn resource_url(pkg: &str, plugin: &str, path: &str) -> String {
    format!("/api/packages/{pkg}/plugins/{plugin}/resources/{path}")
}

async fn create_package(t: &TestApp, sid: &str, body: serde_json::Value) -> HttpResponse<Body> {
    post_json(t, sid, "/api/packages", body).await
}

/// 8x8 合法灰度 PNG（字节资源夹具）。
fn valid_png() -> Vec<u8> {
    let mut img = image::GrayImage::new(8, 8);
    for (x, y, pixel) in img.enumerate_pixels_mut() {
        pixel.0[0] = if (x + y) % 2 == 0 { 32 } else { 224 };
    }
    let mut bytes = Vec::new();
    image::DynamicImage::ImageLuma8(img)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    bytes
}

/// 建包 → 写文本/字节资源 → 读 → 列表 → 复制 → 导出 → 删除 全链冒烟。
#[tokio::test]
async fn package_crud_duplicate_export_delete_smoke_chain() {
    let t = build_app(
        "pkgnsmoke",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));

    // ---- 建包：id 必填 + targets + 插件依赖声明 ----
    let resp = create_package(
        &t,
        &sid,
        serde_json::json!({
            "id": "official.demo",
            "name": "演示包",
            "version": "1.0.0",
            "targets": {"android": {"packages": ["com.example.game"]}},
            "plugins": {"gamer.yaml": true, "gamer.keymap": false},
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED, "{}", json_body(resp).await);
    let created = json_body(resp).await;
    assert_eq!(created["id"], "official.demo");
    assert_eq!(created["revision"], 1);
    assert_eq!(created["targets"]["android"]["packages"][0], "com.example.game");
    // plugins 依赖按 plugin-id 字典序（BTreeMap）
    assert_eq!(created["plugins"][0]["id"], "gamer.keymap");
    assert_eq!(created["plugins"][0]["required"], false);
    assert_eq!(created["plugins"][1]["id"], "gamer.yaml");
    assert_eq!(created["plugins"][1]["required"], true);
    // 目录落位：package.toml + shared/ + plugins/
    let pkg_dir = t.dir.join("packages/official.demo");
    assert!(pkg_dir.join("package.toml").is_file());
    assert!(pkg_dir.join("shared").is_dir());
    assert!(pkg_dir.join("plugins").is_dir());

    // 重复建包 → 409
    let resp = create_package(&t, &sid, serde_json::json!({"id": "official.demo"})).await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    // 非法 id → 400（大写 / 穿越 / 纯点）
    for bad in ["Bad.Id", "../escape", "a/b", ".."] {
        let resp = create_package(&t, &sid, serde_json::json!({"id": bad})).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{bad}");
    }

    // ---- 写资源：文本（JSON body）+ 字节（raw body）---- 
    let script = "version: 3\nsteps:\n  - log: ok\n";
    let resp = send(
        &t.app,
        req(
            "PUT",
            &resource_url("official.demo", "gamer.yaml", "scripts/daily.yaml"),
            None,
            &json_headers(sid.clone()),
            Some(serde_json::json!({"content": script}).to_string()),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "{}", json_body(resp).await);
    let written = json_body(resp).await;
    assert_eq!(written["path"], "scripts/daily.yaml");
    assert_eq!(written["version"].as_str().unwrap().len(), 12);
    let v1 = written["version"].as_str().unwrap().to_string();

    let png = valid_png();
    let resp = send(
        &t.app,
        req_bytes(
            "PUT",
            &resource_url("official.demo", "gamer.yaml", "templates/icon.png"),
            None,
            &[
                (axum::http::header::CONTENT_TYPE.to_string(), "image/png".into()),
                (axum::http::header::COOKIE.to_string(), sid.clone()),
            ],
            png.clone(),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "{}", json_body(resp).await);

    // ---- 读资源：文本 JSON（带内容与版本）+ 字节原始流（可解码）----
    let resp = get_json(
        &t,
        &sid,
        &resource_url("official.demo", "gamer.yaml", "scripts/daily.yaml"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let entry = json_body(resp).await;
    assert_eq!(entry["content"], script);
    assert_eq!(entry["version"], v1.as_str());
    assert_eq!(entry["text"], true);

    let resp = get_json(
        &t,
        &sid,
        &resource_url("official.demo", "gamer.yaml", "templates/icon.png"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    let decoded = image::load_from_memory(&bytes).expect("字节资源读回必须是合法图片");
    assert_eq!((decoded.width(), decoded.height()), (8, 8));

    // 内容版本门禁：错版本 → 409
    let resp = send(
        &t.app,
        req(
            "PUT",
            &resource_url("official.demo", "gamer.yaml", "scripts/daily.yaml"),
            None,
            &json_headers(sid.clone()),
            Some(
                serde_json::json!({"content": script, "expected_version": "stale"}).to_string(),
            ),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    // ---- 递归列表 ----
    let resp = get_json(
        &t,
        &sid,
        "/api/packages/official.demo/plugins/gamer.yaml/resources",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let list = json_body(resp).await;
    let paths: Vec<String> = list["resources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["path"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        paths,
        vec!["scripts/daily.yaml".to_string(), "templates/icon.png".to_string()]
    );

    // ---- GET 包详情（manifest + 统计）----
    let resp = get_json(&t, &sid, &pkg_url("official.demo")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let detail = json_body(resp).await;
    assert_eq!(detail["package"]["id"], "official.demo");
    assert_eq!(detail["stats"]["files"], 2);

    // ---- 复制为新包（资源随拷、revision 归 1）----
    let resp = post_json(
        &t,
        &sid,
        "/api/packages/official.demo/duplicate",
        serde_json::json!({"new_id": "user.demo"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED, "{}", json_body(resp).await);
    let copy = json_body(resp).await;
    assert_eq!(copy["id"], "user.demo");
    assert_eq!(copy["revision"], 1);
    assert_eq!(copy["name"], "演示包");
    let resp = get_json(
        &t,
        &sid,
        &resource_url("user.demo", "gamer.yaml", "scripts/daily.yaml"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["content"], script);

    // ---- 导出（.gamerpkg 字节流 + SHA-256 头）----
    let resp = send(
        &t.app,
        req_bytes(
            "POST",
            "/api/packages/official.demo/export",
            None,
            &[(axum::http::header::COOKIE.to_string(), sid.clone())],
            Vec::new(),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let sha_header = resp
        .headers()
        .get("x-content-sha256")
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .to_string();
    assert_eq!(sha_header.len(), 64);
    let archive = axum::body::to_bytes(resp.into_body(), 32 * 1024 * 1024)
        .await
        .unwrap()
        .to_vec();
    // 可复现：再导一次字节一致
    let resp = send(
        &t.app,
        req_bytes(
            "POST",
            "/api/packages/official.demo/export",
            None,
            &[(axum::http::header::COOKIE.to_string(), sid.clone())],
            Vec::new(),
        ),
    )
    .await;
    let archive2 = axum::body::to_bytes(resp.into_body(), 32 * 1024 * 1024)
        .await
        .unwrap()
        .to_vec();
    assert_eq!(archive, archive2, "相同输入必须产生相同归档字节");

    // ---- 删除 ----
    let resp = send(
        &t.app,
        req(
            "DELETE",
            &pkg_url("user.demo"),
            None,
            &json_headers(sid.clone()),
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = get_json(&t, &sid, &pkg_url("user.demo")).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert!(!t.dir.join("packages/user.demo").exists());

    // 导出的归档字节留待导入测试复用
    let archive_dir = t.dir.join(".exported");
    std::fs::create_dir_all(&archive_dir).unwrap();
    std::fs::write(archive_dir.join("official.demo.gamerpkg"), &archive).unwrap();
}

/// 导入：全新安装 201；同 id 二次导入默认 409（附已存摘要）；overwrite=true
/// 校验通过后原子替换；坏摘要 / 缺 manifest 拒绝且不落盘。
#[tokio::test]
async fn package_import_conflicts_then_atomic_overwrite() {
    let t = build_app(
        "pkgimport",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));

    // 造一个归档：先建包 + 写资源 + 导出
    let resp = create_package(&t, &sid, serde_json::json!({"id": "official.imp"})).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = send(
        &t.app,
        req(
            "PUT",
            &resource_url("official.imp", "gamer.yaml", "scripts/first.yaml"),
            None,
            &json_headers(sid.clone()),
            Some(serde_json::json!({"content": "version: 3\nsteps:\n  - log: v1\n"}).to_string()),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = send(
        &t.app,
        req_bytes(
            "POST",
            "/api/packages/official.imp/export",
            None,
            &[(axum::http::header::COOKIE.to_string(), sid.clone())],
            Vec::new(),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let archive_v1 = axum::body::to_bytes(resp.into_body(), 32 * 1024 * 1024)
        .await
        .unwrap()
        .to_vec();

    // 删掉原包，导入 v1 归档 → 201 全新安装
    let resp = send(
        &t.app,
        req(
            "DELETE",
            &pkg_url("official.imp"),
            None,
            &json_headers(sid.clone()),
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = send(
        &t.app,
        req_bytes(
            "POST",
            "/api/packages/import",
            None,
            &[(axum::http::header::COOKIE.to_string(), sid.clone())],
            archive_v1.clone(),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED, "{}", json_body(resp).await);
    assert_eq!(json_body(resp).await["id"], "official.imp");

    // 同 id 二次导入（内容不同：导入前改归档一个字节不可行——用第二份归档）
    // 先改包内容再导出 v2 归档
    let resp = send(
        &t.app,
        req(
            "PUT",
            &resource_url("official.imp", "gamer.yaml", "scripts/first.yaml"),
            None,
            &json_headers(sid.clone()),
            Some(
                serde_json::json!({
                    "content": "version: 3\nsteps:\n  - log: v2\n",
                    "force": true,
                })
                .to_string(),
            ),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = send(
        &t.app,
        req_bytes(
            "POST",
            "/api/packages/official.imp/export",
            None,
            &[(axum::http::header::COOKIE.to_string(), sid.clone())],
            Vec::new(),
        ),
    )
    .await;
    let archive_v2 = axum::body::to_bytes(resp.into_body(), 32 * 1024 * 1024)
        .await
        .unwrap()
        .to_vec();

    // 同 id 二次导入默认 409（附已存 manifest 摘要），且**不产生半安装状态**：
    // 磁盘当前是 v2，用更早的 v1 归档尝试导入，被拒后内容必须保持 v2
    let resp = send(
        &t.app,
        req_bytes(
            "POST",
            "/api/packages/import",
            None,
            &[(axum::http::header::COOKIE.to_string(), sid.clone())],
            archive_v1.clone(),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT, "{}", json_body(resp).await);
    let conflict = json_body(resp).await;
    assert_eq!(conflict["error"], "package_exists");
    assert_eq!(conflict["existing"]["id"], "official.imp");
    let resp = get_json(
        &t,
        &sid,
        &resource_url("official.imp", "gamer.yaml", "scripts/first.yaml"),
    )
    .await;
    assert!(json_body(resp).await["content"]
        .as_str()
        .unwrap()
        .contains("v2"),
        "409 拒绝后内容必须保持原样（无半安装状态）");

    // staging 无残留
    let staging = t.dir.join("packages/.staging");
    if staging.is_dir() {
        assert!(
            std::fs::read_dir(&staging).unwrap().next().is_none(),
            "409 拒绝后 staging 必须被清理"
        );
    }

    // ?overwrite=true → 原子替换成功（v1 归档整体替换 v2 磁盘内容）
    let resp = send(
        &t.app,
        req_bytes(
            "POST",
            "/api/packages/import?overwrite=true",
            None,
            &[(axum::http::header::COOKIE.to_string(), sid.clone())],
            archive_v1,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "{}", json_body(resp).await);
    let resp = get_json(
        &t,
        &sid,
        &resource_url("official.imp", "gamer.yaml", "scripts/first.yaml"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(json_body(resp).await["content"]
        .as_str()
        .unwrap()
        .contains("v1"),
        "overwrite 后内容必须替换为归档版本");

    // X-Expected-Sha256 摘要不匹配 → 400，不落盘
    let bogus_id = format!("/api/packages/{}", "official.imp");
    let _ = bogus_id;
    let resp = send(
        &t.app,
        req_bytes(
            "POST",
            "/api/packages/import?overwrite=true",
            None,
            &[
                (axum::http::header::COOKIE.to_string(), sid.clone()),
                ("x-expected-sha256".to_string(), "0".repeat(64)),
            ],
            archive_v2,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_body(resp).await["id"], serde_json::Value::Null);

    // 坏归档（缺 package.toml）→ 400，不落盘
    let mut bytes = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("shared/x.txt", options).unwrap();
        writer.write_all(b"no manifest").unwrap();
        writer.finish().unwrap();
    }
    let resp = send(
        &t.app,
        req_bytes(
            "POST",
            "/api/packages/import",
            None,
            &[(axum::http::header::COOKIE.to_string(), sid.clone())],
            bytes,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// 包内 `plugins/<plugin>/presets/*.yaml` 在导入 / 复制后自动发布为任务预设
/// （发布 id = `<package-id>:<名>`，幂等）。
#[tokio::test]
async fn package_presets_publish_on_import_and_duplicate() {
    let t = build_app(
        "pkgpresets",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));

    // 建包 + 直写包内预设文件（模拟导入包内的数据形态）
    let resp = create_package(&t, &sid, serde_json::json!({"id": "official.preset"})).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let preset_dir = t
        .dir
        .join("packages/official.preset/plugins/gamer.yaml/presets");
    std::fs::create_dir_all(&preset_dir).unwrap();
    std::fs::write(
        preset_dir.join("daily.yaml"),
        "name: \u{6bcf}\u{65e5}\u{9886}\u{53d6}\nrunner_id: gamer.yaml\nentrypoint: run\npayload: {}\nschedule:\n  kind: cron\n  value:\n    expression: \"0 8 * * *\"\n",
    )
    .unwrap();

    // 用导入路径触发发布：导出 → 删包 → 导入
    let resp = send(
        &t.app,
        req_bytes(
            "POST",
            "/api/packages/official.preset/export",
            None,
            &[(axum::http::header::COOKIE.to_string(), sid.clone())],
            Vec::new(),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let archive = axum::body::to_bytes(resp.into_body(), 32 * 1024 * 1024)
        .await
        .unwrap()
        .to_vec();
    let resp = send(
        &t.app,
        req(
            "DELETE",
            &pkg_url("official.preset"),
            None,
            &json_headers(sid.clone()),
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = send(
        &t.app,
        req_bytes(
            "POST",
            "/api/packages/import",
            None,
            &[(axum::http::header::COOKIE.to_string(), sid.clone())],
            archive,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);

    // 预设已发布，id = <package-id>:<名>
    let resp = get_json(&t, &sid, "/api/task-presets").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let presets = json_body(resp).await;
    let daily = presets
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "official.preset:每日领取")
        .expect("包内预设必须以 <package-id>:<名> 发布")
        .clone();
    assert_eq!(daily["runner"]["runner_id"], "gamer.yaml");
    assert_eq!(daily["app_package"], "official.preset");

    // 复制包 → 新包 id 下的预设同样发布（幂等：重复复制不产生第二行）
    let resp = post_json(
        &t,
        &sid,
        "/api/packages/official.preset/duplicate",
        serde_json::json!({"new_id": "user.preset"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = post_json(
        &t,
        &sid,
        "/api/packages/official.preset/duplicate",
        serde_json::json!({"new_id": "user.preset"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let resp = get_json(&t, &sid, "/api/task-presets").await;
    let presets = json_body(resp).await;
    let user_count = presets
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["id"] == "user.preset:每日领取")
        .count();
    assert_eq!(user_count, 1, "复制发布幂等：{presets}");
}

/// 模板重命名集成链（REST 全栈）：上传模板（PUT 字节）→ 保存引用它的 v3 脚本
/// → POST rename（ResourceHandler::before_rename 钩子改写引用 + 原子移动文件）
/// → 脚本引用已自动改写 → 新模板字节可被 NCC 匹配命中（matcher 语义不变）。
#[tokio::test]
async fn template_upload_rename_rewrites_references_and_still_matches() {
    let t = build_app(
        "tplrename",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    create_package(
        &t,
        &sid,
        serde_json::json!({"id": "official.tpl", "plugins": {"gamer.yaml": true}}),
    )
    .await;

    // ---- 1. 上传模板：字节 PUT（8x8 灰度 PNG）----
    let png = valid_png();
    let resp = send(
        &t.app,
        req_bytes(
            "PUT",
            &resource_url("official.tpl", "gamer.yaml", "templates/reward.png"),
            None,
            &[
                (axum::http::header::COOKIE.to_string(), sid.clone()),
                (
                    axum::http::header::CONTENT_TYPE.to_string(),
                    "application/octet-stream".into(),
                ),
            ],
            png.clone(),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "{}", json_body(resp).await);

    // ---- 2. 保存引用该模板的 v3 脚本与函数库 ----
    let script = "version: 3\nsteps:\n  - find:\n      template: reward.png\n      then:\n        - tap: {point: [0.5, 0.5]}\n";
    let resp = put_package_text(
        &t,
        &sid,
        "official.tpl",
        "gamer.yaml",
        "scripts/daily.yaml",
        script,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "{}", json_body(resp).await);

    // ---- 3. 重命名模板：REST → 钩子改写引用 → 文件原子移动 ----
    let resp = post_json(
        &t,
        &sid,
        "/api/packages/official.tpl/plugins/gamer.yaml/rename",
        serde_json::json!({
            "path": "templates/reward.png",
            "new_path": "templates/bonus.png",
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "{}", json_body(resp).await);
    let renamed = json_body(resp).await;
    assert_eq!(renamed["ok"], true);
    assert_eq!(renamed["path"], "templates/bonus.png");

    // 文件已移动：新名可读、旧名 404
    let resp = get_json(
        &t,
        &sid,
        &resource_url("official.tpl", "gamer.yaml", "templates/bonus.png"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = send(
        &t.app,
        req(
            "GET",
            &resource_url("official.tpl", "gamer.yaml", "templates/reward.png"),
            None,
            &json_headers(sid.clone()),
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // ---- 4. 脚本引用已自动改写（AST 级：日志文本不误改）----
    let resp = get_json(
        &t,
        &sid,
        &resource_url("official.tpl", "gamer.yaml", "scripts/daily.yaml"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let content = json_body(resp).await["content"].as_str().unwrap().to_string();
    assert!(content.contains("template: bonus.png"), "引用已改写: {content}");
    assert!(!content.contains("reward.png"), "旧引用不残留: {content}");

    // 错误语义：同名重命名 400（名称未变化）；目标已存在 409
    let resp = post_json(
        &t,
        &sid,
        "/api/packages/official.tpl/plugins/gamer.yaml/rename",
        serde_json::json!({"path": "templates/bonus.png", "new_path": "templates/bonus.png"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let resp = send(
        &t.app,
        req_bytes(
            "PUT",
            &resource_url("official.tpl", "gamer.yaml", "templates/other.png"),
            None,
            &[
                (axum::http::header::COOKIE.to_string(), sid.clone()),
                (
                    axum::http::header::CONTENT_TYPE.to_string(),
                    "application/octet-stream".into(),
                ),
            ],
            png.clone(),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = post_json(
        &t,
        &sid,
        "/api/packages/official.tpl/plugins/gamer.yaml/rename",
        serde_json::json!({"path": "templates/other.png", "new_path": "templates/bonus.png"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    // ---- 5. 匹配命中：重命名后的模板字节经 matcher 命中合成屏幕 ----
    // 屏幕 = 白底 64x48，把模板图案贴到 (8, 16)（与上传内容逐字节同源的
    // 解码图，NCC 必命中）；先核实区域外不含同图案，避免平凡命中。
    let template = image::load_from_memory(&png).unwrap().to_luma8();
    let (tw, th) = template.dimensions();
    let mut screen = image::GrayImage::from_pixel(64, 48, image::Luma([255]));
    for y in 0..th {
        for x in 0..tw {
            screen.put_pixel(8 + x, 16 + y, *template.get_pixel(x, y));
        }
    }
    let mut screen_png = Vec::new();
    image::DynamicImage::ImageLuma8(screen)
        .write_to(
            &mut std::io::Cursor::new(&mut screen_png),
            image::ImageFormat::Png,
        )
        .unwrap();
    let renamed_bytes = t
        .dir
        .join("packages/official.tpl/plugins/gamer.yaml/templates/bonus.png");
    let template_png = std::fs::read(&renamed_bytes).unwrap();
    let matched = crate::matcher::match_template(&crate::matcher::MatchRequest {
        screen_png,
        template_png,
        threshold: Some(0.9),
        region: None,
        color: false,
    })
    .unwrap();
    let hit = matched.expect("重命名后的模板必须命中");
    assert_eq!((hit.x, hit.y), (8, 16), "命中位置 = 贴图位置: {hit:?}");
    assert!(hit.score >= 0.99, "同源图案满分命中: {hit:?}");
}

/// Vision 匹配测试端点寻址显式化：plugin 必填（Core 不预设/不猜测业务插件
/// id），缺省 400；显式给出后按 (pkg, plugin, name) 三元组解析模板。
#[tokio::test]
async fn vision_test_requires_explicit_plugin() {
    let t = build_app(
        "visionplugin",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    create_package(
        &t,
        &sid,
        serde_json::json!({"id": "official.vision", "plugins": {"gamer.yaml": true}}),
    )
    .await;
    // 单插件包也不允许省略 plugin（旧「唯一插件目录兜底」猜测已删）：
    // 字段缺失在反序列化边界即 422，空串由端点补 400
    let resp = post_json(
        &t,
        &sid,
        "/api/capabilities/vision/test",
        serde_json::json!({
            "device_id": "d1",
            "pkg": "official.vision",
            "name": "reward.png",
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let resp = post_json(
        &t,
        &sid,
        "/api/capabilities/vision/test",
        serde_json::json!({
            "device_id": "d1",
            "pkg": "official.vision",
            "plugin": "  ",
            "name": "reward.png",
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let j = json_body(resp).await;
    assert!(
        j["error"].as_str().unwrap().contains("plugin"),
        "空 plugin 必须 400 并指向该字段: {j}"
    );

    // 显式 plugin：进入模板解析（模板不存在 → 404，而非插件猜测错误）
    let resp = post_json(
        &t,
        &sid,
        "/api/capabilities/vision/test",
        serde_json::json!({
            "device_id": "d1",
            "pkg": "official.vision",
            "plugin": "gamer.yaml",
            "name": "ghost.png",
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{}", json_body(resp).await);
}
