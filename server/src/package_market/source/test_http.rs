//! An owned loopback fixture, with no device manager, service startup or user data.
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::JoinHandle,
};

pub(crate) struct TestServer {
    pub base: String,
    replies: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    pub calls: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
impl TestServer {
    pub fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let replies = Arc::new(Mutex::new(BTreeMap::<String, Vec<u8>>::new()));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (r, c, s) = (replies.clone(), calls.clone(), stop.clone());
        let worker = std::thread::spawn(move || {
            for stream in listener.incoming() {
                if s.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(mut stream) = stream else { break };
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                    .unwrap();
                let mut buffer = [0; 8192];
                let Ok(n) = stream.read(&mut buffer) else {
                    continue;
                };
                let request = String::from_utf8_lossy(&buffer[..n]);
                let path = request.split_whitespace().nth(1).unwrap_or("").to_string();
                c.lock().unwrap().push(path.clone());
                let reply = r.lock().unwrap().get(&path).cloned().unwrap_or_else(|| {
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .to_vec()
                });
                let _ = stream.write_all(&reply);
            }
        });
        Self {
            base,
            replies,
            calls,
            stop,
            worker: Some(worker),
        }
    }
    pub fn put(&self, path: &str, bytes: &[u8]) {
        let mut reply = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",bytes.len()).into_bytes();
        reply.extend(bytes);
        self.replies.lock().unwrap().insert(path.into(), reply);
    }
    pub fn redirect(&self, path: &str, destination: &str) {
        self.replies.lock().unwrap().insert(path.into(),format!("HTTP/1.1 302 Found\r\nLocation: {destination}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes());
    }
}
impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.base.trim_start_matches("http://"));
        let _ = self.worker.take().unwrap().join();
    }
}
