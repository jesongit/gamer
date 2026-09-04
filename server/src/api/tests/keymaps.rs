use super::*;

const KEYMAP_V1: &str = "version: 1\nname: 战斗方案\nbindings:\n  - key: Space\n    action:\n      type: tap\n      at: [0.72, 0.86]\n  - key: KeyE\n    action:\n      type: swipe\n      from: [0.4, 0.8]\n      to: [0.6, 0.8]\n      duration_ms: 300\n";

const KEYMAP_V2: &str = "version: 1\nname: 探索方案\nbindings:\n  - key: KeyW\n    action:\n      type: hold\n      at: [0.5, 0.5]\n";

#[tokio::test]
async fn keymaps_crud_is_partitioned_and_version_guarded() {
    let t = build_app(
        "keymapcrud",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));

    let resp = post_json(
        &t,
        &sid,
        "/api/keymaps",
        serde_json::json!({
            "pkg": "com.test.app",
            "name": "combat",
            "content": KEYMAP_V1,
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "{:?}", json_body(resp).await);
    let created = json_body(resp).await;
    assert_eq!(created["id"], "com.test.app/combat.yaml");
    assert_eq!(created["package"], "com.test.app");
    assert_eq!(created["name"], "战斗方案");
    assert_eq!(created["binding_count"], 2);
    let v1 = created["version"].as_str().unwrap().to_string();
    assert_eq!(v1.len(), 12);
    assert!(t.dir.join("com.test.app/keymaps/combat.yaml").is_file());

    // POST 只创建，不覆盖同名方案。
    let resp = post_json(
        &t,
        &sid,
        "/api/keymaps",
        serde_json::json!({
            "pkg": "com.test.app",
            "name": "combat.yaml",
            "content": KEYMAP_V1,
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    let resp = get_json(&t, &sid, "/api/keymaps?pkg=com.test.app").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let list = json_body(resp).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["id"], "com.test.app/combat.yaml");
    assert_eq!(list[0]["name"], "战斗方案");
    assert_eq!(list[0]["binding_count"], 2);
    assert_eq!(list[0]["version"], v1.as_str());

    // 详情返回规范化 YAML 和结构化模型。
    let resp = get_json(&t, &sid, "/api/keymaps/com.test.app%2Fcombat.yaml").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let detail = json_body(resp).await;
    assert!(detail["content"].as_str().unwrap().contains("version: 1"));
    assert!(detail["content"].as_str().unwrap().contains("type: tap"));
    assert_eq!(detail["keymap"]["bindings"].as_array().unwrap().len(), 2);

    // PUT 默认需要 expected_version；过期版本必须 409。
    let resp = send(
        &t.app,
        req(
            "PUT",
            "/api/keymaps/com.test.app%2Fcombat.yaml",
            None,
            &json_headers(sid.clone()),
            Some(serde_json::json!({"content": KEYMAP_V2}).to_string()),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(resp).await["code"], "version_required");

    let resp = send(
        &t.app,
        req(
            "PUT",
            "/api/keymaps/com.test.app%2Fcombat.yaml",
            None,
            &json_headers(sid.clone()),
            Some(
                serde_json::json!({
                    "content": KEYMAP_V2,
                    "expected_version": "stale",
                })
                .to_string(),
            ),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(resp).await["code"], "version_conflict");

    let resp = send(
        &t.app,
        req(
            "PUT",
            "/api/keymaps/com.test.app%2Fcombat.yaml",
            None,
            &json_headers(sid.clone()),
            Some(
                serde_json::json!({
                    "content": KEYMAP_V2,
                    "expected_version": v1,
                })
                .to_string(),
            ),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let v2 = json_body(resp).await["version"].as_str().unwrap().to_string();
    assert_ne!(v2, v1);

    // force 明确允许跳过版本门禁，并支持同分区重命名。
    let resp = send(
        &t.app,
        req(
            "PUT",
            "/api/keymaps/com.test.app%2Fcombat.yaml",
            None,
            &json_headers(sid.clone()),
            Some(
                serde_json::json!({
                    "name": "explore",
                    "content": KEYMAP_V1,
                    "force": true,
                })
                .to_string(),
            ),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["id"], "com.test.app/explore.yaml");
    assert!(!t.dir.join("com.test.app/keymaps/combat.yaml").exists());

    // 另一应用分区看不到该方案。
    let resp = get_json(&t, &sid, "/api/keymaps?pkg=com.other.app").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(json_body(resp).await.as_array().unwrap().is_empty());

    let resp = send(
        &t.app,
        req(
            "DELETE",
            "/api/keymaps/com.test.app%2Fexplore.yaml",
            None,
            &json_headers(sid.clone()),
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = get_json(&t, &sid, "/api/keymaps/com.test.app%2Fexplore.yaml").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn keymaps_reject_invalid_yaml_fields_coordinates_and_duplicates() {
    let t = build_app(
        "keymapvalidation",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    let cases = [
        (
            "version: 1\nname: bad\nextra: true\nbindings: []\n",
            "keymap.top_level.unknown_key",
        ),
        (
            "version: 1\nname: bad\nbindings:\n  - key: Space\n    action:\n      type: tap\n      at: [0.5, 1.1]\n      extra: true\n",
            "keymap.coordinate.out_of_range",
        ),
        (
            "version: 1\nname: bad\nbindings:\n  - key: Space\n    action:\n      type: tap\n      at: [0.5, 0.5]\n  - key: Space\n    action:\n      type: tap\n      at: [0.2, 0.2]\n",
            "keymap.binding.duplicate_key",
        ),
        (
            "version: 1\nname: bad\nbindings:\n  - key: Space\n    action:\n      type: swipe\n      from: [0, 0]\n      to: [1, 1]\n      duration_ms: 0\n",
            "keymap.duration.invalid",
        ),
        (
            "version: 1\nname: bad\nbindings:\n  - key: Space\n    action:\n      type: tap\n      at: [0.5, 0.5]\n      type2: tap\n",
            "keymap.action.unknown_key",
        ),
        (
            "version: 1\nname: bad\nbindings:\n  - key: KeyW\n    action:\n      type: hold\n      at: [0.5, 0.5]\n      from: [0.1, 0.2]\n      to: [0.8, 0.9]\n",
            "keymap.action.unknown_key",
        ),
        (
            "version: 1\nname: bad\nbindings:\n  - key: KeyW\n    action:\n      type: hold\n      at: [0.5, 0.5]\n      pointer_id: 1\n",
            "keymap.action.unknown_key",
        ),
        ("version: 1\nname: [bad\nbindings: []\n", "keymap.yaml.syntax"),
    ];
    for (content, code) in cases {
        let resp = post_json(
            &t,
            &sid,
            "/api/keymaps",
            serde_json::json!({"pkg": "com.test.app", "name": "bad", "content": content}),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{content}");
        let body = json_body(resp).await;
        assert_eq!(body["error"], "invalid_yaml", "{content}");
        assert!(body["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["code"] == code), "{body}");
    }
    assert!(!t.dir.join("com.test.app/keymaps/bad.yaml").exists());
}
