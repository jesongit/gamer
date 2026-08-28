//! 模板匹配引擎：灰度 + 归一化互相关（NCC）滑动窗口
//!
//! 性能策略：截图先等比缩放到 ≤540px 宽（模板同比例），步长采样 + rayon 并行，
//! 1080p 全图 + 小模板典型耗时 100~400ms；支持搜索区域裁剪进一步加速。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use image::{DynamicImage, GenericImageView, GrayImage};
use parking_lot::Mutex;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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
    bytes_len: usize,
    last_used: u64,
}

#[derive(Default)]
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
        record_ncc: MatcherStats::record_ncc_noop,
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

    fn record_ncc_noop(_: u64, _: bool, _: bool) {}

    fn record_ncc(&self, duration_ms: u64, hit: bool, region: bool) {
        (self.record_ncc)(duration_ms, hit, region);
    }
}

struct MatcherStatsGuard {
    prev: *mut MatcherStats,
    current: *mut MatcherStats,
}

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
    bytes_len: usize,
    prepared: HashMap<(u32, u32), Arc<PreparedTemplate>>,
    last_used: u64,
}

#[derive(Hash, Eq, PartialEq, Clone)]
struct TemplateResolveKey {
    dir: PathBuf,
    dir_mtime_ns: u128,
    dir_size: u64,
    template: String,
}

struct TemplateResolveEntry {
    resolved: Arc<PathBuf>,
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

fn metadata_key(path: &Path, meta: &std::fs::Metadata, content_hash: [u8; 32]) -> TemplatePathKey {
    TemplatePathKey {
        path: normalize_path(path),
        mtime_ns: file_mtime_ns(meta),
        size: meta.len(),
        content_hash,
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn file_mtime_ns(meta: &std::fs::Metadata) -> u128 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn dir_signature(dir: &Path) -> anyhow::Result<(PathBuf, u128, u64)> {
    let meta = std::fs::metadata(dir)?;
    Ok((normalize_path(dir), file_mtime_ns(&meta), meta.len()))
}

/// 获取 PNG 解码后的源灰度图。锁只覆盖一次性的模板解码，命中时仅复制 Arc。
fn cached_template_source(bytes: &[u8]) -> anyhow::Result<([u8; 32], Arc<DynamicImage>)> {
    let key = template_key(bytes);
    let mut cache = template_cache().lock();
    if !cache.entries.contains_key(&key) {
        let source = decode_image_limited(bytes, TEMPLATE_MAX_INPUT_BYTES, "模板")?;
        let bytes_len = bytes.len();
        if cache.entries.len() >= TEMPLATE_CACHE_CAPACITY {
            if let Some(evicted) = cache.entries.keys().next().copied() {
                if let Some(old) = cache.entries.remove(&evicted) {
                    cache.total_bytes = cache.total_bytes.saturating_sub(old.bytes_len);
                }
            }
        }
        let used = cache_tick(&mut cache);
        cache.entries.insert(
            key,
            TemplateCacheEntry {
                source: Arc::new(source),
                prepared: HashMap::new(),
                bytes_len,
                last_used: used,
            },
        );
        cache.total_bytes = cache.total_bytes.saturating_add(bytes_len);
        evict_template_cache(&mut cache);
    }
    let source = cache
        .entries
        .get(&key)
        .expect("template cache entry inserted")
        .source
        .clone();
    let used = cache_tick(&mut cache);
    if let Some(entry) = cache.entries.get_mut(&key) {
        entry.last_used = used;
    }
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
    let prepared = {
        let entry = cache
            .entries
            .entry(key)
            .or_insert_with(|| TemplateCacheEntry {
                source: source.clone(),
                prepared: HashMap::new(),
                bytes_len: 0,
                last_used: 0,
            });
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
        entry.prepared.insert(dimensions, prepared.clone());
        prepared
    };
    let used = cache_tick(&mut cache);
    if let Some(entry) = cache.entries.get_mut(&key) {
        entry.last_used = used;
    }
    Ok(prepared)
}

fn evict_template_cache(cache: &mut TemplateCache) {
    while cache.total_bytes > TEMPLATE_CACHE_MAX_BYTES {
        let Some((old_key, _)) = cache
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(k, v)| (*k, v.last_used))
        else {
            break;
        };
        if let Some(removed) = cache.entries.remove(&old_key) {
            cache.total_bytes = cache.total_bytes.saturating_sub(removed.bytes_len);
        } else {
            break;
        }
    }
}

fn cached_template_source_from_path_key(
    path: &Path,
) -> anyhow::Result<([u8; 32], Arc<DynamicImage>, TemplatePathKey)> {
    let bytes = std::fs::read(path)?;
    let meta = std::fs::metadata(path)?;
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
    let source = decode_image_limited(&bytes, TEMPLATE_MAX_INPUT_BYTES, "模板")?;
    let bytes_len = bytes.len();
    let used = cache_tick(&mut cache);
    cache.path_entries.insert(
        key.clone(),
        TemplatePathEntry {
            source: Arc::new(source),
            bytes_len,
            prepared: HashMap::new(),
            last_used: used,
        },
    );
    cache.total_bytes = cache.total_bytes.saturating_add(bytes_len);
    evict_template_cache(&mut cache);
    let source = cache
        .path_entries
        .get(&key)
        .expect("path cache inserted")
        .source
        .clone();
    Ok((key.content_hash, source, key))
}

fn cached_resolved_template_file(dir: &Path, template: &str) -> anyhow::Result<PathBuf> {
    let (dir, dir_mtime_ns, dir_size) = dir_signature(dir)?;
    let dir = dir;
    let key = TemplateResolveKey {
        dir: dir.clone(),
        dir_mtime_ns,
        dir_size,
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
    let used = cache_tick(&mut cache);
    cache.path_resolve_entries.insert(
        key,
        TemplateResolveEntry {
            resolved: Arc::new(resolved.clone()),
            last_used: used,
        },
    );
    Ok(resolved)
}

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
        if e.eq_ignore_ascii_case(ext) && stem.starts_with(base) {
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

    let (sw, sh) = (screen.width(), screen.height());
    let (tw, th) = template_source.dimensions();
    if tw >= sw || th >= sh {
        anyhow::bail!("template larger than screen");
    }

    // 搜索区域
    let (rx0, ry0, rx1, ry1) = match req.region {
        Some([x, y, w, h]) => (x, y, (x + w).min(sw), (y + h).min(sh)),
        None => (0, 0, sw, sh),
    };
    if rx1 <= rx0 || ry1 <= ry0 || rx1 - rx0 < tw || ry1 - ry0 < th {
        anyhow::bail!("invalid search region");
    }

    // 有搜索区域时用原始分辨率精匹配（区域小、小模板更准）；无区域全图搜索时缩到 ≤540 保证性能
    let scale = if req.region.is_some() {
        1.0
    } else {
        (540.0 / sw.max(sh) as f32).min(1.0)
    };
    let (sw2, sh2) = (
        (sw as f32 * scale).max(1.0) as u32,
        (sh as f32 * scale).max(1.0) as u32,
    );
    let screen_small = if scale < 1.0 {
        DynamicImage::ImageRgb8(screen).resize(sw2, sh2, image::imageops::FilterType::Triangle)
    } else {
        DynamicImage::ImageRgb8(screen)
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
    let prepared = cached_prepared_template(template_key, &template_source, template_dimensions)?;

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

    let threshold = req.threshold.unwrap_or(0.8);
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
    matcher_stats().record_ncc(duration_ms, result.is_some(), req.region.is_some());
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
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    static TEST_GUARD: Mutex<()> = Mutex::new(());
    static TEST_HITS: AtomicU64 = AtomicU64::new(0);
    static TEST_MISSES: AtomicU64 = AtomicU64::new(0);
    static TEST_REGIONS: AtomicU64 = AtomicU64::new(0);
    static TEST_FULLSCREEN: AtomicU64 = AtomicU64::new(0);
    static TEST_DURATION_MS: AtomicU64 = AtomicU64::new(0);

    fn percentile(samples: &mut [u128], p: f64) -> u128 {
        samples.sort_unstable();
        let rank = ((samples.len() as f64) * p).ceil() as usize;
        samples[rank.saturating_sub(1).min(samples.len() - 1)]
    }

    fn perf_report(metric: &str, samples: &mut [u128]) {
        let p50 = percentile(samples, 0.50);
        let p95 = percentile(samples, 0.95);
        let max = samples.last().copied().unwrap_or_default();
        println!(
            "PERF metric={} samples={} p50_us={} p95_us={} max_us={}",
            metric,
            samples.len(),
            p50,
            p95,
            max
        );
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

        assert_eq!(TEST_HITS.load(Ordering::Relaxed), 2);
        assert_eq!(TEST_MISSES.load(Ordering::Relaxed), 1);
        assert_eq!(TEST_REGIONS.load(Ordering::Relaxed), 1);
        assert_eq!(TEST_FULLSCREEN.load(Ordering::Relaxed), 2);
        assert!(TEST_DURATION_MS.load(Ordering::Relaxed) > 0);
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

    /// 固定 PNG 夹具的真实匹配基准。默认只测带区域元数据的区域搜索，避免普通
    /// 单元测试意外运行数十秒；设置 GAMER_PERF_FULL_SCREEN=1 才额外测全屏路径。
    /// 输出为机器可读的 p50/p95/max 微秒值，不包含任何预设或伪造的性能数据。
    #[test]
    #[ignore = "运行 tools/run-perf-benchmark.ps1 或设置 GAMER_PERF_ITERS 后执行"]
    fn fixed_fixture_benchmark_p50_p95() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/perf");
        let screen = std::fs::read(dir.join("keyframe_001.png"))
            .expect("读取固定夹具 keyframe_001.png 失败");
        let cases = [
            (
                "region_big",
                "tmpl/perf_btn_primary#361_365_639_479.png",
                [390, 700, 300, 220],
            ),
            (
                "region_small",
                "tmpl/perf_txt_status#130_219_185_240.png",
                [140, 420, 60, 40],
            ),
            (
                "region_corner",
                "tmpl/perf_corner_menu#dr.png",
                [540, 960, 540, 960],
            ),
        ];
        let iterations = std::env::var("GAMER_PERF_ITERS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(20);
        let warmup = std::env::var("GAMER_PERF_WARMUP")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(3);

        for (metric, template_rel, region) in cases {
            let template = std::fs::read(dir.join(template_rel))
                .unwrap_or_else(|_| panic!("读取固定夹具 {} 失败", template_rel));
            let mut request = MatchRequest {
                screen_png: screen.clone(),
                template_png: template,
                threshold: Some(0.8),
                region: Some(region),
            };
            for _ in 0..warmup {
                assert!(
                    match_template(&request).unwrap().is_some(),
                    "{} 未命中",
                    metric
                );
            }
            let mut samples = Vec::with_capacity(iterations);
            for _ in 0..iterations {
                let started = Instant::now();
                assert!(
                    match_template(&request).unwrap().is_some(),
                    "{} 未命中",
                    metric
                );
                samples.push(started.elapsed().as_micros());
            }
            perf_report(metric, &mut samples);

            if std::env::var_os("GAMER_PERF_FULL_SCREEN").is_some() {
                request.region = None;
                for _ in 0..warmup {
                    assert!(
                        match_template(&request).unwrap().is_some(),
                        "{} 未命中全屏路径",
                        metric
                    );
                }
                let mut full_samples = Vec::with_capacity(iterations);
                for _ in 0..iterations {
                    let started = Instant::now();
                    assert!(
                        match_template(&request).unwrap().is_some(),
                        "{} 未命中全屏路径",
                        metric
                    );
                    full_samples.push(started.elapsed().as_micros());
                }
                perf_report(&format!("{}_full_screen", metric), &mut full_samples);
            }
        }
    }
}
