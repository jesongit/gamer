use super::*;

// Package API（plan §2-§15）：插件资源经
// `/api/packages/:pkg/plugins/:plugin/resources/*path` 存取；插件数据隔离在
// `plugins/<plugin>/` 前缀内。gamer.keymap 的方案注记（显示名 name /
// binding_count / valid / diagnostics）由扩展注册的 ResourceHandler 提供。

const KEYMAP_V1: &str = "version: 1\nname: 战斗方案\nbindings:\n  - key: Space\n    action:\n      type: tap\n      at: [0.72, 0.86]\n  - key: KeyE\n    action:\n      type: swipe\n      from: [0.4, 0.8]\n      to: [0.6, 0.8]\n      duration_ms: 300\n";

const KEYMAP_V2: &str = "version: 1\nname: 探索方案\nbindings:\n  - key: KeyW\n    action:\n      type: hold\n      at: [0.5, 0.5]\n";

fn pkg_base(pkg: &str) -> String {
    format!("/api/packages/{pkg}/plugins/gamer.keymap/resources")
}

#[tokio::test]
async fn keymaps_crud_is_plugin_scoped_and_version_guarded() {
    let t = build_app(
        "keymapcrud",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));

    // 建包（数据一级作用域 = Package ID）
    let resp = post_json(
        &t,
        &sid,
        "/api/packages",
        serde_json::json!({"id": "com.test.app"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED, "{}", json_body(resp).await);
    // 另一个包用于隔离断言
    let resp = post_json(
        &t,
        &sid,
        "/api/packages",
        serde_json::json!({"id": "com.other.app"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);

    // PUT 创建方案（无 expected_version + 文件不存在 = 创建）
    let resp = send(
        &t.app,
        req(
            "PUT",
            &format!("{pkg_base}/keymaps/combat.yaml", pkg_base = pkg_base("com.test.app")),
            None,
            &json_headers(sid.clone()),
            Some(serde_json::json!({"content": KEYMAP_V1}).to_string()),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "{}", json_body(resp).await);
    let created = json_body(resp).await;
    assert_eq!(created["package"], "com.test.app");
    assert_eq!(created["plugin"], "gamer.keymap");
    assert_eq!(created["path"], "keymaps/combat.yaml");
    assert_eq!(created["name"], "战斗方案");
    assert_eq!(created["binding_count"], 2);
    assert_eq!(created["text"], true);
    let v1 = created["version"].as_str().unwrap().to_string();
    assert_eq!(v1.len(), 12);
    assert!(t
        .dir
        .join("packages/com.test.app/plugins/gamer.keymap/keymaps/combat.yaml")
        .is_file());

    // 已存在文件：无 expected_version → 409 version_required
    let resp = send(
        &t.app,
        req(
            "PUT",
            &format!("{}/keymaps/combat.yaml", pkg_base("com.test.app")),
            None,
            &json_headers(sid.clone()),
            Some(serde_json::json!({"content": KEYMAP_V1}).to_string()),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert!(json_body(resp).await["error"]
        .as_str()
        .unwrap()
        .contains("version_required"));

    // 过期版本 → 409 version_conflict
    let resp = send(
        &t.app,
        req(
            "PUT",
            &format!("{}/keymaps/combat.yaml", pkg_base("com.test.app")),
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
    assert!(json_body(resp).await["error"]
        .as_str()
        .unwrap()
        .contains("version_conflict"));

    // 正确版本 → 更新成功，版本刷新
    let resp = send(
        &t.app,
        req(
            "PUT",
            &format!("{}/keymaps/combat.yaml", pkg_base("com.test.app")),
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

    // force 明确允许跳过版本门禁（内容回到 KEYMAP_V1）
    let resp = send(
        &t.app,
        req(
            "PUT",
            &format!("{}/keymaps/combat.yaml", pkg_base("com.test.app")),
            None,
            &json_headers(sid.clone()),
            Some(
                serde_json::json!({
                    "content": KEYMAP_V1,
                    "force": true,
                })
                .to_string(),
            ),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // 递归列表带注记（?prefix= 限定子目录）
    let resp = get_json(
        &t,
        &sid,
        &format!("{}?prefix=keymaps", pkg_base("com.test.app")),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let list = json_body(resp).await;
    assert_eq!(list["resources"].as_array().unwrap().len(), 1);
    assert_eq!(list["resources"][0]["name"], "战斗方案");
    assert_eq!(list["resources"][0]["binding_count"], 2);
    assert_eq!(list["resources"][0]["version"], v1.as_str(), "force 写回 KEYMAP_V1 后版本回到 v1");

    // 详情返回内容与注记
    let resp = get_json(
        &t,
        &sid,
        &format!("{}/keymaps/combat.yaml", pkg_base("com.test.app")),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let detail = json_body(resp).await;
    assert!(detail["content"].as_str().unwrap().contains("type: tap"));
    assert_eq!(detail["binding_count"], 2);

    // 插件数据隔离：其他包看不到该方案（dormant 插件目录 → 空列表）
    let resp = get_json(
        &t,
        &sid,
        &format!("{}?prefix=keymaps", pkg_base("com.other.app")),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(json_body(resp).await["resources"].as_array().unwrap().is_empty());

    // 删除
    let resp = send(
        &t.app,
        req(
            "DELETE",
            &format!("{}/keymaps/combat.yaml", pkg_base("com.test.app")),
            None,
            &json_headers(sid.clone()),
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = get_json(
        &t,
        &sid,
        &format!("{}/keymaps/combat.yaml", pkg_base("com.test.app")),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// 存储层直接探针：PackageStore.read_text 命中 `plugins/gamer.keymap/keymaps/`
/// 下的方案文件（路径 = 包数据根的新布局）。
#[tokio::test]
async fn keymap_store_read_text_probe() {
    let t = build_app(
        "keymapprobe",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    let resp = post_json(
        &t,
        &sid,
        "/api/packages",
        serde_json::json!({"id": "com.test.app"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = send(
        &t.app,
        req(
            "PUT",
            &format!("{}/keymaps/combat.yaml", pkg_base("com.test.app")),
            None,
            &json_headers(sid.clone()),
            Some(serde_json::json!({"content": KEYMAP_V1}).to_string()),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let dir = t.dir.clone();
    let store = crate::resources::PackageStore::open(&crate::config::Config {
        data_dir: dir,
        ..Default::default()
    })
    .unwrap();
    let hit = store
        .read_text("com.test.app", "gamer.keymap", "keymaps/combat.yaml")
        .unwrap();
    assert!(hit.is_some());
}

#[tokio::test]
async fn keymaps_reject_invalid_yaml_fields_coordinates_and_duplicates() {
    let t = build_app(
        "keymapvalidation",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    let resp = post_json(
        &t,
        &sid,
        "/api/packages",
        serde_json::json!({"id": "com.test.app"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
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
        let resp = send(
            &t.app,
            req(
                "PUT",
                &format!("{}/keymaps/bad.yaml", pkg_base("com.test.app")),
                None,
                &json_headers(sid.clone()),
                Some(serde_json::json!({"content": content}).to_string()),
            ),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{content}");
        let body = json_body(resp).await;
        assert_eq!(body["error"], "invalid_content", "{content}");
        assert!(body["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["code"] == code), "{body}");
    }
    assert!(!t
        .dir
        .join("packages/com.test.app/plugins/gamer.keymap/keymaps/bad.yaml")
        .exists());
}
