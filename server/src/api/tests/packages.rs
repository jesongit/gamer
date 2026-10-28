use super::*;

fn seed_package_import_rollback_media(
    t: &TestApp,
    id: &str,
    package: &str,
) -> (crate::media::MediaMetadata, Vec<u8>) {
    let service = crate::media::service(&Config {
        data_dir: t.dir.clone(),
        ..Default::default()
    });
    let bytes = format!("synthetic-media-{id}").into_bytes();
    service
        .restore_media(
            crate::media::RestoreMedia {
                id: crate::media::MediaId(id.into()),
                name: "fixture.mp4".into(),
                sha256: crate::package_archive::sha256_hex(&bytes),
                size: bytes.len() as u64,
                probe: None,
            },
            &bytes,
            crate::media::MediaRef {
                package_id: package.into(),
                plugin_id: "gamer-video".into(),
                kind: "original".into(),
            },
        )
        .unwrap();
    let path = t.dir.join("media").join(id).join("metadata.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["future_unknown_metadata"] = serde_json::json!({"preserve":"original raw bytes"});
    let original = serde_json::to_vec_pretty(&value).unwrap();
    std::fs::write(path, &original).unwrap();
    (
        service.get(&crate::media::MediaId(id.into())).unwrap(),
        original,
    )
}

fn import_rollback_manifest(package: &str, version: &str) -> Vec<u8> {
    format!("id='{package}'\nversion='{version}'\n[targets.android]\npackages=['*']\n").into_bytes()
}

#[tokio::test]
async fn package_import_conflict_preserves_refs_and_unknown_metadata_before_any_media_write() {
    let t = build_app(
        "import-conflict-rollback",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    assert_eq!(
        create_package(
            &t,
            &sid,
            serde_json::json!({"id":"rollback.conflict","version":"1.0.0"})
        )
        .await
        .status(),
        StatusCode::CREATED
    );
    let (_, original) =
        seed_package_import_rollback_media(&t, "originalconflict", "rollback.conflict");
    let metadata_path = t.dir.join("media/originalconflict/metadata.json");
    let mut value: serde_json::Value = serde_json::from_slice(&original).unwrap();
    value["refs"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "package_id":"other.package","plugin_id":"gamer-video","kind":"foreign"
        }));
    let original = serde_json::to_vec_pretty(&value).unwrap();
    std::fs::write(&metadata_path, &original).unwrap();
    let archive = craft_zip(vec![
        (
            "package.toml",
            import_rollback_manifest("rollback.conflict", "2.0.0"),
        ),
        (
            "media/index.json",
            br#"{"schema_version":1,"entries":[]}"#.to_vec(),
        ),
    ]);
    let response = send(
        &t.app,
        req_bytes(
            "POST",
            "/api/packages/import",
            None,
            &zip_headers(sid.clone()),
            archive,
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        t.packages.manifest("rollback.conflict").unwrap().version,
        "1.0.0"
    );
    assert_eq!(std::fs::read(&metadata_path).unwrap(), original);
    let service = crate::media::service(&Config {
        data_dir: t.dir.clone(),
        ..Default::default()
    });
    let id = crate::media::MediaId("originalconflict".into());
    let file = service.file_path(&id).unwrap();
    let original_bytes = std::fs::read(&file).unwrap();
    let no_media_index = craft_zip(vec![(
        "package.toml",
        import_rollback_manifest("rollback.conflict", "2.0.0"),
    )]);
    let response = send(
        &t.app,
        req_bytes(
            "POST",
            "/api/packages/import?overwrite=true",
            None,
            &zip_headers(sid),
            no_media_index,
        ),
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "{}",
        json_body(response).await
    );
    let retained_media = service.get(&id).unwrap();
    assert_eq!(retained_media.refs.len(), 1);
    assert_eq!(retained_media.refs[0].package_id, "other.package");
    assert_eq!(std::fs::read(file).unwrap(), original_bytes);
    let metadata: serde_json::Value =
        serde_json::from_slice(&std::fs::read(metadata_path).unwrap()).unwrap();
    assert_eq!(
        metadata["future_unknown_metadata"],
        serde_json::json!({"preserve":"original raw bytes"})
    );
}

#[tokio::test]
async fn package_import_media_failure_restores_reused_refs_unknown_metadata_and_new_media() {
    let t = build_app(
        "import-media-rollback",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    assert_eq!(
        create_package(
            &t,
            &sid,
            serde_json::json!({"id":"rollback.media","version":"1.0.0"})
        )
        .await
        .status(),
        StatusCode::CREATED
    );
    let (_, old_metadata) = seed_package_import_rollback_media(&t, "oldmedia", "rollback.media");
    let (reused, reused_metadata) =
        seed_package_import_rollback_media(&t, "areused", "other.package");
    let new = b"new media";
    let new_sha = crate::package_archive::sha256_hex(new);
    let broken_sha = crate::package_archive::sha256_hex(b"BAD");
    let index = serde_json::json!({"schema_version":1,"entries":[
        {"id":"areused","name":"fixture.mp4","sha256":reused.sha256,"size":reused.size,"plugin_id":"gamer-video","kind":"imported","included":false},
        {"id":"bnew","name":"new.mp4","sha256":new_sha,"size":new.len(),"plugin_id":"gamer-video","kind":"imported","included":true},
        {"id":"zbroken","name":"bad.mp4","sha256":broken_sha,"size":3,"plugin_id":"gamer-video","kind":"imported","included":true}
    ]});
    let new_path = format!("media/files/{new_sha}");
    let broken_path = format!("media/files/{broken_sha}");
    let archive = craft_zip(vec![
        (
            "package.toml",
            import_rollback_manifest("rollback.media", "2.0.0"),
        ),
        ("media/index.json", serde_json::to_vec(&index).unwrap()),
        (&new_path, new.to_vec()),
        (&broken_path, b"bad".to_vec()),
    ]);
    let response = send(
        &t.app,
        req_bytes(
            "POST",
            "/api/packages/import?overwrite=true",
            None,
            &zip_headers(sid),
            archive,
        ),
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "{}",
        json_body(response).await
    );
    assert_eq!(
        t.packages.manifest("rollback.media").unwrap().version,
        "1.0.0"
    );
    assert_eq!(
        std::fs::read(t.dir.join("media/oldmedia/metadata.json")).unwrap(),
        old_metadata
    );
    assert_eq!(
        std::fs::read(t.dir.join("media/areused/metadata.json")).unwrap(),
        reused_metadata
    );
    assert!(!t.dir.join("media/bnew").exists());
    assert!(!t.dir.join("media/zbroken").exists());
}

#[tokio::test]
async fn package_import_preset_mid_batch_failure_restores_old_package_all_metadata_and_presets() {
    let t = build_app(
        "import-presets-rollback",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    assert_eq!(
        create_package(
            &t,
            &sid,
            serde_json::json!({"id":"rollback.presets","version":"1.0.0"})
        )
        .await
        .status(),
        StatusCode::CREATED
    );
    let dormant = t
        .dir
        .join("packages/rollback.presets/plugins/not-installed/package.toml");
    std::fs::create_dir_all(dormant.parent().unwrap()).unwrap();
    std::fs::write(&dormant, b"future = { preserve = 'old plugin data' }\n").unwrap();
    let original_manifest =
        std::fs::read(t.dir.join("packages/rollback.presets/package.toml")).unwrap();
    let (_, old_metadata) =
        seed_package_import_rollback_media(&t, "oldpresetmedia", "rollback.presets");
    let (reused, reused_metadata) =
        seed_package_import_rollback_media(&t, "presetreused", "other.package");
    let connection = rusqlite::Connection::open(t.dir.join("gamer.db")).unwrap();
    connection.execute("INSERT INTO task_presets(id,app_package,name,runner_id,entrypoint,payload_json,schedule_json,created_at) VALUES ('rollback.presets:aa','rollback.presets','old','gamer-yaml','old.yaml','{\"unknown\":{\"keep\":1}}','{\"provider_id\":\"cron\",\"config\":{\"expression\":\"0 8 * * *\"}}','2026-01-01T00:00:00+00:00')", []).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_import_preset BEFORE INSERT ON task_presets WHEN NEW.id='rollback.presets:zzfail' BEGIN SELECT RAISE(ABORT,'synthetic preset failure'); END;").unwrap();
    let old_row: Vec<String> = connection.query_row("SELECT id,app_package,name,runner_id,entrypoint,payload_json,schedule_json,created_at FROM task_presets WHERE id='rollback.presets:aa'", [], |row| (0..8).map(|index| row.get(index)).collect()).unwrap();
    let new = b"preset media";
    let new_sha = crate::package_archive::sha256_hex(new);
    let new_path = format!("media/files/{new_sha}");
    let index = serde_json::json!({"schema_version":1,"entries":[
        {"id":"presetreused","name":"fixture.mp4","sha256":reused.sha256,"size":reused.size,"plugin_id":"gamer-video","kind":"imported","included":false},
        {"id":"presetnew","name":"new.mp4","sha256":new_sha,"size":new.len(),"plugin_id":"gamer-video","kind":"imported","included":true}
    ]});
    let preset = |name: &str| {
        format!("name: {name}\nrunner_id: gamer-yaml\nentrypoint: new.yaml\npayload: {{new: true}}\nschedule:\n  kind: cron\n  value:\n    expression: '0 8 * * *'\n").into_bytes()
    };
    let archive = craft_zip(vec![
        (
            "package.toml",
            import_rollback_manifest("rollback.presets", "2.0.0"),
        ),
        ("media/index.json", serde_json::to_vec(&index).unwrap()),
        (&new_path, new.to_vec()),
        ("plugins/not-installed/presets/aa.yaml", preset("aa")),
        ("plugins/not-installed/presets/zz.yaml", preset("zzfail")),
    ]);
    let response = send(
        &t.app,
        req_bytes(
            "POST",
            "/api/packages/import?overwrite=true",
            None,
            &zip_headers(sid),
            archive,
        ),
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::INTERNAL_SERVER_ERROR,
        "{}",
        json_body(response).await
    );
    assert_eq!(
        std::fs::read(t.dir.join("packages/rollback.presets/package.toml")).unwrap(),
        original_manifest
    );
    assert_eq!(
        std::fs::read(&dormant).unwrap(),
        b"future = { preserve = 'old plugin data' }\n"
    );
    assert_eq!(
        std::fs::read(t.dir.join("media/oldpresetmedia/metadata.json")).unwrap(),
        old_metadata
    );
    assert_eq!(
        std::fs::read(t.dir.join("media/presetreused/metadata.json")).unwrap(),
        reused_metadata
    );
    assert!(!t.dir.join("media/presetnew").exists());
    let restored: Vec<String> = connection.query_row("SELECT id,app_package,name,runner_id,entrypoint,payload_json,schedule_json,created_at FROM task_presets WHERE id='rollback.presets:aa'", [], |row| (0..8).map(|index| row.get(index)).collect()).unwrap();
    assert_eq!(restored, old_row);
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM task_presets", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn package_activity_rejects_delete_and_overwrite_with_conflict_without_changing_content() {
    let t = build_app(
        "package-activity",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    let created = create_package(
        &t,
        &sid,
        serde_json::json!({"id":"active.game","version":"1.0.0"}),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let lease = t.packages.acquire_activity("active.game").unwrap();
    let deleted = send(
        &t.app,
        req(
            "DELETE",
            "/api/packages/active.game",
            None,
            &json_headers(sid.clone()),
            None,
        ),
    )
    .await;
    assert_eq!(deleted.status(), StatusCode::CONFLICT);
    assert!(json_body(deleted)
        .await
        .to_string()
        .contains("package_busy"));
    let archive = craft_zip(vec![(
        "package.toml",
        b"id = \"active.game\"\nversion = \"9.0.0\"\n[targets.android]\npackages = [\"*\"]\n"
            .to_vec(),
    )]);
    let imported = send(
        &t.app,
        req_bytes(
            "POST",
            "/api/packages/import?overwrite=true",
            None,
            &zip_headers(sid.clone()),
            archive,
        ),
    )
    .await;
    assert_eq!(imported.status(), StatusCode::CONFLICT);
    assert_eq!(t.packages.manifest("active.game").unwrap().version, "1.0.0");
    drop(lease);
    let deleted = send(
        &t.app,
        req(
            "DELETE",
            "/api/packages/active.game",
            None,
            &json_headers(sid),
            None,
        ),
    )
    .await;
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn package_sources_are_authenticated_persistent_settings_independent_of_packages() {
    let t = build_app(
        "package-sources",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    let response = post_json(
        &t,
        &sid,
        "/api/package-sources",
        serde_json::json!({"repository":"git@github.com:Owner/Repo.git","enabled":true}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let sources = json_body(response).await;
    let source = sources
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["repository"] == "owner/repo")
        .unwrap();
    let id = source["id"].as_str().unwrap();
    for (address, canonical) in [
        (
            "https://independent-mirror.example/cache/https://github.com/Owner/Repo",
            "https://independent-mirror.example/cache/https://github.com/owner/repo",
        ),
        (
            "https://static.example/configs",
            "https://static.example/configs/packages.json",
        ),
    ] {
        let added = post_json(
            &t,
            &sid,
            "/api/package-sources",
            serde_json::json!({"repository":address,"enabled":true}),
        )
        .await;
        assert_eq!(added.status(), StatusCode::OK);
        assert!(json_body(added)
            .await
            .as_array()
            .unwrap()
            .iter()
            .any(|source| source["repository"] == canonical));
    }
    for address in [
        "http://mirror.example/o/r",
        "https://user:password@mirror.example/o/r",
    ] {
        let bad = post_json(
            &t,
            &sid,
            "/api/package-sources",
            serde_json::json!({"repository":address,"enabled":true}),
        )
        .await;
        assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    }
    let removed = send(
        &t.app,
        req(
            "DELETE",
            &format!("/api/package-sources/{id}"),
            None,
            &[("cookie".into(), sid.clone())],
            None,
        ),
    )
    .await;
    assert_eq!(removed.status(), StatusCode::OK);
    let missing = send(
        &t.app,
        req(
            "GET",
            &format!("/api/package-sources/{id}/archives/demo/1.0.0?sha256=abc"),
            None,
            &[("cookie".into(), sid)],
            None,
        ),
    )
    .await;
    assert_eq!(missing.status(), StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn market_source_presets_require_auth_and_preserve_builtin_and_installed_content() {
    let t = build_app("market-presets", test_credential("admin123"), Default::default());
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    for path in ["/api/package-sources", "/api/extensions/market/sources"] {
        let anonymous = send(&t.app, req("GET",path,None,&[],None)).await;
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
        let response = post_json(&t,&sid,path,serde_json::json!({"repository":"http://127.0.0.1:13000/owner/custom.git","name":"我的来源","enabled":true})).await;
        assert_eq!(response.status(),StatusCode::OK);
        let presets = json_body(response).await;
        assert_eq!(presets.as_array().unwrap().iter().filter(|s|s["builtin"]==true).count(),2);
        let custom = presets.as_array().unwrap().iter().find(|s|s["name"]=="我的来源").unwrap();
        let id = custom["id"].as_str().unwrap();
        let edited = post_json(&t,&sid,path,serde_json::json!({"id":id,"repository":"https://other.example/catalog.json","name":"修改后的来源","enabled":true})).await;
        assert_eq!(edited.status(),StatusCode::OK);
        let rejected = post_json(&t,&sid,path,serde_json::json!({"repository":"http://8.8.8.8/owner/repo","name":"不安全地址","enabled":true})).await;
        assert_eq!(rejected.status(),StatusCode::BAD_REQUEST);
        let builtin = presets.as_array().unwrap().iter().find(|s|s["builtin"]==true).unwrap()["id"].as_str().unwrap();
        let rejected = send(&t.app,req("DELETE",&format!("{path}/{builtin}"),None,&[("cookie".into(),sid.clone())],None)).await;
        assert_eq!(rejected.status(),StatusCode::BAD_REQUEST);
        let removed = send(&t.app,req("DELETE",&format!("{path}/{id}"),None,&[("cookie".into(),sid.clone())],None)).await;
        assert_eq!(removed.status(),StatusCode::OK);
        let list = send(&t.app,req("GET",path,None,&[("cookie".into(),sid.clone())],None)).await;
        let list = json_body(list).await;
        assert_eq!(list.as_array().unwrap().len(),2);
    }
    assert!(t.dir.join("plugin-sources.json").is_file());
    assert!(t.dir.join("package-sources.json").is_file());
}

// Package REST 冒烟（plan §2-§15 / §23 验收链）：建包 → 写资源 → 读资源 →
// 列表 → 复制 → 导出 → 删除；导入路径覆盖「同 id 二次导入默认 409 +
// overwrite=true 原子替换」与包内预设发布（id = `<package-id>:<名>`）。

fn pkg_url(pkg: &str) -> String {
    format!("/api/packages/{pkg}")
}

fn resource_url(pkg: &str, plugin: &str, path: &str) -> String {
    format!("/api/packages/{pkg}/plugins/{plugin}/resources/{path}")
}

async fn create_package(t: &TestApp, sid: &str, mut body: serde_json::Value) -> HttpResponse<Body> {
    if body.get("targets").is_none() {
        body["targets"] = serde_json::json!({"android":{"packages":["*"]}});
    }
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
async fn template_replace_updates_region_color_and_references_without_losing_original_on_rejection()
{
    let t = build_app(
        "tplreplace",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    create_package(&t, &sid, serde_json::json!({"id":"replace.test"})).await;
    let old = "templates/reward#100_200_500_500.png";
    let new = "templates/reward#200_300_600_600#1.png";
    let url = resource_url("replace.test", "gamer-yaml", old).replace('#', "%23");
    let headers = vec![
        ("cookie".into(), sid.clone()),
        ("content-type".into(), "application/octet-stream".into()),
    ];
    let response = send(&t.app, req_bytes("PUT", &url, None, &headers, valid_png())).await;
    assert_eq!(response.status(), StatusCode::OK);
    let created = json_body(response).await;
    let response = get_json(&t, &sid, &url).await;
    assert_eq!(
        response.headers().get("etag").unwrap().to_str().unwrap(),
        format!("\"{}\"", created["version"].as_str().unwrap())
    );
    let root = t.dir.join("packages/replace.test/plugins/gamer-yaml");
    let original = std::fs::read(root.join(old)).unwrap();
    let script = "run:\n  - find: reward#100_200_500_500.png\n";
    let response = put_package_text(
        &t,
        &sid,
        "replace.test",
        "gamer-yaml",
        "automations/main.yaml",
        script,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let replacement_url = format!(
        "{url}?new_path={}",
        new.replace('/', "%2F").replace('#', "%23")
    );
    let mut conditional = headers.clone();
    conditional.push((
        "X-Expected-Version".into(),
        created["version"].as_str().unwrap().into(),
    ));
    let response = send(
        &t.app,
        req_bytes(
            "PUT",
            &replacement_url,
            None,
            &conditional,
            b"invalid png".to_vec(),
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(std::fs::read(root.join(old)).unwrap(), original);
    assert!(!root.join(new).exists());
    assert_eq!(
        std::fs::read_to_string(root.join("automations/main.yaml")).unwrap(),
        script
    );
    let mut stale = headers.clone();
    stale.push(("X-Expected-Version".into(), "stale".into()));
    let response = send(
        &t.app,
        req_bytes("PUT", &replacement_url, None, &stale, valid_png()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(std::fs::read(root.join(old)).unwrap(), original);
    assert!(!root.join(new).exists());
    let color = image::RgbImage::from_fn(8, 8, |x, y| {
        if (x + y) % 2 == 0 {
            image::Rgb([230, 20, 30])
        } else {
            image::Rgb([20, 40, 220])
        }
    });
    let mut png = Vec::new();
    color
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let response = send(
        &t.app,
        req_bytes("PUT", &replacement_url, None, &conditional, png),
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "{}",
        json_body(response).await
    );
    assert!(!root.join(old).exists());
    let saved = image::load_from_memory(&std::fs::read(root.join(new)).unwrap())
        .unwrap()
        .to_rgb8();
    assert_eq!(
        saved, color,
        "new color marker must be applied during validation"
    );
    let rewritten = std::fs::read_to_string(root.join("automations/main.yaml")).unwrap();
    assert!(!rewritten.contains("reward#100_200_500_500.png"));
    assert!(
        rewritten.contains("reward.png"),
        "references normalize to the stable short name: {rewritten}"
    );
}

#[tokio::test]
async fn package_crud_duplicate_export_delete_smoke_chain() {
    let t = build_app("pkgnsmoke", test_credential("admin123"), Default::default());
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
            "plugins": {"gamer-yaml": true, "gamer-keymap": false},
        }),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "{}",
        json_body(resp).await
    );
    let created = json_body(resp).await;
    assert_eq!(created["id"], "official.demo");
    assert_eq!(created["revision"], 1);
    assert_eq!(
        created["targets"]["android"]["packages"][0],
        "com.example.game"
    );
    // plugins 依赖按 plugin-id 字典序（BTreeMap）
    assert_eq!(created["plugins"][0]["id"], "gamer-keymap");
    assert_eq!(created["plugins"][0]["required"], false);
    assert_eq!(created["plugins"][1]["id"], "gamer-yaml");
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
    let script = "run:\n  - log: ok\n";
    let resp = send(
        &t.app,
        req(
            "PUT",
            &resource_url("official.demo", "gamer-yaml", "automations/daily.yaml"),
            None,
            &json_headers(sid.clone()),
            Some(serde_json::json!({"content": script}).to_string()),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "{}", json_body(resp).await);
    let written = json_body(resp).await;
    assert_eq!(written["path"], "automations/daily.yaml");
    assert_eq!(written["version"].as_str().unwrap().len(), 12);
    let v1 = written["version"].as_str().unwrap().to_string();

    let png = valid_png();
    let resp = send(
        &t.app,
        req_bytes(
            "PUT",
            &resource_url("official.demo", "gamer-yaml", "templates/icon.png"),
            None,
            &[
                (
                    axum::http::header::CONTENT_TYPE.to_string(),
                    "image/png".into(),
                ),
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
        &resource_url("official.demo", "gamer-yaml", "automations/daily.yaml"),
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
        &resource_url("official.demo", "gamer-yaml", "templates/icon.png"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let decoded = image::load_from_memory(&bytes).expect("字节资源读回必须是合法图片");
    assert_eq!((decoded.width(), decoded.height()), (8, 8));

    // 内容版本门禁：错版本 → 409
    let resp = send(
        &t.app,
        req(
            "PUT",
            &resource_url("official.demo", "gamer-yaml", "automations/daily.yaml"),
            None,
            &json_headers(sid.clone()),
            Some(serde_json::json!({"content": script, "expected_version": "stale"}).to_string()),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    // ---- 递归列表 ----
    let resp = get_json(
        &t,
        &sid,
        "/api/packages/official.demo/plugins/gamer-yaml/resources",
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
        vec![
            "automations/daily.yaml".to_string(),
            "templates/icon.png".to_string()
        ]
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
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "{}",
        json_body(resp).await
    );
    let copy = json_body(resp).await;
    assert_eq!(copy["id"], "user.demo");
    assert_eq!(copy["revision"], 1);
    assert_eq!(copy["name"], "演示包");
    let resp = get_json(
        &t,
        &sid,
        &resource_url("user.demo", "gamer-yaml", "automations/daily.yaml"),
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
    let t = build_app("pkgimport", test_credential("admin123"), Default::default());
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));

    // 造一个归档：先建包 + 写资源 + 导出
    let resp = create_package(&t, &sid, serde_json::json!({"id": "official.imp"})).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = send(
        &t.app,
        req(
            "PUT",
            &resource_url("official.imp", "gamer-yaml", "automations/first.yaml"),
            None,
            &json_headers(sid.clone()),
            Some(serde_json::json!({"content": "run:\n  - log: v1\n"}).to_string()),
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
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "{}",
        json_body(resp).await
    );
    assert_eq!(json_body(resp).await["id"], "official.imp");

    // 同 id 二次导入（内容不同：导入前改归档一个字节不可行——用第二份归档）
    // 先改包内容再导出 v2 归档
    let resp = send(
        &t.app,
        req(
            "PUT",
            &resource_url("official.imp", "gamer-yaml", "automations/first.yaml"),
            None,
            &json_headers(sid.clone()),
            Some(
                serde_json::json!({
                    "content": "run:\n  - log: v2\n",
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
    assert_eq!(
        resp.status(),
        StatusCode::CONFLICT,
        "{}",
        json_body(resp).await
    );
    let conflict = json_body(resp).await;
    assert_eq!(conflict["error"], "package_exists");
    assert_eq!(conflict["existing"]["id"], "official.imp");
    let resp = get_json(
        &t,
        &sid,
        &resource_url("official.imp", "gamer-yaml", "automations/first.yaml"),
    )
    .await;
    assert!(
        json_body(resp).await["content"]
            .as_str()
            .unwrap()
            .contains("v2"),
        "409 拒绝后内容必须保持原样（无半安装状态）"
    );

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
        &resource_url("official.imp", "gamer-yaml", "automations/first.yaml"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        json_body(resp).await["content"]
            .as_str()
            .unwrap()
            .contains("v1"),
        "overwrite 后内容必须替换为归档版本"
    );

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
        .join("packages/official.preset/plugins/gamer-yaml/presets");
    std::fs::create_dir_all(&preset_dir).unwrap();
    std::fs::write(
        preset_dir.join("daily.yaml"),
        "name: \u{6bcf}\u{65e5}\u{9886}\u{53d6}\nrunner_id: gamer-yaml\nentrypoint: run\npayload: {}\nschedule:\n  kind: cron\n  value:\n    expression: \"0 8 * * *\"\n",
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
    assert_eq!(daily["runner"]["runner_id"], "gamer-yaml");
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
    let t = build_app("tplrename", test_credential("admin123"), Default::default());
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    create_package(
        &t,
        &sid,
        serde_json::json!({"id": "official.tpl", "plugins": {"gamer-yaml": true}}),
    )
    .await;

    // ---- 1. 上传模板：字节 PUT（8x8 灰度 PNG）----
    let png = valid_png();
    let resp = send(
        &t.app,
        req_bytes(
            "PUT",
            &resource_url("official.tpl", "gamer-yaml", "templates/reward.png"),
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

    // ---- 2. 保存引用该模板的 V1 脚本与函数库 ----
    let script =
        "run:\n  - find:\n      template: reward.png\n      region: [0, 0, 1, 1]\n    as: hit\n";
    let resp = put_package_text(
        &t,
        &sid,
        "official.tpl",
        "gamer-yaml",
        "automations/daily.yaml",
        script,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "{}", json_body(resp).await);

    // ---- 3. 重命名模板：REST → 钩子改写引用 → 文件原子移动 ----
    let resp = post_json(
        &t,
        &sid,
        "/api/packages/official.tpl/plugins/gamer-yaml/rename",
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
        &resource_url("official.tpl", "gamer-yaml", "templates/bonus.png"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = send(
        &t.app,
        req(
            "GET",
            &resource_url("official.tpl", "gamer-yaml", "templates/reward.png"),
            None,
            &json_headers(sid.clone()),
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // ---- 4. 脚本引用已自动改写（AST 级：短名规则与前端一致）----
    let resp = get_json(
        &t,
        &sid,
        &resource_url("official.tpl", "gamer-yaml", "automations/daily.yaml"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let content = json_body(resp).await["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        content.contains("template: bonus"),
        "引用已改写为短名: {content}"
    );
    assert!(!content.contains("reward"), "旧引用不残留: {content}");

    // 错误语义：同名重命名 400（名称未变化）；目标已存在 409
    let resp = post_json(
        &t,
        &sid,
        "/api/packages/official.tpl/plugins/gamer-yaml/rename",
        serde_json::json!({"path": "templates/bonus.png", "new_path": "templates/bonus.png"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let resp = send(
        &t.app,
        req_bytes(
            "PUT",
            &resource_url("official.tpl", "gamer-yaml", "templates/other.png"),
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
        "/api/packages/official.tpl/plugins/gamer-yaml/rename",
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
        .join("packages/official.tpl/plugins/gamer-yaml/templates/bonus.png");
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
        serde_json::json!({"id": "official.vision", "plugins": {"gamer-yaml": true}}),
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
            "plugin": "gamer-yaml",
            "name": "ghost.png",
        }),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "{}",
        json_body(resp).await
    );
}

/// A2 字节钩子（REST PUT 面）：templates/ 上传彩色 PNG → 落盘 8-bit 灰度
/// 归一化（读回验证）；垃圾字节 → 400 结构化诊断；非模板路径字节透传。
#[tokio::test]
async fn template_upload_binary_hook_normalizes_and_rejects_garbage() {
    let t = build_app("tplhook", test_credential("admin123"), Default::default());
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    let resp = create_package(
        &t,
        &sid,
        serde_json::json!({"id": "official.tpl", "plugins": {"gamer-yaml": true}}),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "{}",
        json_body(resp).await
    );

    // 彩色（RGBA 非）灰度 PNG 夹具：R/G/B 通道取值不同，归一化后必然变色
    let mut color = image::RgbaImage::new(4, 3);
    for (x, _y, pixel) in color.enumerate_pixels_mut() {
        *pixel = image::Rgba([if x % 2 == 0 { 10 } else { 240 }, 90, 160, 255]);
    }
    let mut color_bytes = Vec::new();
    image::DynamicImage::ImageRgba8(color)
        .write_to(
            &mut std::io::Cursor::new(&mut color_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();

    let resp = send(
        &t.app,
        req_bytes(
            "PUT",
            &resource_url("official.tpl", "gamer-yaml", "templates/icon.png"),
            None,
            &[
                (
                    axum::http::header::CONTENT_TYPE.to_string(),
                    "image/png".into(),
                ),
                (axum::http::header::COOKIE.to_string(), sid.clone()),
            ],
            color_bytes.clone(),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "{}", json_body(resp).await);
    let written = json_body(resp).await;
    assert_eq!(written["path"], "templates/icon.png");
    let stored_version = written["version"].as_str().unwrap().to_string();

    // 读回：必须是 8-bit 灰度 PNG（归一化结果），且版本 = 归一化内容哈希
    let resp = get_json(
        &t,
        &sid,
        &resource_url("official.tpl", "gamer-yaml", "templates/icon.png"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    let decoded = image::load_from_memory(&bytes).expect("落盘必须是合法 PNG");
    assert_eq!(
        decoded.color(),
        image::ColorType::L8,
        "templates/ 上传必须归一化为 8-bit 灰度"
    );
    assert_ne!(
        bytes.as_ref(),
        color_bytes.as_slice(),
        "彩色输入必须被重编码，不得原样落盘"
    );

    // 二次 PUT 版本门禁基于归一化内容：带归一化内容的 expected 版本可通过
    let resp = send(
        &t.app,
        req_bytes(
            "PUT",
            &resource_url("official.tpl", "gamer-yaml", "templates/icon.png"),
            None,
            &[
                (
                    axum::http::header::CONTENT_TYPE.to_string(),
                    "image/png".into(),
                ),
                (axum::http::header::COOKIE.to_string(), sid.clone()),
                ("x-expected-version".to_string(), stored_version.clone()),
            ],
            color_bytes,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "{}", json_body(resp).await);

    // 垃圾字节 → 400 结构化诊断（与文本保存校验同形状）
    let resp = send(
        &t.app,
        req_bytes(
            "PUT",
            &resource_url("official.tpl", "gamer-yaml", "templates/broken.png"),
            None,
            &[
                (
                    axum::http::header::CONTENT_TYPE.to_string(),
                    "application/octet-stream".into(),
                ),
                (axum::http::header::COOKIE.to_string(), sid.clone()),
            ],
            b"definitely not an image".to_vec(),
        ),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "{}",
        json_body(resp).await
    );
    let err = json_body(resp).await;
    assert_eq!(err["error"], "invalid_content");
    assert_eq!(
        err["diagnostics"][0]["code"], "template.png.invalid",
        "{err}"
    );
    assert_eq!(err["diagnostics"][0]["path"], "templates/broken.png");

    // 非模板路径字节透传（裸字节区不解释内容）。夹具必须非 UTF-8：
    // 合法 UTF-8 的小文件走文本读取面（GET 返回 JSON entry），测不到字节流
    let payload: Vec<u8> = vec![
        0xFF, 0xFE, b'n', b'o', b't', b' ', b'a', b' ', b'p', b'n', b'g',
    ];
    let resp = send(
        &t.app,
        req_bytes(
            "PUT",
            &resource_url("official.tpl", "gamer-yaml", "assets/blob.bin"),
            None,
            &[
                (
                    axum::http::header::CONTENT_TYPE.to_string(),
                    "application/octet-stream".into(),
                ),
                (axum::http::header::COOKIE.to_string(), sid.clone()),
            ],
            payload.clone(),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "{}", json_body(resp).await);
    let resp = get_json(
        &t,
        &sid,
        &resource_url("official.tpl", "gamer-yaml", "assets/blob.bin"),
    )
    .await;
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(bytes.as_ref(), payload.as_slice());
}

// ---------- Phase 8 §11.1：Package 导入的媒体分发（media/ 白名单受控目录） ----------

/// 含媒体归档导入：素材先校验（大小/哈希/逻辑 id）再原子恢复（响应 `media`
/// 摘要），media/index.json 随包落盘保留引用；不含字节的引用条目按 sha256
/// 自动重挂；删除包后引用解除（素材删除保护随之失效，可回收）。
#[tokio::test]
async fn package_import_with_media_restores_bytes_and_releases_refs_on_delete() {
    let t = build_app("pkgmedia", test_credential("admin123"), Default::default());
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));

    let clip = b"clip-bytes";
    let sha = crate::package_archive::sha256_hex(clip);
    let file_entry = format!("media/files/{sha}");
    let index_json = |included: bool| {
        serde_json::json!({
            "schema_version": 1,
            "entries": [{
                "id": "clipmedia01",
                "name": "clip.mp4",
                "sha256": sha,
                "size": clip.len(),
                "plugin_id": "gamer-video",
                "kind": "project",
                "included": included,
            }],
        })
        .to_string()
        .into_bytes()
    };
    let package_toml = br#"id = "official.media.demo"
version = "1.0.0"
[targets.android]
packages = ["*"]
"#;

    // —— 含素材导入：201，media 摘要 imported=1 ——
    let archive = craft_zip(vec![
        ("package.toml", package_toml.to_vec()),
        ("media/index.json", index_json(true)),
        (file_entry.as_str(), clip.to_vec()),
    ]);
    let resp = send(
        &t.app,
        req_bytes(
            "POST",
            "/api/packages/import",
            None,
            &[
                (
                    axum::http::header::CONTENT_TYPE.to_string(),
                    "application/zip".into(),
                ),
                (axum::http::header::COOKIE.to_string(), sid.clone()),
            ],
            archive,
        ),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "{}",
        json_body(resp).await
    );
    let created = json_body(resp).await;
    assert_eq!(created["id"], "official.media.demo");
    assert_eq!(created["media"]["imported"], 1);
    assert_eq!(created["media"]["total"], 1);

    // 索引随包落盘（引用与缺失状态保留在包内）
    assert!(t
        .dir
        .join("packages/official.media.demo/media/index.json")
        .is_file());

    // 媒体库可查：逻辑 id 保留 + 引用登记 + probe 元数据缺失时按未知兜底
    let resp = get_json(&t, &sid, "/api/media/clipmedia01").await;
    assert_eq!(resp.status(), StatusCode::OK, "{}", json_body(resp).await);
    let meta = json_body(resp).await;
    assert_eq!(meta["sha256"], sha.as_str());
    assert_eq!(meta["size"], clip.len() as u64);
    assert_eq!(meta["refs"][0]["package_id"], "official.media.demo");
    assert_eq!(meta["refs"][0]["plugin_id"], "gamer-video");

    // —— 覆盖导入「仅引用」版（included=false、无字节）→ 引用重挂 ——
    let refs_only = craft_zip(vec![
        ("package.toml", package_toml.to_vec()),
        ("media/index.json", index_json(false)),
    ]);
    let resp = send(
        &t.app,
        req_bytes(
            "POST",
            "/api/packages/import?overwrite=true",
            None,
            &[
                (
                    axum::http::header::CONTENT_TYPE.to_string(),
                    "application/zip".into(),
                ),
                (axum::http::header::COOKIE.to_string(), sid.clone()),
            ],
            refs_only,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "{}", json_body(resp).await);
    let overwritten = json_body(resp).await;
    assert_eq!(
        overwritten["media"]["reattached"], 1,
        "同 id 同 sha 自动重挂"
    );

    // —— included 标记与字节存在性不一致 → 整体拒绝且不落半成品 ——
    let inconsistent = craft_zip(vec![
        ("package.toml", package_toml.to_vec()),
        ("media/index.json", index_json(true)), // 声明含字节但归档没带
    ]);
    let resp = send(
        &t.app,
        req_bytes(
            "POST",
            "/api/packages/import?overwrite=true",
            None,
            &[
                (
                    axum::http::header::CONTENT_TYPE.to_string(),
                    "application/zip".into(),
                ),
                (axum::http::header::COOKIE.to_string(), sid.clone()),
            ],
            inconsistent,
        ),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "{}",
        json_body(resp).await
    );

    // —— 删除包 → 引用解除（best effort 钩子），素材本身保留 ——
    let resp = send(
        &t.app,
        req(
            "DELETE",
            &pkg_url("official.media.demo"),
            None,
            &json_headers(sid.clone()),
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = get_json(&t, &sid, "/api/media/clipmedia01").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let meta = json_body(resp).await;
    // refs 为空时序列化省略字段（skip_serializing_if）
    let refs_left = meta
        .get("refs")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    assert_eq!(refs_left, 0, "包删除后引用必须解除");
}
