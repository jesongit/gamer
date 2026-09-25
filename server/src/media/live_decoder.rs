//! One continuous decoder per output. Its latest decoded frame can be repeated
//! without replaying H.264 reference frames (which corrupts prediction chains).
use crate::device::scrcpy::VideoFrame;
use anyhow::{ensure, Context, Result};
use bytes::Bytes;
use parking_lot::Mutex;
use std::{process::Stdio, sync::Arc};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{broadcast, watch},
};

pub(super) async fn run(
    ffmpeg: &str,
    width: u32,
    height: u32,
    initial: Vec<VideoFrame>,
    mut video: broadcast::Receiver<VideoFrame>,
    latest: Arc<Mutex<Option<Bytes>>>,
    mut stop: watch::Receiver<bool>,
) -> Result<()> {
    let frame_size = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(3))
        .context("视频尺寸溢出")?;
    ensure!(
        frame_size > 0 && frame_size <= 64 * 1024 * 1024,
        "视频尺寸过大"
    );
    let config = initial
        .iter()
        .find(|f| f.is_config)
        .context("缺少视频参数")?
        .data
        .clone();
    let mut cmd = tokio::process::Command::new(ffmpeg);
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostdin",
        "-threads",
        "1",
        "-flags",
        "low_delay",
        "-probesize",
        "32",
        "-analyzeduration",
        "0",
        "-f",
        "h264",
        "-i",
        "pipe:0",
        "-an",
        "-fps_mode",
        "passthrough",
        "-pix_fmt",
        "bgr24",
        "-threads",
        "1",
        "-flush_packets",
        "1",
        "-f",
        "rawvideo",
        "pipe:1",
    ])
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    let mut child = cmd.spawn().context("无法启动视频解码进程")?;
    let mut input = child.stdin.take().unwrap();
    let mut output = child.stdout.take().unwrap();
    let feed = async {
        let mut last = None;
        for f in initial {
            input.write_all(&f.data).await?;
            if !f.is_config {
                input.write_all(&[0, 0, 0, 1, 9, 0xf0]).await?;
            }
            if !f.is_config {
                last = Some(f.pts_us);
            }
        }
        loop {
            let frame = video
                .recv()
                .await
                .context("视频流中断或积压，正在重建输出")?;
            if frame.is_config {
                ensure!(frame.data == config, "视频编码变化，正在重建输出");
                continue;
            }
            if last.is_some_and(|pts| frame.pts_us <= pts) {
                continue;
            }
            input.write_all(&frame.data).await?;
            input.write_all(&[0, 0, 0, 1, 9, 0xf0]).await?;
            last = Some(frame.pts_us);
        }
        #[allow(unreachable_code)]
        Ok::<(), anyhow::Error>(())
    };
    let read = async {
        loop {
            let mut pixels = vec![0; frame_size];
            output
                .read_exact(&mut pixels)
                .await
                .context("视频解码进程已结束")?;
            *latest.lock() = Some(Bytes::from(pixels));
        }
        #[allow(unreachable_code)]
        Ok::<(), anyhow::Error>(())
    };
    let result = tokio::select! {result=feed=>result,result=read=>result,_=stop.changed()=>Ok(())};
    let _ = child.kill().await;
    let _ = child.wait().await;
    result
}
