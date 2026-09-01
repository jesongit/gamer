//! 模板匹配引擎：灰度 + 归一化互相关（NCC）滑动窗口
//!
//! 性能策略：截图先等比缩放到 ≤540px 宽（模板同比例），步长采样 + rayon 并行，
//! 1080p 全图 + 小模板典型耗时 100~400ms；支持搜索区域裁剪进一步加速。

use std::collections::{hash_map::Entry, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use image::{DynamicImage, GenericImageView, GrayImage};
use parking_lot::Mutex;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

// CPU 密集任务专用计算池（PERF-003）：NCC 匹配 / PNG 解码统一经
// `compute::run` 提交，避免占住 Tokio 核心工作线程（文件在 src/compute.rs）
#[path = "compute.rs"]
pub mod compute;

/// 匹配结果（坐标基于原始截图坐标系）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchResult {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub score: f32,
}

/// 匹配请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchRequest {
    /// 截图 PNG 字节
    pub screen_png: Vec<u8>,
    /// 模板 PNG 字节
    pub template_png: Vec<u8>,
    /// 阈值 0~1（默认 0.8）
    pub threshold: Option<f32>,
    /// 搜索区域（原始截图坐标系，None = 全图）
    pub region: Option<[u32; 4]>,
}

/// 从模板实际文件名解析引擎使用的搜索区域。
///
/// 模板没有 `#` 后缀时返回 `None`（全屏）；`#a` 也代表全屏。
/// `#u/#d/...` 是半区，四段数字是相对坐标乘 1000 后的矩形。
/// 编辑器的单次匹配预览与脚本引擎共用此实现，避免两套区域语义漂移。
pub fn template_region_from_name(template: &str, w: u32, h: u32) -> Option<[u32; 4]> {
    let lower = template.to_ascii_lowercase();
    let stem = if lower.ends_with(".jpeg") {
        &template[..template.len() - 5]
    } else if lower.ends_with(".png") || lower.ends_with(".jpg") {
        &template[..template.len() - 4]
    } else {
        template
    };
    let idx = stem.rfind('#')?;
    let suffix = stem[idx + 1..].trim().to_ascii_lowercase();
    if suffix.is_empty() {
        return None;
    }
    let half = match suffix.as_str() {
        "a" => return None,
        "u" => [0, 0, w, h / 2],
        "d" => [0, h / 2, w, h - h / 2],
        "l" => [0, 0, w / 2, h],
        "r" => [w / 2, 0, w - w / 2, h],
        "ul" => [0, 0, w / 2, h / 2],
        "ur" => [w / 2, 0, w - w / 2, h / 2],
        "dl" => [0, h / 2, w / 2, h - h / 2],
        "dr" => [w / 2, h / 2, w - w / 2, h - h / 2],
        _ => {
            let nums: Option<Vec<f64>> = suffix
                .split('_')
                .map(|p| {
                    p.parse::<u32>()
                        .ok()
                        .filter(|n| *n <= 999)
                        .map(|n| n as f64 / 1000.0)
                })
                .collect();
            let nums = nums?;
            if nums.len() != 4 {
                return None;
            }
            let [x1, y1, x2, y2] = [nums[0], nums[1], nums[2], nums[3]];
            if x2 <= x1 || y2 <= y1 {
                return None;
            }
            let x = (x1 * w as f64).round() as u32;
            let y = (y1 * h as f64).round() as u32;
            let rw = (((x2 - x1) * w as f64).round() as u32).max(1);
            let rh = (((y2 - y1) * h as f64).round() as u32).max(1);
            return Some([x, y, rw, rh]);
        }
    };
    Some(half)
}

/// 模板预处理结果：缓存 PNG 解码后的灰度矩阵、f32 数据和 NCC 统计量。
///
/// 缓存键是完整模板字节的 SHA-256，因此覆盖上传或同名文件内容变化会自然
/// 使用新键，不会把旧模板结果带到新内容上。缩放后的模板按目标尺寸另存，
/// 因为全屏匹配和区域匹配的缩放比例可能不同。
#[derive(Clone)]
struct PreparedTemplate {
    image: Arc<GrayImage>,
    data: Arc<Vec<f32>>,
    mean: f32,
    var: f32,
}

struct TemplateCacheEntry {
    source: Arc<DynamicImage>,
    prepared: HashMap<(u32, u32), Arc<PreparedTemplate>>,
    memory_bytes: usize,
    last_used: u64,
}

#[derive(Default)]
#[allow(dead_code)]
struct TemplateCache {
    entries: HashMap<[u8; 32], TemplateCacheEntry>,
    total_bytes: usize,
    clock: u64,
    path_entries: HashMap<TemplatePathKey, TemplatePathEntry>,
    path_resolve_entries: HashMap<TemplateResolveKey, TemplateResolveEntry>,
}

const TEMPLATE_CACHE_CAPACITY: usize = 128;
const TEMPLATE_CACHE_MAX_BYTES: usize = 64 * 1024 * 1024;
static TEMPLATE_CACHE: OnceLock<Mutex<TemplateCache>> = OnceLock::new();
static MATCHER_STATS: AtomicPtr<MatcherStats> = AtomicPtr::new(std::ptr::null_mut());

fn default_matcher_stats() -> &'static MatcherStats {
    static DEFAULT: MatcherStats = MatcherStats {
        now: MatcherStats::now_instant,
        record_ncc: MatcherStats::record_ncc_to_metrics,
    };
    &DEFAULT
}

fn matcher_stats() -> &'static MatcherStats {
    let ptr = MATCHER_STATS.load(Ordering::Relaxed);
    if ptr.is_null() {
        default_matcher_stats()
    } else {
        unsafe { &*ptr }
    }
}

#[cfg(test)]
fn install_matcher_stats(stats: MatcherStats) -> MatcherStatsGuard {
    let boxed = Box::new(stats);
    let raw = Box::into_raw(boxed);
    let prev = MATCHER_STATS.swap(raw, Ordering::AcqRel);
    MatcherStatsGuard { prev, current: raw }
}

#[derive(Clone, Copy)]
pub struct MatcherStats {
    now: fn() -> Instant,
    record_ncc: fn(u64, bool, bool),
}

impl MatcherStats {
    fn now_instant() -> Instant {
        Instant::now()
    }

    /// 生产默认埋点：转发进程级共享 metrics（/metrics 的数据源）。main 启动时
    /// 已安装 Store 的实例；未安装（如未装 global 的测试进程）时
    /// `metrics::global()` 惰性兜底创建默认实例——观测为旁路，绝不 panic、
    /// 不影响匹配结果。测试经 `install_matcher_stats` 覆盖此函数实现计数隔离。
    fn record_ncc_to_metrics(duration_ms: u64, hit: bool, region: bool) {
        crate::metrics::global().record_ncc(duration_ms, hit, region);
    }

    fn record_ncc(&self, duration_ms: u64, hit: bool, region: bool) {
        (self.record_ncc)(duration_ms, hit, region);
    }
}

#[cfg(test)]
struct MatcherStatsGuard {
    prev: *mut MatcherStats,
    current: *mut MatcherStats,
}

#[cfg(test)]
impl Drop for MatcherStatsGuard {
    fn drop(&mut self) {
        MATCHER_STATS.store(self.prev, Ordering::Release);
        unsafe {
            drop(Box::from_raw(self.current));
        }
    }
}

#[cfg(test)]
fn test_matcher_stats(now: fn() -> Instant, record_ncc: fn(u64, bool, bool)) -> MatcherStats {
    MatcherStats { now, record_ncc }
}

#[derive(Hash, Eq, PartialEq, Clone)]
struct TemplatePathKey {
    path: PathBuf,
    mtime_ns: u128,
    size: u64,
    content_hash: [u8; 32],
}

struct TemplatePathEntry {
    source: Arc<DynamicImage>,
    memory_bytes: usize,
    prepared: HashMap<(u32, u32), Arc<PreparedTemplate>>,
    last_used: u64,
}

#[derive(Hash, Eq, PartialEq, Clone)]
struct TemplateResolveKey {
    dir: PathBuf,
    dir_generation: [u8; 32],
    template: String,
}

struct TemplateResolveEntry {
    resolved: Arc<PathBuf>,
    memory_bytes: usize,
    last_used: u64,
}

fn template_cache() -> &'static Mutex<TemplateCache> {
    TEMPLATE_CACHE.get_or_init(|| Mutex::new(TemplateCache::default()))
}

fn cache_tick(cache: &mut TemplateCache) -> u64 {
    cache.clock = cache.clock.wrapping_add(1).max(1);
    cache.clock
}

fn template_key(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

#[allow(dead_code)]
fn metadata_key(path: &Path, meta: &std::fs::Metadata, content_hash: [u8; 32]) -> TemplatePathKey {
    TemplatePathKey {
        path: normalize_path(path),
        mtime_ns: file_mtime_ns(meta),
        size: meta.len(),
        content_hash,
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    if let Ok(normalized) = path.canonicalize() {
        return normalized;
    }
    // 删除/重命名后的路径本身不存在，canonicalize 失败；父目录通常仍在，
    // 先规范化父目录再拼回文件名，保证主动失效仍命中既有绝对路径键。
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
        if let Ok(normalized_parent) = parent.canonicalize() {
            return normalized_parent.join(name);
        }
    }
    path.to_path_buf()
}

#[allow(dead_code)]
fn file_mtime_ns(meta: &std::fs::Metadata) -> u128 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn dir_signature(dir: &Path) -> anyhow::Result<(PathBuf, [u8; 32])> {
    let dir = normalize_path(dir);
    let mut entries = std::fs::read_dir(&dir)?
        .map(|entry| {
            let entry = entry?;
            let path = normalize_path(&entry.path());
            let is_file = entry.file_type()?.is_file();
            Ok((path, is_file))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    entries.sort_unstable_by(|(path_a, file_a), (path_b, file_b)| {
        path_a.cmp(path_b).then(file_a.cmp(file_b))
    });
    let mut hasher = Sha256::new();
    for (path, is_file) in entries {
        hasher.update(path.to_string_lossy().as_bytes());
        hasher.update([0]);
        hasher.update([is_file as u8]);
    }
    Ok((dir, hasher.finalize().into()))
}

fn image_memory_bytes(width: u32, height: u32, bytes_per_pixel: usize) -> usize {
    (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(bytes_per_pixel)
}

fn source_memory_bytes(source: &DynamicImage) -> usize {
    // PNG/JPEG 输入可解码为多种 DynamicImage 像素格式；8 字节/像素是对
    // 当前支持格式的保守预算，避免只按压缩后的 PNG 字节数放行大图。
    let (width, height) = source.dimensions();
    image_memory_bytes(width, height, 8)
}

fn prepared_memory_bytes(prepared: &PreparedTemplate) -> usize {
    let (width, height) = prepared.image.dimensions();
    image_memory_bytes(width, height, 5)
}

fn cache_entry_count(cache: &TemplateCache) -> usize {
    cache.entries.len() + cache.path_entries.len() + cache.path_resolve_entries.len()
}

/// 获取 PNG 解码后的源灰度图。锁只覆盖一次性的模板解码，命中时仅复制 Arc。
fn cached_template_source(bytes: &[u8]) -> anyhow::Result<([u8; 32], Arc<DynamicImage>)> {
    let key = template_key(bytes);
    let mut cache = template_cache().lock();
    if let Some(source) = cache.entries.get(&key).map(|entry| entry.source.clone()) {
        let used = cache_tick(&mut cache);
        if let Some(entry) = cache.entries.get_mut(&key) {
            entry.last_used = used;
        }
        return Ok((key, source));
    }
    let source = Arc::new(decode_image_limited(
        bytes,
        TEMPLATE_MAX_INPUT_BYTES,
        "模板",
    )?);
    let memory_bytes = source_memory_bytes(&source);
    let used = cache_tick(&mut cache);
    cache.entries.insert(
        key,
        TemplateCacheEntry {
            source: source.clone(),
            prepared: HashMap::new(),
            memory_bytes,
            last_used: used,
        },
    );
    cache.total_bytes = cache.total_bytes.saturating_add(memory_bytes);
    evict_template_cache(&mut cache);
    Ok((key, source))
}

fn build_prepared_template(image: GrayImage) -> anyhow::Result<PreparedTemplate> {
    let data: Vec<f32> = image.as_raw().iter().map(|&v| v as f32 / 255.0).collect();
    if data.is_empty() {
        anyhow::bail!("template is empty");
    }
    let mean = data.iter().sum::<f32>() / data.len() as f32;
    let var = data.iter().map(|&v| (v - mean) * (v - mean)).sum();
    if var < 1e-6 {
        anyhow::bail!("template is uniform color");
    }
    Ok(PreparedTemplate {
        image: Arc::new(image),
        data: Arc::new(data),
        mean,
        var,
    })
}

/// 获取指定尺寸的模板统计量。尺寸是缩放后的实际模板尺寸，避免重复灰度化、
/// f32 转换和均值/方差计算；首次 miss 才做一次这些工作。
fn cached_prepared_template(
    key: [u8; 32],
    source: &Arc<DynamicImage>,
    dimensions: (u32, u32),
) -> anyhow::Result<Arc<PreparedTemplate>> {
    let mut cache = template_cache().lock();
    let hit = cache.entries.get(&key).and_then(|entry| {
        entry
            .prepared
            .get(&dimensions)
            .cloned()
            .map(|prepared| (prepared, entry.last_used))
    });
    if let Some((prepared, _)) = hit {
        let used = cache_tick(&mut cache);
        if let Some(entry) = cache.entries.get_mut(&key) {
            entry.last_used = used;
        }
        return Ok(prepared);
    }
    if let Entry::Vacant(entry) = cache.entries.entry(key) {
        let source_memory = source_memory_bytes(source);
        entry.insert(TemplateCacheEntry {
            source: source.clone(),
            prepared: HashMap::new(),
            memory_bytes: source_memory,
            last_used: 0,
        });
        cache.total_bytes = cache.total_bytes.saturating_add(source_memory);
    }
    let (prepared, prepared_bytes) = {
        let entry = cache
            .entries
            .get_mut(&key)
            .expect("template cache entry inserted");
        let image = if entry.source.dimensions() == dimensions {
            to_gray(entry.source.as_ref())
        } else {
            entry
                .source
                .resize(
                    dimensions.0,
                    dimensions.1,
                    image::imageops::FilterType::Triangle,
                )
                .to_luma8()
        };
        let prepared = Arc::new(build_prepared_template(image)?);
        let prepared_bytes = prepared_memory_bytes(&prepared);
        entry.prepared.insert(dimensions, prepared.clone());
        entry.memory_bytes = entry.memory_bytes.saturating_add(prepared_bytes);
        (prepared, prepared_bytes)
    };
    cache.total_bytes = cache.total_bytes.saturating_add(prepared_bytes);
    let used = cache_tick(&mut cache);
    if let Some(entry) = cache.entries.get_mut(&key) {
        entry.last_used = used;
    }
    evict_template_cache(&mut cache);
    Ok(prepared)
}

fn evict_template_cache(cache: &mut TemplateCache) {
    while cache.total_bytes > TEMPLATE_CACHE_MAX_BYTES
        || cache_entry_count(cache) > TEMPLATE_CACHE_CAPACITY
    {
        enum Oldest {
            Content([u8; 32]),
            Path(TemplatePathKey),
            Resolve(TemplateResolveKey),
        }

        let oldest = cache
            .entries
            .iter()
            .map(|(key, entry)| (entry.last_used, Oldest::Content(*key)))
            .chain(
                cache
                    .path_entries
                    .iter()
                    .map(|(key, entry)| (entry.last_used, Oldest::Path(key.clone()))),
            )
            .chain(
                cache
                    .path_resolve_entries
                    .iter()
                    .map(|(key, entry)| (entry.last_used, Oldest::Resolve(key.clone()))),
            )
            .min_by_key(|(last_used, _)| *last_used)
            .map(|(_, oldest)| oldest);
        let Some(oldest) = oldest else {
            break;
        };
        let removed_bytes = match oldest {
            Oldest::Content(key) => cache.entries.remove(&key).map(|entry| entry.memory_bytes),
            Oldest::Path(key) => cache
                .path_entries
                .remove(&key)
                .map(|entry| entry.memory_bytes),
            Oldest::Resolve(key) => cache
                .path_resolve_entries
                .remove(&key)
                .map(|entry| entry.memory_bytes),
        };
        let Some(removed_bytes) = removed_bytes else {
            break;
        };
        cache.total_bytes = cache.total_bytes.saturating_sub(removed_bytes);
    }
}

/// 读取模板时同时取前后元数据，避免原子替换边界把旧字节和新 mtime/size
/// 拼成一个缓存键。内容哈希仍保留，用于覆盖但 mtime/size 未变的情况。
#[allow(dead_code)]
fn read_template_consistently(path: &Path) -> anyhow::Result<(Vec<u8>, std::fs::Metadata)> {
    for _ in 0..2 {
        let before = std::fs::metadata(path)?;
        let bytes = std::fs::read(path)?;
        let after = std::fs::metadata(path)?;
        if file_mtime_ns(&before) == file_mtime_ns(&after) && before.len() == after.len() {
            return Ok((bytes, after));
        }
    }
    let bytes = std::fs::read(path)?;
    let meta = std::fs::metadata(path)?;
    Ok((bytes, meta))
}

#[allow(dead_code)]
fn cached_template_source_from_path_key(
    path: &Path,
) -> anyhow::Result<([u8; 32], Arc<DynamicImage>, TemplatePathKey)> {
    let (bytes, meta) = read_template_consistently(path)?;
    let key = metadata_key(path, &meta, template_key(&bytes));
    let mut cache = template_cache().lock();
    if let Some(source) = cache
        .path_entries
        .get(&key)
        .map(|entry| entry.source.clone())
    {
        let used = cache_tick(&mut cache);
        if let Some(entry) = cache.path_entries.get_mut(&key) {
            entry.last_used = used;
        }
        return Ok((key.content_hash, source, key));
    }
    let source = Arc::new(decode_image_limited(
        &bytes,
        TEMPLATE_MAX_INPUT_BYTES,
        "模板",
    )?);
    let memory_bytes = source_memory_bytes(&source);
    let used = cache_tick(&mut cache);
    cache.path_entries.insert(
        key.clone(),
        TemplatePathEntry {
            source: source.clone(),
            memory_bytes,
            prepared: HashMap::new(),
            last_used: used,
        },
    );
    cache.total_bytes = cache.total_bytes.saturating_add(memory_bytes);
    evict_template_cache(&mut cache);
    Ok((key.content_hash, source, key))
}

/// 获取路径入口指定尺寸的模板预处理结果。路径键和字节键分开保存，避免
/// 一个文件覆盖后仅因与另一个文件同内容而跳过路径级失效边界。
#[allow(dead_code)]
fn cached_prepared_template_from_path_key(
    key: &TemplatePathKey,
    source: &Arc<DynamicImage>,
    dimensions: (u32, u32),
) -> anyhow::Result<Arc<PreparedTemplate>> {
    let mut cache = template_cache().lock();
    if let Some(prepared) = cache
        .path_entries
        .get(key)
        .and_then(|entry| entry.prepared.get(&dimensions).cloned())
    {
        let used = cache_tick(&mut cache);
        if let Some(entry) = cache.path_entries.get_mut(key) {
            entry.last_used = used;
        }
        return Ok(prepared);
    }
    if let Entry::Vacant(entry) = cache.path_entries.entry(key.clone()) {
        let source_memory = source_memory_bytes(source);
        entry.insert(TemplatePathEntry {
            source: source.clone(),
            memory_bytes: source_memory,
            prepared: HashMap::new(),
            last_used: 0,
        });
        cache.total_bytes = cache.total_bytes.saturating_add(source_memory);
    }
    let (prepared, prepared_bytes) = {
        let entry = cache
            .path_entries
            .get_mut(key)
            .expect("path template cache entry inserted");
        let image = if entry.source.dimensions() == dimensions {
            to_gray(entry.source.as_ref())
        } else {
            entry
                .source
                .resize(
                    dimensions.0,
                    dimensions.1,
                    image::imageops::FilterType::Triangle,
                )
                .to_luma8()
        };
        let prepared = Arc::new(build_prepared_template(image)?);
        let prepared_bytes = prepared_memory_bytes(&prepared);
        entry.prepared.insert(dimensions, prepared.clone());
        entry.memory_bytes = entry.memory_bytes.saturating_add(prepared_bytes);
        (prepared, prepared_bytes)
    };
    cache.total_bytes = cache.total_bytes.saturating_add(prepared_bytes);
    let used = cache_tick(&mut cache);
    if let Some(entry) = cache.path_entries.get_mut(key) {
        entry.last_used = used;
    }
    evict_template_cache(&mut cache);
    Ok(prepared)
}

/// 主动使单个模板文件及其父目录的短名解析缓存失效。
///
/// 覆盖、重命名和删除后都可以调用此方法；文件不存在时仍会清理旧路径键。
/// mtime/size/内容哈希和目录代数仍保留，作为调用方漏发通知时的兜底。
pub fn invalidate_template_cache_path(path: &Path) {
    let normalized = normalize_path(path);
    let current_hash = std::fs::read(path).ok().map(|bytes| template_key(&bytes));
    let mut cache = template_cache().lock();
    let mut hashes = current_hash.into_iter().collect::<HashSet<_>>();
    let path_keys: Vec<_> = cache
        .path_entries
        .keys()
        .filter(|key| key.path == normalized)
        .cloned()
        .collect();
    for key in path_keys {
        hashes.insert(key.content_hash);
        if let Some(entry) = cache.path_entries.remove(&key) {
            cache.total_bytes = cache.total_bytes.saturating_sub(entry.memory_bytes);
        }
    }
    remove_content_cache_hashes(&mut cache, hashes);
    invalidate_resolve_cache_dir_locked(&mut cache, normalized.parent());
}

/// 主动使模板目录中的文件缓存和短名解析代数缓存失效。
///
/// 上传、覆盖、重命名、删除等目录操作完成后调用即可；下次路径匹配会重新
/// 读取并预处理，短名解析也会重新枚举目录。
pub fn invalidate_template_cache_dir(dir: &Path) {
    let normalized = normalize_path(dir);
    let mut cache = template_cache().lock();
    let mut hashes = HashSet::new();
    let path_keys: Vec<_> = cache
        .path_entries
        .keys()
        .filter(|key| key.path.parent() == Some(normalized.as_path()))
        .cloned()
        .collect();
    for key in path_keys {
        hashes.insert(key.content_hash);
        if let Some(entry) = cache.path_entries.remove(&key) {
            cache.total_bytes = cache.total_bytes.saturating_sub(entry.memory_bytes);
        }
    }
    invalidate_resolve_cache_dir_locked(&mut cache, Some(normalized.as_path()));
    remove_content_cache_hashes(&mut cache, hashes);
}

#[allow(dead_code)]
fn remove_content_cache_hashes(cache: &mut TemplateCache, hashes: HashSet<[u8; 32]>) {
    for hash in hashes {
        if let Some(entry) = cache.entries.remove(&hash) {
            cache.total_bytes = cache.total_bytes.saturating_sub(entry.memory_bytes);
        }
    }
}

#[allow(dead_code)]
fn invalidate_resolve_cache_dir_locked(cache: &mut TemplateCache, dir: Option<&Path>) {
    let Some(dir) = dir else {
        return;
    };
    let resolve_keys: Vec<_> = cache
        .path_resolve_entries
        .keys()
        .filter(|key| key.dir == dir)
        .cloned()
        .collect();
    for key in resolve_keys {
        if let Some(entry) = cache.path_resolve_entries.remove(&key) {
            cache.total_bytes = cache.total_bytes.saturating_sub(entry.memory_bytes);
        }
    }
}

#[allow(dead_code)]
fn cached_resolved_template_file(dir: &Path, template: &str) -> anyhow::Result<PathBuf> {
    let (dir, dir_generation) = dir_signature(dir)?;
    let key = TemplateResolveKey {
        dir: dir.clone(),
        dir_generation,
        template: template.to_string(),
    };
    let mut cache = template_cache().lock();
    if let Some(resolved) = cache
        .path_resolve_entries
        .get(&key)
        .map(|entry| (*entry.resolved).clone())
    {
        let used = cache_tick(&mut cache);
        if let Some(entry) = cache.path_resolve_entries.get_mut(&key) {
            entry.last_used = used;
        }
        return Ok(resolved);
    }
    let resolved = resolve_template_file_impl(dir.as_path(), template)?;
    let memory_bytes = resolved.as_os_str().len().saturating_add(template.len());
    let used = cache_tick(&mut cache);
    cache.path_resolve_entries.insert(
        key,
        TemplateResolveEntry {
            resolved: Arc::new(resolved.clone()),
            memory_bytes,
            last_used: used,
        },
    );
    cache.total_bytes = cache.total_bytes.saturating_add(memory_bytes);
    evict_template_cache(&mut cache);
    Ok(resolved)
}

#[allow(dead_code)]
fn resolve_template_file_impl(tpl_dir: &Path, template: &str) -> anyhow::Result<PathBuf> {
    let exact = tpl_dir.join(template);
    if exact.is_file() {
        return Ok(exact);
    }
    let Some((base, ext)) = template.rsplit_once('.') else {
        anyhow::bail!("模板 {} 不存在 (path={})", template, exact.display());
    };
    let mut cands = Vec::new();
    for entry in std::fs::read_dir(tpl_dir)? {
        let entry = entry?;
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        let Some(name) = p.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some((stem, e)) = name.rsplit_once('.') else {
            continue;
        };
        if e.eq_ignore_ascii_case(ext)
            && stem
                .strip_prefix(base)
                .is_some_and(|suffix| suffix.starts_with('#'))
        {
            cands.push(p);
        }
    }
    match cands.len() {
        0 => anyhow::bail!("模板 {} 不存在 (path={})", template, exact.display()),
        1 => Ok(cands.remove(0)),
        _ => anyhow::bail!("模板 {} 短名匹配到多个候选：{}", template, exact.display()),
    }
}

pub fn match_template(req: &MatchRequest) -> anyhow::Result<Option<MatchResult>> {
    let started = (matcher_stats().now)();
    let screen = decode_image_limited(&req.screen_png, SCREEN_MAX_INPUT_BYTES, "截图")?.to_rgb8();
    let (template_key, template_source) = cached_template_source(&req.template_png)?;

    match_template_with_source(
        &screen,
        req.threshold,
        req.region,
        template_key,
        template_source,
        None,
        started,
    )
}

/// 通过规范化路径读取并匹配模板。路径入口的缓存键包含路径、mtime、文件大小
/// 和内容哈希；现有字节入口保持兼容，不改变 MatchRequest 或调用方语义。
#[allow(dead_code)]
pub fn match_template_from_path(
    screen_png: &[u8],
    template_path: &Path,
    threshold: Option<f32>,
    region: Option<[u32; 4]>,
) -> anyhow::Result<Option<MatchResult>> {
    let started = (matcher_stats().now)();
    let screen = decode_image_limited(screen_png, SCREEN_MAX_INPUT_BYTES, "截图")?.to_rgb8();
    let (template_key, template_source, path_key) =
        cached_template_source_from_path_key(template_path)?;

    match_template_with_source(
        &screen,
        threshold,
        region,
        template_key,
        template_source,
        Some(&path_key),
        started,
    )
}

fn match_template_with_source(
    screen: &image::RgbImage,
    threshold: Option<f32>,
    region: Option<[u32; 4]>,
    template_key: [u8; 32],
    template_source: Arc<DynamicImage>,
    path_key: Option<&TemplatePathKey>,
    started: Instant,
) -> anyhow::Result<Option<MatchResult>> {
    let (sw, sh) = (screen.width(), screen.height());
    let (tw, th) = template_source.dimensions();
    if tw >= sw || th >= sh {
        anyhow::bail!("template larger than screen");
    }

    // 搜索区域
    let (rx0, ry0, rx1, ry1) = match region {
        Some([x, y, w, h]) => (x, y, (x + w).min(sw), (y + h).min(sh)),
        None => (0, 0, sw, sh),
    };
    if rx1 <= rx0 || ry1 <= ry0 || rx1 - rx0 < tw || ry1 - ry0 < th {
        anyhow::bail!("invalid search region");
    }

    // 有搜索区域时用原始分辨率精匹配（区域小、小模板更准）；无区域全图搜索时缩到 ≤540 保证性能
    let scale = if region.is_some() {
        1.0
    } else {
        (540.0 / sw.max(sh) as f32).min(1.0)
    };
    let (sw2, sh2) = (
        (sw as f32 * scale).max(1.0) as u32,
        (sh as f32 * scale).max(1.0) as u32,
    );
    let screen_small = if scale < 1.0 {
        DynamicImage::ImageRgb8(screen.clone()).resize(
            sw2,
            sh2,
            image::imageops::FilterType::Triangle,
        )
    } else {
        DynamicImage::ImageRgb8(screen.clone())
    };
    let screen_gray = to_gray(&screen_small);
    let template_dimensions = if scale < 1.0 {
        let (tw2, th2) = (
            (tw as f32 * scale).max(1.0) as u32,
            (th as f32 * scale).max(1.0) as u32,
        );
        (tw2, th2)
    } else {
        (tw, th)
    };
    let prepared = match path_key {
        Some(key) => {
            cached_prepared_template_from_path_key(key, &template_source, template_dimensions)?
        }
        None => cached_prepared_template(template_key, &template_source, template_dimensions)?,
    };

    let (sw2, sh2) = screen_gray.dimensions();
    let (tw2, th2) = prepared.image.dimensions();
    if tw2 >= sw2 || th2 >= sh2 {
        // 缩放后模板仍过大，直接失败
        anyhow::bail!("template too large after scaling");
    }

    let t_data = prepared.data.as_slice();
    let t_mean = prepared.mean;
    let t_var = prepared.var;

    // 区域映射到缩放坐标系（上界截断到缩放后图像尺寸，防止浮点误差越界）
    let (rx0s, ry0s) = ((rx0 as f32 * scale) as u32, (ry0 as f32 * scale) as u32);
    let (rx1s, ry1s) = (
        ((rx1 as f32 * scale) as u32).min(sw2),
        ((ry1 as f32 * scale) as u32).min(sh2),
    );
    if rx1s <= rx0s || ry1s <= ry0s || rx1s - rx0s < tw2 || ry1s - ry0s < th2 {
        return Ok(None);
    }
    let x_range = rx0s..=rx1s.saturating_sub(tw2);
    let y_range = ry0s..=ry1s.saturating_sub(th2);

    let s_raw = screen_gray.as_raw();
    let s_w = sw2 as usize;
    let t_w = tw2 as usize;
    let t_h = th2 as usize;

    // NCC 滑动窗口（步长 2，最后精化到相邻像素）
    let step = 2usize;
    let xs: Vec<u32> = x_range.clone().step_by(step).collect();
    let ys: Vec<u32> = y_range.clone().step_by(step).collect();

    let best: Option<(f32, usize, usize)> = xs
        .par_iter()
        .map(|&x0| {
            let x0 = x0 as usize;
            let mut local_best: Option<(f32, usize, usize)> = None;
            for &y0 in &ys {
                let y0 = y0 as usize;
                let score = ncc_at(s_raw, s_w, t_data, t_w, t_h, x0, y0, t_mean, t_var);
                if local_best.is_none_or(|(b, _, _)| score > b) {
                    local_best = Some((score, x0, y0));
                }
            }
            local_best
        })
        .reduce(
            || None,
            |a, b| match (a, b) {
                (Some((s1, x1, y1)), Some((s2, x2, y2))) => {
                    Some(if s1 >= s2 { (s1, x1, y1) } else { (s2, x2, y2) })
                }
                (Some(v), None) | (None, Some(v)) => Some(v),
                (None, None) => None,
            },
        );

    let Some((score, bx, by)) = best else {
        return Ok(None);
    };

    // 邻域精化（±1 像素）
    let mut best_score = score;
    let mut best_pos = (bx, by);
    for dx in -1i32..=1 {
        for dy in -1i32..=1 {
            let nx = bx as i32 + dx;
            let ny = by as i32 + dy;
            if nx < rx0s as i32 || ny < ry0s as i32 {
                continue;
            }
            let nx = nx as usize;
            let ny = ny as usize;
            if nx + t_w > rx1s as usize || ny + t_h > ry1s as usize {
                continue;
            }
            let s = ncc_at(s_raw, s_w, t_data, t_w, t_h, nx, ny, t_mean, t_var);
            if s > best_score {
                best_score = s;
                best_pos = (nx, ny);
            }
        }
    }

    let threshold = threshold.unwrap_or(0.8);
    let result = if best_score < threshold {
        None
    } else {
        // 映射回原始坐标系
        let inv = 1.0 / scale;
        let (ox, oy) = (
            (best_pos.0 as f32 * inv) as u32,
            (best_pos.1 as f32 * inv) as u32,
        );
        Some(MatchResult {
            x: ox,
            y: oy,
            width: (tw2 as f32 * inv) as u32,
            height: (th2 as f32 * inv) as u32,
            score: best_score,
        })
    };
    let duration_ms = started.elapsed().as_millis() as u64;
    // NCC 观测唯一计数点：字节入口 match_template / 路径入口
    // match_template_from_path / 计算池（matcher::compute::run 只移动执行位置，
    // 提交的闭包仍执行本函数）全部汇聚于此——一次匹配请求只记一次，不存在
    // 调用点与池内工作函数双层重复计数。命中/未命中 = 阈值判定结果
    // （result.is_some()），区域/全屏 = 是否传了搜索区域（region.is_some()），
    // 口径与 metrics.rs 的 ncc_* 字段定义一致。
    matcher_stats().record_ncc(duration_ms, result.is_some(), region.is_some());
    Ok(result)
}

/// 计算 (x0, y0) 处的 NCC
#[allow(clippy::too_many_arguments)]
fn ncc_at(
    s_raw: &[u8],
    s_w: usize,
    t_data: &[f32],
    t_w: usize,
    t_h: usize,
    x0: usize,
    y0: usize,
    t_mean: f32,
    t_var: f32,
) -> f32 {
    // 防御：窗口超出图像范围直接返回 -1（浮点坐标截断可能差 1px）
    if x0 + t_w > s_w || y0 + t_h > s_raw.len() / s_w.max(1) {
        return -1.0;
    }
    let mut sum_i = 0f32;
    let mut sum_i2 = 0f32;
    let mut sum_it = 0f32;
    let n = (t_w * t_h) as f32;
    for ty in 0..t_h {
        let row = (y0 + ty) * s_w + x0;
        let t_row = ty * t_w;
        for tx in 0..t_w {
            let iv = s_raw[row + tx] as f32 / 255.0;
            let tv = t_data[t_row + tx];
            sum_i += iv;
            sum_i2 += iv * iv;
            sum_it += iv * tv;
        }
    }
    let i_var = sum_i2 - sum_i * sum_i / n;
    if i_var < 1e-9 {
        return -1.0;
    }
    let cov = sum_it - sum_i * t_mean;
    cov / (i_var * t_var).sqrt()
}

fn to_gray(img: &DynamicImage) -> GrayImage {
    match img {
        DynamicImage::ImageLuma8(g) => g.clone(),
        _ => img.to_luma8(),
    }
}

/// 模板落盘重编码：任意图片字节 → 8-bit 灰度 PNG（最高压缩 + 自适应滤波）。
///
/// 匹配只消费灰度（match_template 内统一 to_luma8），存灰度 = 存匹配器
/// 实际读取的像素值——对匹配**零损失**（区域匹配逐位一致；全图缩放路径
/// 仅存在 ±1 灰度级的滤波舍入差，对 NCC 分数影响 <0.001），体积较
/// RGB PNG（尤其画布直出 PNG）典型下降 60~75%。缩略图/预览变灰是
/// 已知取舍：颜色信息匹配从不使用（选型依据：灰度图上 WebP 无损相对
/// PNG 无优势，无需引入新解码依赖）。JPEG 上传模板顺带摆脱再压缩损伤。
///
/// 资源防护（阶段 2 SEC-004）：解码前字节数预检 + image crate 解码限额
/// （单边尺寸/总分配）+ 解码后像素总量复核——三层挡"像素炸弹"
/// （小体积声明超大分辨率，数十倍放大内存占用）。超限报清晰 4xx 文案。
pub fn reencode_template_gray_png(bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    let img = decode_image_limited(bytes, TEMPLATE_MAX_INPUT_BYTES, "图片")?;
    let gray = img.to_luma8();
    let mut out = Vec::new();
    let enc = image::codecs::png::PngEncoder::new_with_quality(
        &mut out,
        image::codecs::png::CompressionType::Best,
        image::codecs::png::FilterType::Adaptive,
    );
    DynamicImage::ImageLuma8(gray)
        .write_with_encoder(enc)
        .map_err(|e| anyhow::anyhow!("PNG 编码失败: {}", e))?;
    Ok(out)
}

/// 在所有图片解码入口统一执行字节数、单边尺寸、解码分配和像素总量限制。
/// `image::load_from_memory` 本身不带这些业务上限，不能直接用于来自设备、ZIP
/// 或 HTTP 的不可信字节。
fn decode_image_limited(
    bytes: &[u8],
    max_input_bytes: usize,
    label: &str,
) -> anyhow::Result<DynamicImage> {
    if bytes.len() > max_input_bytes {
        let limit_label = if label == "图片" || label == "模板" {
            "上传上限"
        } else {
            "输入上限"
        };
        anyhow::bail!(
            "{} {} 字节超过{} {} MiB",
            label,
            bytes.len(),
            limit_label,
            max_input_bytes / (1024 * 1024)
        );
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(TEMPLATE_MAX_DIM);
    limits.max_image_height = Some(TEMPLATE_MAX_DIM);
    // 灰度/RGB/RGBA 多通道叠加的内存上界（≈8 字节/像素）
    limits.max_alloc = Some(TEMPLATE_MAX_PIXELS * 8);
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes));
    reader.limits(limits);
    let img = reader
        .with_guessed_format()
        .map_err(|e| anyhow::anyhow!("识别{}格式失败: {e}", label))?
        .decode()
        .map_err(|e| match e {
            image::ImageError::Limits(_) => anyhow::anyhow!(
                "{}尺寸或内存占用超限（像素预算 {TEMPLATE_MAX_PIXELS_MB} MP / 单边 ≤{TEMPLATE_MAX_DIM}px），疑似像素炸弹，已拒绝",
                label
            ),
            other => anyhow::anyhow!("不是有效的{}: {}", label, other),
        })?;
    if img.width() as u64 * img.height() as u64 > TEMPLATE_MAX_PIXELS {
        anyhow::bail!(
            "{} {}x{} 共 {:.1} MP 超过像素预算 {} MP",
            label,
            img.width(),
            img.height(),
            img.width() as f64 * img.height() as f64 / 1_000_000.0,
            TEMPLATE_MAX_PIXELS_MB
        );
    }
    Ok(img)
}

/// 模板上传原始字节上限（10MiB；ZIP 导入内模板条目同一口径；api 层 base64
/// 护栏同源于此值）
pub(crate) const TEMPLATE_MAX_INPUT_BYTES: usize = 10 * 1024 * 1024;
/// 截图输入上限：设备截图通常大于模板，但仍必须有字节硬限，避免不可信
/// 响应体在解码前无限增长。
const SCREEN_MAX_INPUT_BYTES: usize = 32 * 1024 * 1024;
/// 像素总量预算 32MP（典型截图 1080p≈2MP、4K≈8.3MP，富余 3 倍以上）
const TEMPLATE_MAX_PIXELS: u64 = 32_000_000;
/// 像素预算换算的展示值（MP）
const TEMPLATE_MAX_PIXELS_MB: u64 = TEMPLATE_MAX_PIXELS / 1_000_000;
/// 图片单边硬上限：直接拒绝畸变分辨率（解码器在触达该限后中止分配）
const TEMPLATE_MAX_DIM: u32 = 16384;

// Arc 辅助（为后续缓存接口预留）
#[allow(dead_code)]
pub type SharedMatcher = Arc<Matcher>;

#[derive(Default)]
pub struct Matcher;

// 统一匹配入口（预留挂载点）：当前引擎直接调用自由函数 match_template，
// 该对象为模板缓存/计算池阶段预留的统一入口（见 docs/OPTIMIZATION_PLAN.md §11），
// 预留期仅 tests 间接触达
#[allow(dead_code)]
impl Matcher {
    pub fn new() -> Self {
        Self
    }
    pub fn find(&self, req: &MatchRequest) -> anyhow::Result<Option<MatchResult>> {
        match_template(req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};
    use std::hint::black_box;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    static TEST_GUARD: Mutex<()> = Mutex::new(());
    static TEST_HITS: AtomicU64 = AtomicU64::new(0);
    static TEST_MISSES: AtomicU64 = AtomicU64::new(0);
    static TEST_REGIONS: AtomicU64 = AtomicU64::new(0);
    static TEST_FULLSCREEN: AtomicU64 = AtomicU64::new(0);
    static TEST_DURATION_MS: AtomicU64 = AtomicU64::new(0);

    fn percentile(samples: &[u128], p: f64) -> u128 {
        let mut sorted = samples.to_vec();
        sorted.sort_unstable();
        let rank = ((sorted.len() as f64) * p).ceil() as usize;
        sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
    }

    #[derive(Clone, Copy)]
    struct PerfSample {
        wall_us: u128,
        cpu_us: Option<u128>,
        peak_mem_bytes: Option<u64>,
    }

    fn format_optional<T: std::fmt::Display>(value: Option<T>) -> String {
        value
            .map(|value| value.to_string())
            .unwrap_or_else(|| "未实测".to_string())
    }

    fn perf_report(metric: &str, samples: &[PerfSample]) {
        assert!(!samples.is_empty(), "{metric} 没有样本");
        let wall: Vec<_> = samples.iter().map(|sample| sample.wall_us).collect();
        let cpu: Vec<_> = samples.iter().filter_map(|sample| sample.cpu_us).collect();
        let peak_mem = samples
            .iter()
            .filter_map(|sample| sample.peak_mem_bytes)
            .max();
        let cpu_ms = if cpu.is_empty() {
            "未实测".to_string()
        } else {
            format!("{:.3}", cpu.iter().sum::<u128>() as f64 / 1000.0)
        };
        println!(
            "PERF metric={} samples={} p50_us={} p95_us={} max_us={} cpu_ms={} cpu_p50_us={} cpu_p95_us={} cpu_max_us={} peak_mem_bytes={} platform={}",
            metric,
            samples.len(),
            percentile(&wall, 0.50),
            percentile(&wall, 0.95),
            wall.iter().copied().max().unwrap_or_default(),
            cpu_ms,
            format_optional(cpu.first().map(|_| percentile(&cpu, 0.50))),
            format_optional(cpu.first().map(|_| percentile(&cpu, 0.95))),
            format_optional(cpu.iter().copied().max()),
            format_optional(peak_mem),
            std::env::consts::OS,
        );
    }

    #[cfg(unix)]
    #[repr(C)]
    struct TimeVal {
        tv_sec: std::os::raw::c_long,
        tv_usec: std::os::raw::c_long,
    }

    #[cfg(unix)]
    #[repr(C)]
    struct ResourceUsage {
        ru_utime: TimeVal,
        ru_stime: TimeVal,
        ru_maxrss: std::os::raw::c_long,
        ru_ixrss: std::os::raw::c_long,
        ru_idrss: std::os::raw::c_long,
        ru_isrss: std::os::raw::c_long,
        ru_minflt: std::os::raw::c_long,
        ru_majflt: std::os::raw::c_long,
        ru_nswap: std::os::raw::c_long,
        ru_inblock: std::os::raw::c_long,
        ru_oublock: std::os::raw::c_long,
        ru_msgsnd: std::os::raw::c_long,
        ru_msgrcv: std::os::raw::c_long,
        ru_nsignals: std::os::raw::c_long,
        ru_nvcsw: std::os::raw::c_long,
        ru_nivcsw: std::os::raw::c_long,
    }

    #[cfg(unix)]
    fn resource_usage() -> Option<ResourceUsage> {
        unsafe extern "C" {
            fn getrusage(
                who: std::os::raw::c_int,
                usage: *mut ResourceUsage,
            ) -> std::os::raw::c_int;
        }
        let mut usage = std::mem::MaybeUninit::<ResourceUsage>::uninit();
        let status = unsafe { getrusage(0, usage.as_mut_ptr()) };
        (status == 0).then(|| unsafe { usage.assume_init() })
    }

    #[cfg(unix)]
    fn process_cpu_time_ns() -> Option<u128> {
        let usage = resource_usage()?;
        let to_ns = |time: TimeVal| {
            (time.tv_sec.max(0) as u128) * 1_000_000_000 + (time.tv_usec.max(0) as u128) * 1_000
        };
        Some(to_ns(usage.ru_utime) + to_ns(usage.ru_stime))
    }

    #[cfg(unix)]
    fn process_peak_memory_bytes() -> Option<u64> {
        let usage = resource_usage()?;
        let maxrss = usage.ru_maxrss.max(0) as u64;
        #[cfg(target_os = "macos")]
        {
            Some(maxrss)
        }
        #[cfg(not(target_os = "macos"))]
        {
            Some(maxrss.saturating_mul(1024))
        }
    }

    #[cfg(windows)]
    fn process_cpu_time_ns() -> Option<u128> {
        #[repr(C)]
        #[derive(Clone, Copy)]
        struct FileTime {
            low: u32,
            high: u32,
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentProcess() -> *mut std::ffi::c_void;
            fn GetProcessTimes(
                process: *mut std::ffi::c_void,
                creation: *mut FileTime,
                exit: *mut FileTime,
                kernel: *mut FileTime,
                user: *mut FileTime,
            ) -> i32;
        }
        let mut creation = FileTime { low: 0, high: 0 };
        let mut exit = FileTime { low: 0, high: 0 };
        let mut kernel = FileTime { low: 0, high: 0 };
        let mut user = FileTime { low: 0, high: 0 };
        let ok = unsafe {
            GetProcessTimes(
                GetCurrentProcess(),
                &mut creation,
                &mut exit,
                &mut kernel,
                &mut user,
            )
        };
        if ok == 0 {
            return None;
        }
        let to_ns = |time: FileTime| (((time.high as u64) << 32 | time.low as u64) as u128) * 100;
        Some(to_ns(kernel) + to_ns(user))
    }

    #[cfg(windows)]
    fn process_peak_memory_bytes() -> Option<u64> {
        #[repr(C)]
        struct ProcessMemoryCounters {
            cb: u32,
            page_fault_count: u32,
            peak_working_set_size: usize,
            working_set_size: usize,
            quota_peak_paged_pool_usage: usize,
            quota_paged_pool_usage: usize,
            quota_peak_non_paged_pool_usage: usize,
            quota_non_paged_pool_usage: usize,
            pagefile_usage: usize,
            peak_pagefile_usage: usize,
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentProcess() -> *mut std::ffi::c_void;
        }
        #[link(name = "psapi")]
        unsafe extern "system" {
            fn GetProcessMemoryInfo(
                process: *mut std::ffi::c_void,
                counters: *mut ProcessMemoryCounters,
                size: u32,
            ) -> i32;
        }
        let mut counters = ProcessMemoryCounters {
            cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
            page_fault_count: 0,
            peak_working_set_size: 0,
            working_set_size: 0,
            quota_peak_paged_pool_usage: 0,
            quota_paged_pool_usage: 0,
            quota_peak_non_paged_pool_usage: 0,
            quota_non_paged_pool_usage: 0,
            pagefile_usage: 0,
            peak_pagefile_usage: 0,
        };
        let ok = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };
        (ok != 0).then_some(counters.peak_working_set_size as u64)
    }

    #[cfg(not(any(unix, windows)))]
    fn process_cpu_time_ns() -> Option<u128> {
        None
    }

    #[cfg(not(any(unix, windows)))]
    fn process_peak_memory_bytes() -> Option<u64> {
        None
    }

    fn sample_sync<T>(operation: impl FnOnce() -> T) -> (T, PerfSample) {
        let cpu_before = process_cpu_time_ns();
        let started = Instant::now();
        let value = operation();
        let wall_us = started.elapsed().as_micros();
        let cpu_us = cpu_before.and_then(|before| {
            process_cpu_time_ns().map(|after| after.saturating_sub(before) / 1000)
        });
        (
            value,
            PerfSample {
                wall_us,
                cpu_us,
                peak_mem_bytes: process_peak_memory_bytes(),
            },
        )
    }

    async fn sample_async<T>(operation: impl std::future::Future<Output = T>) -> (T, PerfSample) {
        let cpu_before = process_cpu_time_ns();
        let started = Instant::now();
        let value = operation.await;
        let wall_us = started.elapsed().as_micros();
        let cpu_us = cpu_before.and_then(|before| {
            process_cpu_time_ns().map(|after| after.saturating_sub(before) / 1000)
        });
        (
            value,
            PerfSample {
                wall_us,
                cpu_us,
                peak_mem_bytes: process_peak_memory_bytes(),
            },
        )
    }

    fn split_perf_gop(stream: &[u8]) -> (Vec<u8>, Vec<crate::device::scrcpy::VideoFrame>) {
        use crate::device::scrcpy::VideoFrame;

        let mut starts = Vec::new();
        let mut index = 0;
        while index + 3 <= stream.len() {
            let prefix = if stream[index..].starts_with(&[0, 0, 0, 1]) {
                4
            } else if stream[index..].starts_with(&[0, 0, 1]) {
                3
            } else {
                index += 1;
                continue;
            };
            starts.push((index, prefix));
            index += prefix;
        }

        let mut config = Vec::new();
        let mut frames = Vec::new();
        let mut current = Vec::new();
        let mut current_has_slice = false;
        let mut current_is_keyframe = false;
        for (position, (start, prefix)) in starts.iter().enumerate() {
            let end = starts
                .get(position + 1)
                .map(|(next, _)| *next)
                .unwrap_or(stream.len());
            let nal = &stream[start + prefix..end];
            let nal_type = nal.first().copied().unwrap_or_default() & 0x1f;
            let first_slice =
                matches!(nal_type, 1 | 5) && nal.get(1).is_some_and(|byte| byte & 0x80 != 0);
            if matches!(nal_type, 7 | 8) && frames.is_empty() && current.is_empty() {
                config.extend_from_slice(&stream[*start..end]);
                continue;
            }
            if first_slice && current_has_slice {
                frames.push(VideoFrame {
                    data: std::mem::take(&mut current),
                    pts_us: frames.len() as u64 * 33_333,
                    is_config: false,
                    is_keyframe: current_is_keyframe,
                    annex_b: true,
                });
                current_has_slice = false;
                current_is_keyframe = false;
            }
            if first_slice {
                current_has_slice = true;
                current_is_keyframe = nal_type == 5;
            }
            current.extend_from_slice(&stream[*start..end]);
        }
        if !current.is_empty() {
            frames.push(VideoFrame {
                data: current,
                pts_us: frames.len() as u64 * 33_333,
                is_config: false,
                is_keyframe: current_is_keyframe,
                annex_b: true,
            });
        }
        (config, frames)
    }

    fn encode_png(image: &RgbImage) -> Vec<u8> {
        let mut out = Vec::new();
        image
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    fn encode_luma_png(image: &GrayImage) -> Vec<u8> {
        let mut out = Vec::new();
        DynamicImage::ImageLuma8(image.clone())
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    /// 手工构造最小 PNG（签名 + IHDR + IEND），IHDR 声明指定分辨率——
    /// 合法头但超大声明：解码器在分配任何像素缓冲前即被限额拦截
    fn pixel_bomb_png(width: u32, height: u32) -> Vec<u8> {
        fn crc32(data: &[u8]) -> u32 {
            let mut crc: u32 = 0xFFFF_FFFF;
            for &b in data {
                crc ^= b as u32;
                for _ in 0..8 {
                    let mask = (crc & 1).wrapping_neg();
                    crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
                }
            }
            !crc
        }
        let mut out = vec![137, 80, 78, 71, 13, 10, 26, 10];
        let ihdr = [
            (13u32.to_be_bytes().as_slice()),
            b"IHDR".as_slice(),
            &width.to_be_bytes()[..],
            &height.to_be_bytes()[..],
            &[8u8, 0, 0, 0, 0], // 8bit 灰度，无压缩/滤波/隔行修饰位全默认
        ]
        .concat();
        out.extend_from_slice(&ihdr);
        out.extend_from_slice(&crc32(&ihdr[4..]).to_be_bytes());
        out.extend_from_slice(&0u32.to_be_bytes()); // IEND 长度
        out.extend_from_slice(b"IEND");
        out.extend_from_slice(&crc32(b"IEND").to_be_bytes());
        out
    }

    #[test]
    fn pixel_bomb_and_oversize_input_rejected_before_allocation() {
        // ① 像素炸弹：几百字节的 PNG 声明 30000x30000（≈900MP >> 32MP 预算）
        let bomb = pixel_bomb_png(30_000, 30_000);
        assert!(bomb.len() < 256, "样本必须是极小体积的大图");
        let err = reencode_template_gray_png(&bomb).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("超限") && msg.contains("像素"), "{msg}");

        // ② 单边畸变分辨率（65535px）同样在解码前被拒
        let err = reencode_template_gray_png(&pixel_bomb_png(65_535, 5)).unwrap_err();
        assert!(err.to_string().contains("超限"), "{}", err);

        // ③ 字节数预检：>10MiB 的垃圾输入不解码直接拒绝
        let junk = vec![0u8; 10 * 1024 * 1024 + 128];
        let err = reencode_template_gray_png(&junk).unwrap_err();
        assert!(err.to_string().contains("上传上限"), "{}", err);

        // ④ 无效数据仍然报原本的"不是有效的图片"
        assert!(reencode_template_gray_png(b"not an image").is_err());
    }

    #[test]
    fn match_template_applies_decode_limits_to_screen_and_template() {
        let _lock = TEST_GUARD.lock().unwrap();
        let mut screen = RgbImage::new(10, 10);
        for (_, _, p) in screen.enumerate_pixels_mut() {
            *p = Rgb([10, 20, 30]);
        }
        let mut screen_png = Vec::new();
        screen
            .write_to(
                &mut std::io::Cursor::new(&mut screen_png),
                image::ImageFormat::Png,
            )
            .unwrap();

        let oversized_template = vec![0u8; TEMPLATE_MAX_INPUT_BYTES + 1];
        let err = match_template(&MatchRequest {
            screen_png: screen_png.clone(),
            template_png: oversized_template,
            threshold: None,
            region: None,
        })
        .unwrap_err();
        assert!(err.to_string().contains("模板"), "{}", err);

        let err = match_template(&MatchRequest {
            screen_png: pixel_bomb_png(30_000, 30_000),
            template_png: screen_png,
            threshold: None,
            region: None,
        })
        .unwrap_err();
        assert!(err.to_string().contains("截图"), "{}", err);
    }

    #[test]
    fn legit_large_template_within_budget_passes_guard() {
        // 2048x2048（4MP）灰度渐变图在预算内，完整走通重编码
        let mut img = GrayImage::new(2048, 2048);
        for y in 0..64 {
            for x in 0..64 {
                img.put_pixel(x, y, image::Luma([((x * y) % 255) as u8]));
            }
        }
        let mut src = Vec::new();
        DynamicImage::ImageLuma8(img)
            .write_to(&mut std::io::Cursor::new(&mut src), image::ImageFormat::Png)
            .unwrap();
        let out = reencode_template_gray_png(&src).unwrap();
        assert!(!out.is_empty());
    }

    #[test]
    fn test_template_match_hit() {
        let _lock = TEST_GUARD.lock().unwrap();
        // 400x600 截图：紫底 + 绿色方块
        let mut screen = RgbImage::new(400, 600);
        for (_, _, p) in screen.enumerate_pixels_mut() {
            *p = Rgb([40, 20, 80]);
        }
        for y in 200..260 {
            for x in 150..230 {
                screen.put_pixel(x, y, Rgb([30, 200, 90]));
            }
        }
        // 模板 60x60：跨绿色方块与紫色背景边界（有纹理）
        let mut tpl = RgbImage::new(60, 60);
        for y in 0..60 {
            for x in 0..60 {
                tpl.put_pixel(x, y, *screen.get_pixel(140 + x, 190 + y));
            }
        }
        let screen_bytes = encode_png(&screen);
        let tpl_bytes = encode_png(&tpl);

        let req = MatchRequest {
            screen_png: screen_bytes,
            template_png: tpl_bytes,
            threshold: Some(0.9),
            region: None,
        };
        let m = match_template(&req).unwrap().expect("should hit");
        assert!((m.x as i64 - 140).abs() <= 2, "x={}", m.x);
        assert!((m.y as i64 - 190).abs() <= 2, "y={}", m.y);
        assert!(m.score > 0.9, "score={}", m.score);
    }

    #[test]
    fn test_template_reencode_gray_roundtrip() {
        // RGB 渐变图重编码后：解码灰度 == 原图直接 to_luma8（匹配零损失的核心不变式）
        let mut tpl = RgbImage::new(64, 48);
        for (x, y, p) in tpl.enumerate_pixels_mut() {
            *p = Rgb([(x * 4) as u8, (y * 5) as u8, ((x + y) * 2) as u8]);
        }
        let mut src = Vec::new();
        tpl.write_to(&mut std::io::Cursor::new(&mut src), image::ImageFormat::Png)
            .unwrap();
        let out = reencode_template_gray_png(&src).unwrap();
        let expect = DynamicImage::ImageRgb8(tpl).to_luma8();
        let got = image::load_from_memory(&out).unwrap().to_luma8();
        assert_eq!(got.dimensions(), expect.dimensions());
        assert_eq!(got.into_raw(), expect.into_raw());
        // 无效输入报错而非 panic
        assert!(reencode_template_gray_png(b"not an image").is_err());
    }

    #[test]
    fn test_template_match_miss() {
        let _lock = TEST_GUARD.lock().unwrap();
        let mut screen = RgbImage::new(200, 200);
        for (_, _, p) in screen.enumerate_pixels_mut() {
            *p = Rgb([10, 10, 10]);
        }
        let mut tpl = RgbImage::new(20, 20);
        for (x, y, p) in tpl.enumerate_pixels_mut() {
            // 棋盘纹理：避免纯色（NCC 对方差 0 的模板无意义）
            let v = if (x + y) % 2 == 0 { 200u8 } else { 100u8 };
            *p = Rgb([v, v, v]);
        }
        let screen_bytes = encode_png(&screen);
        let tpl_bytes = encode_png(&tpl);
        let req = MatchRequest {
            screen_png: screen_bytes,
            template_png: tpl_bytes,
            threshold: Some(0.9),
            region: None,
        };
        assert!(match_template(&req).unwrap().is_none());
    }

    #[test]
    fn matcher_stats_hook_records_success_miss_and_region() {
        let _lock = TEST_GUARD.lock().unwrap();
        TEST_HITS.store(0, Ordering::Relaxed);
        TEST_MISSES.store(0, Ordering::Relaxed);
        TEST_REGIONS.store(0, Ordering::Relaxed);
        TEST_FULLSCREEN.store(0, Ordering::Relaxed);
        TEST_DURATION_MS.store(0, Ordering::Relaxed);

        fn record(duration_ms: u64, hit: bool, region: bool) {
            TEST_DURATION_MS.fetch_add(duration_ms, Ordering::Relaxed);
            if hit {
                TEST_HITS.fetch_add(1, Ordering::Relaxed);
            } else {
                TEST_MISSES.fetch_add(1, Ordering::Relaxed);
            }
            if region {
                TEST_REGIONS.fetch_add(1, Ordering::Relaxed);
            } else {
                TEST_FULLSCREEN.fetch_add(1, Ordering::Relaxed);
            }
        }

        let stats = test_matcher_stats(Instant::now, record);
        let _guard = install_matcher_stats(stats);

        let mut screen = GrayImage::new(160, 120);
        for (_, _, p) in screen.enumerate_pixels_mut() {
            *p = image::Luma([20]);
        }
        for y in 40..70 {
            for x in 50..90 {
                let v = ((x - 50) * 7 + (y - 40) * 11) as u8;
                let gray = if (x + y) % 2 == 0 {
                    40u8.saturating_add(v / 2)
                } else {
                    180u8.saturating_add(v / 3)
                };
                screen.put_pixel(x, y, image::Luma([gray]));
            }
        }
        let mut tpl = GrayImage::new(18, 18);
        for y in 0..18 {
            for x in 0..18 {
                tpl.put_pixel(x, y, *screen.get_pixel(54 + x, 44 + y));
            }
        }
        let hit_req = MatchRequest {
            screen_png: encode_luma_png(&screen),
            template_png: encode_luma_png(&tpl),
            threshold: Some(0.9),
            region: None,
        };
        assert!(match_template(&hit_req).unwrap().is_some());

        let mut miss_tpl = GrayImage::new(18, 18);
        for (x, y, p) in miss_tpl.enumerate_pixels_mut() {
            let gray = if (x + y) % 2 == 0 { 235 } else { 245 };
            *p = image::Luma([gray]);
        }
        let miss_req = MatchRequest {
            screen_png: hit_req.screen_png.clone(),
            template_png: encode_luma_png(&miss_tpl),
            threshold: Some(0.99),
            region: None,
        };
        assert!(match_template(&miss_req).unwrap().is_none());

        let region_req = MatchRequest {
            screen_png: hit_req.screen_png,
            template_png: encode_luma_png(&tpl),
            threshold: Some(0.9),
            region: Some([42, 38, 44, 44]),
        };
        assert!(match_template(&region_req).unwrap().is_some());

        // 命中/全屏只断言下界：无锁并发的计算池测试会额外产生全屏命中，
        // 精确断言与本测试的窗口存在竞态（偶发 3≠2，2026-08-30 实证）；
        // 未命中/区域无并发写入者，精确断言保证分类口径不串。
        assert!(TEST_HITS.load(Ordering::Relaxed) >= 2);
        assert_eq!(TEST_MISSES.load(Ordering::Relaxed), 1);
        assert_eq!(TEST_REGIONS.load(Ordering::Relaxed), 1);
        assert!(TEST_FULLSCREEN.load(Ordering::Relaxed) >= 2);
        assert!(TEST_DURATION_MS.load(Ordering::Relaxed) > 0);
    }

    /// 生产接线（OBS）：不安装测试钩子时，默认统计必须把 NCC 观测写入进程级
    /// 共享 metrics（GET /metrics 的数据源），且命中/未命中与区域/全屏分类
    /// 口径正确。持 TEST_GUARD 排除本模块其余真实匹配测试；唯一无锁的并发
    /// 写入者是计算池测试（只产生全屏命中），故未命中与区域分类的增量可
    /// 精确断言，命中/全屏只断言下界。
    #[test]
    fn production_matches_record_into_global_metrics() {
        let _lock = TEST_GUARD.lock().unwrap();
        let metrics = crate::metrics::global();
        let before = metrics.snapshot();

        // 480x360 截图（全屏路径 scale=1.0，无缩放）+ 自截图裁切的 60x60 模板
        let mut screen = RgbImage::new(480, 360);
        for (_, _, p) in screen.enumerate_pixels_mut() {
            *p = Rgb([40, 20, 80]);
        }
        for y in 120..180 {
            for x in 150..230 {
                let v = ((x - 150) * 5 + (y - 120) * 9) as u8;
                let gray = if (x + y) % 2 == 0 {
                    40u8.saturating_add(v / 2)
                } else {
                    180u8.saturating_add(v / 3)
                };
                screen.put_pixel(x, y, Rgb([gray, gray, gray]));
            }
        }
        let mut tpl = RgbImage::new(60, 60);
        for y in 0..60 {
            for x in 0..60 {
                tpl.put_pixel(x, y, *screen.get_pixel(140 + x, 115 + y));
            }
        }
        let hit_req = MatchRequest {
            screen_png: encode_png(&screen),
            template_png: encode_png(&tpl),
            threshold: Some(0.9),
            region: None,
        };
        assert!(match_template(&hit_req).unwrap().is_some(), "全屏应命中");

        let mut miss_tpl = RgbImage::new(24, 24);
        for (x, y, p) in miss_tpl.enumerate_pixels_mut() {
            let v = if (x + y) % 2 == 0 { 235 } else { 245 };
            *p = Rgb([v, v, v]);
        }
        let miss_req = MatchRequest {
            screen_png: hit_req.screen_png.clone(),
            template_png: encode_png(&miss_tpl),
            threshold: Some(0.99),
            region: None,
        };
        assert!(match_template(&miss_req).unwrap().is_none(), "应未命中");

        // 搜索区域覆盖模板真实位置 (140, 115)：区域路径命中 → 计入 ncc_region
        let region_req = MatchRequest {
            screen_png: hit_req.screen_png.clone(),
            template_png: encode_png(&tpl),
            threshold: Some(0.9),
            region: Some([100, 90, 220, 180]),
        };
        assert!(match_template(&region_req).unwrap().is_some(), "区域应命中");

        let after = metrics.snapshot();
        let delta = |later: u64, earlier: u64| later.saturating_sub(earlier);
        assert!(
            delta(after.ncc_matches_total, before.ncc_matches_total) >= 3,
            "本测试 3 次匹配至少应计入 ncc_matches_total"
        );
        assert!(
            delta(after.ncc_hits_total, before.ncc_hits_total) >= 2,
            "两次命中应计入 ncc_hits_total"
        );
        assert_eq!(
            delta(after.ncc_misses_total, before.ncc_misses_total),
            1,
            "未命中增量应恰为 1（真实匹配测试均持 TEST_GUARD，无锁并发只可能产生命中）"
        );
        assert_eq!(
            delta(after.ncc_region_total, before.ncc_region_total),
            1,
            "区域分类增量应恰为 1（其余真实匹配均为全屏）"
        );
        assert!(
            delta(after.ncc_fullscreen_total, before.ncc_fullscreen_total) >= 2,
            "两次全屏匹配应计入 ncc_fullscreen_total"
        );
        assert!(
            delta(after.ncc_duration_ms_total, before.ncc_duration_ms_total) >= 1,
            "耗时累计应增长"
        );
    }

    #[test]
    fn template_cache_reuses_decoded_source_and_statistics() {
        let mut tpl = GrayImage::new(17, 13);
        for (x, y, pixel) in tpl.enumerate_pixels_mut() {
            pixel.0[0] = ((x * 17 + y * 31) % 251) as u8;
        }
        let mut bytes = Vec::new();
        DynamicImage::ImageLuma8(tpl)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        let key = template_key(&bytes);
        template_cache().lock().entries.remove(&key);

        let (key1, source1) = cached_template_source(&bytes).unwrap();
        let (key2, source2) = cached_template_source(&bytes).unwrap();
        assert_eq!(key1, key2);
        assert!(Arc::ptr_eq(&source1, &source2));

        let prepared1 = cached_prepared_template(key1, &source1, (17, 13)).unwrap();
        let prepared2 = cached_prepared_template(key2, &source2, (17, 13)).unwrap();
        assert!(Arc::ptr_eq(&prepared1, &prepared2));
        assert_eq!(prepared1.image.dimensions(), (17, 13));
        assert_eq!(prepared1.data.len(), 17 * 13);
        assert!(prepared1.var > 1e-6);
    }

    #[test]
    fn short_name_generation_and_delete_invalidation_clear_resolver_cache() {
        let _lock = TEST_GUARD.lock().unwrap();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "gamer-matcher-short-name-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&dir).unwrap();
        let first = dir.join("login#u.png");
        let second = dir.join("login#d.png");
        std::fs::write(&first, b"placeholder").unwrap();

        let resolved = cached_resolved_template_file(&dir, "login.png").unwrap();
        assert_eq!(resolved, normalize_path(&first));

        // Directory generation changes after a short-name candidate is added, so the
        // stale unique resolution cannot be reused and the duplicate is rejected.
        std::fs::write(&second, b"placeholder").unwrap();
        assert!(cached_resolved_template_file(&dir, "login.png").is_err());

        invalidate_template_cache_dir(&dir);
        assert!(template_cache()
            .lock()
            .path_resolve_entries
            .keys()
            .all(|key| key.dir != normalize_path(&dir)));

        std::fs::remove_file(&second).unwrap();
        assert_eq!(
            cached_resolved_template_file(&dir, "login.png").unwrap(),
            normalize_path(&first)
        );

        std::fs::remove_file(&first).unwrap();
        invalidate_template_cache_path(&first);
        assert!(template_cache()
            .lock()
            .path_resolve_entries
            .keys()
            .all(|key| key.dir != normalize_path(&dir)));
        let _ = std::fs::remove_dir(&dir);
    }

    /// PERF-002 回归：同名覆盖上传 + 主动失效后，路径匹配必须使用新内容。
    /// 截图左右各放一块互为反相的棋盘纹理，v1/v2 模板分别只在各自位置命中；
    /// 覆盖后旧路径键与旧内容缓存被清空，再次匹配命中点移到 v2 位置。
    #[test]
    fn overwrite_same_name_template_and_invalidate_matches_new_content() {
        let _lock = TEST_GUARD.lock().unwrap();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "gamer-matcher-overwrite-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&dir).unwrap();

        // 400x200 截图：左块 (40..100, 70..130) 棋盘 A，右块 (300..360, 70..130) 反相棋盘
        let mut screen = RgbImage::new(400, 200);
        for (_, _, p) in screen.enumerate_pixels_mut() {
            *p = Rgb([128, 128, 128]);
        }
        for y in 70..130 {
            for x in 40..100 {
                let v = if (x + y) % 2 == 0 { 255 } else { 60 };
                screen.put_pixel(x, y, Rgb([v, v, v]));
            }
        }
        for y in 70..130 {
            for x in 300..360 {
                let v = if (x + y) % 2 == 0 { 60 } else { 255 };
                screen.put_pixel(x, y, Rgb([v, v, v]));
            }
        }
        let crop = |x0: u32, y0: u32| {
            let mut tpl = RgbImage::new(60, 60);
            for y in 0..60 {
                for x in 0..60 {
                    tpl.put_pixel(x, y, *screen.get_pixel(x0 + x, y0 + y));
                }
            }
            encode_png(&tpl)
        };
        let v1 = crop(40, 70);
        let v2 = crop(300, 70);
        let screen_png = encode_png(&screen);
        let path = dir.join("cover.png");

        // ① 首次“上传”v1 并匹配：命中左块，路径缓存已建立
        std::fs::write(&path, &v1).unwrap();
        let m1 = match_template_from_path(&screen_png, &path, Some(0.9), None)
            .unwrap()
            .expect("v1 应命中");
        assert!(
            (m1.x as i64 - 40).abs() <= 2 && (m1.y as i64 - 70).abs() <= 2,
            "v1 命中点应在左块: {:?}",
            (m1.x, m1.y)
        );
        assert!(template_cache()
            .lock()
            .path_entries
            .keys()
            .any(|key| key.path == normalize_path(&path)));

        // ② 同名覆盖为 v2（模拟上传覆盖）+ 主动失效：路径键与旧内容缓存清空
        std::fs::write(&path, &v2).unwrap();
        invalidate_template_cache_path(&path);
        assert!(template_cache()
            .lock()
            .path_entries
            .keys()
            .all(|key| key.path != normalize_path(&path)));

        // ③ 立刻再匹配必须用新内容：命中点移到右块
        let m2 = match_template_from_path(&screen_png, &path, Some(0.9), None)
            .unwrap()
            .expect("v2 应命中");
        assert!(
            (m2.x as i64 - 300).abs() <= 2 && (m2.y as i64 - 70).abs() <= 2,
            "覆盖后应命中右块: {:?}",
            (m2.x, m2.y)
        );
        assert!(
            (m2.x as i64 - m1.x as i64).abs() > 100,
            "新旧命中点应明显不同: {:?} vs {:?}",
            (m1.x, m1.y),
            (m2.x, m2.y)
        );

        std::fs::remove_file(&path).unwrap();
        let _ = std::fs::remove_dir(&dir);
    }

    /// 固定 fixture 的离线基准。每个指标输出墙钟 p50/p95/max、CPU 时间分位数、
    /// CPU 总耗时和峰值内存；当前平台无法读取资源时输出“未实测”。
    /// 默认测区域路径；设置 GAMER_PERF_FULL_SCREEN=1 追加全屏 NCC。
    #[tokio::test]
    #[ignore = "运行 tools/run-perf-benchmark.ps1 或设置 GAMER_PERF_ITERS 后执行"]
    async fn fixed_fixture_benchmark_p50_p95() {
        let _lock = TEST_GUARD.lock().unwrap();
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/perf");
        let screen = std::fs::read(dir.join("keyframe_001.png"))
            .expect("读取固定夹具 keyframe_001.png 失败");
        let stream = std::fs::read(dir.join("stream.h264")).expect("读取固定夹具 stream.h264 失败");
        let (config, gop) = split_perf_gop(&stream);
        assert!(!config.is_empty(), "固定 H.264 fixture 缺少 SPS/PPS");
        assert!(!gop.is_empty(), "固定 H.264 fixture 未解析出视频帧");
        let ffmpeg = std::env::var("GAMER_PERF_FFMPEG").unwrap_or_else(|_| "ffmpeg".to_string());
        let cases = [
            (
                "tmpl/perf_btn_primary#361_365_639_479.png",
                [390, 700, 300, 220],
            ),
            (
                "tmpl/perf_txt_status#130_219_185_240.png",
                [140, 420, 60, 40],
            ),
            ("tmpl/perf_corner_menu#dr.png", [540, 960, 540, 960]),
        ];
        let templates: Vec<_> = cases
            .iter()
            .map(|(relative, _)| {
                std::fs::read(dir.join(relative))
                    .unwrap_or_else(|_| panic!("读取固定夹具 {} 失败", relative))
            })
            .collect();
        let iterations = std::env::var("GAMER_PERF_ITERS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(20);
        let warmup = std::env::var("GAMER_PERF_WARMUP")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(3);

        let full_screen = std::env::var_os("GAMER_PERF_FULL_SCREEN").is_some();
        let mut reports: HashMap<&'static str, Vec<PerfSample>> = HashMap::new();
        let mut push = |metric: &'static str, sample: PerfSample| {
            reports.entry(metric).or_default().push(sample);
        };
        let run_match = |screen: &[u8], template: &[u8], region: Option<[u32; 4]>| {
            match_template(&MatchRequest {
                screen_png: screen.to_vec(),
                template_png: template.to_vec(),
                threshold: Some(0.8),
                region,
            })
        };
        // 后续循环含 await，std MutexGuard 不能跨 await（clippy await_holding_lock）；
        // 本基准默认 #[ignore] 经 perf 脚本独占运行，统计隔离另有 MatcherStatsGuard
        // RAII 兜底，此处提前放锁不影响其余测试
        drop(_lock);

        for _ in 0..warmup {
            crate::device::frames::FrameCache::benchmark_decode_latest_png(&ffmpeg, &config, &gop)
                .await
                .expect("固定 GOP 解码失败");
            let decoded = image::load_from_memory(&screen).expect("PNG fixture 解码失败");
            black_box(decoded.to_luma8());
            for ((_, region), template) in cases.iter().zip(templates.iter()) {
                run_match(&screen, template, Some(*region))
                    .expect("区域 NCC 失败")
                    .expect("区域 NCC 未命中");
                if full_screen {
                    run_match(&screen, template, None)
                        .expect("全屏 NCC 失败")
                        .expect("全屏 NCC 未命中");
                }
            }
        }

        for _ in 0..iterations {
            let (decoded, decode_sample) = sample_async(
                crate::device::frames::FrameCache::benchmark_decode_latest_png(
                    &ffmpeg, &config, &gop,
                ),
            )
            .await;
            black_box(decoded.expect("固定 GOP 解码失败"));
            push("decode_latest_png", decode_sample);

            let (decoded, png_decode_sample) = sample_sync(|| {
                black_box(image::load_from_memory(&screen).expect("PNG fixture 解码失败"))
            });
            push("png_decode", png_decode_sample);
            let (_, grayscale_sample) = sample_sync(|| black_box(decoded.to_luma8()));
            push("png_grayscale", grayscale_sample);

            for ((relative, region), template) in cases.iter().zip(templates.iter()) {
                let (_, read_sample) = sample_sync(|| {
                    black_box(std::fs::read(dir.join(relative)).expect("模板 fixture 读取失败"))
                });
                push("template_read", read_sample);

                let (_, preprocess_sample) = sample_sync(|| {
                    let source = decode_image_limited(template, TEMPLATE_MAX_INPUT_BYTES, "模板")
                        .expect("模板 PNG 解码失败");
                    black_box(build_prepared_template(to_gray(&source)).expect("模板预处理失败"))
                });
                push("template_preprocess", preprocess_sample);

                let (_, region_sample) = sample_sync(|| {
                    black_box(
                        run_match(&screen, template, Some(*region))
                            .expect("区域 NCC 失败")
                            .expect("区域 NCC 未命中"),
                    )
                });
                push("ncc_region", region_sample);

                if full_screen {
                    let (_, full_sample) = sample_sync(|| {
                        black_box(
                            run_match(&screen, template, None)
                                .expect("全屏 NCC 失败")
                                .expect("全屏 NCC 未命中"),
                        )
                    });
                    push("ncc_fullscreen", full_sample);
                }
            }

            let (_, find_sample) = sample_sync(|| {
                let main = run_match(&screen, &templates[0], Some(cases[0].1))
                    .expect("find 主模板 NCC 失败");
                black_box(main);
                for ((_, region), template) in cases.iter().skip(1).zip(templates.iter().skip(1)) {
                    black_box(
                        run_match(&screen, template, Some(*region)).expect("find block NCC 失败"),
                    );
                }
            });
            push("find_round", find_sample);
        }

        for metric in [
            "decode_latest_png",
            "png_decode",
            "png_grayscale",
            "ncc_region",
            "ncc_fullscreen",
            "template_read",
            "template_preprocess",
            "find_round",
        ] {
            if let Some(samples) = reports.get(metric) {
                perf_report(metric, samples);
            }
        }
        println!(
            "PERF fixture=server/testdata/perf iterations={} warmup={} platform={} full_screen={} gop_frames={}",
            iterations,
            warmup,
            std::env::consts::OS,
            full_screen,
            gop.len(),
        );
    }
}
