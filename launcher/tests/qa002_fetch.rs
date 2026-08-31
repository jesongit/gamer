//! QA-002：下载与 seed/cache 故障测试。
//! 覆盖：慢流完成 / 整体超时 / 断流截断 / 404 / 内容 hash 错 / content-length 超长，
//! 全部断言 cache/artifacts 与 runtime 不被污染（无半截 <name>.part、无坏 <name>）；
//! seed 命中同样过 hash 校验且不触网；seed 坏 → 降级 cache/远端；离线全源耗尽。
//! HTTP 服务为 std::net 手写夹具（tests/common），响应完全可控。
//! 进程级代理环境变量不在集成测试中改写（避免并发测试串扰），代理解析逻辑由
//! src/fetch.rs 的单测覆盖（resolve_proxy 注入式查找）。

mod common;

use std::fs;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use common::{cleanup, http_ok, http_server, sha256_hex, unique_root, write_response};
use gamer_launcher::fetch::{self, DownloadError, FetchError, FetchOptions, Obtained};
use gamer_launcher::layout::InstallLayout;

const NAME: &str = "qa002-artifact.zip";

fn opts(overall_ms: u64) -> FetchOptions {
    FetchOptions {
        connect_timeout: Duration::from_secs(2),
        read_timeout: Duration::from_secs(2),
        overall_timeout: Duration::from_millis(overall_ms),
        progress_interval_bytes: 1,
    }
}

fn assert_no_cache_pollution(layout: &InstallLayout, name: &str) {
    let dir = layout.artifacts_dir();
    if dir.is_dir() {
        let leftovers: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter(|n| n.as_str() == name || n.as_str() == format!("{name}.part"))
            .collect();
        assert!(leftovers.is_empty(), "cache 不应被污染: {leftovers:?}");
    }
}

/// 内容 = 100 字节确定模式，返回 (内容, sha256)。
fn payload(seed: u8) -> (Vec<u8>, String) {
    let content = vec![seed; 100];
    let hash = sha256_hex(&content);
    (content, hash)
}

#[test]
fn download_ok_then_cache_hit_without_network() {
    let (layout, root) = setup("ok");
    let (content, hash) = payload(1);
    let len = content.len();
    let contacted = AtomicBool::new(false);
    let flag = Arc::new(contacted);
    let f = flag.clone();
    let addr = http_server(Arc::new(move |_req, stream| {
        f.store(true, Ordering::SeqCst);
        write_response(
            stream,
            &format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                len
            ),
        );
        let _ = stream.write_all(&content);
    }));
    let url = format!("http://{addr}/{NAME}");

    let got = fetch::obtain_artifact(&layout, NAME, &hash, len as u64, Some(&url), &opts(10_000))
        .expect("首次应下载成功");
    assert!(
        matches!(got, Obtained::Downloaded { .. }),
        "首次应为 remote 下载"
    );
    assert_eq!(
        got.path().display().to_string(),
        layout.artifacts_dir().join(NAME).display().to_string()
    );

    // 二次获取必须命中 cache，不再触网（先清掉首次下载留下的触网标记）
    flag.store(false, Ordering::SeqCst);
    let got2 = fetch::obtain_artifact(&layout, NAME, &hash, len as u64, Some(&url), &opts(10_000))
        .expect("二次应命中 cache");
    assert!(
        matches!(got2, Obtained::Cache { .. }),
        "二次应为 cache 命中"
    );
    assert!(
        !flag.load(Ordering::SeqCst),
        "二次获取不应触网（实际访问了 {url}）"
    );
    cleanup(&root);
}

#[test]
fn slow_stream_within_budget_succeeds() {
    let (layout, root) = setup("slow-ok");
    let (content, hash) = payload(2);
    let len = content.len();
    let addr = http_server(Arc::new(move |_req, stream| {
        write_response(
            stream,
            &format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                len
            ),
        );
        for chunk in content.chunks(20) {
            let _ = stream.write_all(chunk);
            let _ = stream.flush();
            std::thread::sleep(Duration::from_millis(80));
        }
    }));
    let url = format!("http://{addr}/{NAME}");
    let got = fetch::obtain_artifact(&layout, NAME, &hash, len as u64, Some(&url), &opts(10_000));
    assert!(got.is_ok(), "预算内的慢流应成功: {:?}", got.err());
    cleanup(&root);
}

#[test]
fn slow_stream_beyond_deadline_times_out_without_pollution() {
    let (layout, root) = setup("slow-timeout");
    let (content, hash) = payload(3);
    let len = content.len();
    let addr = http_server(Arc::new(move |_req, stream| {
        write_response(
            stream,
            &format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                len
            ),
        );
        for chunk in content.chunks(25) {
            let _ = stream.write_all(chunk);
            let _ = stream.flush();
            std::thread::sleep(Duration::from_millis(150));
        }
    }));
    let url = format!("http://{addr}/{NAME}");
    // 整体 deadline 400ms < 4x150ms 分块节奏 → 必超时
    let err = fetch::obtain_artifact(&layout, NAME, &hash, len as u64, Some(&url), &opts(400))
        .expect_err("超过整体 deadline 应失败");
    assert!(
        matches!(err, FetchError::AllSourcesExhausted { .. }),
        "应报全源耗尽（内含超时），实际 {err}"
    );
    assert_no_cache_pollution(&layout, NAME);
    cleanup(&root);
}

#[test]
fn truncated_stream_fails_without_pollution() {
    let (layout, root) = setup("truncated");
    let (content, hash) = payload(4);
    let sent = &content[..40];
    let sent_bytes = sent.to_vec();
    let addr = http_server(Arc::new(move |_req, stream| {
        // 谎报 content-length=100 只发 40 字节即断流
        write_response(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n",
        );
        let _ = stream.write_all(&sent_bytes);
    }));
    let url = format!("http://{addr}/{NAME}");
    let err = fetch::obtain_artifact(
        &layout,
        NAME,
        &hash,
        content.len() as u64,
        Some(&url),
        &opts(10_000),
    )
    .expect_err("断流必须失败");
    // 断流要么报「截断」，要么被 HTTP 栈识别为 body 提前关闭——关键是失败且不污染
    let msg = err.to_string();
    assert!(
        msg.contains("均不可用") && (msg.contains("截断") || msg.contains("closed")),
        "应包含截断/提前关闭原因: {msg}"
    );
    assert_no_cache_pollution(&layout, NAME);
    cleanup(&root);
}

#[test]
fn http_404_fails_without_pollution() {
    let (layout, root) = setup("not-found");
    let (_content, hash) = payload(5);
    let addr = http_server(Arc::new(|_req, stream| {
        write_response(
            stream,
            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
    }));
    let url = format!("http://{addr}/{NAME}");
    let err = fetch::obtain_artifact(&layout, NAME, &hash, 100, Some(&url), &opts(10_000))
        .expect_err("404 必须失败");
    assert!(err.to_string().contains("404"), "应报 404: {err}");
    assert_no_cache_pollution(&layout, NAME);
    cleanup(&root);
}

#[test]
fn wrong_hash_fails_without_pollution() {
    let (layout, root) = setup("wrong-hash");
    let (content, _) = payload(6);
    let (_, expected_hash) = payload(7); // 声明 hash 与实际内容不符
    let len = content.len();
    let body = content.clone();
    let addr = http_server(Arc::new(move |_req, stream| {
        write_response(
            stream,
            &format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            ),
        );
        let _ = stream.write_all(&body);
    }));
    let url = format!("http://{addr}/{NAME}");
    let err = fetch::obtain_artifact(
        &layout,
        NAME,
        &expected_hash,
        len as u64,
        Some(&url),
        &opts(10_000),
    )
    .expect_err("hash 不符必须失败");
    assert!(
        err.to_string().contains("sha256 不符"),
        "应报 hash 不符: {err}"
    );
    assert_no_cache_pollution(&layout, NAME);
    cleanup(&root);
}

#[test]
fn oversized_content_length_rejected_before_body() {
    let (layout, root) = setup("oversized-cl");
    let (_content, hash) = payload(8);
    let contacted = AtomicBool::new(false);
    let flag = Arc::new(contacted);
    let f = flag.clone();
    let addr = http_server(Arc::new(move |_req, stream| {
        f.store(true, Ordering::SeqCst);
        // content-length 远超声明 size：读取 body 前即可判定
        write_response(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: 10000000\r\nConnection: close\r\n\r\n",
        );
    }));
    let url = format!("http://{addr}/{NAME}");
    let err = fetch::obtain_artifact(&layout, NAME, &hash, 100, Some(&url), &opts(10_000))
        .expect_err("content-length 超长必须失败");
    assert!(
        err.to_string().contains("content-length"),
        "应报 content-length 超长: {err}"
    );
    assert_no_cache_pollution(&layout, NAME);
    cleanup(&root);
}

#[test]
fn oversized_body_rejected_mid_stream() {
    // 声明 content-length 正确，但实际持续发数据超过声明 size（谎报流）
    let (layout, root) = setup("oversized-body");
    let (_, hash) = payload(9);
    fs::create_dir_all(layout.artifacts_dir()).unwrap();
    let addr = http_server(Arc::new(move |_req, stream| {
        write_response(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n",
        );
        let _ = stream.write_all(&vec![b'x'; 300]);
    }));
    let url = format!("http://{addr}/{NAME}");
    let err = fetch::download_bounded(
        &url,
        &layout.artifacts_dir().join("direct.part"),
        &hash,
        100,
        &opts(10_000),
    )
    .expect_err("body 超长必须失败");
    // HTTP 栈会把 reader 截到 content-length，谎报流落地为 HashMismatch；
    // 我们自己的逐块上限（OversizedBody）是第二道防线。关键：拒绝且无残留。
    assert!(
        matches!(
            err,
            DownloadError::OversizedBody { .. } | DownloadError::HashMismatch { .. }
        ),
        "应拒绝谎报流，实际 {err}"
    );
    assert!(
        !layout.artifacts_dir().join("direct.part").exists(),
        ".part 不应残留"
    );
    cleanup(&root);
}

#[test]
fn seed_hit_skips_remote_and_verifies_hash() {
    let (layout, root) = setup("seed-hit");
    let (content, hash) = payload(10);
    let len = content.len();
    fs::create_dir_all(layout.seeds_dir()).unwrap();
    fs::write(layout.seeds_dir().join(NAME), &content).unwrap();
    let flag = Arc::new(AtomicBool::new(false));
    let f = flag.clone();
    let addr = http_server(Arc::new(move |_req, stream| {
        f.store(true, Ordering::SeqCst);
        write_response(stream, &http_ok("{}"));
    }));
    let url = format!("http://{addr}/{NAME}");
    let got = fetch::obtain_artifact(&layout, NAME, &hash, len as u64, Some(&url), &opts(10_000))
        .expect("seed 命中应成功");
    assert!(matches!(got, Obtained::Seed { .. }), "应为 seed 命中");
    assert!(!flag.load(Ordering::SeqCst), "seed 命中不应触网");

    // seed 内容被篡改 → 校验失败必须拒绝（seed 也要过 hash 门禁）
    fs::write(layout.seeds_dir().join(NAME), vec![b'z'; 100]).unwrap();
    let err = fetch::obtain_artifact(&layout, NAME, &hash, len as u64, None, &opts(10_000))
        .expect_err("坏 seed 必须被拒绝");
    assert!(
        err.to_string().contains("seed"),
        "应报 seed 校验失败: {err}"
    );
    cleanup(&root);
}

#[test]
fn corrupt_seed_falls_through_to_remote() {
    let (layout, root) = setup("seed-fallthrough");
    let (content, hash) = payload(11);
    let len = content.len();
    fs::create_dir_all(layout.seeds_dir()).unwrap();
    fs::write(layout.seeds_dir().join(NAME), vec![b'q'; 100]).unwrap(); // 坏 seed
    let body = content.clone();
    let addr = http_server(Arc::new(move |_req, stream| {
        write_response(
            stream,
            &format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            ),
        );
        let _ = stream.write_all(&body);
    }));
    let url = format!("http://{addr}/{NAME}");
    let got = fetch::obtain_artifact(&layout, NAME, &hash, len as u64, Some(&url), &opts(10_000))
        .expect("坏 seed 应回落远端成功");
    assert!(
        matches!(got, Obtained::Downloaded { .. }),
        "应从远端下载入库"
    );
    // seed 原样保留（只读区），cache 入库正确
    assert_eq!(fs::read(layout.seeds_dir().join(NAME)).unwrap().len(), 100);
    assert_eq!(
        fs::read(layout.artifacts_dir().join(NAME)).unwrap(),
        content
    );
    cleanup(&root);
}

#[test]
fn offline_with_no_sources_reports_exhausted() {
    let (layout, root) = setup("offline");
    let (_content, hash) = payload(12);
    let err = fetch::obtain_artifact(&layout, NAME, &hash, 100, None, &opts(10_000))
        .expect_err("离线且无 seed/cache 必须失败");
    assert!(matches!(err, FetchError::AllSourcesExhausted { .. }));
    assert_no_cache_pollution(&layout, NAME);
    cleanup(&root);
}

#[test]
fn corrupt_cache_file_is_removed_and_replaced() {
    let (layout, root) = setup("cache-corrupt");
    let (content, hash) = payload(13);
    let len = content.len();
    fs::create_dir_all(layout.artifacts_dir()).unwrap();
    fs::write(layout.artifacts_dir().join(NAME), vec![b'c'; 100]).unwrap(); // 坏 cache
    let body = content.clone();
    let addr = http_server(Arc::new(move |_req, stream| {
        write_response(
            stream,
            &format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            ),
        );
        let _ = stream.write_all(&body);
    }));
    let url = format!("http://{addr}/{NAME}");
    let got = fetch::obtain_artifact(&layout, NAME, &hash, len as u64, Some(&url), &opts(10_000))
        .expect("坏 cache 应回落远端");
    assert!(matches!(got, Obtained::Downloaded { .. }));
    assert_eq!(
        fs::read(layout.artifacts_dir().join(NAME)).unwrap(),
        content
    );
    cleanup(&root);
}

fn setup(tag: &str) -> (InstallLayout, std::path::PathBuf) {
    let root = unique_root(tag);
    let layout = InstallLayout { root: root.clone() };
    (layout, root)
}
