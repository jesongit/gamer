use super::*;

const TEST_PASSWORD: &str = "admin123";

#[tokio::test]
async fn force_reconnect_requires_auth_and_existing_device() {
    let t = build_app(
        "reconnect-auth",
        test_credential(TEST_PASSWORD),
        Default::default(),
    );
    let response = send(
        &t.app,
        req(
            "POST",
            "/api/devices/missing/force-reconnect",
            None,
            &[],
            None,
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    let response = post_json(
        &t,
        &sid,
        "/api/devices/missing/force-reconnect",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn force_reconnect_rejects_activity_on_other_device_before_adb_reset() {
    let t = build_app(
        "reconnect-busy",
        test_credential(TEST_PASSWORD),
        Default::default(),
    );
    seed_device(&t, "target", "").await;
    seed_device(&t, "other", "").await;
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    let _lease = t
        .devices
        .activity()
        .acquire("other", crate::core::ActivityKind::Run);
    let response = post_json(
        &t,
        &sid,
        "/api/devices/target/force-reconnect",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(json_body(response).await.to_string().contains("other"));
}

#[tokio::test]
async fn force_reconnect_rejects_concurrent_connection() {
    let t = build_app(
        "reconnect-lock",
        test_credential(TEST_PASSWORD),
        Default::default(),
    );
    seed_device(&t, "target", "").await;
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    let _connection = t.devices.connection_gate.read().await;
    let response = post_json(
        &t,
        &sid,
        "/api/devices/target/force-reconnect",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn force_reconnect_restarts_adb_even_when_device_list_is_empty() {
    let dir = tmp_dir("reconnect-empty-adb");
    let log = dir.join("commands.log");
    // 私有假 ADB：只记录命令并返回空设备列表，绝不重置开发机的真实 ADB。
    let adb_path = if cfg!(windows) {
        dir.join("fake-adb.cmd")
    } else {
        dir.join("fake-adb")
    };
    let script = if cfg!(windows) {
        format!("@echo off\r\necho %*>>\"{}\"\r\nif \"%1\"==\"devices\" echo List of devices attached\r\nexit /b 0\r\n", log.display())
    } else {
        format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nif [ \"$1\" = devices ]; then echo 'List of devices attached'; fi\nexit 0\n", log.display())
    };
    std::fs::write(&adb_path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&adb_path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let cfg = Config {
        data_dir: dir.clone(),
        adb_path: adb_path.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let t = build_app_with_config(cfg, test_credential(TEST_PASSWORD), Default::default());
    seed_device(&t, "target", "").await;
    // 使用 USB 串号，覆盖本次故障的非网络重连分支。
    t.devices
        .devices
        .write()
        .get_mut("target")
        .unwrap()
        .device
        .addr = "missing-usb".into();
    let sid = first_cookie_pair(&cookie_of(&login(&t.app).await));
    let response = post_json(
        &t,
        &sid,
        "/api/devices/target/force-reconnect",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert!(json_body(response)
        .await
        .to_string()
        .contains("ADB 已重启，设备重连失败"));
    let commands = std::fs::read_to_string(log).unwrap();
    let commands: Vec<_> = commands.lines().map(str::trim).collect();
    assert_eq!(&commands[..2], &["kill-server", "start-server"]);
    assert_eq!(commands.iter().filter(|c| **c == "kill-server").count(), 1);
    assert!(commands.contains(&"devices"));
    assert_eq!(
        t.devices.snapshot("target").unwrap().1,
        crate::device::DeviceStatus::Offline
    );
}
