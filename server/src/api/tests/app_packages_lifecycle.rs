// App Package 全生命周期 E2E（收尾计划书 §37 ①-⑱ + 追加断言，单测试覆盖）。
// include 于 api/tests.rs 的 sec_tests 模块内，复用其 build_app / login /
// post_json / valid_template_png 等装配助手。
//
// 无头环境（无 adb / 无真机）：运行生命周期用 harness 假执行器
// （build_app_with_executor）驱动到终态，resolver 层语义不受影响——运行提交前
// 的参数解析（resolve_entry_args）与引擎运行快照（RunSnapshot）都走
// EditableLocal > UserOverride > InstalledPackage 的 composite 缝，测试另外
// 直接断言该快照的内容来源。
//
// 已知实现边界（按实际行为断言，不放宽语义）：统一执行入口 /api/runs 的
// 脚本存在性前置校验只读本地编辑区（ResourceStore::get_text），纯包内脚本
// 在本地删空后手动运行返回结构化 not_found；包内容来源改经引擎运行快照 +
// composite 资源面
// （keymap GET/list、脚本保存期模板校验）断言，见步骤 ⑩ 注释。
use super::*;

const ANDROID: &str = "com.test.game";
const PKG_ID: &str = "official.test";
const SCRIPT_URI: &str = "/api/apps/com.test.game/resources/scripts/com.test.game%2Fdaily.yaml";
const FUNC_URI: &str = "/api/apps/com.test.game/resources/functions/com.test.game%2Fcommon.yaml";
const KEYMAP_URI: &str = "/api/apps/com.test.game/resources/keymaps/com.test.game%2Fwasd.yaml";
const DEVICE: &str = "device-e2e";

const KEYMAP_YAML: &str = "version: 1\nname: wasd\nbindings:\n  - key: KeyW\n    action:\n      type: tap\n      at: [0.5, 0.5]\n";
const FUNC_YAML_V1: &str = "greet:\n  params:\n    - 'text:who:称呼:\"caller-v1\"'\n  steps:\n    - log: 你好, $who\n    - return: true\n";
const FUNC_YAML_V2: &str = "greet:\n  params:\n    - 'text:who:称呼:\"caller-v2\"'\n  steps:\n    - log: 你好, $who\n    - return: false\n";

/// 脚本源码：参数默认值（banner）即「参数化输出」探针——它被写进 log 步骤并
/// 传给 func 步骤；运行端点 202 响应的 resolved_args 暴露当前生效值，脚本
/// 内容属哪一层（EditableLocal / InstalledPackage）由此可断言。
fn script_yaml(banner_default: &str) -> String {
    format!(
        "params:\n  - 'text:banner:横幅:\"{banner_default}\"'\nsteps:\n  - log: $banner\n  - func: common/greet\n    args:\n      who: $banner\n"
    )
}

/// 无头假执行器：prepare/acquire/execute 全部直接成功，运行到达 success 终态。
struct OkExecutor;

impl crate::run_manager::RunExecutor for OkExecutor {
    fn prepare<'a>(
        &'a self,
        _: &'a crate::core::RunContext,
        _: &'a crate::core::RunRequest,
    ) -> futures_util::future::BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async { Ok(()) })
    }
    fn execute<'a>(
        &'a self,
        _: &'a crate::core::RunContext,
        _: &'a crate::core::RunRequest,
        _: bool,
        _: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> futures_util::future::BoxFuture<'a, anyhow::Result<Vec<(String, String)>>> {
        Box::pin(async { Ok(vec![]) })
    }
    fn acquire(
        &self,
        _: &crate::core::RunContext,
    ) -> anyhow::Result<Box<dyn crate::core::ActivityLease>> {
        Ok(Box::new(crate::core::NoopLease))
    }
}

fn sha256_of(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

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

async fn delete_json(t: &TestApp, sid: &str, uri: &str) -> HttpResponse<Body> {
    send(
        &t.app,
        req("DELETE", uri, None, &json_headers(sid.to_string()), None),
    )
    .await
}

/// 导出当前工作区并断言响应头（Content-Type / Content-Disposition / 摘要头），
/// 返回归档字节与其 SHA-256。
async fn export_archive(t: &TestApp, sid: &str, expected_filename: &str) -> (Vec<u8>, String) {
    let resp = send(
        &t.app,
        req(
            "POST",
            "/api/app-packages/export",
            None,
            &json_headers(sid.to_string()),
            Some(serde_json::json!({ "android_package": ANDROID }).to_string()),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "{:?}", json_body(resp).await);
    assert_eq!(resp.headers()["content-type"], "application/octet-stream");
    let disposition = resp
        .headers()
        .get(header::CONTENT_DISPOSITION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(
        disposition,
        format!("attachment; filename=\"{expected_filename}\""),
        "Content-Disposition 必须是 attachment + id-version.gamerpkg"
    );
    let sha = resp
        .headers()
        .get("x-content-sha256")
        .expect("X-Content-Sha256 响应头缺失")
        .to_str()
        .unwrap()
        .to_string();
    let body = axum::body::to_bytes(resp.into_body(), 32 * 1024 * 1024)
        .await
        .unwrap()
        .to_vec();
    assert_eq!(sha, sha256_of(&body), "摘要头必须等于归档字节摘要");
    assert_eq!(&body[..2], b"PK", "导出产物必须是 zip 归档");
    (body, sha)
}

/// 安装归档（可选 X-Expected-Sha256 校验头），断言 201 并返回响应 JSON。
async fn install_archive(
    t: &TestApp,
    sid: &str,
    archive: Vec<u8>,
    sha: Option<&str>,
) -> serde_json::Value {
    let mut headers = vec![
        (header::COOKIE.to_string(), sid.to_string()),
        (
            header::CONTENT_TYPE.to_string(),
            "application/zip".to_string(),
        ),
    ];
    if let Some(sha) = sha {
        headers.push(("X-Expected-Sha256".to_string(), sha.to_string()));
    }
    let resp = send(
        &t.app,
        req_bytes("POST", "/api/app-packages/install", None, &headers, archive),
    )
    .await;
    let status = resp.status();
    let body = json_body(resp).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body
}

/// 轮询 run 直到到达期望终态（假执行器下毫秒级，超时 8s 防挂死）。
async fn wait_run_state(t: &TestApp, sid: &str, run_id: &str, want: &str) {
    for _ in 0..800 {
        let resp = get_json(t, sid, &format!("/api/runs/{run_id}")).await;
        if json_body(resp).await["state"] == want {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("run {run_id} 未在限时内到达 {want}");
}

/// 提交脚本运行并等 success 终态，返回提交响应 JSON（含 resolved_args）。
async fn run_script_to_success(t: &TestApp, sid: &str) -> serde_json::Value {
    let resp = post_json(
        t,
        sid,
        "/api/runs",
        serde_json::json!({
            "runner_id": "gamer.yaml",
            "entrypoint": "com.test.game/daily.yaml",
            "device_id": DEVICE,
            "payload": {},
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED, "{:?}", json_body(resp).await);
    let body = json_body(resp).await;
    let run_id = body["run_id"].as_str().expect("run_id").to_string();
    wait_run_state(t, sid, &run_id, "success").await;
    body
}

/// 读取脚本/函数当前内容版本（expected_version 门禁用）。
async fn current_version(t: &TestApp, sid: &str, uri: &str) -> String {
    let resp = get_json(t, sid, uri).await;
    assert_eq!(resp.status(), StatusCode::OK, "{:?}", json_body(resp).await);
    json_body(resp).await["version"]
        .as_str()
        .expect("内容版本短码")
        .to_string()
}

/// 本地编辑区四资源目录必须无文件（package.toml 等自产文件不计）。
fn assert_local_resource_files(t: &TestApp, present: bool) {
    for kind in ["scripts", "functions", "templates", "keymaps"] {
        let dir = t.dir.join(ANDROID).join(kind);
        let has_files = dir.exists() && std::fs::read_dir(&dir).unwrap().next().is_some();
        assert_eq!(
            has_files, present,
            "data/{ANDROID}/{kind}/ 文件存在性应为 {present}"
        );
    }
}

#[tokio::test]
async fn app_package_full_lifecycle_workspace_export_install_edit_rerelease() {
    let test_app = build_app_with_executor(
        "apklifecycle",
        test_credential("admin123"),
        Default::default(),
        std::sync::Arc::new(OkExecutor),
    );
    let sid = first_cookie_pair(&cookie_of(&login(&test_app.app).await));

    // ① 初始化工作区元数据（package.toml，format_version=2）
    let init = put_json(
        &test_app,
        &sid,
        &format!("/api/workspace/{ANDROID}"),
        serde_json::json!({
            "id": PKG_ID,
            "version": "1.0.0",
            "android_packages": [ANDROID]
        }),
    )
    .await;
    assert_eq!(init.status(), StatusCode::OK, "{:?}", json_body(init).await);
    let body = json_body(init).await;
    assert_eq!(body["metadata"]["format_version"], 2);
    assert_eq!(body["metadata"]["id"], PKG_ID);
    assert!(test_app.dir.join(format!("{ANDROID}/package.toml")).is_file());

    // ③ 新建函数库（先于 ②：脚本保存校验 func 目标存在性，函数必须已存在；
    //    计划书 ②③ 顺序按此实现约束对调）
    let resp = post_json(
        &test_app,
        &sid,
        "/api/apps/com.test.game/resources/functions",
        serde_json::json!({
            "name": "common",
            "content": FUNC_YAML_V1,
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED, "{:?}", json_body(resp).await);
    assert_eq!(json_body(resp).await["id"], "com.test.game/common.yaml");

    // ② 新建脚本：log 参数化输出 + func 步骤调用 ③ 的函数（无 find/match 模板步骤）
    let resp = post_json(
        &test_app,
        &sid,
        "/api/apps/com.test.game/resources/scripts",
        serde_json::json!({
            "name": "daily.yaml",
            "content": script_yaml("editable-v1"),
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED, "{:?}", json_body(resp).await);
    assert_eq!(json_body(resp).await["id"], "com.test.game/daily.yaml");

    // ④ 添加模板（通用资源 API：原始字节 body + ?name=）
    let resp = send(
        &test_app.app,
        req_bytes(
            "POST",
            "/api/apps/com.test.game/resources/templates?name=icon.png",
            None,
            &[
                (header::COOKIE.to_string(), sid.clone()),
                (header::CONTENT_TYPE.to_string(), "image/png".to_string()),
            ],
            valid_template_png(),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED, "{:?}", json_body(resp).await);
    assert_eq!(json_body(resp).await["name"], "icon.png");

    // ⑤ 添加按键映射
    let resp = post_json(
        &test_app,
        &sid,
        "/api/apps/com.test.game/resources/keymaps",
        serde_json::json!({
            "name": "wasd",
            "content": KEYMAP_YAML,
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED, "{:?}", json_body(resp).await);
    assert_eq!(json_body(resp).await["id"], "com.test.game/wasd.yaml");

    // ⑥ 直接运行：EditableLocal 层生效 → success；resolved_args 暴露本地参数默认值
    let submitted = run_script_to_success(&test_app, &sid).await;
    assert_eq!(
        submitted["resolved_args"]["banner"], "editable-v1",
        "运行提交解析必须取到本地编辑区脚本内容"
    );

    // ⑦ 导出 official.test@1.0.0（响应头断言见 export_archive）
    let (archive_v1, sha_v1) = export_archive(&test_app, &sid, "official.test-1.0.0.gamerpkg").await;

    // ⑧ 经 API 删除本地四类资源
    for (method_uri, kind) in [
        (SCRIPT_URI.to_string(), "脚本"),
        (FUNC_URI.to_string(), "函数"),
        (KEYMAP_URI.to_string(), "keymap"),
        (format!("/api/apps/{ANDROID}/resources/templates/icon.png"), "模板"),
    ] {
        let resp = delete_json(&test_app, &sid, &method_uri).await;
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "删除{kind}失败: {:?}",
            json_body(resp).await
        );
    }
    assert_local_resource_files(&test_app, false);
    // 本地删空后：本地视角全部不可见 + 工作区统计归零
    let resp = get_json(&test_app, &sid, SCRIPT_URI).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let resp = get_json(&test_app, &sid, FUNC_URI).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let resp = get_json(&test_app, &sid, &format!("/api/apps/{ANDROID}/resources/functions")).await;
    assert!(json_body(resp).await.as_array().unwrap().is_empty());
    let resp = get_json(&test_app, &sid, &format!("/api/apps/{ANDROID}/resources/templates")).await;
    assert!(json_body(resp).await.as_array().unwrap().is_empty());
    let resp = get_json(&test_app, &sid, &format!("/api/apps/{ANDROID}/resources/keymaps")).await;
    assert!(json_body(resp).await.as_array().unwrap().is_empty());
    let resp = get_json(&test_app, &sid, &format!("/api/workspace/{ANDROID}")).await;
    let body = json_body(resp).await;
    for kind in ["scripts", "functions", "templates", "keymaps", "presets", "resources"] {
        assert_eq!(body["stats"][kind], 0, "删空后 {kind} 统计应为 0");
    }

    // ⑨ 安装刚导出的 .gamerpkg（带 X-Expected-Sha256）→ 自动激活 1.0.0
    let installed = install_archive(&test_app, &sid, archive_v1.clone(), Some(&sha_v1)).await;
    assert_eq!(installed["id"], PKG_ID);
    assert_eq!(installed["active_version"], "1.0.0", "安装即激活");
    assert_eq!(
        installed["versions"][0]["sha256"], sha_v1,
        "install.json 必须记录归档 SHA-256"
    );

    // ⑩ 只装包不装本地的运行语义：
    //   a) 统一执行入口的脚本存在性前置校验只读本地编辑区 → 结构化 not_found
    //      （当前实现边界，如实断言；语义是「本地删空后手动 run 入口不可达」，
    //      不是包内容缺失）
    let resp = post_json(
        &test_app,
        &sid,
        "/api/runs",
        serde_json::json!({
            "runner_id": "gamer.yaml",
            "entrypoint": "com.test.game/daily.yaml",
            "device_id": DEVICE,
            "payload": {},
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = json_body(resp).await;
    assert_eq!(body["error"], "not_found", "{body}");
    //   b) 引擎运行快照（composite：EditableLocal > UserOverride > InstalledPackage）
    //      必须取到包内脚本/函数内容——本地已删空，内容只能来自 InstalledPackage 层
    let cfg = crate::config::Config {
        data_dir: test_app.dir.clone(),
        ..Default::default()
    };
    let store = crate::resources::ResourceStore::open(&cfg).unwrap();
    let snapshot = crate::extensions::gamer_yaml::engine::snapshot::RunSnapshot::capture(&store, ANDROID).unwrap();
    let app = crate::core::AppContext::new(
        crate::core::DeviceId::new(DEVICE).unwrap(),
        crate::core::AndroidPackageName::new(ANDROID).unwrap(),
        Some(crate::core::AppPackageId::new(ANDROID).unwrap()),
    );
    let resources = crate::extensions::gamer_yaml::engine::snapshot::RunResources::new(&snapshot, &store, app);
    assert_eq!(
        resources.as_provider().script_content("daily.yaml"),
        Some(script_yaml("editable-v1")),
        "运行快照必须从 InstalledPackage 层解析出 1.0.0 脚本内容"
    );
    assert_eq!(
        resources.as_provider().function_file_content("common"),
        Some(FUNC_YAML_V1.to_string()),
        "运行快照必须从 InstalledPackage 层解析出 1.0.0 函数内容"
    );
    assert_eq!(
        resources.as_provider().resolve_template("icon.png"),
        crate::extensions::gamer_yaml::script_v2::validate::TemplateAvail::Found,
        "包内模板必须经 composite 解析可见"
    );
    //   c) 包内 keymap 经 composite 读面可见（GET/list 只读兜底）
    let resp = get_json(&test_app, &sid, &format!("/api/apps/{ANDROID}/resources/keymaps")).await;
    let list = json_body(resp).await;
    assert_eq!(
        list.as_array().unwrap().len(),
        1,
        "本地删空后 keymap 列表必须浮现包内方案: {list}"
    );
    assert_eq!(list[0]["id"], "com.test.game/wasd.yaml");
    let resp = get_json(&test_app, &sid, KEYMAP_URI).await;
    assert_eq!(resp.status(), StatusCode::OK, "{:?}", json_body(resp).await);
    //   d) 包内模板经运行链路同源校验可见：引用包内模板的脚本保存成功，
    //      引用不存在模板的脚本仍 400（证明正向不是恒真）
    let resp = post_json(
        &test_app,
        &sid,
        "/api/apps/com.test.game/resources/scripts",
        serde_json::json!({
            "name": "probe-pkg-template.yaml",
            "content": "steps:\n  - check: icon.png\n",
        }),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "引用包内模板的脚本必须保存成功: {:?}",
        json_body(resp).await
    );
    let resp = post_json(
        &test_app,
        &sid,
        "/api/apps/com.test.game/resources/scripts",
        serde_json::json!({
            "name": "probe-missing-template.yaml",
            "content": "steps:\n  - check: missing-tpl.png\n",
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let resp = delete_json(&test_app, &sid, "/api/apps/com.test.game/resources/scripts/com.test.game%2Fprobe-pkg-template.yaml").await;
    assert_eq!(resp.status(), StatusCode::OK);

    // ⑪ 编辑提取：official.test@1.0.0 → 本地编辑区（受管理条目 1:1 还原）
    let resp = post_json(
        &test_app,
        &sid,
        &format!("/api/app-packages/{PKG_ID}/1.0.0/edit"),
        serde_json::json!({ "android_package": ANDROID }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "{:?}", json_body(resp).await);
    let body = json_body(resp).await;
    assert_eq!(body["metadata"]["version"], "1.0.0");
    for kind in ["scripts", "functions", "templates", "keymaps"] {
        assert_eq!(body["replaced"][kind], 1, "replaced.{kind} 应为 1");
    }
    assert_local_resource_files(&test_app, true);
    assert_eq!(
        std::fs::read_to_string(test_app.dir.join(format!("{ANDROID}/scripts/daily.yaml")))
            .unwrap(),
        script_yaml("editable-v1"),
        "提取必须还原包内脚本字节"
    );

    // ⑫ 修改本地脚本：参数默认值 editable-v1 → editable-v2（参数化输出变更）
    let version = current_version(&test_app, &sid, SCRIPT_URI).await;
    let resp = send(
        &test_app.app,
        req(
            "PUT",
            SCRIPT_URI,
            None,
            &json_headers(sid.to_string()),
            Some(
                serde_json::json!({
                    "content": script_yaml("editable-v2"),
                    "expected_version": version,
                })
                .to_string(),
            ),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "{:?}", json_body(resp).await);

    // ⑬ 修改本地函数：返回值 true → false + 参数默认值变更（返回值仅布尔、
    //    无独立 API 观察面，参数默认值经函数测试 resolved_args 断言）
    let version = current_version(&test_app, &sid, FUNC_URI).await;
    let resp = send(
        &test_app.app,
        req(
            "PUT",
            FUNC_URI,
            None,
            &json_headers(sid.to_string()),
            Some(
                serde_json::json!({
                    "content": FUNC_YAML_V2,
                    "expected_version": version,
                })
                .to_string(),
            ),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "{:?}", json_body(resp).await);

    // ⑭ 立即再运行：必须取到修改后的 EditableLocal 内容（对比 ⑥/⑩ 的 v1 输出），
    //    且包内 1.0.0 字节保持不可变
    let submitted = run_script_to_success(&test_app, &sid).await;
    assert_eq!(
        submitted["resolved_args"]["banner"], "editable-v2",
        "EditableLocal 必须胜过 InstalledPackage 同名脚本"
    );
    let resp = post_json(
        &test_app,
        &sid,
        "/api/runs",
        serde_json::json!({
            "runner_id": "gamer.yaml",
            "entrypoint": "com.test.game/common.yaml#greet",
            "device_id": DEVICE,
            "payload": {},
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED, "{:?}", json_body(resp).await);
    let body = json_body(resp).await;
    let run_id = body["run_id"].as_str().expect("run_id").to_string();
    wait_run_state(&test_app, &sid, &run_id, "success").await;
    assert_eq!(
        body["resolved_args"]["who"], "caller-v2",
        "EditableLocal 必须胜过 InstalledPackage 同名函数库"
    );
    assert_eq!(
        std::fs::read_to_string(
            test_app
                .dir
                .join("app-packages/official.test/1.0.0/scripts/daily.yaml")
        )
        .unwrap(),
        script_yaml("editable-v1"),
        "已安装包字节必须保持不可变"
    );

    // ⑮ 工作区版本改为 1.0.1
    let bump = put_json(
        &test_app,
        &sid,
        &format!("/api/workspace/{ANDROID}"),
        serde_json::json!({
            "id": PKG_ID,
            "version": "1.0.1",
            "android_packages": [ANDROID]
        }),
    )
    .await;
    assert_eq!(bump.status(), StatusCode::OK, "{:?}", json_body(bump).await);
    assert_eq!(json_body(bump).await["metadata"]["version"], "1.0.1");

    // ⑯ 再次导出 1.0.1（携带 ⑫⑬ 的修改内容）
    let (archive_v2, sha_v2) = export_archive(&test_app, &sid, "official.test-1.0.1.gamerpkg").await;
    assert_ne!(sha_v1, sha_v2, "内容变更后归档摘要必须变化");

    // ⑰ 导入 1.0.1 → 安装并激活
    let installed = install_archive(&test_app, &sid, archive_v2, Some(&sha_v2)).await;
    assert_eq!(installed["id"], PKG_ID);
    assert_eq!(installed["active_version"], "1.0.1", "新版本安装即切换激活");

    // ⑱ 列表断言：1.0.0 仍存在、1.0.1 已激活
    let resp = get_json(&test_app, &sid, "/api/app-packages").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    let packages = body["packages"].as_array().unwrap();
    let entry = packages
        .iter()
        .find(|p| p["id"] == PKG_ID)
        .expect("official.test 必须在已装列表中");
    assert_eq!(entry["active_version"], "1.0.1");
    let versions: Vec<&str> = entry["versions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["version"].as_str().unwrap())
        .collect();
    assert!(
        versions.contains(&"1.0.0") && versions.contains(&"1.0.1"),
        "两个版本都必须保留: {versions:?}"
    );

    // 追加 1：同版本禁止覆盖——再导入 1.0.0 归档 → 409
    let resp = send(
        &test_app.app,
        req_bytes(
            "POST",
            "/api/app-packages/install",
            None,
            &[
                (header::COOKIE.to_string(), sid.clone()),
                (
                    header::CONTENT_TYPE.to_string(),
                    "application/zip".to_string(),
                ),
                ("X-Expected-Sha256".to_string(), sha_v1.clone()),
            ],
            archive_v1,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT, "{:?}", json_body(resp).await);
}
