// App Package → 本地编辑区提取（POST /api/app-packages/:id/:version/edit）
// REST 集成测试（include 于 api/tests.rs 的 sec_tests 模块内，复用其 build_app /
// login / post_json / valid_template_png 等装配助手）。
use super::*;

const EDIT_URI: &str = "/api/app-packages/official.demo/1.0.0/edit";
const ANDROID: &str = "com.example.game";

fn edit_body(android: &str) -> serde_json::Value {
    serde_json::json!({ "android_package": android })
}

fn preset_yaml(name: &str, expression: &str) -> Vec<u8> {
    format!(
        "name: {name}\nrunner_id: gamer.yaml\nentrypoint: run\npayload: {{}}\nschedule:\n  kind: cron\n  value:\n    expression: \"{expression}\"\n"
    )
    .into_bytes()
}

fn sha256_of(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

/// PUT JSON（工作区元数据端点是 PUT 语义；harness 原生只有 get/post 助手）。
async fn put_json(
    t: &TestApp,
    sid: &str,
    uri: &str,
    body: serde_json::Value,
) -> HttpResponse<Body> {
    send(
        &t.app,
        req(
            "PUT",
            uri,
            None,
            &json_headers(sid.to_string()),
            Some(body.to_string()),
        ),
    )
    .await
}

/// 在 TestApp 数据目录下布置一个完整合法的工作区（六目录各一文件），
/// 内容与 install_package_fixture 打包内容一致（package 字节）。
fn seed_workspace(dir: &std::path::Path, android: &str) {
    let ws = dir.join(android);
    std::fs::create_dir_all(ws.join("scripts")).unwrap();
    std::fs::write(ws.join("scripts/daily.yaml"), b"steps: []\n").unwrap();
    std::fs::create_dir_all(ws.join("functions")).unwrap();
    std::fs::write(ws.join("functions/common.yaml"), b"login:\n  steps: []\n").unwrap();
    std::fs::create_dir_all(ws.join("templates")).unwrap();
    std::fs::write(ws.join("templates/icon.png"), valid_template_png()).unwrap();
    std::fs::create_dir_all(ws.join("keymaps")).unwrap();
    std::fs::write(
        ws.join("keymaps/wasd.yaml"),
        b"version: 1\nname: wasd\nbindings: []\n",
    )
    .unwrap();
    std::fs::create_dir_all(ws.join("presets")).unwrap();
    std::fs::write(ws.join("presets/daily.yaml"), preset_yaml("daily", "0 8 * * *")).unwrap();
    std::fs::create_dir_all(ws.join("resources")).unwrap();
    std::fs::write(ws.join("resources/config.json"), b"{}").unwrap();
}

fn build_zip(entries: Vec<(&str, Vec<u8>)>) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, content) in entries {
            writer.start_file(name, options).unwrap();
            writer.write_all(&content).unwrap();
        }
        writer.finish().unwrap();
    }
    bytes
}

/// 经安装 API 安装并激活一个归档（不走校验和头）。
async fn install_bytes(t: &TestApp, sid: &str, bytes: Vec<u8>) -> HttpResponse<Body> {
    send(
        &t.app,
        req_bytes(
            "POST",
            "/api/app-packages/install",
            None,
            &[
                (header::COOKIE.to_string(), sid.to_string()),
                (header::CONTENT_TYPE.to_string(), "application/zip".into()),
            ],
            bytes,
        ),
    )
    .await
}

/// 工作区→元数据→导出→安装：得到一个与 seed_workspace 内容一致的已激活包。
async fn seed_installed_package_from_workspace(t: &TestApp, sid: &str) {
    seed_workspace(&t.dir, ANDROID);
    let init = put_json(
        t,
        sid,
        "/api/workspace/com.example.game",
        serde_json::json!({
            "id": "official.demo",
            "version": "1.0.0",
            "android_packages": [ANDROID]
        }),
    )
    .await;
    assert_eq!(init.status(), StatusCode::OK, "{:?}", json_body(init).await);

    let exported = send(
        &t.app,
        req(
            "POST",
            "/api/app-packages/export",
            None,
            &json_headers(sid.to_string()),
            Some(edit_body(ANDROID).to_string()),
        ),
    )
    .await;
    assert_eq!(exported.status(), StatusCode::OK);
    let archive = axum::body::to_bytes(exported.into_body(), 32 * 1024 * 1024)
        .await
        .unwrap()
        .to_vec();

    let installed = send(
        &t.app,
        req_bytes(
            "POST",
            "/api/app-packages/install",
            None,
            &[
                (header::COOKIE.to_string(), sid.to_string()),
                (header::CONTENT_TYPE.to_string(), "application/zip".into()),
                ("X-Expected-Sha256".to_string(), sha256_of(&archive)),
            ],
            archive,
        ),
    )
    .await;
    assert_eq!(
        installed.status(),
        StatusCode::CREATED,
        "{:?}",
        json_body(installed).await
    );
}

fn collect_files(root: &std::path::Path, dir: &std::path::Path, out: &mut std::collections::BTreeMap<String, Vec<u8>>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, out);
        } else if path.is_file() {
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            out.insert(relative, std::fs::read(&path).unwrap());
        }
    }
}

fn assert_no_edit_residue(data_root: &std::path::Path) {
    for name in [".edit-staging", ".edit-backup"] {
        let dir = data_root.join(name);
        assert!(
            !dir.exists() || std::fs::read_dir(&dir).unwrap().next().is_none(),
            "{name} 不应残留条目"
        );
    }
}

/// Round trip：工作区漂移后 edit，工作区文件与包内逐字节一致、package.toml
/// 与 manifest 字段一致、replaced 计数正确、未管理兄弟条目保留。
#[tokio::test]
async fn edit_round_trip_replaces_workspace_with_package_contents() {
    let test_app = build_app("pkgedit1", test_credential("admin123"), Default::default());
    let session = first_cookie_pair(&cookie_of(&login(&test_app.app).await));
    seed_installed_package_from_workspace(&test_app, &session).await;

    // 工作区漂移：改内容 + 多出脚本 + 未管理兄弟文件/目录
    let ws = test_app.dir.join(ANDROID);
    std::fs::write(ws.join("scripts/daily.yaml"), b"steps: []\n# drifted\n").unwrap();
    std::fs::write(ws.join("scripts/extra.yaml"), b"steps: []\n").unwrap();
    std::fs::write(ws.join("templates/icon.png"), b"drifted-not-png").unwrap();
    std::fs::write(ws.join("notes.txt"), b"sibling stays").unwrap();
    std::fs::create_dir_all(ws.join("extra_dir")).unwrap();
    std::fs::write(ws.join("extra_dir/keep.txt"), b"sibling dir stays").unwrap();

    let resp = post_json(&test_app, &session, EDIT_URI, edit_body(ANDROID)).await;
    assert_eq!(resp.status(), StatusCode::OK, "{:?}", json_body(resp).await);
    let body = json_body(resp).await;
    assert_eq!(body["android_package"], ANDROID);
    // metadata 形状与 GET /api/workspace 一致
    assert_eq!(body["metadata"]["format_version"], 2);
    assert_eq!(body["metadata"]["id"], "official.demo");
    assert_eq!(body["metadata"]["version"], "1.0.0");
    assert_eq!(body["metadata"]["android_packages"][0], ANDROID);
    for kind in ["scripts", "functions", "templates", "keymaps", "presets", "resources"] {
        assert_eq!(body["replaced"][kind], 1, "replaced.{kind} 应为 1");
    }

    // 包内资源文件与工作区逐字节一致（manifest.toml/install.json/package.toml
    // 是两侧自产文件，不参与对比）
    let package_root = test_app.dir.join("app-packages/official.demo/1.0.0");
    let mut package_files = std::collections::BTreeMap::new();
    collect_files(&package_root, &package_root, &mut package_files);
    package_files.remove("manifest.toml");
    package_files.remove("install.json");
    assert!(!package_files.is_empty());
    for (relative, bytes) in &package_files {
        let workspace_file = relative.split('/').fold(ws.clone(), |acc, part| acc.join(part));
        assert_eq!(
            std::fs::read(&workspace_file).unwrap(),
            *bytes,
            "工作区 {relative} 必须与包内逐字节一致"
        );
    }

    // 替换语义：漂移的旧管理资源消失（daily.yaml 回到包内容、icon.png 复原、
    // extra.yaml 被清除），未管理兄弟条目保留
    assert_eq!(
        std::fs::read(ws.join("scripts/daily.yaml")).unwrap(),
        b"steps: []\n"
    );
    assert!(!ws.join("scripts/extra.yaml").exists(), "包外脚本应被清除");
    assert_eq!(
        std::fs::read(ws.join("templates/icon.png")).unwrap(),
        package_files["templates/icon.png"]
    );
    assert_eq!(std::fs::read(ws.join("notes.txt")).unwrap(), b"sibling stays");
    assert_eq!(
        std::fs::read(ws.join("extra_dir/keep.txt")).unwrap(),
        b"sibling dir stays"
    );

    // package.toml 与 manifest 字段一致（固定字段序列化）
    assert_eq!(
        std::fs::read_to_string(ws.join("package.toml")).unwrap(),
        "format_version = 2\nid = \"official.demo\"\nversion = \"1.0.0\"\n\n[android]\npackages = [\"com.example.game\"]\n"
    );
    assert_no_edit_residue(&test_app.dir);
}

/// 解析优先级：edit 后（不删包）修改工作区脚本，引擎运行快照取到编辑区版本。
#[tokio::test]
async fn edit_makes_workspace_highest_priority_for_engine_snapshot() {
    let test_app = build_app("pkgedit2", test_credential("admin123"), Default::default());
    let session = first_cookie_pair(&cookie_of(&login(&test_app.app).await));
    seed_installed_package_from_workspace(&test_app, &session).await;
    let resp = post_json(&test_app, &session, EDIT_URI, edit_body(ANDROID)).await;
    assert_eq!(resp.status(), StatusCode::OK, "{:?}", json_body(resp).await);

    // 编辑区脚本改动（包保持原样）
    let ws = test_app.dir.join(ANDROID);
    std::fs::write(ws.join("scripts/daily.yaml"), b"steps: []\n# edited-local\n").unwrap();

    let cfg = crate::config::Config {
        data_dir: test_app.dir.clone(),
        ..Default::default()
    };
    let store = crate::scripts::ScriptStore::open(&cfg).unwrap();
    let snapshot = crate::extensions::gamer_yaml::engine::snapshot::RunSnapshot::capture(&store, ANDROID).unwrap();
    let app = crate::core::AppContext::new(
        crate::core::DeviceId::new("device-1").unwrap(),
        crate::core::AndroidPackageName::new(ANDROID).unwrap(),
        None,
    );
    let resources = crate::extensions::gamer_yaml::engine::snapshot::RunResources::new(&snapshot, &store, app);
    // as_provider() 返回 &dyn ResourceProvider，方法调用无需导入 trait
    assert_eq!(
        resources.as_provider().script_content("daily.yaml"),
        Some("steps: []\n# edited-local\n".to_string()),
        "运行快照必须取到编辑区版本"
    );
    // 包内字节不动（immutable）
    assert_eq!(
        std::fs::read(test_app.dir.join("app-packages/official.demo/1.0.0/scripts/daily.yaml"))
            .unwrap(),
        b"steps: []\n"
    );
}

/// Preflight 失败回滚：坏脚本包安装成功（安装侧不做脚本校验），edit 返回 400
/// 且工作区完全保持原状、无 .edit-staging/.edit-backup 残留。
#[tokio::test]
async fn edit_preflight_failure_keeps_workspace_intact_and_cleans_staging() {
    let test_app = build_app("pkgedit3", test_credential("admin123"), Default::default());
    let session = first_cookie_pair(&cookie_of(&login(&test_app.app).await));

    let manifest = b"format_version = 2\nid = \"official.demo\"\nversion = \"1.0.0\"\n[android]\npackages = [\"com.example.game\"]\n";
    let installed = install_bytes(
        &test_app,
        &session,
        build_zip(vec![
            ("manifest.toml", manifest.to_vec()),
            ("scripts/bad.yaml", b"steps: 42\n".to_vec()),
        ]),
    )
    .await;
    assert_eq!(
        installed.status(),
        StatusCode::CREATED,
        "{:?}",
        json_body(installed).await
    );

    // 既有工作区哨兵内容（编辑失败必须原样保留）
    let ws = test_app.dir.join(ANDROID);
    std::fs::create_dir_all(ws.join("scripts")).unwrap();
    std::fs::write(ws.join("scripts/keep.yaml"), b"steps: []\n# sentinel\n").unwrap();
    std::fs::write(ws.join("sibling.txt"), b"untouched").unwrap();

    let resp = post_json(&test_app, &session, EDIT_URI, edit_body(ANDROID)).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{:?}", json_body(resp).await);
    let body = json_body(resp).await;
    assert_eq!(body["code"], "preflight_failed", "preflight 失败必须带机器码");
    let message = body["error"].as_str().unwrap();
    assert!(message.contains("scripts/bad.yaml"), "{message}");

    // 工作区原状：哨兵保留、包内坏脚本未被带入、package.toml 未被写入
    assert_eq!(
        std::fs::read(ws.join("scripts/keep.yaml")).unwrap(),
        b"steps: []\n# sentinel\n"
    );
    assert_eq!(std::fs::read(ws.join("sibling.txt")).unwrap(), b"untouched");
    assert!(!ws.join("scripts/bad.yaml").exists());
    assert!(!ws.join("package.toml").exists());
    assert_no_edit_residue(&test_app.dir);
}

/// targets 校验：android_package 不在 manifest targets → 400（列出合法
/// targets）；未安装 id/version → 404；非法包名/未知字段 → 4xx。
#[tokio::test]
async fn edit_rejects_unknown_target_and_uninstalled_version() {
    let test_app = build_app("pkgedit4", test_credential("admin123"), Default::default());
    let session = first_cookie_pair(&cookie_of(&login(&test_app.app).await));

    let manifest = b"format_version = 2\nid = \"official.demo\"\nversion = \"1.0.0\"\n[android]\npackages = [\"com.example.game\"]\n";
    let installed = install_bytes(
        &test_app,
        &session,
        build_zip(vec![("manifest.toml", manifest.to_vec())]),
    )
    .await;
    assert_eq!(installed.status(), StatusCode::CREATED);

    // 未安装版本 → 404
    let missing = post_json(
        &test_app,
        &session,
        "/api/app-packages/official.demo/9.9.9/edit",
        edit_body(ANDROID),
    )
    .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    // 未安装 id → 404
    let unknown = post_json(
        &test_app,
        &session,
        "/api/app-packages/official.nope/1.0.0/edit",
        edit_body(ANDROID),
    )
    .await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);

    // android_package 不在 targets → 400，消息列出合法 targets
    let wrong_target = post_json(
        &test_app,
        &session,
        EDIT_URI,
        edit_body("com.other.game"),
    )
    .await;
    assert_eq!(
        wrong_target.status(),
        StatusCode::BAD_REQUEST,
        "{:?}",
        json_body(wrong_target).await
    );
    let message = json_body(wrong_target).await["error"].as_str().unwrap().to_string();
    assert!(message.contains(ANDROID), "应列出合法 targets: {message}");

    // 非法 android 包名 → 400
    let bad = post_json(&test_app, &session, EDIT_URI, edit_body("bad pkg!")).await;
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    // deny_unknown_fields：未知字段 → 4xx
    let extra = post_json(
        &test_app,
        &session,
        EDIT_URI,
        serde_json::json!({ "android_package": ANDROID, "extra": 1 }),
    )
    .await;
    assert!(extra.status().is_client_error());
}

/// 多 target 包：body 指定其一成功，package.toml 仍保存完整 targets 列表
///（含 name/revision 的字段全保留）。
#[tokio::test]
async fn edit_multi_target_package_keeps_full_targets_and_revision() {
    let test_app = build_app("pkgedit5", test_credential("admin123"), Default::default());
    let session = first_cookie_pair(&cookie_of(&login(&test_app.app).await));

    let manifest = b"format_version = 2\nid = \"official.multi\"\nversion = \"2.0.0\"\nname = \"Multi\"\nrevision = \"r7\"\n\n[android]\npackages = [\"com.example.game\", \"com.other.game\"]\n";
    let installed = install_bytes(
        &test_app,
        &session,
        build_zip(vec![
            ("manifest.toml", manifest.to_vec()),
            ("scripts/one.yaml", b"steps: []\n".to_vec()),
            ("resources/blob.txt", b"blob".to_vec()),
        ]),
    )
    .await;
    assert_eq!(
        installed.status(),
        StatusCode::CREATED,
        "{:?}",
        json_body(installed).await
    );

    // 指定第二个 target 提取 → 工作区落在 com.other.game/
    let resp = post_json(
        &test_app,
        &session,
        "/api/app-packages/official.multi/2.0.0/edit",
        edit_body("com.other.game"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "{:?}", json_body(resp).await);
    let body = json_body(resp).await;
    assert_eq!(body["android_package"], "com.other.game");
    assert_eq!(body["metadata"]["id"], "official.multi");
    assert_eq!(body["metadata"]["revision"], "r7");
    assert_eq!(body["metadata"]["name"], "Multi");
    assert_eq!(body["metadata"]["android_packages"].as_array().unwrap().len(), 2);
    assert_eq!(body["replaced"]["scripts"], 1);
    assert_eq!(body["replaced"]["resources"], 1);
    assert_eq!(body["replaced"]["templates"], 0);

    let ws = test_app.dir.join("com.other.game");
    assert_eq!(std::fs::read(ws.join("scripts/one.yaml")).unwrap(), b"steps: []\n");
    // package.toml 字段全保留：revision/name/完整 targets 列表
    assert_eq!(
        std::fs::read_to_string(ws.join("package.toml")).unwrap(),
        "format_version = 2\nid = \"official.multi\"\nversion = \"2.0.0\"\nname = \"Multi\"\nrevision = \"r7\"\n\n[android]\npackages = [\"com.example.game\", \"com.other.game\"]\n"
    );
    assert_no_edit_residue(&test_app.dir);
}

#[tokio::test]
async fn edit_requires_login() {
    let test_app = build_app("pkgedit6", test_credential("admin123"), Default::default());
    let resp = send(
        &test_app.app,
        req("POST", EDIT_URI, None, &[], Some(edit_body(ANDROID).to_string())),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
