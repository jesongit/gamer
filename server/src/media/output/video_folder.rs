//! Local playlist decoding. File paths are process arguments, never shell input.
//! Prepared producers hold their first frame until the output commits admission.
use super::source::{Frame, Source};
use crate::device::scrcpy::AudioFrame;
use anyhow::{ensure, Context, Result};
use parking_lot::Mutex;
use serde::Serialize;
use std::{collections::VecDeque, path::PathBuf, process::Stdio, sync::Arc, time::Duration};
use tokio::{
    io::AsyncReadExt,
    process::{Child, ChildStdout, Command},
    sync::{broadcast, watch},
};

#[derive(Clone, Serialize)]
pub struct VideoFolderInfo {
    pub folder: String,
    pub files: Vec<String>,
}

pub async fn inspect_video_folder(folder: &str) -> Result<VideoFolderInfo> {
    ensure!(!folder.trim().is_empty(), "请填写本地视频文件夹");
    let root = tokio::fs::canonicalize(folder)
        .await
        .context("视频文件夹不存在或无法读取")?;
    ensure!(
        tokio::fs::metadata(&root).await?.is_dir(),
        "指定路径不是文件夹"
    );
    let mut entries = tokio::fs::read_dir(&root)
        .await
        .context("无法读取视频文件夹")?;
    let mut files = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        if !entry.file_type().await?.is_file() {
            continue;
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("视频文件名必须使用有效 Unicode"))?;
        let extension = entry
            .path()
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if matches!(
            extension.as_str(),
            "mp4" | "mkv" | "mov" | "webm" | "avi" | "m4v" | "ts" | "flv" | "wmv" | "mpeg" | "mpg"
        ) {
            files.push(name);
            ensure!(files.len() <= 1000, "视频文件夹最多支持 1000 个视频");
        }
    }
    files.sort();
    ensure!(!files.is_empty(), "视频文件夹内没有支持的视频文件");
    Ok(VideoFolderInfo {
        folder: root.to_string_lossy().into_owned(),
        files,
    })
}

#[derive(Clone, Default)]
struct Cursor {
    index: usize,
    position: f64,
}
#[derive(Default)]
struct Bookmark {
    owner: Option<String>,
    cursor: Cursor,
}
pub(super) struct Playlist {
    root: PathBuf,
    files: Vec<String>,
    bookmark: Mutex<Bookmark>,
}
impl Playlist {
    pub async fn open(folder: &str) -> Result<Arc<Self>> {
        let info = inspect_video_folder(folder).await?;
        Ok(Arc::new(Self {
            root: info.folder.into(),
            files: info.files,
            bookmark: Mutex::new(Bookmark::default()),
        }))
    }
    #[cfg(test)]
    pub fn saved_position(&self) -> f64 {
        self.bookmark.lock().cursor.position
    }
    #[cfg(test)]
    pub fn inactive(&self) -> bool {
        self.bookmark.lock().owner.is_none()
    }
}
pub(super) struct Producer {
    playlist: Arc<Playlist>,
    id: String,
    displayed: Arc<Mutex<Cursor>>,
    activated: watch::Sender<bool>,
}
impl Producer {
    pub fn activate(&self) {
        let mut bookmark = self.playlist.bookmark.lock();
        bookmark.owner = Some(self.id.clone());
        bookmark.cursor = self.displayed.lock().clone();
        let _ = self.activated.send(true);
    }
    pub fn deactivate(&self) {
        let mut bookmark = self.playlist.bookmark.lock();
        if bookmark.owner.as_deref() == Some(&self.id) {
            bookmark.owner = None;
        }
        let _ = self.activated.send(false);
    }
    pub fn presented(&self, frame: &Frame) {
        if let Some((index, position)) = frame.playback {
            let mut bookmark = self.playlist.bookmark.lock();
            if bookmark.owner.as_deref() == Some(&self.id) {
                bookmark.cursor = Cursor { index, position };
            }
        }
    }
    pub fn position(&self) -> (String, f64) {
        let bookmark = self.playlist.bookmark.lock();
        let cursor = if bookmark.owner.as_deref() == Some(&self.id) {
            bookmark.cursor.clone()
        } else {
            self.displayed.lock().clone()
        };
        (self.playlist.files[cursor.index].clone(), cursor.position)
    }
}

pub(super) async fn start(
    ffmpeg: &str,
    playlist: Arc<Playlist>,
    fps: u32,
    audio: bool,
) -> Result<Source> {
    let latest = Arc::new(Mutex::new(None));
    let error = Arc::new(Mutex::new(None));
    let (stop, stopped) = watch::channel(false);
    let (activated, active) = watch::channel(false);
    let (audio_tx, audio_rx) = broadcast::channel(16);
    let initial = playlist.bookmark.lock().cursor.clone();
    let displayed = Arc::new(Mutex::new(initial.clone()));
    let id = uuid::Uuid::new_v4().simple().to_string();
    let producer = Producer {
        playlist: playlist.clone(),
        id: id.clone(),
        displayed: displayed.clone(),
        activated,
    };
    let output = latest.clone();
    let errors = error.clone();
    let executable = ffmpeg.to_string();
    let task = tokio::spawn(async move {
        let result = play(
            &executable,
            playlist,
            id,
            initial,
            fps,
            audio,
            output,
            displayed,
            audio_tx,
            stopped,
            active,
        )
        .await;
        if let Err(failure) = result {
            *errors.lock() = Some(format!("视频轮播失败：{failure:#}"));
        }
    });
    let source = Source {
        device_id: String::new(),
        latest,
        error,
        audio: audio.then_some(audio_rx),
        stop,
        task: Some(task),
        playlist: Some(producer),
    };
    let ready = tokio::time::timeout(Duration::from_secs(18), async {
        loop {
            source.check()?;
            if source.frame().is_some() {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("等待视频文件首帧超时")
    .and_then(|r| r);
    if let Err(error) = ready {
        source.stop().await;
        return Err(error);
    }
    Ok(source)
}

#[allow(clippy::too_many_arguments)]
async fn play(
    ffmpeg: &str,
    playlist: Arc<Playlist>,
    id: String,
    mut cursor: Cursor,
    fps: u32,
    audio: bool,
    latest: Arc<Mutex<Option<Frame>>>,
    displayed: Arc<Mutex<Cursor>>,
    audio_tx: broadcast::Sender<AudioFrame>,
    mut stop: watch::Receiver<bool>,
    active: watch::Receiver<bool>,
) -> Result<()> {
    let mut failed = 0usize;
    loop {
        if *stop.borrow() {
            return Ok(());
        }
        let path = playlist.root.join(&playlist.files[cursor.index]);
        let metadata = tokio::select! {
            _ = stop.changed() => return Ok(()),
            metadata = probe(ffmpeg, &path) => metadata,
        };
        let result = match metadata {
            Ok(metadata) => {
                run_clip(
                    ffmpeg,
                    &path,
                    &metadata,
                    &playlist,
                    &id,
                    &cursor,
                    fps,
                    audio,
                    &latest,
                    &displayed,
                    &audio_tx,
                    &mut stop,
                    active.clone(),
                )
                .await
            }
            Err(error) => Err(error),
        };
        if *stop.borrow() {
            return Ok(());
        }
        match result {
            Ok(()) => failed = 0,
            Err(error) => {
                failed += 1;
                tracing::warn!(file = %path.display(), "视频轮播跳过不可用文件：{error:#}");
                if failed >= playlist.files.len() {
                    return Err(error.context("文件夹内所有视频均不可播放"));
                }
            }
        }
        cursor = Cursor {
            index: (cursor.index + 1) % playlist.files.len(),
            position: 0.0,
        };
    }
}

struct Metadata {
    width: u32,
    height: u32,
    audio: bool,
    duration: f64,
}
async fn probe(ffmpeg: &str, path: &std::path::Path) -> Result<Metadata> {
    let ffprobe = super::super::derive_ffprobe_path(ffmpeg);
    let mut command = process(&ffprobe);
    command
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration:stream=codec_type,width,height:stream_side_data=rotation",
            "-of",
            "json",
        ])
        .arg(path)
        .stdout(Stdio::piped());
    let output = tokio::time::timeout(Duration::from_secs(10), command.output())
        .await
        .context("视频信息读取超时")??;
    ensure!(output.status.success(), "无法读取视频信息");
    let data: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let streams = data["streams"].as_array().context("文件没有视频流")?;
    let video = streams
        .iter()
        .find(|stream| stream["codec_type"] == "video")
        .context("文件没有视频流")?;
    let mut width = video["width"].as_u64().context("视频尺寸未知")? as u32;
    let mut height = video["height"].as_u64().context("视频尺寸未知")? as u32;
    if video["side_data_list"].as_array().is_some_and(|list| {
        list.iter().any(|data| {
            data["rotation"]
                .as_i64()
                .is_some_and(|rotation| rotation.rem_euclid(180) == 90)
        })
    }) {
        std::mem::swap(&mut width, &mut height);
    }
    let size = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(3))
        .context("视频尺寸溢出")?;
    ensure!(size > 0 && size <= 64 * 1024 * 1024, "视频尺寸过大");
    let duration = data["format"]["duration"]
        .as_str()
        .and_then(|duration| duration.parse::<f64>().ok())
        .context("视频时长未知")?;
    ensure!(duration.is_finite() && duration > 0.0, "视频时长无效");
    Ok(Metadata {
        width,
        height,
        audio: streams.iter().any(|stream| stream["codec_type"] == "audio"),
        duration,
    })
}
fn process(executable: &str) -> Command {
    let mut command = Command::new(executable);
    command
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    command
}
fn decoder(ffmpeg: &str, path: &std::path::Path, position: f64) -> Command {
    let mut command = process(ffmpeg);
    command
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-threads",
            "1",
            "-ss",
        ])
        .arg(format!("{position:.6}"))
        .arg("-i")
        .arg(path)
        .stdout(Stdio::piped());
    command
}
async fn activated(active: &mut watch::Receiver<bool>) -> Result<()> {
    while !*active.borrow_and_update() {
        active.changed().await.context("视频来源准备已取消")?;
    }
    Ok(())
}
async fn reap(child: &mut Child) {
    let _ = child.kill().await;
    let _ = child.wait().await;
}

#[allow(clippy::too_many_arguments)]
async fn run_clip(
    ffmpeg: &str,
    path: &std::path::Path,
    metadata: &Metadata,
    playlist: &Playlist,
    id: &str,
    cursor: &Cursor,
    fps: u32,
    audio: bool,
    latest: &Arc<Mutex<Option<Frame>>>,
    displayed: &Arc<Mutex<Cursor>>,
    audio_tx: &broadcast::Sender<AudioFrame>,
    stop: &mut watch::Receiver<bool>,
    mut active: watch::Receiver<bool>,
) -> Result<()> {
    let position = cursor
        .position
        .min((metadata.duration - 1.0 / fps as f64).max(0.0));
    let mut video_command = decoder(ffmpeg, path, position);
    video_command
        .args(["-map", "0:v:0", "-an", "-sn", "-dn", "-vf"])
        .arg(format!(
            "fps={fps},scale={}:{}",
            metadata.width, metadata.height
        ))
        .args([
            "-pix_fmt", "bgr24", "-threads", "1", "-f", "rawvideo", "pipe:1",
        ]);
    let mut video = video_command.spawn().context("无法启动视频文件解码")?;
    let mut pixels = video.stdout.take().context("视频解码输出不可用")?;
    let mut audio_child = if audio && metadata.audio {
        let mut command = decoder(ffmpeg, path, position);
        command.args([
            "-map",
            "0:a:0",
            "-vn",
            "-sn",
            "-dn",
            "-ac",
            "2",
            "-ar",
            "48000",
            "-c:a",
            "libopus",
            "-frame_duration",
            "20",
            "-page_duration",
            "20000",
            "-flush_packets",
            "1",
            "-f",
            "ogg",
            "pipe:1",
        ]);
        match command.spawn() {
            Ok(child) => Some(child),
            Err(error) => {
                reap(&mut video).await;
                return Err(anyhow::Error::from(error).context("无法启动视频音频转码"));
            }
        }
    } else {
        None
    };
    let mut audio_output = audio_child.as_mut().and_then(|child| child.stdout.take());
    let mut audio_active = active.clone();
    let video_loop = async {
        let mut index = 0u64;
        let mut origin = None;
        loop {
            let mut frame = vec![0; metadata.width as usize * metadata.height as usize * 3];
            match pixels.read_exact(&mut frame).await {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                    ensure!(index > 0, "视频文件没有可解码画面");
                    let exit = video.wait().await?;
                    ensure!(exit.success(), "视频文件解码失败");
                    tokio::time::sleep_until(
                        origin.unwrap() + Duration::from_secs_f64(index as f64 / fps as f64),
                    )
                    .await;
                    return Ok::<_, anyhow::Error>(());
                }
                Err(error) => return Err(error.into()),
            }
            if index == 0 {
                *displayed.lock() = Cursor {
                    index: cursor.index,
                    position,
                };
                *latest.lock() = Some(Frame {
                    pixels: frame.into(),
                    width: metadata.width,
                    height: metadata.height,
                    playback: Some((cursor.index, position)),
                });
                activated(&mut active).await?;
                origin = Some(tokio::time::Instant::now());
            } else {
                tokio::time::sleep_until(
                    origin.unwrap() + Duration::from_secs_f64(index as f64 / fps as f64),
                )
                .await;
                *latest.lock() = Some(Frame {
                    pixels: frame.into(),
                    width: metadata.width,
                    height: metadata.height,
                    playback: Some((
                        cursor.index,
                        (position + index as f64 / fps as f64).min(metadata.duration),
                    )),
                });
            }
            let bookmark = playlist.bookmark.lock();
            if bookmark.owner.as_deref() != Some(id) {
                return Ok(());
            }
            *displayed.lock() = Cursor {
                index: cursor.index,
                position: (position + index as f64 / fps as f64).min(metadata.duration),
            };
            index += 1;
        }
    };
    let audio_loop = async {
        if let Some(output) = audio_output.as_mut() {
            let mut ogg = OggPackets::new(output);
            let first = ogg.next().await?.context("视频音轨没有可解码音频")?;
            activated(&mut audio_active).await?;
            let origin = tokio::time::Instant::now();
            let mut packet = Some(first);
            let mut index = 0u64;
            while let Some(data) = packet {
                tokio::time::sleep_until(origin + Duration::from_millis(index * 20)).await;
                if playlist.bookmark.lock().owner.as_deref() != Some(id) {
                    break;
                }
                let _ = audio_tx.send(AudioFrame {
                    data,
                    pts_us: index * 20_000,
                    is_config: false,
                });
                index += 1;
                packet = ogg.next().await?;
            }
        }
        std::future::pending::<Result<()>>().await
    };
    let result = tokio::select! { result = video_loop => result, result = audio_loop => result, _ = stop.changed() => Ok(()) };
    reap(&mut video).await;
    if let Some(child) = audio_child.as_mut() {
        reap(child).await;
    }
    result
}

struct OggPackets<'a> {
    output: &'a mut ChildStdout,
    packets: VecDeque<Vec<u8>>,
    pending: Vec<u8>,
}
impl<'a> OggPackets<'a> {
    fn new(output: &'a mut ChildStdout) -> Self {
        Self {
            output,
            packets: VecDeque::new(),
            pending: vec![],
        }
    }
    async fn next(&mut self) -> Result<Option<Vec<u8>>> {
        loop {
            if let Some(packet) = self.packets.pop_front() {
                if !packet.starts_with(b"OpusHead") && !packet.starts_with(b"OpusTags") {
                    return Ok(Some(packet));
                }
                continue;
            }
            let mut header = [0; 27];
            match self.output.read_exact(&mut header).await {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                    ensure!(self.pending.is_empty(), "Opus 音频包不完整");
                    return Ok(None);
                }
                Err(error) => return Err(error.into()),
            }
            ensure!(
                &header[..4] == b"OggS" && header[4] == 0,
                "Opus 音频封装无效"
            );
            let mut sizes = vec![0; header[26] as usize];
            self.output.read_exact(&mut sizes).await?;
            for size in sizes {
                let mut data = vec![0; size as usize];
                self.output.read_exact(&mut data).await?;
                self.pending.extend_from_slice(&data);
                ensure!(self.pending.len() <= 1024 * 1024, "Opus 音频包过大");
                if size < 255 {
                    self.packets.push_back(std::mem::take(&mut self.pending));
                }
            }
        }
    }
}
