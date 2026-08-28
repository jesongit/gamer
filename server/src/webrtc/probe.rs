//! 编码器输出质量诊断探针（默认关闭，config `probe_encoder`）。
//!
//! 与生产推流隔离：门控（`should_probe_encoder`）关闭时不开线程、不起
//! ffmpeg 进程、不做任何分配；`probe_encoder_blockiness` 只在门控放行的
//! 采样帧上以独立任务运行。

use tracing::warn;

/// 诊断探针的唯一开关：关闭时不应为生产推流创建任何 ffmpeg 任务。
/// frame_no=0 不是推流循环的有效帧号，不能误把普通 P 帧当成采样帧。
pub(super) fn should_probe_encoder(enabled: bool, frame_no: u64, is_keyframe: bool) -> bool {
    enabled && (is_keyframe || (frame_no != 0 && frame_no.is_multiple_of(30)))
}

/// 编码器输出质量探针：用 ffmpeg 解码"原始 H.264 帧（config + 本帧）"，做宏块网格
/// 块效应检测（16px 规则网格边缘强度 vs 非边界基线）。ratio > 1.25 → 该帧是编码器
/// 输出的低质量帧（块效应）。用于区分：编码器坏帧 vs 浏览器解码路径问题。
pub(super) fn probe_encoder_blockiness(
    ffmpeg_path: &str,
    cfg: Option<bytes::Bytes>,
    frame: &[u8],
    frame_no: u64,
    is_key: bool,
    size: usize,
) {
    use std::io::Write;
    let dir = std::env::temp_dir();
    let h264_path = dir.join(format!("gamer-probe-{}.h264", std::process::id()));
    let mut data = Vec::with_capacity(cfg.as_ref().map(|c| c.len()).unwrap_or(0) + frame.len());
    if let Some(c) = &cfg {
        data.extend_from_slice(c);
    }
    data.extend_from_slice(frame);
    if std::fs::File::create(&h264_path)
        .and_then(|mut f| f.write_all(&data))
        .is_err()
    {
        return;
    }
    let out = std::process::Command::new(ffmpeg_path)
        .args(["-y", "-loglevel", "error", "-f", "h264", "-i"])
        .arg(&h264_path)
        .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
        .output();
    let _ = std::fs::remove_file(&h264_path);
    let Ok(out) = out else { return };
    if !out.status.success() || out.stdout.len() < 1000 {
        return;
    }
    let buf = &out.stdout;
    // 从解码输出大小推断分辨率（常见组合）
    let (w, h) = [
        (1920usize, 1080usize),
        (1440, 2560),
        (1080, 2400),
        (1440, 3200),
    ]
    .into_iter()
    .find(|&(w, h)| buf.len() >= w * h * 3)
    .unwrap_or((0, 0));
    if w == 0 || h == 0 {
        return;
    }
    let lum = |x: usize, y: usize| -> f64 {
        let i = (y * w + x) * 3;
        0.299 * buf[i] as f64 + 0.587 * buf[i + 1] as f64 + 0.114 * buf[i + 2] as f64
    };
    let mut edge_diff = 0f64;
    let mut edge_n = 0u64;
    let mut base_diff = 0f64;
    let mut base_n = 0u64;
    // 垂直宏块边界（x=16k）
    let mut x = 16usize;
    while x < w {
        let mut y = 8usize;
        while y + 8 < h {
            let d = (lum(x - 1, y) - lum(x, y)).abs() + (lum(x, y) - lum(x + 1, y)).abs();
            edge_diff += d;
            edge_n += 1;
            y += 4;
        }
        x += 16;
    }
    // 水平宏块边界（y=16k）
    let mut y = 16usize;
    while y < h {
        let mut x = 8usize;
        while x + 8 < w {
            let d = (lum(x, y - 1) - lum(x, y)).abs() + (lum(x, y) - lum(x, y + 1)).abs();
            edge_diff += d;
            edge_n += 1;
            x += 4;
        }
        y += 16;
    }
    // 基线：非边界（偏移 8px）
    let mut x = 24usize;
    while x < w {
        let mut y = 8usize;
        while y + 8 < h {
            let d = (lum(x - 1, y) - lum(x, y)).abs() + (lum(x, y) - lum(x + 1, y)).abs();
            base_diff += d;
            base_n += 1;
            y += 4;
        }
        x += 16;
    }
    let mut y = 24usize;
    while y < h {
        let mut x = 8usize;
        while x + 8 < w {
            let d = (lum(x, y - 1) - lum(x, y)).abs() + (lum(x, y) - lum(x, y + 1)).abs();
            base_diff += d;
            base_n += 1;
            x += 4;
        }
        y += 16;
    }
    if edge_n == 0 || base_n == 0 {
        return;
    }
    let ratio = (edge_diff / edge_n as f64) / (base_diff / base_n as f64);
    if ratio > 1.25 {
        warn!(
            "ENCODER FRAME blockiness: frame_no={} key={} size={} ratio={:.2} {}x{}",
            frame_no, is_key, size, ratio, w, h
        );
    }
}

#[cfg(test)]
mod tests {
    use super::should_probe_encoder;

    #[test]
    fn encoder_probe_gate_is_closed_for_disabled_or_unsampled_frames() {
        assert!(!should_probe_encoder(false, 30, true));
        assert!(should_probe_encoder(true, 0, true));
        assert!(!should_probe_encoder(true, 0, false));
        assert!(should_probe_encoder(true, 30, false));
        assert!(!should_probe_encoder(true, 29, false));
        assert!(!should_probe_encoder(true, 31, false));
    }
}
