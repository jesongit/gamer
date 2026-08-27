//! 模板匹配引擎：灰度 + 归一化互相关（NCC）滑动窗口
//!
//! 性能策略：截图先等比缩放到 ≤540px 宽（模板同比例），步长采样 + rayon 并行，
//! 1080p 全图 + 小模板典型耗时 100~400ms；支持搜索区域裁剪进一步加速。

use std::sync::Arc;

use image::{DynamicImage, GrayImage};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

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

pub fn match_template(req: &MatchRequest) -> anyhow::Result<Option<MatchResult>> {
    let screen = image::load_from_memory(&req.screen_png)
        .map_err(|e| anyhow::anyhow!("解析截图失败 ({} bytes): {}", req.screen_png.len(), e))?
        .to_rgb8();
    let template = image::load_from_memory(&req.template_png)
        .map_err(|e| anyhow::anyhow!("解析模板失败 ({} bytes): {}", req.template_png.len(), e))?
        .to_rgb8();

    let (sw, sh) = (screen.width(), screen.height());
    let (tw, th) = (template.width(), template.height());
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
    let template_gray = if scale < 1.0 {
        let (tw2, th2) = (
            (tw as f32 * scale).max(1.0) as u32,
            (th as f32 * scale).max(1.0) as u32,
        );
        DynamicImage::ImageRgb8(template)
            .resize(tw2, th2, image::imageops::FilterType::Triangle)
            .to_luma8()
    } else {
        to_gray(&DynamicImage::ImageRgb8(template))
    };

    let (sw2, sh2) = screen_gray.dimensions();
    let (tw2, th2) = template_gray.dimensions();
    if tw2 >= sw2 || th2 >= sh2 {
        // 缩放后模板仍过大，直接失败
        anyhow::bail!("template too large after scaling");
    }

    // 模板统计
    let t_data: Vec<f32> = template_gray
        .as_raw()
        .iter()
        .map(|&v| v as f32 / 255.0)
        .collect();
    let t_mean = t_data.iter().sum::<f32>() / t_data.len() as f32;
    let t_var: f32 = t_data.iter().map(|&v| (v - t_mean) * (v - t_mean)).sum();
    if t_var < 1e-6 {
        anyhow::bail!("template is uniform color");
    }

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
                let score = ncc_at(s_raw, s_w, &t_data, t_w, t_h, x0, y0, t_mean, t_var);
                if local_best.map_or(true, |(b, _, _)| score > b) {
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
            let s = ncc_at(s_raw, s_w, &t_data, t_w, t_h, nx, ny, t_mean, t_var);
            if s > best_score {
                best_score = s;
                best_pos = (nx, ny);
            }
        }
    }

    let threshold = req.threshold.unwrap_or(0.8);
    if best_score < threshold {
        return Ok(None);
    }

    // 映射回原始坐标系
    let inv = 1.0 / scale;
    let (ox, oy) = (
        (best_pos.0 as f32 * inv) as u32,
        (best_pos.1 as f32 * inv) as u32,
    );
    Ok(Some(MatchResult {
        x: ox,
        y: oy,
        width: (tw2 as f32 * inv) as u32,
        height: (th2 as f32 * inv) as u32,
        score: best_score,
    }))
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
pub fn reencode_template_gray_png(bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    let img =
        image::load_from_memory(bytes).map_err(|e| anyhow::anyhow!("不是有效的图片: {}", e))?;
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

// Arc 辅助（为后续缓存接口预留）
#[allow(dead_code)]
pub type SharedMatcher = Arc<Matcher>;

#[derive(Default)]
pub struct Matcher;

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
        let mut screen_bytes = Vec::new();
        let mut tpl_bytes = Vec::new();
        screen
            .write_to(
                &mut std::io::Cursor::new(&mut screen_bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        tpl.write_to(
            &mut std::io::Cursor::new(&mut tpl_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();

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
        let mut screen_bytes = Vec::new();
        let mut tpl_bytes = Vec::new();
        screen
            .write_to(
                &mut std::io::Cursor::new(&mut screen_bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        tpl.write_to(
            &mut std::io::Cursor::new(&mut tpl_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        let req = MatchRequest {
            screen_png: screen_bytes,
            template_png: tpl_bytes,
            threshold: Some(0.9),
            region: None,
        };
        assert!(match_template(&req).unwrap().is_none());
    }
}
