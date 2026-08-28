//! 进程内低基数运行指标。
//!
//! 指标只接受代码定义的枚举或数值，不接受设备 ID、脚本 ID、路径、日志消息
//! 等外部字符串作为标签。这样 `/metrics` 可以直接暴露给本地探针，而不会把
//! 请求内容或用户数据带入监控系统。

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

const RELAXED: Ordering = Ordering::Relaxed;

/// 进程级共享指标实例（OBS-003）：main 启动时用 Store 的 Metrics 调用
/// [`install_global`] 安装一次；采集点散布在 webrtc pusher / 帧缓存 / 设备消费
/// 任务等远离 AppState 的位置，经 [`global`] 取用，避免逐层传参改造签名。
static GLOBAL: OnceLock<Arc<Metrics>> = OnceLock::new();

/// 安装进程级共享指标实例。返回 false = 已被安装（或已被惰性兜底创建），
/// 本次忽略——观测为旁路，不因重复安装改变行为。
pub fn install_global(metrics: Arc<Metrics>) -> bool {
    GLOBAL.set(metrics).is_ok()
}

/// 进程级共享指标：未安装时惰性创建默认实例，采集函数在任意时机都可安全调用。
pub fn global() -> &'static Metrics {
    GLOBAL.get_or_init(|| Arc::new(Metrics::default()))
}

/// [`global`] 的 Arc 形态：需要持有句柄的结构（如 FrameCache）用它，
/// 测试可注入独立实例实现计数隔离。
pub(crate) fn global_arc() -> Arc<Metrics> {
    GLOBAL.get_or_init(|| Arc::new(Metrics::default())).clone()
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub enum ReconnectReason {
    ManualRetry,
    WatchdogDead,
    WatchdogSilent,
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub enum FfmpegResult {
    Success,
    Timeout,
    Failure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum NccResult {
    Hit,
    Miss,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum NccScope {
    Region,
    Fullscreen,
}

#[derive(Debug, Clone, Copy)]
pub enum SchedulerEvent {
    Conflict,
    Skipped,
    Failed,
}

#[derive(Debug, Default)]
#[allow(dead_code)]
struct RateState {
    input_started: Option<Instant>,
    input_frames: u64,
    input_fps_milli: u64,
    rtp_started: Option<Instant>,
    rtp_frames: u64,
    rtp_fps_milli: u64,
}

/// 所有字段都是进程内累计器或当前值；没有任何用户输入作为 label。
#[derive(Debug, Default)]
pub struct Metrics {
    db_queue_depth: AtomicI64,
    db_batches_total: AtomicU64,
    db_batch_rows_total: AtomicU64,
    db_batch_duration_ms_total: AtomicU64,
    db_flush_errors_total: AtomicU64,
    db_logs_dropped_debug_total: AtomicU64,

    scrcpy_connect_success_total: AtomicU64,
    scrcpy_connect_failure_total: AtomicU64,
    scrcpy_reconnect_manual_total: AtomicU64,
    scrcpy_reconnect_watchdog_dead_total: AtomicU64,
    scrcpy_reconnect_watchdog_silent_total: AtomicU64,

    video_input_frames_total: AtomicU64,
    video_input_fps_milli: AtomicU64,
    rtp_sent_frames_total: AtomicU64,
    rtp_sent_fps_milli: AtomicU64,
    rtp_queue_depth: AtomicI64,
    rtp_dropped_frames_total: AtomicU64,
    gop_frames: AtomicI64,
    gop_bytes: AtomicI64,

    ffmpeg_decode_total: AtomicU64,
    ffmpeg_decode_success_total: AtomicU64,
    ffmpeg_decode_timeout_total: AtomicU64,
    ffmpeg_decode_failure_total: AtomicU64,
    ffmpeg_decode_duration_ms_total: AtomicU64,

    ncc_matches_total: AtomicU64,
    ncc_hits_total: AtomicU64,
    ncc_misses_total: AtomicU64,
    ncc_region_total: AtomicU64,
    ncc_fullscreen_total: AtomicU64,
    ncc_duration_ms_total: AtomicU64,

    scheduler_triggers_total: AtomicU64,
    scheduler_trigger_latency_ms_total: AtomicU64,
    scheduler_conflicts_total: AtomicU64,
    scheduler_skipped_total: AtomicU64,
    scheduler_failures_total: AtomicU64,

    #[allow(dead_code)]
    rate_state: Mutex<RateState>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MetricsSnapshot {
    pub db_queue_depth: i64,
    pub db_batches_total: u64,
    pub db_batch_rows_total: u64,
    pub db_batch_duration_ms_total: u64,
    pub db_flush_errors_total: u64,
    pub db_logs_dropped_debug_total: u64,
    pub scrcpy_connect_success_total: u64,
    pub scrcpy_connect_failure_total: u64,
    pub scrcpy_reconnect_manual_total: u64,
    pub scrcpy_reconnect_watchdog_dead_total: u64,
    pub scrcpy_reconnect_watchdog_silent_total: u64,
    pub video_input_frames_total: u64,
    pub video_input_fps_milli: u64,
    pub rtp_sent_frames_total: u64,
    pub rtp_sent_fps_milli: u64,
    pub rtp_queue_depth: i64,
    pub rtp_dropped_frames_total: u64,
    pub gop_frames: i64,
    pub gop_bytes: i64,
    pub ffmpeg_decode_total: u64,
    pub ffmpeg_decode_success_total: u64,
    pub ffmpeg_decode_timeout_total: u64,
    pub ffmpeg_decode_failure_total: u64,
    pub ffmpeg_decode_duration_ms_total: u64,
    pub ncc_matches_total: u64,
    pub ncc_hits_total: u64,
    pub ncc_misses_total: u64,
    pub ncc_region_total: u64,
    pub ncc_fullscreen_total: u64,
    pub ncc_duration_ms_total: u64,
    pub scheduler_triggers_total: u64,
    pub scheduler_trigger_latency_ms_total: u64,
    pub scheduler_conflicts_total: u64,
    pub scheduler_skipped_total: u64,
    pub scheduler_failures_total: u64,
}

#[allow(dead_code)]
impl Metrics {
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            db_queue_depth: self.db_queue_depth.load(RELAXED),
            db_batches_total: self.db_batches_total.load(RELAXED),
            db_batch_rows_total: self.db_batch_rows_total.load(RELAXED),
            db_batch_duration_ms_total: self.db_batch_duration_ms_total.load(RELAXED),
            db_flush_errors_total: self.db_flush_errors_total.load(RELAXED),
            db_logs_dropped_debug_total: self.db_logs_dropped_debug_total.load(RELAXED),
            scrcpy_connect_success_total: self.scrcpy_connect_success_total.load(RELAXED),
            scrcpy_connect_failure_total: self.scrcpy_connect_failure_total.load(RELAXED),
            scrcpy_reconnect_manual_total: self.scrcpy_reconnect_manual_total.load(RELAXED),
            scrcpy_reconnect_watchdog_dead_total: self
                .scrcpy_reconnect_watchdog_dead_total
                .load(RELAXED),
            scrcpy_reconnect_watchdog_silent_total: self
                .scrcpy_reconnect_watchdog_silent_total
                .load(RELAXED),
            video_input_frames_total: self.video_input_frames_total.load(RELAXED),
            video_input_fps_milli: self.video_input_fps_milli.load(RELAXED),
            rtp_sent_frames_total: self.rtp_sent_frames_total.load(RELAXED),
            rtp_sent_fps_milli: self.rtp_sent_fps_milli.load(RELAXED),
            rtp_queue_depth: self.rtp_queue_depth.load(RELAXED),
            rtp_dropped_frames_total: self.rtp_dropped_frames_total.load(RELAXED),
            gop_frames: self.gop_frames.load(RELAXED),
            gop_bytes: self.gop_bytes.load(RELAXED),
            ffmpeg_decode_total: self.ffmpeg_decode_total.load(RELAXED),
            ffmpeg_decode_success_total: self.ffmpeg_decode_success_total.load(RELAXED),
            ffmpeg_decode_timeout_total: self.ffmpeg_decode_timeout_total.load(RELAXED),
            ffmpeg_decode_failure_total: self.ffmpeg_decode_failure_total.load(RELAXED),
            ffmpeg_decode_duration_ms_total: self.ffmpeg_decode_duration_ms_total.load(RELAXED),
            ncc_matches_total: self.ncc_matches_total.load(RELAXED),
            ncc_hits_total: self.ncc_hits_total.load(RELAXED),
            ncc_misses_total: self.ncc_misses_total.load(RELAXED),
            ncc_region_total: self.ncc_region_total.load(RELAXED),
            ncc_fullscreen_total: self.ncc_fullscreen_total.load(RELAXED),
            ncc_duration_ms_total: self.ncc_duration_ms_total.load(RELAXED),
            scheduler_triggers_total: self.scheduler_triggers_total.load(RELAXED),
            scheduler_trigger_latency_ms_total: self
                .scheduler_trigger_latency_ms_total
                .load(RELAXED),
            scheduler_conflicts_total: self.scheduler_conflicts_total.load(RELAXED),
            scheduler_skipped_total: self.scheduler_skipped_total.load(RELAXED),
            scheduler_failures_total: self.scheduler_failures_total.load(RELAXED),
        }
    }

    pub(crate) fn db_enqueue(&self) {
        self.db_queue_depth.fetch_add(1, RELAXED);
    }

    pub(crate) fn db_dequeue(&self) {
        let mut current = self.db_queue_depth.load(RELAXED);
        loop {
            let next = current.saturating_sub(1).max(0);
            match self
                .db_queue_depth
                .compare_exchange_weak(current, next, RELAXED, RELAXED)
            {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
    }

    pub(crate) fn db_batch(&self, rows: usize, duration_ms: u64, failed: bool) {
        self.db_batches_total.fetch_add(1, RELAXED);
        self.db_batch_rows_total.fetch_add(rows as u64, RELAXED);
        self.db_batch_duration_ms_total
            .fetch_add(duration_ms, RELAXED);
        if failed {
            self.db_flush_errors_total.fetch_add(1, RELAXED);
        }
    }

    pub(crate) fn db_drop_debug_log(&self) {
        self.db_logs_dropped_debug_total.fetch_add(1, RELAXED);
    }

    pub(crate) fn scrcpy_connect(&self, success: bool) {
        let counter = if success {
            &self.scrcpy_connect_success_total
        } else {
            &self.scrcpy_connect_failure_total
        };
        counter.fetch_add(1, RELAXED);
    }

    pub(crate) fn scrcpy_reconnect(&self, reason: ReconnectReason) {
        let counter = match reason {
            ReconnectReason::ManualRetry => &self.scrcpy_reconnect_manual_total,
            ReconnectReason::WatchdogDead => &self.scrcpy_reconnect_watchdog_dead_total,
            ReconnectReason::WatchdogSilent => &self.scrcpy_reconnect_watchdog_silent_total,
        };
        counter.fetch_add(1, RELAXED);
    }

    pub fn record_video_input_frame(&self) {
        self.video_input_frames_total.fetch_add(1, RELAXED);
        let mut rate = self.rate_state.lock().unwrap();
        record_rate(&mut rate, true);
        self.video_input_fps_milli
            .store(rate.input_fps_milli, RELAXED);
    }

    pub fn record_rtp_sent_frame(&self) {
        self.rtp_sent_frames_total.fetch_add(1, RELAXED);
        let mut rate = self.rate_state.lock().unwrap();
        record_rate(&mut rate, false);
        self.rtp_sent_fps_milli.store(rate.rtp_fps_milli, RELAXED);
    }

    pub fn set_rtp_queue_depth(&self, depth: i64) {
        self.rtp_queue_depth.store(depth.max(0), RELAXED);
    }

    pub fn record_rtp_drop(&self) {
        self.rtp_dropped_frames_total.fetch_add(1, RELAXED);
    }

    /// 批量丢帧（pusher 积压跳帧/断链清队一次丢多帧时避免逐帧自增）
    pub fn record_rtp_drops(&self, n: u64) {
        if n > 0 {
            self.rtp_dropped_frames_total.fetch_add(n, RELAXED);
        }
    }

    pub fn set_gop_size(&self, frames: i64, bytes: i64) {
        self.gop_frames.store(frames.max(0), RELAXED);
        self.gop_bytes.store(bytes.max(0), RELAXED);
    }

    pub fn record_ffmpeg_decode(&self, duration_ms: u64, result: FfmpegResult) {
        self.ffmpeg_decode_total.fetch_add(1, RELAXED);
        self.ffmpeg_decode_duration_ms_total
            .fetch_add(duration_ms, RELAXED);
        match result {
            FfmpegResult::Success => self.ffmpeg_decode_success_total.fetch_add(1, RELAXED),
            FfmpegResult::Timeout => self.ffmpeg_decode_timeout_total.fetch_add(1, RELAXED),
            FfmpegResult::Failure => self.ffmpeg_decode_failure_total.fetch_add(1, RELAXED),
        };
    }

    pub fn record_ncc(&self, duration_ms: u64, hit: bool, region: bool) {
        let result = if hit { NccResult::Hit } else { NccResult::Miss };
        let scope = if region {
            NccScope::Region
        } else {
            NccScope::Fullscreen
        };
        self.record_ncc_observation(duration_ms, result, scope);
    }

    pub fn record_ncc_observation(&self, duration_ms: u64, result: NccResult, scope: NccScope) {
        self.ncc_matches_total.fetch_add(1, RELAXED);
        if matches!(result, NccResult::Hit) {
            self.ncc_hits_total.fetch_add(1, RELAXED);
        } else {
            self.ncc_misses_total.fetch_add(1, RELAXED);
        }
        if matches!(scope, NccScope::Region) {
            self.ncc_region_total.fetch_add(1, RELAXED);
        } else {
            self.ncc_fullscreen_total.fetch_add(1, RELAXED);
        }
        self.ncc_duration_ms_total.fetch_add(duration_ms, RELAXED);
    }

    pub fn record_scheduler_trigger(&self, latency_ms: u64) {
        self.scheduler_triggers_total.fetch_add(1, RELAXED);
        self.scheduler_trigger_latency_ms_total
            .fetch_add(latency_ms, RELAXED);
    }

    pub fn record_scheduler_event(&self, event: SchedulerEvent) {
        let counter = match event {
            SchedulerEvent::Conflict => &self.scheduler_conflicts_total,
            SchedulerEvent::Skipped => &self.scheduler_skipped_total,
            SchedulerEvent::Failed => &self.scheduler_failures_total,
        };
        counter.fetch_add(1, RELAXED);
    }
}

#[allow(dead_code)]
fn record_rate(rate: &mut RateState, input: bool) {
    let now = Instant::now();
    let (started, frames, fps_milli) = if input {
        (
            &mut rate.input_started,
            &mut rate.input_frames,
            &mut rate.input_fps_milli,
        )
    } else {
        (
            &mut rate.rtp_started,
            &mut rate.rtp_frames,
            &mut rate.rtp_fps_milli,
        )
    };
    let start = started.get_or_insert(now);
    *frames += 1;
    let elapsed_ms = now.duration_since(*start).as_millis() as u64;
    if elapsed_ms >= 1_000 {
        *fps_milli = frames.saturating_mul(1_000_000) / elapsed_ms.max(1);
        *started = Some(now);
        *frames = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_are_low_cardinality_and_snapshot_consistent() {
        let metrics = Metrics::default();
        metrics.scrcpy_connect(true);
        metrics.scrcpy_connect(false);
        metrics.scrcpy_reconnect(ReconnectReason::ManualRetry);
        metrics.scrcpy_reconnect(ReconnectReason::WatchdogDead);
        metrics.scrcpy_reconnect(ReconnectReason::WatchdogSilent);
        metrics.record_video_input_frame();
        metrics.record_rtp_sent_frame();
        metrics.record_rtp_drop();
        metrics.set_rtp_queue_depth(-3);
        metrics.set_gop_size(-1, 42);
        metrics.record_ffmpeg_decode(12, FfmpegResult::Success);
        metrics.record_ffmpeg_decode(13, FfmpegResult::Timeout);
        metrics.record_ffmpeg_decode(14, FfmpegResult::Failure);
        metrics.record_ncc_observation(4, NccResult::Hit, NccScope::Fullscreen);
        metrics.record_ncc_observation(5, NccResult::Miss, NccScope::Region);
        metrics.record_scheduler_trigger(7);
        metrics.record_scheduler_event(SchedulerEvent::Conflict);
        metrics.record_scheduler_event(SchedulerEvent::Skipped);
        metrics.record_scheduler_event(SchedulerEvent::Failed);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.scrcpy_connect_success_total, 1);
        assert_eq!(snapshot.scrcpy_connect_failure_total, 1);
        assert_eq!(snapshot.scrcpy_reconnect_manual_total, 1);
        assert_eq!(snapshot.scrcpy_reconnect_watchdog_dead_total, 1);
        assert_eq!(snapshot.scrcpy_reconnect_watchdog_silent_total, 1);
        assert_eq!(snapshot.video_input_frames_total, 1);
        assert_eq!(snapshot.rtp_sent_frames_total, 1);
        assert_eq!(snapshot.rtp_dropped_frames_total, 1);
        metrics.record_rtp_drops(7);
        metrics.record_rtp_drops(0);
        assert_eq!(metrics.snapshot().rtp_dropped_frames_total, 8);
        assert_eq!(snapshot.rtp_queue_depth, 0);
        assert_eq!(snapshot.gop_frames, 0);
        assert_eq!(snapshot.gop_bytes, 42);
        assert_eq!(snapshot.ffmpeg_decode_success_total, 1);
        assert_eq!(snapshot.ffmpeg_decode_timeout_total, 1);
        assert_eq!(snapshot.ffmpeg_decode_failure_total, 1);
        assert_eq!(snapshot.ffmpeg_decode_total, 3);
        assert_eq!(snapshot.ncc_hits_total, 1);
        assert_eq!(snapshot.ncc_misses_total, 1);
        assert_eq!(snapshot.ncc_region_total, 1);
        assert_eq!(snapshot.ncc_fullscreen_total, 1);
        assert_eq!(snapshot.ncc_matches_total, 2);
        assert_eq!(snapshot.scheduler_conflicts_total, 1);
        assert_eq!(snapshot.scheduler_skipped_total, 1);
        assert_eq!(snapshot.scheduler_failures_total, 1);
    }

    /// 全局访问器必须返回稳定实例：采集点分散在多个模块，同一进程内
    /// 观测的必须是同一个计数器集合（未安装时惰性兜底创建同样稳定）。
    #[test]
    fn global_accessor_returns_stable_instance() {
        let first = global();
        let second = global();
        assert!(
            std::ptr::eq(first, second),
            "global() must return the same instance within a process"
        );
        assert!(std::ptr::eq(first, global_arc().as_ref()));
    }

    #[test]
    fn db_queue_depth_is_saturating() {
        let metrics = Metrics::default();
        metrics.db_dequeue();
        assert_eq!(metrics.db_queue_depth.load(RELAXED), 0);
        metrics.db_enqueue();
        assert_eq!(metrics.db_queue_depth.load(RELAXED), 1);
        metrics.db_enqueue();
        assert_eq!(metrics.db_queue_depth.load(RELAXED), 2);
        metrics.db_dequeue();
        assert_eq!(metrics.db_queue_depth.load(RELAXED), 1);
        assert_eq!(metrics.snapshot().db_queue_depth, 1);
        metrics.db_dequeue();
        metrics.db_dequeue();
        assert_eq!(metrics.snapshot().db_queue_depth, 0);
    }
}
