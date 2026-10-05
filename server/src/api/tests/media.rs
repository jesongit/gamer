use super::*;

#[tokio::test]
async fn media_rename_requires_auth_and_preserves_bytes_and_references() {
    let t = build_app(
        "media-rename",
        test_credential("admin123"),
        Default::default(),
    );
    let media_dir = t.dir.join("media/renameclip");
    std::fs::create_dir_all(&media_dir).unwrap();
    let before = serde_json::json!({
        "id": "renameclip", "name": "旧视频.mp4", "sha256": "ab", "size": 1,
        "duration_us": 1_000_000, "container": "mp4", "codec": "h264",
        "width": 320, "height": 240, "rotation": 0, "source": "import", "state": "ready",
        "created_at": "2026-10-05T00:00:00Z",
        "refs": [{"package_id":"pkg", "plugin_id":"gamer-video", "kind":"project"}]
    });
    std::fs::write(
        media_dir.join("metadata.json"),
        serde_json::to_vec(&before).unwrap(),
    )
    .unwrap();
    std::fs::write(media_dir.join("original.mp4"), b"x").unwrap();

    let response = send(
        &t.app,
        req(
            "PATCH",
            "/api/media/renameclip",
            None,
            &[(header::CONTENT_TYPE.to_string(), "application/json".into())],
            Some(r#"{"name":"新视频.mp4"}"#.into()),
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let cookie = first_cookie_pair(&cookie_of(&login(&t.app).await));
    let headers = [
        (header::COOKIE.to_string(), cookie),
        (header::CONTENT_TYPE.to_string(), "application/json".into()),
    ];
    let response = send(
        &t.app,
        req(
            "PATCH",
            "/api/media/renameclip",
            None,
            &headers,
            Some(r#"{"name":"  新视频.mp4  "}"#.into()),
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut expected = before.clone();
    expected["name"] = "新视频.mp4".into();
    assert_eq!(json_body(response).await, expected);
    let persisted: serde_json::Value =
        serde_json::from_slice(&std::fs::read(media_dir.join("metadata.json")).unwrap()).unwrap();
    assert_eq!(persisted, expected);
    assert_eq!(std::fs::read(media_dir.join("original.mp4")).unwrap(), b"x");

    for name in ["", "  ", "invalid\nname"] {
        let response = send(
            &t.app,
            req(
                "PATCH",
                "/api/media/renameclip",
                None,
                &headers,
                Some(serde_json::json!({"name": name}).to_string()),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    let response = send(
        &t.app,
        req(
            "PATCH",
            "/api/media/missingclip",
            None,
            &headers,
            Some(r#"{"name":"新名称"}"#.into()),
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(json_body(response).await["error"], "media_not_found");
}
