use super::*;

#[tokio::test]
async fn browser_targets_and_scheduled_context_roundtrip_without_android_package() {
    let t = build_app(
        "browser-context",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    let target = serde_json::json!({"id":"browser-a","name":"Account A","profile_id":"a","url":"https://example.com","width":1280,"height":720});
    assert_eq!(
        post_json(&t, "", "/api/browser-targets", target.clone())
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post_json(&t, &sid, "/api/browser-targets", target.clone())
            .await
            .status(),
        StatusCode::OK
    );
    let list = json_body(get_json(&t, &sid, "/api/devices").await).await;
    assert_eq!(list[0]["kind"], "browser");
    assert_eq!(list[0]["id"], "browser-a");
    let mut duplicate = target.clone();
    duplicate["id"] = serde_json::json!("browser-b");
    assert_eq!(
        post_json(&t, &sid, "/api/browser-targets", duplicate)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    let mut bad = target;
    bad["profile_id"] = serde_json::json!("../escape");
    assert_eq!(
        post_json(&t, &sid, "/api/browser-targets", bad)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    let task = serde_json::json!({"name":"Browser timer","app":{"device_id":"browser-a","content_package":"default"},"runner":{"runner_id":"gamer-yaml","entrypoint":"default/daily.yaml","payload":{}},"schedule":{"provider_id":"cron","config":{"expression":"0 8 * * *"}}});
    let result = post_json(&t, &sid, "/api/tasks", task.clone()).await;
    assert_eq!(result.status(), StatusCode::CREATED);
    let saved = json_body(result).await;
    let id = saved["id"].as_str().unwrap();
    let fetched = json_body(get_json(&t, &sid, &format!("/api/tasks/{id}")).await).await;
    assert_eq!(fetched["app"]["device_id"], "browser-a");
    assert!(fetched["app"]["android_package"].is_null());
    let mut invalid = task.clone();
    invalid["app"]["android_package"] = serde_json::json!("com.game");
    assert_eq!(
        post_json(&t, &sid, "/api/tasks", invalid).await.status(),
        StatusCode::BAD_REQUEST
    );
    let mut android = task;
    android["app"]["device_id"] = serde_json::json!("android-device");
    assert_eq!(
        post_json(&t, &sid, "/api/tasks", android).await.status(),
        StatusCode::BAD_REQUEST
    );
    let context = crate::targets::app_context(&t.devices, "browser-a", None).unwrap();
    assert!(context.android_package.is_none());
}
