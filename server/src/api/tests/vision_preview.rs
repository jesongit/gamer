use super::*;
use base64::Engine as _;

#[tokio::test]
async fn browser_preview_matches_uploaded_pixels_without_device_or_media() {
    let t = build_app(
        "vision-preview",
        test_credential("admin123"),
        Default::default(),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    let created = post_json(
        &t,
        &sid,
        "/api/packages",
        serde_json::json!({"id":"preview", "name":"preview"}),
    )
    .await;
    assert!(created.status().is_success());
    let screen = image::RgbImage::from_fn(96, 64, |x, y| {
        image::Rgb([
            ((x * 71 + y * 29) % 256) as u8,
            ((x * y * 13) % 256) as u8,
            ((x * 17 + y * 41) % 256) as u8,
        ])
    });
    let template = image::imageops::crop_imm(&screen, 30, 20, 16, 16).to_image();
    let png = |image: image::RgbImage| {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    };
    let path = t
        .dir
        .join("packages/preview/plugins/vision-preview/templates");
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(path.join("target.png"), png(template)).unwrap();
    let mut body = serde_json::json!({
        "pkg":"preview", "plugin":"vision-preview", "name":"target.png", "threshold":0.99,
        "region":[20,10,40,40],
        "image_png":base64::engine::general_purpose::STANDARD.encode(png(screen)),
    });
    let response = post_json(&t, &sid, "/api/capabilities/vision/test", body.clone()).await;
    assert_eq!(response.status(), StatusCode::OK);
    let result = json_body(response).await;
    assert_eq!(result["hit"], true, "{result}");
    assert_eq!(
        (result["x"].as_u64(), result["y"].as_u64()),
        (Some(30), Some(20))
    );
    assert!(
        result["frame"].is_null(),
        "浏览器像素不能冒充媒体精确帧身份"
    );
    body["image_png"] = serde_json::json!("aW52YWxpZA==");
    assert_eq!(
        post_json(&t, &sid, "/api/capabilities/vision/test", body.clone())
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    body["device_id"] = serde_json::json!("must-not-connect");
    assert_eq!(
        post_json(&t, &sid, "/api/capabilities/vision/test", body)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
}
