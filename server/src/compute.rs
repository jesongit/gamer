//! PERF-003：CPU 密集任务专用计算池（NCC 匹配 / PNG 解码 / 灰度化）。
//!
//! 设计（docs/OPTIMIZATION_PLAN.md §11.3）：
//! - **专用 rayon 线程池**（Cargo.toml 已有 rayon，复用）：线程数固定 = 并发
//!   上限。NCC 滑窗的 `par_iter`（`match_template_with_source`）经
//!   `pool.install` 落在本池，CPU 并行度被硬性限制在上限内；
//! - **异步侧同上限 Semaphore 排队**：`spawn_blocking` 前先取许可，池满时
//!   调用方 await 等待（背压——不丢弃、不报错），同刻占用的 Tokio blocking
//!   线程 ≤ 上限。防「Tokio blocking 池 × rayon 池」双层无界扩张：两层各管
//!   一半——rayon 池管真实线程数、信号量管在途任务数，任一层单独有界即可，
//!   这里取同值双保险；
//! - **并发上限可配置**：config.toml `compute_max_concurrency`（0 或缺省 =
//!   按 CPU 核数自动），由 `Config::load_from` 启动期调 [`configure`] 注入；
//!   池在首次使用时惰性创建，此后配置变更不再生效（进程内一次性）。
//!
//! 语义承诺：只移动执行位置，匹配结果、截图 freshness/generation 与脚本
//! 点击语义零变化。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use tokio::sync::Semaphore;

/// config.toml 注入的并发上限（0 = 未配置/自动）。池创建后修改不再生效。
static CONFIGURED_MAX: AtomicUsize = AtomicUsize::new(0);

/// 注入 config.toml `compute_max_concurrency`（0 = 按 CPU 核数自动）。
/// 由 `Config::load_from` 在启动期调用；池创建之后的调用不生效。
pub fn configure(max_concurrency: usize) {
    CONFIGURED_MAX.store(max_concurrency, Ordering::Relaxed);
}

fn auto_limit() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
}

fn resolve_limit() -> usize {
    match CONFIGURED_MAX.load(Ordering::Relaxed) {
        0 => auto_limit(),
        n => n,
    }
}

/// 专用计算池：rayon 线程数与信号量许可同为上限，双层有界。
struct Pool {
    pool: Arc<rayon::ThreadPool>,
    semaphore: Arc<Semaphore>,
}

impl Pool {
    fn new(max_concurrency: usize) -> Self {
        let max_concurrency = max_concurrency.max(1);
        let pool = Arc::new(
            rayon::ThreadPoolBuilder::new()
                .num_threads(max_concurrency)
                .thread_name(|i| format!("gamer-compute-{i}"))
                .build()
                .expect("创建计算线程池失败"),
        );
        Self {
            pool,
            semaphore: Arc::new(Semaphore::new(max_concurrency)),
        }
    }

    /// 提交一个 CPU 密集闭包：池满时在本 future 内排队等待（背压），
    /// 闭包内的 rayon 并行段落（如 NCC 滑窗 par_iter）在本池执行。
    async fn run<T, F>(&self, task: F) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| anyhow::anyhow!("计算池信号量已关闭"))?;
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            pool.install(task)
        })
        .await
        .map_err(|e| anyhow::anyhow!("计算任务异常退出: {e}"))
    }
}

fn global_pool() -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(|| Pool::new(resolve_limit()))
}

/// 进程级共享入口：engine / api 的 NCC 与解码类 CPU 工作统一走这里。
pub async fn run<T, F>(task: F) -> anyhow::Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    global_pool().run(task).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 并发上限真的有界：8 个任务挤上限 2 的池，在途峰值不得越过 2，
    /// 且全部任务正常完成、结果不丢不失序。
    #[tokio::test]
    async fn pool_caps_in_flight_cpu_jobs() {
        let pool = Arc::new(Pool::new(2));
        let current = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for i in 0..8usize {
            let (pool, current, peak) = (pool.clone(), current.clone(), peak.clone());
            handles.push(tokio::spawn(async move {
                pool.run(move || {
                    let now = current.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    // 模拟 NCC/解码的持续占用，给并发叠出峰值的机会
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    current.fetch_sub(1, Ordering::SeqCst);
                    i
                })
                .await
                .unwrap()
            }));
        }
        let mut results = Vec::new();
        for handle in handles {
            results.push(handle.await.unwrap());
        }
        results.sort_unstable();
        assert_eq!(results, (0..8).collect::<Vec<_>>(), "任务不得丢弃或报错");
        assert!(
            peak.load(Ordering::SeqCst) <= 2,
            "并发峰值 {} 超过上限 2",
            peak.load(Ordering::SeqCst)
        );
    }

    /// 语义零变化：同一匹配请求经计算池与直接执行结果一致（只是执行位置移动）。
    #[tokio::test]
    async fn ncc_via_pool_matches_direct_execution() {
        use crate::matcher::MatchRequest;
        use image::{Rgb, RgbImage};

        let mut screen = RgbImage::new(300, 300);
        for (_, _, p) in screen.enumerate_pixels_mut() {
            *p = Rgb([40, 20, 80]);
        }
        for y in 100..160 {
            for x in 90..150 {
                screen.put_pixel(x, y, Rgb([30, 200, 90]));
            }
        }
        let mut tpl = RgbImage::new(60, 60);
        for y in 0..60 {
            for x in 0..60 {
                tpl.put_pixel(x, y, *screen.get_pixel(80 + x, 90 + y));
            }
        }
        let encode = |img: &RgbImage| {
            let mut out = Vec::new();
            img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
                .unwrap();
            out
        };
        let req = MatchRequest {
            screen_png: encode(&screen),
            template_png: encode(&tpl),
            threshold: Some(0.9),
            region: None,
        };
        let direct = crate::matcher::match_template(&req)
            .unwrap()
            .expect("直跑应命中");
        let via_pool = run(move || crate::matcher::match_template(&req))
            .await
            .unwrap() // 池层错误
            .unwrap() // match_template 错误层
            .expect("计算池应命中");
        assert!(
            (direct.x as i64 - via_pool.x as i64).abs() <= 1,
            "x 不一致: {} vs {}",
            direct.x,
            via_pool.x
        );
        assert!(
            (direct.y as i64 - via_pool.y as i64).abs() <= 1,
            "y 不一致: {} vs {}",
            direct.y,
            via_pool.y
        );
        assert_eq!(direct.width, via_pool.width);
        assert_eq!(direct.height, via_pool.height);
        assert_eq!(direct.score.to_bits(), via_pool.score.to_bits());
    }
}
