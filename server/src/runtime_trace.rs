//! Bounded, runner-neutral temporary image evidence. No device or viewer loop is
//! started here. Producers submit only frames they actually consumed or explicit
//! boundary captures; a dedicated worker performs hashing, lossless PNG encoding
//! and disk IO. Base run events keep their own retention policy.
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::matcher::DecodedFrame;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TraceConfig {
    pub retain_hours: u32,
    pub global_max_bytes: u64,
    pub run_max_bytes: u64,
    pub run_max_images: usize,
    pub queue_capacity: usize,
    pub min_observation_ms: u64,
    pub retained_max_bytes: u64,
    pub retained_max_runs: usize,
}
impl Default for TraceConfig {
    fn default() -> Self {
        Self {
            retain_hours: 24,
            global_max_bytes: 512 * 1024 * 1024,
            run_max_bytes: 64 * 1024 * 1024,
            run_max_images: 1000,
            queue_capacity: 4,
            min_observation_ms: 1000,
            retained_max_bytes: 256 * 1024 * 1024,
            retained_max_runs: 10,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct TraceImage {
    pub seq: u64,
    pub image_id: String,
    pub captured_at: String,
    pub recorded_at: String,
    pub width: u32,
    pub height: u32,
    pub kind: String,
    pub source: Option<crate::capabilities::FrameStamp>,
    pub metadata: Value,
    pub url: String,
    asset: String,
    #[serde(default)]
    template_asset: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct TraceGap {
    pub time: String,
    pub reason: String,
    pub count: u64,
}
#[derive(Clone, Serialize, Deserialize)]
struct Manifest {
    run_id: String,
    status: String,
    enabled: bool,
    started_at: String,
    finished_at: Option<String>,
    bytes: u64,
    #[serde(default)]
    metadata_bytes: u64,
    gaps: Vec<TraceGap>,
    images: Vec<TraceImage>,
    snapshot: Option<Value>,
    #[serde(default)]
    record: Option<Value>,
    #[serde(default)]
    stopped: bool,
    #[serde(default)]
    last_observation_ms: i64,
}
impl Manifest {
    fn new(run_id: &str) -> Self {
        Self {
            run_id: run_id.into(),
            status: "available".into(),
            enabled: true,
            started_at: now(),
            finished_at: None,
            bytes: 512,
            metadata_bytes: 512,
            gaps: vec![],
            images: vec![],
            snapshot: None,
            record: None,
            stopped: false,
            last_observation_ms: 0,
        }
    }
}
#[derive(Clone)]
pub(crate) struct CapturedEvidence {
    pub frame: Arc<DecodedFrame>,
    pub captured_at: String,
    pub source: Option<crate::capabilities::FrameStamp>,
}
type ConsumedEvidence = (CapturedEvidence, Value, Option<Arc<Vec<u8>>>);

struct State {
    runs: HashMap<String, Manifest>,
    retained: HashMap<String, Manifest>,
    last_consumed: HashMap<String, ConsumedEvidence>,
    root_errors: HashSet<String>,
    dirty: HashSet<String>,
}
struct Work {
    run_id: String,
    image_id: String,
    evidence: CapturedEvidence,
    metadata: Value,
    template: Option<Arc<Vec<u8>>>,
}
enum Command {
    Image(Work),
    Wake,
    Flush(mpsc::Sender<()>),
    Shutdown,
}

pub(crate) struct TraceStore {
    root: PathBuf,
    retained_root: PathBuf,
    cfg: TraceConfig,
    state: Arc<Mutex<State>>,
    io: Arc<Mutex<()>>,
    tx: mpsc::SyncSender<Command>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
}
fn now() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id != "."
        && id != ".."
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
}
fn regular_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
}
fn private_dir(path: &Path) -> anyhow::Result<()> {
    if path.exists() {
        anyhow::ensure!(
            regular_dir(path),
            "trace directory is not a regular directory"
        );
    } else {
        std::fs::create_dir_all(path)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
fn atomic_write(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let tmp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(tmp);
    }
    result
}
fn load(root: &Path, recovered: bool) -> HashMap<String, Manifest> {
    let mut runs = HashMap::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let id = entry.file_name().to_string_lossy().into_owned();
            if !valid_id(&id) || !regular_dir(&entry.path()) {
                continue;
            }
            let path = entry.path().join("index.json");
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || metadata.len() > 8 * 1024 * 1024
            {
                continue;
            }
            if let Ok(bytes) = std::fs::read(path) {
                if let Ok(mut run) = serde_json::from_slice::<Manifest>(&bytes) {
                    if run.run_id != id || run.images.len() > 10000 {
                        continue;
                    }
                    if recovered && run.finished_at.is_none() {
                        run.finished_at = Some(now());
                    }
                    runs.insert(id, run);
                }
            }
        }
    }
    runs
}
/// Remove only trace-owned payload names that were not committed into an index.
/// This bounds crash leftovers; foreign files and symlinks are never traversed.
fn recover_payloads(root: &Path, runs: &mut HashMap<String, Manifest>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let id = entry.file_name().to_string_lossy().into_owned();
        if !valid_id(&id) || !regular_dir(&entry.path()) {
            continue;
        }
        let mut expected = HashSet::new();
        if let Some(run) = runs.get(&id) {
            for image in &run.images {
                expected.insert(image.asset.clone());
                if let Some(template) = &image.template_asset {
                    expected.insert(template.clone());
                }
            }
        }
        let Ok(files) = std::fs::read_dir(entry.path()) else {
            continue;
        };
        for file in files.flatten() {
            let name = file.file_name().to_string_lossy().into_owned();
            if (valid_asset(&name) || name.ends_with(".tmp"))
                && !expected.contains(&name)
                && file
                    .file_type()
                    .is_ok_and(|kind| kind.is_file() && !kind.is_symlink())
            {
                let _ = std::fs::remove_file(file.path());
            }
        }
        if let Some(run) = runs.get_mut(&id) {
            let missing = expected
                .iter()
                .filter(|name| {
                    !std::fs::symlink_metadata(entry.path().join(name))
                        .is_ok_and(|m| m.is_file() && !m.file_type().is_symlink())
                })
                .count();
            if missing > 0
                && !run
                    .gaps
                    .iter()
                    .any(|g| g.reason == "image_payload_missing_after_restart")
                && run.gaps.len() < 32
            {
                run.gaps.push(TraceGap {
                    time: now(),
                    reason: "image_payload_missing_after_restart".into(),
                    count: missing as u64,
                });
            }
        }
    }
}
impl TraceStore {
    pub(crate) fn open(data: &Path, mut cfg: TraceConfig) -> anyhow::Result<Arc<Self>> {
        // Safety caps also apply to custom configurations; zero cannot disable bounds.
        cfg.queue_capacity = cfg.queue_capacity.clamp(1, 16);
        cfg.run_max_images = cfg.run_max_images.clamp(1, 10000);
        cfg.retain_hours = cfg.retain_hours.clamp(1, 720);
        cfg.run_max_bytes = cfg.run_max_bytes.clamp(1024, 1024 * 1024 * 1024);
        cfg.global_max_bytes = cfg.global_max_bytes.clamp(1024, 16 * 1024 * 1024 * 1024);
        cfg.run_max_bytes = cfg.run_max_bytes.min(cfg.global_max_bytes);
        cfg.retained_max_bytes = cfg.retained_max_bytes.min(16 * 1024 * 1024 * 1024);
        cfg.retained_max_runs = cfg.retained_max_runs.min(1000);
        let root = data.join("runtime-traces");
        let retained_root = data.join("retained-traces");
        private_dir(&root)?;
        private_dir(&retained_root)?;
        // Interrupted copies were never published. Only the reserved staging
        // namespace is eligible; committed retained runs are never expired.
        for entry in std::fs::read_dir(&retained_root)?.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name
                .strip_prefix("staging-")
                .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
                || !regular_dir(&entry.path())
            {
                continue;
            }
            if let Ok(files) = std::fs::read_dir(entry.path()) {
                for file in files.flatten() {
                    let name = file.file_name().to_string_lossy().into_owned();
                    if (name == "index.json" || valid_asset(&name) || name.ends_with(".tmp"))
                        && file
                            .file_type()
                            .is_ok_and(|kind| kind.is_file() && !kind.is_symlink())
                    {
                        let _ = std::fs::remove_file(file.path());
                    }
                }
            }
            let _ = std::fs::remove_dir(entry.path());
        }
        let mut runs = load(&root, true);
        recover_payloads(&root, &mut runs);
        let dirty = runs.keys().cloned().collect();
        let state = Arc::new(Mutex::new(State {
            runs,
            retained: load(&retained_root, false),
            last_consumed: HashMap::new(),
            root_errors: HashSet::new(),
            dirty,
        }));
        let io = Arc::new(Mutex::new(()));
        let (tx, rx) = mpsc::sync_channel(cfg.queue_capacity);
        let (worker_state, worker_io, worker_root, worker_cfg) =
            (state.clone(), io.clone(), root.clone(), cfg.clone());
        let worker = std::thread::Builder::new()
            .name("gamer-trace-encoder".into())
            .spawn(move || loop {
                let command = rx.recv_timeout(Duration::from_secs(30));
                let _io = worker_io.lock().unwrap_or_else(|p| p.into_inner());
                cleanup(&worker_root, &worker_state, &worker_cfg, Utc::now());
                let stop = match command {
                    Ok(Command::Image(work)) => {
                        process_image(&worker_root, &worker_state, &worker_cfg, work);
                        false
                    }
                    Ok(Command::Flush(reply)) => {
                        persist_dirty(&worker_root, &worker_state);
                        let _ = reply.send(());
                        false
                    }
                    Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => true,
                    _ => false,
                };
                persist_dirty(&worker_root, &worker_state);
                if stop {
                    break;
                }
            })?;
        Ok(Arc::new(Self {
            root,
            retained_root,
            cfg,
            state,
            io,
            tx,
            worker: Mutex::new(Some(worker)),
        }))
    }
    pub(crate) fn register(&self, run_id: &str, finished_at: Option<String>) {
        if !valid_id(run_id) {
            return;
        }
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let run = state
            .runs
            .entry(run_id.into())
            .or_insert_with(|| Manifest::new(run_id));
        if let Some(finished) = finished_at {
            run.finished_at = Some(finished);
            state.last_consumed.remove(run_id);
            state.root_errors.remove(run_id);
        }
        state.dirty.insert(run_id.into());
        drop(state);
        let _ = self.tx.try_send(Command::Wake);
    }
    pub(crate) fn record(&self, run_id: &str, record: Value) {
        if !valid_id(run_id) {
            return;
        }
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let run = state
            .runs
            .entry(run_id.into())
            .or_insert_with(|| Manifest::new(run_id));
        run.record = Some(record);
        state.dirty.insert(run_id.into());
    }
    pub(crate) fn retained_record(&self, run_id: &str) -> Option<Value> {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retained
            .get(run_id)
            .and_then(|run| run.record.clone())
    }
    /// Drop tiny expired indexes when the independent base-history window ends.
    /// Durable copies and all Package/template/sample paths remain untouched.
    pub(crate) fn prune_history(&self, days: u32) {
        if days == 0 {
            return;
        }
        let cutoff = Utc::now() - chrono::Duration::days(days as i64);
        let _io = self.io.lock().unwrap_or_else(|p| p.into_inner());
        let ids: Vec<_> = self
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .runs
            .iter()
            .filter(|(_, r)| {
                r.finished_at
                    .as_deref()
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .is_some_and(|t| t < cutoff)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            if expire_run(&self.root, &self.state, &id).is_err() {
                continue;
            }
            let dir = self.root.join(&id);
            if std::fs::remove_file(dir.join("index.json")).is_err() {
                continue;
            }
            let _ = std::fs::remove_dir(&dir);
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            state.runs.remove(&id);
            state.dirty.remove(&id);
            state.last_consumed.remove(&id);
            state.root_errors.remove(&id);
        }
    }
    pub(crate) fn gap(&self, run_id: &str, reason: &str) {
        if !valid_id(run_id) {
            return;
        }
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        record_gap(&mut state, run_id, reason);
        drop(state);
        let _ = self.tx.try_send(Command::Wake);
    }
    pub(crate) fn configure(&self, metadata: &Value) {
        let Some(run_id) = metadata["run_id"].as_str().filter(|id| valid_id(id)) else {
            return;
        };
        let Some(enabled) = metadata["trace_enabled"].as_bool() else {
            return;
        };
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state
            .runs
            .entry(run_id.into())
            .or_insert_with(|| Manifest::new(run_id))
            .enabled = enabled;
        state.dirty.insert(run_id.into());
        drop(state);
        let _ = self.tx.try_send(Command::Wake);
    }
    pub(crate) fn snapshot(&self, run_id: &str, snapshot: Value) -> anyhow::Result<()> {
        anyhow::ensure!(valid_id(run_id), "invalid run id");
        if serde_json::to_vec(&snapshot)?.len() > 2 * 1024 * 1024 {
            self.gap(run_id, "snapshot_exceeds_2_mib");
            return Ok(());
        }
        let size = serde_json::to_vec(&snapshot)?.len() as u64;
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let total: u64 = state.runs.values().map(|r| r.bytes).sum();
        let run = state
            .runs
            .entry(run_id.into())
            .or_insert_with(|| Manifest::new(run_id));
        // The first snapshot is frozen. Late execution callbacks cannot overwrite it.
        if run.snapshot.is_none() && run.status != "expired" {
            if run.bytes.saturating_add(size) > self.cfg.run_max_bytes
                || total.saturating_add(size) > self.cfg.global_max_bytes
            {
                record_gap(&mut state, run_id, "snapshot_quota_exceeded");
            } else {
                run.bytes += size;
                run.metadata_bytes += size;
                run.snapshot = Some(snapshot);
                state.dirty.insert(run_id.into());
            }
        }
        drop(state);
        let _ = self.tx.try_send(Command::Wake);
        Ok(())
    }
    pub(crate) fn submit(
        &self,
        evidence: CapturedEvidence,
        metadata: Value,
        forced: bool,
        template: Option<Arc<Vec<u8>>>,
    ) -> Option<String> {
        let run_id = metadata["run_id"].as_str()?.to_owned();
        if !valid_id(&run_id) {
            return None;
        }
        if metadata.to_string().len() > 64 * 1024 {
            self.gap(&run_id, "image_metadata_exceeds_64_kib");
            return None;
        }
        let kind = metadata["kind"].as_str().unwrap_or("consumed");
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let run = state
            .runs
            .entry(run_id.clone())
            .or_insert_with(|| Manifest::new(&run_id));
        if let Some(enabled) = metadata["trace_enabled"].as_bool() {
            run.enabled = enabled;
        }
        let disabled = !run.enabled;
        let stopped = run.stopped || run.status == "expired";
        if kind == "consumed" {
            state.last_consumed.insert(
                run_id.clone(),
                (evidence.clone(), metadata.clone(), template.clone()),
            );
        }
        state.dirty.insert(run_id.clone());
        if (disabled && !forced) || stopped {
            return None;
        }
        if evidence.frame.dimensions().0 as u64 * evidence.frame.dimensions().1 as u64 > 16_000_000
        {
            record_gap(&mut state, &run_id, "frame_exceeds_16_megapixels");
            return None;
        }
        drop(state);
        let image_id = uuid::Uuid::new_v4().to_string();
        let work = Work {
            run_id: run_id.clone(),
            image_id: image_id.clone(),
            evidence,
            metadata,
            template,
        };
        match self.tx.try_send(Command::Image(work)) {
            Ok(()) => Some(image_id),
            Err(mpsc::TrySendError::Full(_)) => {
                self.gap(&run_id, "encoding_queue_full");
                None
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                self.gap(&run_id, "encoder_unavailable");
                None
            }
        }
    }
    /// True only once for a root failure. The caller must use a separate fresh
    /// capture; this retained frame keeps its original timestamp and identity.
    pub(crate) fn root_error(&self, metadata: &Value) -> bool {
        let Some(run_id) = metadata["run_id"].as_str().filter(|id| valid_id(id)) else {
            return false;
        };
        let last = {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if !state.root_errors.insert(run_id.into()) {
                return false;
            }
            state.last_consumed.get(run_id).cloned()
        };
        if let Some((last, mut consumed_metadata, template)) = last {
            consumed_metadata["kind"] = "error_last_consumed".into();
            consumed_metadata["error_context"] = metadata.clone();
            if let Some(enabled) = metadata.get("trace_enabled") {
                consumed_metadata["trace_enabled"] = enabled.clone();
            }
            self.submit(last, consumed_metadata, true, template);
        } else {
            self.gap(run_id, "no_last_consumed_frame");
        }
        true
    }
    pub(crate) fn page(&self, run_id: &str, after: u64, limit: usize) -> Value {
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let Some(run) = state
            .retained
            .get(run_id)
            .or_else(|| state.runs.get(run_id))
        else {
            return json!({"run_id":run_id,"status":"unavailable","enabled":true,"bytes":0,"images":[],"gaps":[],"snapshot":null,"next":after,"has_more":false});
        };
        let limit = limit.clamp(1, 100);
        let mut images: Vec<_> = run
            .images
            .iter()
            .filter(|i| i.seq > after)
            .take(limit + 1)
            .cloned()
            .collect();
        let more = images.len() > limit;
        images.truncate(limit);
        let next = images.last().map(|i| i.seq).unwrap_or(after);
        json!({"run_id":run_id,"status":if state.retained.contains_key(run_id) {"retained"} else {&run.status},"enabled":run.enabled,"finished_at":run.finished_at,"bytes":run.bytes,"gaps":run.gaps,"images":images,"next":next,"has_more":more,"snapshot":run.snapshot,"stopped":run.stopped})
    }
    pub(crate) fn read_image(
        &self,
        run_id: &str,
        image_id: &str,
        template: bool,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        anyhow::ensure!(
            valid_id(run_id) && uuid::Uuid::parse_str(image_id).is_ok(),
            "invalid evidence id"
        );
        let _io = self.io.lock().unwrap_or_else(|p| p.into_inner());
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let retained = state.retained.contains_key(run_id);
        let Some(run) = state
            .retained
            .get(run_id)
            .or_else(|| state.runs.get(run_id))
        else {
            return Ok(None);
        };
        anyhow::ensure!(run.status != "expired", "trace_expired");
        let Some(record) = run.images.iter().find(|i| i.image_id == image_id) else {
            return Ok(None);
        };
        let asset = if template {
            let Some(asset) = record.template_asset.as_ref() else {
                return Ok(None);
            };
            asset
        } else {
            &record.asset
        };
        anyhow::ensure!(valid_asset(asset), "invalid evidence asset");
        let root = if retained {
            &self.retained_root
        } else {
            &self.root
        };
        let directory = root.join(run_id);
        let path = directory.join(asset);
        drop(state);
        anyhow::ensure!(
            regular_dir(root) && regular_dir(&directory),
            "invalid evidence directory"
        );
        let info = std::fs::symlink_metadata(&path)?;
        anyhow::ensure!(
            info.is_file()
                && !info.file_type().is_symlink()
                && info.len() <= self.cfg.run_max_bytes,
            "invalid evidence file"
        );
        Ok(Some(std::fs::read(path)?))
    }
    pub(crate) fn retain(&self, run_id: &str) -> anyhow::Result<Value> {
        anyhow::ensure!(valid_id(run_id), "invalid run id");
        self.flush();
        let _io = self.io.lock().unwrap_or_else(|p| p.into_inner());
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.retained.contains_key(run_id) {
            return Ok(json!({"run_id":run_id,"status":"retained"}));
        }
        let run = state
            .runs
            .get(run_id)
            .ok_or_else(|| anyhow::anyhow!("trace_not_found"))?
            .clone();
        anyhow::ensure!(
            run.finished_at.is_some(),
            "only completed traces can be retained"
        );
        anyhow::ensure!(
            run.status != "expired" && !run.images.is_empty(),
            "trace_expired_or_empty"
        );
        let used: u64 = state.retained.values().map(|r| r.bytes).sum();
        anyhow::ensure!(
            state.retained.len() < self.cfg.retained_max_runs
                && used.saturating_add(run.bytes) <= self.cfg.retained_max_bytes,
            "retained_trace_quota_exceeded"
        );
        let destination = self.retained_root.join(run_id);
        let staging = self
            .retained_root
            .join(format!("staging-{}", uuid::Uuid::new_v4()));
        drop(state);
        private_dir(&staging)?;
        let result = (|| -> anyhow::Result<()> {
            let mut assets = HashSet::new();
            for image in &run.images {
                assets.insert(image.asset.clone());
                if let Some(template) = &image.template_asset {
                    assets.insert(template.clone());
                }
            }
            for asset in assets {
                anyhow::ensure!(valid_asset(&asset), "invalid evidence asset");
                anyhow::ensure!(
                    regular_dir(&self.root.join(run_id)),
                    "invalid evidence directory"
                );
                let source = self.root.join(run_id).join(&asset);
                let info = std::fs::symlink_metadata(&source)?;
                anyhow::ensure!(
                    info.is_file() && !info.file_type().is_symlink(),
                    "invalid evidence source"
                );
                atomic_write(&staging.join(asset), &std::fs::read(source)?)?;
            }
            atomic_write(&staging.join("index.json"), &serde_json::to_vec(&run)?)?;
            std::fs::rename(&staging, &destination)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_dir_all(&staging);
        }
        result?;
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retained
            .insert(run_id.into(), run);
        Ok(json!({"run_id":run_id,"status":"retained"}))
    }
    pub(crate) fn flush(&self) {
        let (tx, rx) = mpsc::channel();
        if self.tx.send(Command::Flush(tx)).is_ok() {
            let _ = rx.recv_timeout(Duration::from_secs(30));
        }
    }
}
impl Drop for TraceStore {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
        if let Some(worker) = self.worker.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = worker.join();
        }
    }
}
fn valid_asset(asset: &str) -> bool {
    asset.len() == 68
        && asset.ends_with(".png")
        && asset
            .as_bytes()
            .iter()
            .take(64)
            .all(|b| b.is_ascii_hexdigit())
}
fn record_gap(state: &mut State, run_id: &str, reason: &str) {
    let run = state
        .runs
        .entry(run_id.into())
        .or_insert_with(|| Manifest::new(run_id));
    if let Some(gap) = run
        .gaps
        .iter_mut()
        .find(|g| g.reason.split(':').next() == reason.split(':').next())
    {
        gap.count = gap.count.saturating_add(1);
        gap.time = now();
        gap.reason = reason.chars().take(256).collect();
    } else if run.gaps.len() < 32 {
        run.gaps.push(TraceGap {
            time: now(),
            reason: reason.chars().take(256).collect(),
            count: 1,
        });
    }
    state.dirty.insert(run_id.into());
}
fn persist_dirty(root: &Path, state: &Mutex<State>) {
    let manifests = {
        let mut state = state.lock().unwrap_or_else(|p| p.into_inner());
        let dirty = std::mem::take(&mut state.dirty);
        dirty
            .into_iter()
            .filter_map(|id| state.runs.get(&id).cloned())
            .collect::<Vec<_>>()
    };
    for run in manifests {
        let result = (|| -> anyhow::Result<()> {
            let dir = root.join(&run.run_id);
            private_dir(&dir)?;
            atomic_write(&dir.join("index.json"), &serde_json::to_vec(&run)?)
        })();
        if let Err(error) = result {
            let mut state = state.lock().unwrap_or_else(|p| p.into_inner());
            record_gap(&mut state, &run.run_id, "metadata_write_failed");
            tracing::warn!(%error,"cannot persist trace metadata");
        }
    }
}
fn expire(root: &Path, run: &mut Manifest) -> anyhow::Result<()> {
    let dir = root.join(&run.run_id);
    if dir.exists() {
        anyhow::ensure!(regular_dir(&dir), "unsafe trace directory");
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if valid_asset(&name) || name.ends_with(".tmp") {
                let info = entry.file_type()?;
                if info.is_file() && !info.is_symlink() {
                    std::fs::remove_file(entry.path())?;
                }
            }
        }
    }
    run.status = "expired".into();
    run.bytes = 0;
    run.metadata_bytes = 512;
    run.images.clear();
    run.snapshot = None;
    Ok(())
}
fn expire_run(root: &Path, state: &Mutex<State>, id: &str) -> anyhow::Result<()> {
    let mut run = {
        let state = state.lock().unwrap_or_else(|p| p.into_inner());
        let Some(run) = state.runs.get(id) else {
            return Ok(());
        };
        run.clone()
    };
    // Never hold the producer-facing state mutex across disk IO.
    expire(root, &mut run)?;
    let mut state = state.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(current) = state.runs.get_mut(id) {
        current.status = run.status;
        current.bytes = 0;
        current.metadata_bytes = 512;
        current.images.clear();
        current.snapshot = None;
    }
    state.dirty.insert(id.into());
    Ok(())
}
fn cleanup(root: &Path, state: &Mutex<State>, cfg: &TraceConfig, time: DateTime<Utc>) {
    let cutoff = time - chrono::Duration::hours(cfg.retain_hours as i64);
    let ids: Vec<_> = {
        let state = state.lock().unwrap_or_else(|p| p.into_inner());
        state
            .runs
            .iter()
            .filter(|(_, run)| {
                run.status != "expired"
                    && run
                        .finished_at
                        .as_deref()
                        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                        .is_some_and(|finished| finished <= cutoff)
            })
            .map(|(id, _)| id.clone())
            .collect()
    };
    for id in ids {
        if expire_run(root, state, &id).is_err() {
            record_gap(
                &mut state.lock().unwrap_or_else(|p| p.into_inner()),
                &id,
                "cleanup_failed",
            );
        }
    }
    // Reconcile lower configured quotas on startup and idle maintenance too,
    // not just when a new image happens to arrive.
    loop {
        let oldest = {
            let state = state.lock().unwrap_or_else(|p| p.into_inner());
            if state.runs.values().map(|r| r.bytes).sum::<u64>() <= cfg.global_max_bytes {
                break;
            }
            state
                .runs
                .iter()
                .filter(|(_, r)| r.status != "expired" && r.finished_at.is_some())
                .min_by_key(|(_, r)| &r.finished_at)
                .map(|(id, _)| id.clone())
        };
        let Some(oldest) = oldest else {
            break;
        };
        if expire_run(root, state, &oldest).is_err() {
            record_gap(
                &mut state.lock().unwrap_or_else(|p| p.into_inner()),
                &oldest,
                "cleanup_failed",
            );
            break;
        }
    }
}

fn process_image(root: &Path, state: &Mutex<State>, cfg: &TraceConfig, work: Work) {
    let result = process_image_inner(root, state, cfg, &work);
    if let Err(error) = result {
        tracing::warn!(%error,"trace image unavailable");
        let mut state = state.lock().unwrap_or_else(|p| p.into_inner());
        record_gap(&mut state, &work.run_id, "image_encode_or_write_failed");
    }
}
fn process_image_inner(
    root: &Path,
    state: &Mutex<State>,
    cfg: &TraceConfig,
    work: &Work,
) -> anyhow::Result<()> {
    let kind = work.metadata["kind"].as_str().unwrap_or("consumed");
    {
        let mut state = state.lock().unwrap_or_else(|p| p.into_inner());
        let run = state
            .runs
            .get_mut(&work.run_id)
            .ok_or_else(|| anyhow::anyhow!("run not registered"))?;
        if run.stopped || run.status == "expired" {
            return Ok(());
        }
        if run.images.len() >= cfg.run_max_images {
            run.stopped = true;
            record_gap(&mut state, &work.run_id, "run_image_quota_exceeded");
            return Ok(());
        }
        if kind == "observation" {
            let captured =
                DateTime::parse_from_rfc3339(&work.evidence.captured_at)?.timestamp_millis();
            if captured.saturating_sub(run.last_observation_ms) < cfg.min_observation_ms as i64 {
                return Ok(());
            }
            run.last_observation_ms = captured;
        }
    }
    let image = work.evidence.frame.image();
    let mut hash = Sha256::new();
    hash.update(image.width().to_le_bytes());
    hash.update(image.height().to_le_bytes());
    hash.update(image.as_raw());
    let asset = format!("{:x}.png", hash.finalize());
    let dir = root.join(&work.run_id);
    private_dir(&dir)?;
    let path = dir.join(&asset);
    let mut files = vec![];
    if !path.exists() {
        let mut encoded = std::io::Cursor::new(Vec::new());
        image.write_to(&mut encoded, image::ImageFormat::Png)?;
        files.push((asset.clone(), encoded.into_inner()));
    }
    let template_asset = work
        .template
        .as_ref()
        .filter(|b| b.len() <= 4 * 1024 * 1024)
        .map(|bytes| {
            let asset = format!("{:x}.png", Sha256::digest(bytes.as_slice()));
            if !dir.join(&asset).exists() {
                files.push((asset.clone(), bytes.as_ref().clone()));
            }
            asset
        });
    if work
        .template
        .as_ref()
        .is_some_and(|bytes| bytes.len() > 4 * 1024 * 1024)
    {
        record_gap(
            &mut state.lock().unwrap_or_else(|p| p.into_inner()),
            &work.run_id,
            "template_exceeds_4_mib",
        );
    }
    let (width, height) = image.dimensions();
    let metadata_added = work.metadata.to_string().len() as u64 + 1024;
    let added: u64 = files
        .iter()
        .map(|(_, bytes)| bytes.len() as u64)
        .sum::<u64>()
        + work.metadata.to_string().len() as u64
        + 1024;
    loop {
        let oldest = {
            let mut state = state.lock().unwrap_or_else(|p| p.into_inner());
            let run = state.runs.get(&work.run_id).unwrap();
            if run.metadata_bytes.saturating_add(metadata_added) > 8 * 1024 * 1024 - 16 * 1024 {
                state.runs.get_mut(&work.run_id).unwrap().stopped = true;
                record_gap(&mut state, &work.run_id, "run_metadata_quota_exceeded");
                return Ok(());
            }
            if run.bytes.saturating_add(added) > cfg.run_max_bytes {
                state.runs.get_mut(&work.run_id).unwrap().stopped = true;
                record_gap(&mut state, &work.run_id, "run_byte_quota_exceeded");
                return Ok(());
            }
            if state
                .runs
                .values()
                .map(|r| r.bytes)
                .sum::<u64>()
                .saturating_add(added)
                <= cfg.global_max_bytes
            {
                // Reserve before dropping the lock; concurrent snapshot producers
                // cannot overrun the shared budget while this image is written.
                state.runs.get_mut(&work.run_id).unwrap().bytes += added;
                break;
            }
            let oldest = state
                .runs
                .iter()
                .filter(|(id, r)| {
                    *id != &work.run_id && r.status != "expired" && r.finished_at.is_some()
                })
                .min_by_key(|(_, r)| &r.finished_at)
                .map(|(id, _)| id.clone());
            if oldest.is_none() {
                state.runs.get_mut(&work.run_id).unwrap().stopped = true;
                record_gap(&mut state, &work.run_id, "global_byte_quota_exceeded");
                return Ok(());
            }
            oldest.unwrap()
        };
        expire_run(root, state, &oldest)?;
    }
    let write = files
        .iter()
        .try_for_each(|(name, bytes)| atomic_write(&dir.join(name), bytes));
    if let Err(error) = write {
        for (name, _) in &files {
            let _ = std::fs::remove_file(dir.join(name));
        }
        let mut state = state.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(run) = state.runs.get_mut(&work.run_id) {
            run.bytes = run.bytes.saturating_sub(added);
        }
        return Err(error);
    }
    let mut state = state.lock().unwrap_or_else(|p| p.into_inner());
    let run = state.runs.get_mut(&work.run_id).unwrap();
    let seq = run.images.last().map(|i| i.seq + 1).unwrap_or(1);
    run.metadata_bytes += metadata_added;
    run.images.push(TraceImage {
        seq,
        image_id: work.image_id.clone(),
        captured_at: work.evidence.captured_at.clone(),
        recorded_at: now(),
        width,
        height,
        kind: kind.into(),
        source: work.evidence.source.clone(),
        metadata: work.metadata.clone(),
        url: format!("/api/runs/{}/trace/images/{}", work.run_id, work.image_id),
        asset,
        template_asset,
    });
    state.dirty.insert(work.run_id.clone());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn evidence(color: u8, time: &str) -> CapturedEvidence {
        CapturedEvidence {
            frame: Arc::new(DecodedFrame::from_rgb(image::RgbImage::from_pixel(
                16,
                10,
                image::Rgb([color, 1, 2]),
            ))),
            captured_at: time.into(),
            source: Some(crate::capabilities::FrameStamp {
                target: "device".into(),
                epoch: "epoch".into(),
                revision: 7,
            }),
        }
    }
    fn submit(
        store: &TraceStore,
        run: &str,
        color: u8,
        kind: &str,
        enabled: bool,
        forced: bool,
    ) -> Option<String> {
        store.submit(
            evidence(color, "2026-10-05T10:00:00.000Z"),
            json!({"run_id":run,"kind":kind,"trace_enabled":enabled,"path":"run[1]","frame_id":42}),
            forced,
            None,
        )
    }
    #[test]
    fn lossless_exact_frame_ids_and_dedup_are_independent_of_call_frames() {
        let dir = tempfile::tempdir().unwrap();
        let store = TraceStore::open(dir.path(), Default::default()).unwrap();
        let a = submit(&store, "run", 10, "consumed", true, false).unwrap();
        store.flush();
        let b = submit(&store, "run", 10, "consumed", true, false).unwrap();
        store.flush();
        assert_ne!(a, b);
        let page = store.page("run", 0, 1);
        assert_eq!(page["has_more"], true);
        assert_eq!(page["images"][0]["captured_at"], "2026-10-05T10:00:00.000Z");
        assert_eq!(page["images"][0]["metadata"]["frame_id"], 42);
        assert_eq!(page["images"][0]["width"], 16);
        let pixels = image::load_from_memory(&store.read_image("run", &a, false).unwrap().unwrap())
            .unwrap()
            .to_rgb8();
        assert_eq!(pixels.get_pixel(0, 0).0, [10, 1, 2]);
        let count = std::fs::read_dir(dir.path().join("runtime-traces/run"))
            .unwrap()
            .flatten()
            .filter(|f| f.path().extension().is_some_and(|x| x == "png"))
            .count();
        assert_eq!(count, 1);
    }
    #[test]
    fn trace_edges_disabled_consumption_and_one_root_error_remain_truthful() {
        let dir = tempfile::tempdir().unwrap();
        let store = TraceStore::open(dir.path(), Default::default()).unwrap();
        submit(&store, "run", 1, "trace_off", false, true);
        store.flush();
        assert!(submit(&store, "run", 2, "consumed", false, false).is_none());
        let error =
            json!({"run_id":"run","kind":"error_fresh","trace_enabled":false,"error":"root"});
        assert!(store.root_error(&error));
        assert!(!store.root_error(&error));
        store.flush();
        store.gap("run", "capture_failed: disconnected");
        store.flush();
        submit(&store, "run", 3, "trace_on", true, true);
        store.flush();
        let page = store.page("run", 0, 100);
        let images = page["images"].as_array().unwrap();
        assert_eq!(images.len(), 3);
        assert_eq!(images[1]["kind"], "error_last_consumed");
        let id = images[1]["image_id"].as_str().unwrap();
        let pixels = image::load_from_memory(&store.read_image("run", id, false).unwrap().unwrap())
            .unwrap()
            .to_rgb8();
        assert_eq!(pixels.get_pixel(0, 0).0, [2, 1, 2]);
        assert_eq!(images[1]["captured_at"], "2026-10-05T10:00:00.000Z");
        assert_eq!(page["enabled"], true);
        assert_eq!(page["gaps"][0]["reason"], "capture_failed: disconnected");
    }
    #[test]
    fn ttl_boundary_preserves_base_files_and_retained_copy_and_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let store = TraceStore::open(dir.path(), Default::default()).unwrap();
        std::fs::create_dir(dir.path().join("packages")).unwrap();
        std::fs::write(dir.path().join("packages/permanent.png"), b"permanent").unwrap();
        let id = submit(&store, "run", 9, "consumed", true, false).unwrap();
        store.flush();
        // Registration/retention use the real clock. Keep completion recent so
        // this boundary test does not expire merely because its calendar date
        // has passed; exercise the exact TTL through cleanup's explicit clock.
        let finished = Utc::now();
        store.register("run", Some(finished.to_rfc3339()));
        store.flush();
        store.retain("run").unwrap();
        {
            let _io = store.io.lock().unwrap();
            cleanup(
                &store.root,
                &store.state,
                &store.cfg,
                finished + chrono::Duration::hours(24) - chrono::Duration::seconds(1),
            );
            assert_eq!(store.state.lock().unwrap().runs["run"].status, "available");
            cleanup(
                &store.root,
                &store.state,
                &store.cfg,
                finished + chrono::Duration::hours(24),
            );
            assert_eq!(store.state.lock().unwrap().runs["run"].status, "expired");
        }
        store.flush();
        assert_eq!(store.page("run", 0, 100)["status"], "retained");
        assert!(store.read_image("run", &id, false).unwrap().is_some());
        assert_eq!(
            std::fs::read(dir.path().join("packages/permanent.png")).unwrap(),
            b"permanent"
        );
        drop(store);
        let reopened = TraceStore::open(dir.path(), Default::default()).unwrap();
        assert_eq!(reopened.page("run", 0, 100)["status"], "retained");
        assert!(reopened.read_image("run", &id, false).unwrap().is_some());
    }
    #[test]
    fn run_and_retained_quotas_stop_capture_with_visible_gap() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = TraceConfig {
            run_max_images: 2,
            retained_max_runs: 1,
            ..Default::default()
        };
        let store = TraceStore::open(dir.path(), cfg).unwrap();
        for color in 0..3 {
            submit(&store, "a", color, "consumed", true, false);
            store.flush();
        }
        assert_eq!(
            store.page("a", 0, 100)["images"].as_array().unwrap().len(),
            2
        );
        assert_eq!(store.page("a", 0, 100)["stopped"], true);
        assert_eq!(
            store.page("a", 0, 100)["gaps"][0]["reason"],
            "run_image_quota_exceeded"
        );
        store.register("a", Some(now()));
        store.retain("a").unwrap();
        submit(&store, "b", 8, "consumed", true, false);
        store.flush();
        store.register("b", Some(now()));
        assert!(store.retain("b").unwrap_err().to_string().contains("quota"));
    }
    #[test]
    fn queue_full_and_disk_failure_record_bounded_gaps() {
        let dir = tempfile::tempdir().unwrap();
        let store = TraceStore::open(
            dir.path(),
            TraceConfig {
                queue_capacity: 1,
                ..Default::default()
            },
        )
        .unwrap();
        // Block only the storage worker, proving submission itself never waits for IO.
        let guard = store.io.lock().unwrap();
        for i in 0..100 {
            submit(&store, "a", i, "consumed", true, false);
        }
        assert!(store.page("a", 0, 100)["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g["reason"] == "encoding_queue_full"));
        drop(guard);
        store.flush();
        let bad = dir.path().join("runtime-traces/b");
        std::fs::write(&bad, b"not a directory").unwrap();
        submit(&store, "b", 1, "consumed", true, false);
        store.flush();
        assert!(store.page("b", 0, 100)["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g["reason"] == "image_encode_or_write_failed"));
    }
    #[test]
    fn invalid_identifiers_cannot_read_or_write_outside_private_trace_root() {
        let dir = tempfile::tempdir().unwrap();
        let store = TraceStore::open(dir.path(), Default::default()).unwrap();
        assert!(submit(&store, "../escape", 1, "consumed", true, false).is_none());
        assert!(store.read_image("../escape", "bad", false).is_err());
        assert!(store.snapshot("../escape", json!({})).is_err());
        assert!(!dir.path().join("escape").exists());
    }
    #[test]
    fn concurrent_runs_do_not_mix_pixels_or_snapshots() {
        let dir = tempfile::tempdir().unwrap();
        let store = TraceStore::open(dir.path(), Default::default()).unwrap();
        let mut workers = vec![];
        for color in 0..4 {
            let store = store.clone();
            workers.push(std::thread::spawn(move || {
                let run = format!("r{color}");
                store.snapshot(&run, json!({"source":run})).unwrap();
                let id = submit(&store, &run, color, "consumed", true, false);
                (run, color, id)
            }));
        }
        for worker in workers {
            let (run, color, id) = worker.join().unwrap();
            store.flush();
            if let Some(id) = id {
                let bytes = store.read_image(&run, &id, false).unwrap().unwrap();
                assert_eq!(
                    image::load_from_memory(&bytes)
                        .unwrap()
                        .to_rgb8()
                        .get_pixel(0, 0)
                        .0,
                    [color, 1, 2]
                );
            }
            assert_eq!(store.page(&run, 0, 100)["snapshot"]["source"], run);
        }
    }
}

#[cfg(test)]
mod bounds_tests {
    use super::*;
    fn image(store: &TraceStore, run: &str, color: u8) -> Option<String> {
        store.submit(
            CapturedEvidence {
                frame: Arc::new(DecodedFrame::from_rgb(image::RgbImage::from_pixel(
                    16,
                    10,
                    image::Rgb([color, 2, 3]),
                ))),
                captured_at: now(),
                source: None,
            },
            json!({"run_id":run,"kind":"consumed","trace_enabled":true}),
            false,
            None,
        )
    }
    #[test]
    fn global_quota_prunes_oldest_completed_but_bounds_active_run() {
        let dir = tempfile::tempdir().unwrap();
        let store = TraceStore::open(
            dir.path(),
            TraceConfig {
                run_max_bytes: 4000,
                global_max_bytes: 4000,
                ..Default::default()
            },
        )
        .unwrap();
        image(&store, "completed", 1);
        store.flush();
        store.register("completed", Some(now()));
        store.flush();
        image(&store, "active-a", 2);
        store.flush();
        image(&store, "active-b", 3);
        store.flush();
        assert_eq!(store.page("completed", 0, 100)["status"], "expired");
        image(&store, "active-c", 4);
        store.flush();
        let page = store.page("active-c", 0, 100);
        assert_eq!(page["stopped"], true);
        assert_eq!(page["gaps"][0]["reason"], "global_byte_quota_exceeded");
        assert!(!store.page("active-a", 0, 100)["images"]
            .as_array()
            .unwrap()
            .is_empty());
        assert!(!store.page("active-b", 0, 100)["images"]
            .as_array()
            .unwrap()
            .is_empty());
    }
    #[test]
    fn restart_removes_uncommitted_payloads_without_touching_retained_or_foreign_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = TraceStore::open(dir.path(), Default::default()).unwrap();
        let id = image(&store, "a", 1).unwrap();
        store.flush();
        store.register("a", Some(now()));
        store.retain("a").unwrap();
        drop(store);
        let orphan = dir
            .path()
            .join("runtime-traces/a")
            .join(format!("{}.png", "a".repeat(64)));
        std::fs::write(&orphan, b"uncommitted").unwrap();
        let foreign = dir.path().join("runtime-traces/a/notes.txt");
        std::fs::write(&foreign, b"keep").unwrap();
        let staging = dir
            .path()
            .join("retained-traces")
            .join(format!("staging-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&staging).unwrap();
        std::fs::write(staging.join("index.json"), b"interrupted").unwrap();
        let reopened = TraceStore::open(dir.path(), Default::default()).unwrap();
        reopened.flush();
        assert!(!orphan.exists());
        assert!(!staging.exists());
        assert_eq!(std::fs::read(foreign).unwrap(), b"keep");
        assert!(reopened.read_image("a", &id, false).unwrap().is_some());
    }
    #[test]
    fn restart_reconciles_lower_global_limit_without_new_capture() {
        let dir = tempfile::tempdir().unwrap();
        let store = TraceStore::open(dir.path(), Default::default()).unwrap();
        for (run, color) in [("a", 1), ("b", 2)] {
            image(&store, run, color);
            store.flush();
            store.register(run, Some(now()));
            store.flush();
        }
        drop(store);
        let store = TraceStore::open(
            dir.path(),
            TraceConfig {
                global_max_bytes: 2000,
                ..Default::default()
            },
        )
        .unwrap();
        store.flush();
        let statuses = [
            store.page("a", 0, 100)["status"].clone(),
            store.page("b", 0, 100)["status"].clone(),
        ];
        assert_eq!(statuses.iter().filter(|s| **s == "available").count(), 1);
        assert_eq!(statuses.iter().filter(|s| **s == "expired").count(), 1);
    }
    #[test]
    fn snapshot_is_immutable_and_charged_to_run_quota() {
        let dir = tempfile::tempdir().unwrap();
        let store = TraceStore::open(
            dir.path(),
            TraceConfig {
                run_max_bytes: 1024,
                ..Default::default()
            },
        )
        .unwrap();
        store.snapshot("a", json!({"source":"first"})).unwrap();
        store.snapshot("a", json!({"source":"overwrite"})).unwrap();
        assert_eq!(store.page("a", 0, 100)["snapshot"]["source"], "first");
        store
            .snapshot("b", json!({"source":"x".repeat(1024)}))
            .unwrap();
        assert_eq!(
            store.page("b", 0, 100)["gaps"][0]["reason"],
            "snapshot_quota_exceeded"
        );
    }
    #[test]
    fn observation_minimum_interval_does_not_suppress_forced_edges() {
        let dir = tempfile::tempdir().unwrap();
        let store = TraceStore::open(dir.path(), Default::default()).unwrap();
        for (millis, kind, forced) in [
            (0, "observation", false),
            (1, "observation", false),
            (2, "trace_off", true),
            (1000, "observation", false),
        ] {
            store.submit(
                CapturedEvidence {
                    frame: Arc::new(DecodedFrame::from_rgb(image::RgbImage::new(2, 2))),
                    captured_at: ("2026-10-05T10:00:00Z".parse::<DateTime<Utc>>().unwrap()
                        + chrono::Duration::milliseconds(millis))
                    .to_rfc3339(),
                    source: None,
                },
                json!({"run_id":"a","kind":kind}),
                forced,
                None,
            );
            store.flush();
        }
        assert_eq!(
            store.page("a", 0, 100)["images"].as_array().unwrap().len(),
            3
        );
    }
    #[cfg(unix)]
    #[test]
    fn symlink_trace_and_asset_cannot_escape_to_private_files() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), dir.path().join("runtime-traces")).unwrap();
        assert!(TraceStore::open(dir.path(), Default::default()).is_err());
        std::fs::remove_file(dir.path().join("runtime-traces")).unwrap();
        let store = TraceStore::open(dir.path(), Default::default()).unwrap();
        let id = image(&store, "a", 1).unwrap();
        store.flush();
        let state = store.state.lock().unwrap();
        let asset = state.runs["a"].images[0].asset.clone();
        drop(state);
        let path = dir.path().join("runtime-traces/a").join(asset);
        std::fs::remove_file(&path).unwrap();
        std::fs::write(outside.path().join("secret"), b"private").unwrap();
        symlink(outside.path().join("secret"), path).unwrap();
        assert!(store.read_image("a", &id, false).is_err());
    }
    /// Cloud filesystem/codec measurement only. This deliberately does not make
    /// any claim about Android, CDP capture cost, or real input latency.
    #[test]
    fn synthetic_png_storage_benchmark() {
        let dir = tempfile::tempdir().unwrap();
        let store = TraceStore::open(dir.path(), Default::default()).unwrap();
        let start = std::time::Instant::now();
        let mut enqueue_max = 0;
        for frame_index in 0..4u32 {
            let mut seed = frame_index + 1;
            let frame = image::RgbImage::from_fn(1280, 720, |_, _| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                image::Rgb([(seed >> 16) as u8, (seed >> 8) as u8, seed as u8])
            });
            let begin = std::time::Instant::now();
            assert!(store
                .submit(
                    CapturedEvidence {
                        frame: Arc::new(DecodedFrame::from_rgb(frame)),
                        captured_at: now(),
                        source: None
                    },
                    json!({"run_id":"benchmark","kind":"consumed"}),
                    false,
                    None
                )
                .is_some());
            enqueue_max = enqueue_max.max(begin.elapsed().as_micros());
        }
        store.flush();
        let page = store.page("benchmark", 0, 100);
        assert_eq!(page["images"].as_array().unwrap().len(), 4);
        eprintln!("TRACE_BENCH synthetic 1280x720 RGB, frames=4, format=lossless PNG, encode_and_disk_ms={}, max_enqueue_us={}, accounted_bytes={}",start.elapsed().as_millis(),enqueue_max,page["bytes"]);
    }
}
