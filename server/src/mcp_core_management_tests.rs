struct PublishedFixture;
impl crate::scheduler::EntrypointDescriber for PublishedFixture {
    fn describe(
        &self,
        entrypoint: &str,
    ) -> Result<Value, crate::scheduler::EntrypointDescribeError> {
        if entrypoint == "default/next.yaml" {
            Ok(json!({"schema":[]}))
        } else {
            Err(crate::scheduler::EntrypointDescribeError::NotFound {
                resource: entrypoint.into(),
            })
        }
    }
}

#[tokio::test]
async fn task_retarget_pins_snapshot_preserves_configuration_and_replays_once() {
    let f = fixture().await;
    f.core.scheduler.register_entrypoint_describer(
        "fixture-runner",
        "fixture-plugin",
        Arc::new(PublishedFixture),
    );
    let p = principal(&f, &["task.write", "task.read"]);
    let saved = data(media_call(&f, &p, "gamer.core.task_save", task()).await);
    let id = saved["id"].as_str().unwrap();
    let mut old = f.core.db.get_timer_task_async(id).await.unwrap().unwrap();
    old.payload = json!({"args":{"keep":42}});
    old.extensions["untouched"] = json!({"notify":true});
    f.core.db.upsert_timer_task_async(&old).await.unwrap();
    let args = json!({"task_id":id,"expected_updated_at":old.updated_at.to_rfc3339(),"expected_entrypoint":old.entrypoint,"entrypoint":"default/next.yaml","operation_id":"retarget-once"});
    for (field, value) in [
        ("expected_entrypoint", json!("default/stale.yaml")),
        ("entrypoint", json!("other/next.yaml")),
        ("entrypoint", json!("default/missing.yaml")),
    ] {
        let mut bad = args.clone();
        bad[field] = value;
        bad["operation_id"] = json!(uuid::Uuid::new_v4().to_string());
        assert!(
            media_call(&f, &p, "gamer.core.task_retarget", bad)
                .await
                .is_error
        );
        assert_eq!(
            f.core.db.get_timer_task_async(id).await.unwrap().unwrap(),
            old
        );
    }
    let result = media_call(&f, &p, "gamer.core.task_retarget", args.clone()).await;
    let changed = data(result.clone());
    let mut expected = old.clone();
    expected.entrypoint = "default/next.yaml".into();
    let current = f.core.db.get_timer_task_async(id).await.unwrap().unwrap();
    expected.updated_at = current.updated_at;
    expected.extensions = current.extensions.clone();
    assert_eq!(current, expected);
    assert_eq!(current.extensions["untouched"], old.extensions["untouched"]);
    assert_eq!(changed["execution_policy"], "restricted_external");
    assert_eq!(
        media_call(&f, &p, "gamer.core.task_retarget", args.clone()).await,
        result
    );
    let mut stale = args;
    stale["operation_id"] = json!("stale-retarget");
    assert!(
        media_call(&f, &p, "gamer.core.task_retarget", stale)
            .await
            .is_error
    );
    assert_eq!(f.accepted.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn task_retarget_cannot_indirectly_authorize_an_enabled_task() {
    let f = fixture().await;
    f.core.scheduler.register_entrypoint_describer(
        "fixture-runner",
        "fixture-plugin",
        Arc::new(PublishedFixture),
    );
    let p = principal(&f, &["task.write"]);
    let saved = data(media_call(&f, &p, "gamer.core.task_save", task()).await);
    let id = saved["id"].as_str().unwrap();
    let mut old = f.core.db.get_timer_task_async(id).await.unwrap().unwrap();
    old.enabled = true;
    f.core.db.upsert_timer_task_async(&old).await.unwrap();
    let args = json!({"task_id":id,"expected_updated_at":old.updated_at.to_rfc3339(),"expected_entrypoint":old.entrypoint,"entrypoint":"default/next.yaml","operation_id":"enabled-retarget"});
    assert!(
        media_call(&f, &p, "gamer.core.task_retarget", args)
            .await
            .is_error
    );
    assert_eq!(
        f.core.db.get_timer_task_async(id).await.unwrap().unwrap(),
        old
    );
}

fn plugin_principal(f: &Fixture, scopes: &[&str], plugin: &str) -> crate::mcp::ExternalPrincipal {
    let created = f
        .service
        .credentials()
        .create(CreateTokenRequest {
            label: "isolated plugin update".into(),
            scopes: scopes.iter().map(|s| s.to_string()).collect(),
            expires_in_seconds: 3600,
            allowed: AllowedObjects {
                plugins: [plugin.to_owned()].into(),
                ..Default::default()
            },
        })
        .unwrap();
    f.service
        .credentials()
        .authenticate(&created.token)
        .unwrap()
}

fn update_fixture_archive(version: &str, permissions: &str) -> Vec<u8> {
    use std::io::Write;
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zip.start_file("manifest.toml", zip::write::SimpleFileOptions::default())
        .unwrap();
    write!(zip,"manifest_version = 2\nid = \"gamer-video\"\nversion = \"{version}\"\nname = \"isolated fixture\"\npermissions = [{permissions}]\n[execution]\nkind = \"builtin\"\nbuiltin_id = \"gamer-video\"\n").unwrap();
    zip.finish().unwrap().into_inner()
}

fn seed_update(f: &Fixture, version: &str, permissions: &str) -> String {
    use sha2::{Digest, Sha256};
    let bytes = update_fixture_archive(version, permissions);
    let hash = format!("{:x}", Sha256::digest(&bytes));
    let dir = f.core.packages.data_root().join("web-dist");
    std::fs::create_dir_all(dir.join("plugins")).unwrap();
    let filename = format!("gamer-video-{version}.gplugin");
    std::fs::write(dir.join("plugins").join(&filename), &bytes).unwrap();
    let permissions: Vec<String> = serde_json::from_str(&format!("[{permissions}]")).unwrap();
    std::fs::write(dir.join("registry.json"),serde_json::to_vec(&json!({"schema_version":2,"plugins":[{
        "id":"gamer-video","name":"isolated fixture","version":version,"download_url":format!("/plugins/{filename}"),
        "sha256":hash,"size":bytes.len(),"execution":{"kind":"builtin","builtin_id":"gamer-video"},"permissions":permissions
    }]})).unwrap()).unwrap();
    f.core.plugin_market.prime_offline();
    hash
}

#[tokio::test]
async fn plugin_update_pins_archive_and_preserves_disabled_state_and_receipts() {
    let f = fixture().await;
    let id = crate::extensions::ExtensionId::parse("gamer-video").unwrap();
    f.core
        .extensions
        .install(&update_fixture_archive("1.0.0", ""))
        .await
        .unwrap();
    f.core.extensions.disable(&id).await.unwrap();
    let p = plugin_principal(&f, &["plugin.update"], "gamer-video");
    let denied = plugin_principal(&f, &["observe"], "gamer-video");
    let foreign = plugin_principal(&f, &["plugin.update"], "fixture-plugin");
    let hash = seed_update(&f, "1.0.1", "");
    let plan = data(
        media_call(
            &f,
            &p,
            "gamer.core.plugin_update_plan",
            json!({"plugin_id":"gamer-video","inspect":true}),
        )
        .await,
    );
    assert_eq!(plan["updates"][0]["sha256"], hash);
    assert_eq!(plan["updates"][0]["added_permissions"], json!([]));
    assert_eq!(plan["updates"][0]["inspected"], true);
    let args = json!({"plugin_id":"gamer-video","from_version":"1.0.0","to_version":"1.0.1","sha256":hash,"operation_id":"update-once"});
    for principal in [&denied, &foreign] {
        assert!(
            media_call(&f, principal, "gamer.core.plugin_update", args.clone())
                .await
                .is_error
        );
    }
    for (field, value) in [
        ("sha256", json!("0".repeat(64))),
        ("from_version", json!("0.9.0")),
    ] {
        let mut bad = args.clone();
        bad[field] = value;
        bad["operation_id"] = json!(uuid::Uuid::new_v4().to_string());
        assert!(
            media_call(&f, &p, "gamer.core.plugin_update", bad)
                .await
                .is_error
        );
        assert_eq!(
            f.core.extensions.list().unwrap()[0]
                .active_version()
                .to_string(),
            "1.0.0"
        );
    }
    let busy = f.core.runs.try_idle_maintenance().unwrap();
    let mut busy_args = args.clone();
    busy_args["operation_id"] = json!("busy-update");
    assert!(
        media_call(&f, &p, "gamer.core.plugin_update", busy_args)
            .await
            .is_error
    );
    drop(busy);
    let result = media_call(&f, &p, "gamer.core.plugin_update", args.clone()).await;
    let updated = data(result.clone());
    assert_eq!(updated["active_version"], "1.0.1");
    assert_eq!(
        f.core.extensions.list().unwrap()[0].state(),
        crate::extensions::ExtensionState::Disabled
    );
    assert_eq!(updated["installed_versions"].as_array().unwrap().len(), 2);
    assert_eq!(
        media_call(&f, &p, "gamer.core.plugin_update", args).await,
        result
    );
    assert!(f.core.runs.try_idle_maintenance().is_some());
}

#[tokio::test]
async fn plugin_update_new_permissions_require_exact_separate_grant() {
    let f = fixture().await;
    f.core
        .extensions
        .install(&update_fixture_archive("1.0.0", ""))
        .await
        .unwrap();
    let p = plugin_principal(&f, &["plugin.update"], "gamer-video");
    let grant = plugin_principal(
        &f,
        &["plugin.update", "plugin.permissions.grant"],
        "gamer-video",
    );
    let hash = seed_update(&f, "1.0.1", "\"resource.read\"");
    let args = json!({"plugin_id":"gamer-video","from_version":"1.0.0","to_version":"1.0.1","sha256":hash,"operation_id":"without-grant"});
    assert!(
        media_call(&f, &p, "gamer.core.plugin_update", args.clone())
            .await
            .is_error
    );
    let mut permission_args = args;
    permission_args["grant_permissions"] = json!(["resource.read"]);
    permission_args["operation_id"] = json!("missing-scope");
    assert!(
        media_call(
            &f,
            &p,
            "gamer.core.plugin_update_with_permissions",
            permission_args.clone()
        )
        .await
        .is_error
    );
    for grants in [json!([]), json!(["resource.read", "resource.write"])] {
        let mut bad = permission_args.clone();
        bad["grant_permissions"] = grants;
        bad["operation_id"] = json!(uuid::Uuid::new_v4().to_string());
        assert!(
            media_call(&f, &grant, "gamer.core.plugin_update_with_permissions", bad)
                .await
                .is_error
        );
    }
    assert_eq!(
        f.core.extensions.list().unwrap()[0]
            .active_version()
            .to_string(),
        "1.0.0"
    );
    permission_args["operation_id"] = json!("exact-grant");
    let updated = data(
        media_call(
            &f,
            &grant,
            "gamer.core.plugin_update_with_permissions",
            permission_args,
        )
        .await,
    );
    assert_eq!(updated["active_version"], "1.0.1");
}
