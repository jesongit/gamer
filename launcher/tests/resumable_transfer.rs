mod common;
use common::{cleanup, http_server, sha256_hex, unique_root, write_response};
use gamer_launcher::{fetch::FetchOptions, transfer::download};
use std::{
    fs,
    io::Write,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

#[test]
fn interrupted_download_resumes_and_verifies_the_entire_artifact() {
    let root = unique_root("resume");
    let dest = root.join("test.part");
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let addr = http_server(Arc::new(move |request, stream| {
        let request = String::from_utf8_lossy(request);
        if counter.fetch_add(1, Ordering::SeqCst) == 0 {
            write_response(stream, "HTTP/1.1 200 OK\r\nContent-Length: 10\r\nETag: \"a\"\r\nConnection: close\r\n\r\nabcde");
        } else {
            assert!(request.to_ascii_lowercase().contains("range: bytes=5-"));
            assert!(request.to_ascii_lowercase().contains("if-range: \"a\""));
            write_response(stream, "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 5-9/10\r\nContent-Length: 5\r\nConnection: close\r\n\r\nfghij");
        }
    }));
    let url = format!("http://{addr}/a");
    let hash = sha256_hex(b"abcdefghij");
    assert!(download(&url, &dest, &hash, 10, &FetchOptions::default()).is_err());
    assert_eq!(fs::read(&dest).unwrap(), b"abcde");
    assert_eq!(
        download(&url, &dest, &hash, 10, &FetchOptions::default()).unwrap(),
        10
    );
    assert_eq!(fs::read(&dest).unwrap(), b"abcdefghij");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    cleanup(&root);
}

#[test]
fn range_ignored_restarts_instead_of_appending_and_bad_hash_is_discarded() {
    for valid in [true, false] {
        let root = unique_root("range-ignored");
        let dest = root.join("test.part");
        let counter = Arc::new(AtomicUsize::new(0));
        let addr = http_server(Arc::new(move |_, stream| {
            write_response(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\n",
            );
            let _ = stream.write_all(if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                b"abcde"
            } else {
                b"abcdefghij"
            });
        }));
        let url = format!("http://{addr}/a");
        let hash = sha256_hex(if valid { b"abcdefghij" } else { b"0123456789" });
        assert!(download(&url, &dest, &hash, 10, &FetchOptions::default()).is_err());
        let result = download(&url, &dest, &hash, 10, &FetchOptions::default());
        assert_eq!(result.is_ok(), valid);
        assert_eq!(dest.exists(), valid);
        cleanup(&root);
    }
}

#[test]
fn paused_request_does_not_contact_server_or_discard_partial_bytes() {
    let root = unique_root("paused");
    let dest = root.join("test.part");
    fs::write(&dest, b"partial").unwrap();
    let opts = FetchOptions::default();
    opts.control.pause();
    assert!(matches!(
        download("http://127.0.0.1:1/a", &dest, "hash", 99, &opts),
        Err(gamer_launcher::fetch::DownloadError::Paused)
    ));
    assert_eq!(fs::read(&dest).unwrap(), b"partial");
    cleanup(&root);
}

#[test]
fn mismatched_range_is_rejected_without_appending() {
    let root = unique_root("wrong-range");
    let dest = root.join("test.part");
    let calls = Arc::new(AtomicUsize::new(0));
    let addr = http_server(Arc::new(move |_, stream| {
        if calls.fetch_add(1, Ordering::SeqCst) == 0 {
            write_response(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\nabcde",
            );
        } else {
            write_response(stream,"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 4-8/10\r\nContent-Length: 5\r\nConnection: close\r\n\r\nefghi");
        }
    }));
    let url = format!("http://{addr}/a");
    let hash = sha256_hex(b"abcdefghij");
    assert!(download(&url, &dest, &hash, 10, &Default::default()).is_err());
    assert!(matches!(
        download(&url, &dest, &hash, 10, &Default::default()),
        Err(gamer_launcher::fetch::DownloadError::InvalidRange)
    ));
    assert_eq!(fs::read(&dest).unwrap(), b"abcde");
    cleanup(&root);
}
