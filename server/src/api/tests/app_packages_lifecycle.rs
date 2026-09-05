// App Package 完整生命周期 E2E（收尾计划书 §13 / P11.8），按节组织：
//
// - §13.1 主链十四步：创建编辑区 → 编辑资源 → 运行 → 导出 → 删除本地 →
//   安装 → 运行 Installed → Edit → 恢复 Editable → 修改 → 再运行 →
//   再次导出（+ ⑮⑯⑰ 版本迭代与列表对账），单测试顺序覆盖；
// - §13.2 六类资源：scripts/functions/templates/keymaps/presets/resources
//   全部走 REST 创建 → 导出 → 安装 → 内容逐字节一致 → edit 提取还原；
// - §13.3 composite 三层优先级：EditableLocal > UserOverride > InstalledPackage；
// - §13.4 编辑恢复正确性：manifest/metadata/六类完整/hash 重算对账；
// - §13.5 安装覆盖：同 package_id+version 重装 = overwrite（P11.8 唯一
//   行为修改点，旧行为 409 AlreadyInstalled）；
// - 任务依赖联动：卸载最后一版 → 既有任务挂起 + presets 发布记录保留。
//
// include 于 api/tests.rs 的 sec_tests 模块内，复用其 build_app_with_executor /
// login / post_json / valid_template_png 等装配助手。
//
// 无头环境（无 adb / 无真机）：运行生命周期用 harness 假执行器
// （build_app_with_executor + OkExecutor）驱动到终态，resolver 层语义不受
// 影响——运行提交前的参数解析（202 响应的 resolved_args 探针，参数默认值
// 即「参数化输出」）与引擎运行快照（RunSnapshot::capture）都走
// EditableLocal > UserOverride > InstalledPackage 的 composite 缝，内容属于
// 哪一层由此断言。
//
// 已知实现边界（按实际行为断言，不放宽语义）：统一执行入口 /api/runs 的
// 脚本存在性前置校验只读本地编辑区（ResourceStore::get_text），纯包内脚本
// 在本地删空后手动运行返回结构化 not_found；包内容来源改经引擎运行快照 +
// composite 资源面（keymap GET/list、脚本保存期模板校验）断言。
use super::*;

const ANDROID: &str = "com.test.game";
const PKG_ID: &str = "official.test";
const SCRIPT_URI: &str = "/api/apps/com.test.game/resources/scripts/com.test.game%2Fdaily.yaml";
const FUNC_URI: &str = "/api/apps/com.test.game/resources/functions/com.test.game%2Fcommon.yaml";
const KEYMAP_URI: &str = "/api/apps/com.test.game/resources/keymaps/com.test.game%2Fwasd.yaml";
const PRESET_URI: &str = "/api/apps/com.test.game/resources/presets/com.test.game%2Fdaily.yaml";
const TEMPLATE_URI: &str = "/api/apps/com.test.game/resources/templates/icon.png";
const RESOURCE_URI: &str = "/api/apps/com.test.game/resources/resources/asset.png";
const DEVICE: &str = "device-e2e";

/// 六类资源的（kind 目录, 文件名）清单：§13.2 的覆盖基线，创建/删除/
/// 安装内容对账/edit 还原共用同一份。
const SIX_KIND_FILES: [(&str, &str); 6] = [
    ("scripts", "daily.yaml"),
    ("functions", "common.yaml"),
    ("templates", "icon.png"),
    ("keymaps", "wasd.yaml"),
    ("presets", "daily.yaml"),
    ("resources", "asset.png"),
];

const KEYMAP_YAML: &str = "version: 1\nname: wasd\nbindings:\n  - key: KeyW\n    action:\n      type: tap\n      at: [0.5, 0.5]\n";
const FUNC_YAML_V1: &str = "greet:\n  params:\n    - 'text:who:称呼:\"caller-v1\"'\n  steps:\n    - log: 你好, $who\n    - return: true\n";
const FUNC_YAML_V2: &str = "greet:\n  params:\n    - 'text:who:称呼:\"caller-v2\"'\n  steps:\n    - log: 你好, $who\n    - return: false\n";
/// 工作区任务预设：经导出/安装/激活发布为任务预设（id = pkg:<包>/<名>）。
const PRESET_YAML: &str = "name: daily\nrunner_id: gamer.yaml\nentrypoint: run\npayload: {}\nschedule:\n  kind: cron\n  value:\n    expression: \"0 8 * * *\"\n";

/// 脚本源码：参数默认值（banner）即「参数化输出」探针——它被写进 log 步骤并
/// 传给 func 步骤；运行端点 202 响应的 resolved_args 暴露当前生效值，脚本
/// 内容属哪一层（EditableLocal / InstalledPackage）由此可断言。
fn script_yaml(banner_default: &str) -> String {
    format!(
        "params:\n  - 'text:banner:横幅:\"{banner_default}\"'\nsteps:\n  - log: $banner\n  - func: common/greet\n    args:\n      who: $banner\n"
    )
}

/// 无 func 步骤的脚本源码（ §13.3/§13.5/任务链路不放函数库，保存校验的
/// func 引用解析不会因引用悬空而 400）。
fn plain_script_yaml(banner_default: &str) -> String {
    format!(
        "params:\n  - 'text:banner:横幅:\"{banner_default}\"'\nsteps:\n  - log: $banner\n"
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

async fn login_session(t: &TestApp) -> String {
    first_cookie_pair(&cookie_of(&login(&t.app).await))
}

/// ① 创建编辑区：初始化工作区元数据（package.toml，format_version=2）。
async fn init_workspace(t: &TestApp, sid: &str, version: &str) {
    let init = put_json(
        t,
        sid,
        &format!("/api/workspace/{ANDROID}"),
        serde_json::json!({
            "id": PKG_ID,
            "version": version,
            "android_packages": [ANDROID]
        }),
    )
    .await;
    assert_eq!(init.status(), StatusCode::OK, "{:?}", json_body(init).await);
    assert!(t.dir.join(format!("{ANDROID}/package.toml")).is_file());
}

/// GET /api/workspace 的六目录统计。
async fn workspace_stats(t: &TestApp, sid: &str) -> serde_json::Value {
    let resp = get_json(t, sid, &format!("/api/workspace/{ANDROID}")).await;
    assert_eq!(resp.status(), StatusCode::OK, "{:?}", json_body(resp).await);
    json_body(resp).await["stats"].clone()
}

/// 经 REST 创建六类资源各一份（文本 kind 收 JSON、字节 kind 收原始字节 +
/// ?name=），返回「相对路径 → 本地落盘字节」映射（模板/资源类经服务端重编码，
/// 后续内容对账必须以落盘字节为准而非上传字节）。
async fn create_six_kinds(t: &TestApp, sid: &str) -> std::collections::BTreeMap<String, Vec<u8>> {
    let created: [(&str, HttpResponse<Body>); 6] = [
        (
            "functions/common.yaml",
            post_json(
                t,
                sid,
                "/api/apps/com.test.game/resources/functions",
                serde_json::json!({ "name": "common", "content": FUNC_YAML_V1 }),
            )
            .await,
        ),
        (
            "scripts/daily.yaml",
            post_json(
                t,
                sid,
                "/api/apps/com.test.game/resources/scripts",
                serde_json::json!({
                    "name": "daily.yaml",
                    "content": script_yaml("editable-v1"),
                }),
            )
            .await,
        ),
        (
            "templates/icon.png",
            send(
                &t.app,
                req_bytes(
                    "POST",
                    "/api/apps/com.test.game/resources/templates?name=icon.png",
                    None,
                    &[
                        (header::COOKIE.to_string(), sid.to_string()),
                        (header::CONTENT_TYPE.to_string(), "image/png".to_string()),
                    ],
                    valid_template_png(),
                ),
            )
            .await,
        ),
        (
            "keymaps/wasd.yaml",
            post_json(
                t,
                sid,
                "/api/apps/com.test.game/resources/keymaps",
                serde_json::json!({ "name": "wasd", "content": KEYMAP_YAML }),
            )
            .await,
        ),
        (
            "presets/daily.yaml",
            post_json(
                t,
                sid,
                "/api/apps/com.test.game/resources/presets",
                serde_json::json!({ "name": "daily", "content": PRESET_YAML }),
            )
            .await,
        ),
        (
            "resources/asset.png",
            send(
                &t.app,
                req_bytes(
                    "POST",
                    "/api/apps/com.test.game/resources/resources?name=asset.png",
                    None,
                    &[
                        (header::COOKIE.to_string(), sid.to_string()),
                        (header::CONTENT_TYPE.to_string(), "image/png".to_string()),
                    ],
                    valid_template_png(),
                ),
            )
            .await,
        ),
    ];
    let mut files = std::collections::BTreeMap::new();
    for (relative, resp) in created {
        assert_eq!(
            resp.status(),
            StatusCode::CREATED,
            "创建 {relative} 失败: {:?}",
            json_body(resp).await
        );
        let path = relative.split('/').fold(t.dir.join(ANDROID), |acc, part| acc.join(part));
        files.insert(relative.to_string(), std::fs::read(&path).unwrap());
    }
    files
}

/// 经 REST 删除本地六类资源，断言全部成功且工作区统计归零。
async fn delete_six_kinds(t: &TestApp, sid: &str) {
    for (kind, name) in SIX_KIND_FILES {
        let uri = match kind {
            // 文本 kind 的 id 带分区前缀；字节 kind 是分区内裸文件名
            "templates" => format!("/api/apps/{ANDROID}/resources/templates/{name}"),
            "resources" => format!("/api/apps/{ANDROID}/resources/resources/{name}"),
            other => format!(
                "/api/apps/{ANDROID}/resources/{other}/{}",
                urlencoded_kind_id(name)
            ),
        };
        let resp = delete_json(t, sid, &uri).await;
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "删除 {kind}/{name} 失败: {:?}",
            json_body(resp).await
        );
    }
    let stats = workspace_stats(t, sid).await;
    for kind in ["scripts", "functions", "templates", "keymaps", "presets", "resources"] {
        assert_eq!(stats[kind], 0, "删空后 {kind} 统计应为 0: {stats}");
    }
    // 本地视角：文本/字节 kind 全部不可见
    for uri in [SCRIPT_URI, FUNC_URI, KEYMAP_URI, PRESET_URI, TEMPLATE_URI, RESOURCE_URI] {
        let resp = get_json(t, sid, uri).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{uri} 删空后应 404");
    }
}

fn urlencoded_kind_id(name: &str) -> String {
    format!("{ANDROID}%2F{name}")
}

/// 收集目录下全部文件的「相对路径 → 字节」（相对 root，路径统一 `/`）。
fn collect_files(
    root: &std::path::Path,
    dir: &std::path::Path,
    out: &mut std::collections::BTreeMap<String, Vec<u8>>,
) {
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

/// 已安装版本目录的资源文件（manifest.toml / install.json 是安装侧自产文件，
/// 不参与内容对账）。
fn installed_version_files(
    t: &TestApp,
    package_id: &str,
    version: &str,
) -> std::collections::BTreeMap<String, Vec<u8>> {
    let root = t.dir.join("app-packages").join(package_id).join(version);
    let mut files = std::collections::BTreeMap::new();
    collect_files(&root, &root, &mut files);
    files.remove("manifest.toml");
    files.remove("install.json");
    files
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

/// 归档必须包含的条目名列表（§13.2 六类资源进包证据）。
fn assert_archive_contains(archive: &[u8], wanted: &[&str]) {
    let mut reader = zip::ZipArchive::new(std::io::Cursor::new(archive)).unwrap();
    let names: Vec<String> = (0..reader.len())
        .map(|index| reader.by_index_raw(index).unwrap().name().to_string())
        .collect();
    for entry in wanted {
        assert!(
            names.iter().any(|name| name == entry),
            "归档缺少条目 {entry}: {names:?}"
        );
    }
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

/// 更新文本资源（expected_version 版本门禁），断言 200。
async fn put_text_resource(t: &TestApp, sid: &str, uri: &str, content: String) {
    let version = current_version(t, sid, uri).await;
    let resp = send(
        &t.app,
        req(
            "PUT",
            uri,
            None,
            &json_headers(sid.to_string()),
            Some(
                serde_json::json!({ "content": content, "expected_version": version }).to_string(),
            ),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "{:?}", json_body(resp).await);
}

/// 引擎运行快照（composite 三层合并）里的脚本源码——包内容来源的运行面探针。
fn composite_script_source(t: &TestApp, script_key: &str) -> Option<String> {
    let cfg = crate::config::Config {
        data_dir: t.dir.clone(),
        ..Default::default()
    };
    let store = crate::resources::ResourceStore::open(&cfg).unwrap();
    let snapshot = crate::extensions::gamer_yaml::engine::snapshot::RunSnapshot::capture(&store, ANDROID)
        .unwrap();
    let app = crate::core::AppContext::new(
        crate::core::DeviceId::new(DEVICE).unwrap(),
        crate::core::AndroidPackageName::new(ANDROID).unwrap(),
        Some(crate::core::AppPackageId::new(ANDROID).unwrap()),
    );
    let resources =
        crate::extensions::gamer_yaml::engine::snapshot::RunResources::new(&snapshot, &store, app);
    resources.as_provider().script_content(script_key)
}

/// 按来源包查询已发布的任务预设数量。
async fn published_preset_count(t: &TestApp, sid: &str) -> usize {
    let resp = get_json(
        t,
        sid,
        &format!("/api/task-presets?app_package={PKG_ID}"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "{:?}", json_body(resp).await);
    json_body(resp).await.as_array().unwrap().len()
}

// ---------------------------------------------------------------------------
// §13.1 + §13.2 + §13.4：主链十四步（六类资源 / 编辑恢复对账）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn app_package_full_lifecycle_workspace_export_install_edit_rerelease() {
    let test_app = build_app_with_executor(
        "apklifecycle",
        test_credential("admin123"),
        Default::default(),
        std::sync::Arc::new(OkExecutor),
    );
    let sid = login_session(&test_app).await;

    // ① 创建编辑区：初始化工作区元数据（package.toml，format_version=2）
    init_workspace(&test_app, &sid, "1.0.0").await;
    let ws = get_json(&test_app, &sid, &format!("/api/workspace/{ANDROID}")).await;
    assert_eq!(json_body(ws).await["metadata"]["format_version"], 2);

    // ② 编辑资源（§13.2）：六类各一份，全部走 REST 创建；工作区统计 1/1/1/1/1/1
    let created_files = create_six_kinds(&test_app, &sid).await;
    let stats = workspace_stats(&test_app, &sid).await;
    for kind in ["scripts", "functions", "templates", "keymaps", "presets", "resources"] {
        assert_eq!(stats[kind], 1, "创建后 {kind} 统计应为 1: {stats}");
    }

    // ③ 运行：EditableLocal 层生效 → success；resolved_args 暴露本地参数默认值
    let submitted = run_script_to_success(&test_app, &sid).await;
    assert_eq!(
        submitted["resolved_args"]["banner"], "editable-v1",
        "运行提交解析必须取到本地编辑区脚本内容"
    );

    // ④ 导出 Package：official.test@1.0.0，六类资源条目全部进包（§13.2）
    let (archive_v1, sha_v1) = export_archive(&test_app, &sid, "official.test-1.0.0.gamerpkg").await;
    assert_archive_contains(
        &archive_v1,
        &[
            "scripts/daily.yaml",
            "functions/common.yaml",
            "templates/icon.png",
            "keymaps/wasd.yaml",
            "presets/daily.yaml",
            "resources/asset.png",
        ],
    );

    // ⑤ 删除本地编辑区：六类资源经 REST 删空，工作区统计归零
    delete_six_kinds(&test_app, &sid).await;

    // ⑥ 安装 Package（带 X-Expected-Sha256）→ 自动激活 1.0.0；包内六类文件与
    //    创建时落盘字节逐字节一致（§13.2「内容一致」）；包内预设经激活发布
    let installed = install_archive(&test_app, &sid, archive_v1.clone(), Some(&sha_v1)).await;
    assert_eq!(installed["id"], PKG_ID);
    assert_eq!(installed["active_version"], "1.0.0", "安装即激活");
    assert_eq!(
        installed["versions"][0]["sha256"], sha_v1,
        "install.json 必须记录归档 SHA-256"
    );
    assert_eq!(
        installed_version_files(&test_app, PKG_ID, "1.0.0"),
        created_files,
        "包内资源必须与创建时的工作区文件逐字节一致（六类齐全）"
    );
    assert_eq!(
        published_preset_count(&test_app, &sid).await,
        1,
        "包内 presets/daily.yaml 必须经激活发布为任务预设"
    );

    // ⑦ 运行 Installed Package：
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
    assert_eq!(
        composite_script_source(&test_app, "daily.yaml"),
        Some(script_yaml("editable-v1")),
        "运行快照必须从 InstalledPackage 层解析出 1.0.0 脚本内容"
    );
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

    // ⑧ Edit → 恢复到 Editable：official.test@1.0.0 → 本地编辑区（受管理条目
    //    1:1 还原，§13.4 资源完整：六类不丢）
    let resp = post_json(
        &test_app,
        &sid,
        &format!("/api/app-packages/{PKG_ID}/1.0.0/edit"),
        serde_json::json!({ "android_package": ANDROID }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "{:?}", json_body(resp).await);
    let body = json_body(resp).await;
    // §13.4 manifest 正确 / metadata 正确
    assert_eq!(body["android_package"], ANDROID);
    assert_eq!(body["metadata"]["format_version"], 2);
    assert_eq!(body["metadata"]["id"], PKG_ID);
    assert_eq!(body["metadata"]["version"], "1.0.0");
    assert_eq!(body["metadata"]["android_packages"][0], ANDROID);
    for (kind, _) in SIX_KIND_FILES {
        assert_eq!(body["replaced"][kind], 1, "replaced.{kind} 应为 1");
    }
    let stats = workspace_stats(&test_app, &sid).await;
    for kind in ["scripts", "functions", "templates", "keymaps", "presets", "resources"] {
        assert_eq!(stats[kind], 1, "edit 还原后 {kind} 统计应为 1: {stats}");
    }
    // §13.4 资源完整：本地六类文件与创建时字节一致
    let mut restored_files = std::collections::BTreeMap::new();
    let ws_root = test_app.dir.join(ANDROID);
    collect_files(&ws_root, &ws_root, &mut restored_files);
    for (kind, name) in SIX_KIND_FILES {
        let relative = format!("{kind}/{name}");
        assert_eq!(
            restored_files.remove(relative.as_str()),
            Some(created_files[relative.as_str()].clone()),
            "edit 必须逐字节还原 {relative}"
        );
    }
    // §13.4 metadata：package.toml 与 manifest 固定字段序列化一致
    assert_eq!(
        std::fs::read_to_string(test_app.dir.join(format!("{ANDROID}/package.toml"))).unwrap(),
        format!(
            "format_version = 2\nid = \"{PKG_ID}\"\nversion = \"1.0.0\"\n\n[android]\npackages = [\"{ANDROID}\"]\n"
        )
    );

    // ⑨ §13.4 hash 重算对账：PackageBuilder 可复现打包（固定 mtime + 排序），
    //    提取出的工作区再导出必须与原归档逐字节同摘要——同时证明 manifest、
    //    六类资源与元数据在 导出→安装→edit→再导出 全链无损耗
    let (archive_again, sha_again) =
        export_archive(&test_app, &sid, "official.test-1.0.0.gamerpkg").await;
    assert_eq!(sha_again, sha_v1, "edit 还原后重导出必须与原归档同摘要");
    assert_eq!(archive_again, archive_v1, "归档字节必须完全一致");

    // ⑩ 修改本地脚本：参数默认值 editable-v1 → editable-v2（参数化输出变更）
    put_text_resource(&test_app, &sid, SCRIPT_URI, script_yaml("editable-v2")).await;

    // ⑪ 修改本地函数：返回值 true → false + 参数默认值变更（返回值仅布尔、
    //    无独立 API 观察面，参数默认值经函数测试 resolved_args 断言）
    put_text_resource(&test_app, &sid, FUNC_URI, FUNC_YAML_V2.to_string()).await;

    // ⑫ 立即再运行：必须取到修改后的 EditableLocal 内容（对比 ③/⑦ 的 v1 输出），
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

    // ⑬ 工作区版本改为 1.0.1 并再次导出（携带 ⑩⑪ 的修改内容）
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
    let (archive_v2, sha_v2) = export_archive(&test_app, &sid, "official.test-1.0.1.gamerpkg").await;
    assert_ne!(sha_v1, sha_v2, "内容变更后归档摘要必须变化");

    // ⑭ 导入 1.0.1 → 安装并激活；列表断言：1.0.0 仍存在、1.0.1 已激活
    let installed = install_archive(&test_app, &sid, archive_v2, Some(&sha_v2)).await;
    assert_eq!(installed["id"], PKG_ID);
    assert_eq!(installed["active_version"], "1.0.1", "新版本安装即切换激活");
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

    // ⑮ §13.5（主链尾巴）：同 id+version 重装 1.0.0 归档 → overwrite 成功
    //    （旧行为 409 AlreadyInstalled，P11.8 按 §13.5 简单规则改为覆盖），
    //    双版本都保留，install.json 摘要与重装归档对账
    let reinstalled = install_archive(&test_app, &sid, archive_v1.clone(), Some(&sha_v1)).await;
    assert_eq!(reinstalled["active_version"], "1.0.0", "重装即切换激活");
    assert_eq!(
        reinstalled["versions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["version"] == "1.0.0")
            .expect("1.0.0 仍在版本列表")["sha256"],
        sha_v1,
        "重装后 install.json 必须记录新归档摘要"
    );
    assert_eq!(
        installed_version_files(&test_app, PKG_ID, "1.0.0"),
        created_files,
        "重装必须把 1.0.0 目录整体替换回原内容"
    );
    assert_eq!(
        published_preset_count(&test_app, &sid).await,
        1,
        "重装激活的预设重发布必须是幂等更新，不产生第二行"
    );
}

// ---------------------------------------------------------------------------
// §13.3：composite 三层优先级（EditableLocal > UserOverride > InstalledPackage）
// ---------------------------------------------------------------------------

/// 同一脚本在三层并存时的生效层断言（引擎运行快照 = 引擎真实读路径）：
/// Editable 在场胜 Override；删 Editable 回落 Override；删 Override 回落
/// Installed。UserOverride 没有 REST 面，经产品存储缝
/// （AppPackageStore::write_user_override）构造。
#[tokio::test]
async fn composite_priority_editable_over_user_override_over_installed() {
    let test_app = build_app_with_executor(
        "apklifecycle-prio",
        test_credential("admin123"),
        Default::default(),
        std::sync::Arc::new(OkExecutor),
    );
    let sid = login_session(&test_app).await;

    // 装一个只含 daily.yaml 的包并删掉本地副本 → Installed 是唯一层
    init_workspace(&test_app, &sid, "1.0.0").await;
    let resp = post_json(
        &test_app,
        &sid,
        "/api/apps/com.test.game/resources/scripts",
        serde_json::json!({
            "name": "daily.yaml",
            "content": plain_script_yaml("layer-installed"),
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED, "{:?}", json_body(resp).await);
    let (archive, sha) = export_archive(&test_app, &sid, "official.test-1.0.0.gamerpkg").await;
    delete_json(&test_app, &sid, SCRIPT_URI).await;
    install_archive(&test_app, &sid, archive, Some(&sha)).await;
    assert_eq!(
        composite_script_source(&test_app, "daily.yaml"),
        Some(plain_script_yaml("layer-installed")),
        "本地删空后只有 Installed 层"
    );

    // 写入 UserOverride 层 → 胜过 Installed
    let pkg_store = crate::app_packages::AppPackageStore::new(&test_app.dir);
    let android = crate::app_packages::parse_android_package_name(ANDROID).unwrap();
    let resource_path = crate::app_packages::ResourcePath::parse("scripts/daily.yaml").unwrap();
    pkg_store
        .write_user_override(&android, &resource_path, plain_script_yaml("layer-override").as_bytes())
        .unwrap();
    assert_eq!(
        composite_script_source(&test_app, "daily.yaml"),
        Some(plain_script_yaml("layer-override")),
        "UserOverride 必须胜过 InstalledPackage"
    );

    // Editable 回归（REST 重建本地脚本）→ 胜过 Override
    let resp = post_json(
        &test_app,
        &sid,
        "/api/apps/com.test.game/resources/scripts",
        serde_json::json!({
            "name": "daily.yaml",
            "content": plain_script_yaml("layer-editable"),
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED, "{:?}", json_body(resp).await);
    assert_eq!(
        composite_script_source(&test_app, "daily.yaml"),
        Some(plain_script_yaml("layer-editable")),
        "EditableLocal 必须胜过 UserOverride 与 InstalledPackage"
    );

    // 删 Editable → 回落 Override
    let resp = delete_json(&test_app, &sid, SCRIPT_URI).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        composite_script_source(&test_app, "daily.yaml"),
        Some(plain_script_yaml("layer-override")),
        "删本地编辑区后必须回落到 UserOverride"
    );

    // 删 Override → 回落 Installed
    assert!(
        pkg_store
            .remove_user_override(&android, &resource_path)
            .unwrap(),
        "override 必须存在且被删除"
    );
    assert_eq!(
        composite_script_source(&test_app, "daily.yaml"),
        Some(plain_script_yaml("layer-installed")),
        "删 override 后必须回落到 InstalledPackage"
    );
}

// ---------------------------------------------------------------------------
// §13.5：安装覆盖（同 package_id + version → overwrite / reinstall）
// ---------------------------------------------------------------------------

/// 同 id+version 重装不同内容 → 整体替换版本目录（不 409）；install.json
/// 摘要刷新；激活重发布幂等；运行面吃到重装后的包内容。本测试连同
/// store 单测 `install_is_staged_and_same_version_reinstall_overwrites`
/// 锁定 P11.8 的唯一行为修改点。
#[tokio::test]
async fn install_same_id_version_reinstall_overwrites_in_place() {
    let test_app = build_app_with_executor(
        "apklifecycle-overwrite",
        test_credential("admin123"),
        Default::default(),
        std::sync::Arc::new(OkExecutor),
    );
    let sid = login_session(&test_app).await;

    init_workspace(&test_app, &sid, "1.0.0").await;
    let resp = post_json(
        &test_app,
        &sid,
        "/api/apps/com.test.game/resources/scripts",
        serde_json::json!({
            "name": "daily.yaml",
            "content": plain_script_yaml("first-install"),
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = post_json(
        &test_app,
        &sid,
        "/api/apps/com.test.game/resources/presets",
        serde_json::json!({ "name": "daily", "content": PRESET_YAML }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);

    let (archive_a, sha_a) = export_archive(&test_app, &sid, "official.test-1.0.0.gamerpkg").await;
    install_archive(&test_app, &sid, archive_a, Some(&sha_a)).await;

    // 同版本改内容：本地脚本 v1 → v2（package.toml 版本不 bump）→ 导出 B
    put_text_resource(&test_app, &sid, SCRIPT_URI, plain_script_yaml("reinstalled")).await;
    let (archive_b, sha_b) = export_archive(&test_app, &sid, "official.test-1.0.0.gamerpkg").await;
    assert_ne!(sha_a, sha_b, "同版本不同内容的归档摘要必须不同");

    // 重装 B → 201 overwrite（旧实现在此返回 409）
    let reinstalled = install_archive(&test_app, &sid, archive_b, Some(&sha_b)).await;
    assert_eq!(reinstalled["id"], PKG_ID);
    assert_eq!(reinstalled["active_version"], "1.0.0");
    assert_eq!(
        std::fs::read_to_string(
            test_app
                .dir
                .join("app-packages/official.test/1.0.0/scripts/daily.yaml")
        )
        .unwrap(),
        plain_script_yaml("reinstalled"),
        "重装必须整体替换版本目录内容"
    );
    let resp = get_json(&test_app, &sid, "/api/app-packages").await;
    let entry = json_body(resp).await["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == PKG_ID)
        .unwrap()
        .clone();
    let versions: Vec<&str> = entry["versions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["version"].as_str().unwrap())
        .collect();
    assert_eq!(versions, vec!["1.0.0"], "重装不产生第二个版本目录");
    assert_eq!(entry["versions"][0]["sha256"], sha_b, "install.json 摘要必须刷新");
    assert_eq!(
        published_preset_count(&test_app, &sid).await,
        1,
        "重装激活的预设重发布幂等"
    );

    // 运行面确认：删本地后引擎快照吃到重装后的包内容
    let resp = delete_json(&test_app, &sid, SCRIPT_URI).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        composite_script_source(&test_app, "daily.yaml"),
        Some(plain_script_yaml("reinstalled")),
        "composite 必须取到重装后的包内容"
    );
}

// ---------------------------------------------------------------------------
// 任务依赖联动：卸载最后一版 → 既有任务挂起 + presets 发布记录保留
// ---------------------------------------------------------------------------

/// 卸载包的最后一个版本：绑定该包的 Active 任务挂起（reason =
/// app package unavailable，任务行保留）、无关任务不受影响；包内预设的
/// 发布记录保留；重装激活后预设仍是一行且任务不自动复活。
#[tokio::test]
async fn uninstalling_last_version_suspends_bound_tasks_and_keeps_published_presets() {
    let test_app = build_app_with_executor(
        "apklifecycle-task",
        test_credential("admin123"),
        Default::default(),
        std::sync::Arc::new(OkExecutor),
    );
    let sid = login_session(&test_app).await;

    init_workspace(&test_app, &sid, "1.0.0").await;
    let resp = post_json(
        &test_app,
        &sid,
        "/api/apps/com.test.game/resources/scripts",
        serde_json::json!({
            "name": "daily.yaml",
            "content": plain_script_yaml("task-chain"),
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = post_json(
        &test_app,
        &sid,
        "/api/apps/com.test.game/resources/presets",
        serde_json::json!({ "name": "daily", "content": PRESET_YAML }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let (archive, sha) = export_archive(&test_app, &sid, "official.test-1.0.0.gamerpkg").await;
    install_archive(&test_app, &sid, archive, Some(&sha)).await;

    // 绑定该包的任务 + 无关包的对照任务（ADR-12 形状：app/runner/schedule）
    let bound_body = serde_json::json!({
        "id": "task-bound",
        "name": "包任务",
        "app": {
            "device_id": DEVICE,
            "android_package": ANDROID,
            "content_package": PKG_ID,
        },
        "runner": {
            "runner_id": "gamer.yaml",
            "entrypoint": "com.test.game/daily.yaml",
            "payload": {},
        },
        "schedule": {"provider_id": "cron", "config": {"expression": "0 8 * * *"}},
        "enabled": true,
    });
    let resp = post_json(&test_app, &sid, "/api/tasks", bound_body).await;
    assert_eq!(resp.status(), StatusCode::CREATED, "{:?}", json_body(resp).await);
    assert_eq!(json_body(resp).await["state"], "active");
    let other_body = serde_json::json!({
        "id": "task-other",
        "name": "无关任务",
        "app": {
            "device_id": DEVICE,
            "android_package": "com.other.game",
            "content_package": "official.other",
        },
        "runner": {
            "runner_id": "gamer.yaml",
            "entrypoint": "com.other.game/daily.yaml",
            "payload": {},
        },
        "schedule": {"provider_id": "cron", "config": {"expression": "0 8 * * *"}},
        "enabled": true,
    });
    let resp = post_json(&test_app, &sid, "/api/tasks", other_body).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert_eq!(published_preset_count(&test_app, &sid).await, 1);

    // 卸载最后一版 → 绑定任务挂起、无关任务不动、任务行保留
    let resp = delete_json(&test_app, &sid, &format!("/api/app-packages/{PKG_ID}/1.0.0")).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = get_json(&test_app, &sid, "/api/tasks/task-bound").await;
    let body = json_body(resp).await;
    assert_eq!(body["state"], "suspended", "{body}");
    assert_eq!(body["suspend_reason"], "app package unavailable");
    assert_eq!(
        body["enabled"], false,
        "包卸载挂起同时落 enabled=0（区别于 dependency_missing 保留 enabled 意图）"
    );
    let resp = get_json(&test_app, &sid, "/api/tasks/task-other").await;
    assert_eq!(json_body(resp).await["state"], "active", "无关任务不得被挂起");

    // presets 发布记录保留（卸载只摘注册表，不删预设行）
    assert_eq!(
        published_preset_count(&test_app, &sid).await,
        1,
        "卸载最后一版不得删除已发布的预设记录"
    );

    // 重装激活：预设仍是同一行（幂等更新），挂起的任务不自动复活
    let (archive, sha) = export_archive(&test_app, &sid, "official.test-1.0.0.gamerpkg").await;
    install_archive(&test_app, &sid, archive, Some(&sha)).await;
    assert_eq!(published_preset_count(&test_app, &sid).await, 1);
    let resp = get_json(&test_app, &sid, "/api/tasks/task-bound").await;
    assert_eq!(
        json_body(resp).await["state"], "suspended",
        "重装不自动复活挂起任务（恢复是显式动作）"
    );
}
