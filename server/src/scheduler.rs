//! Timer Core composition.
//!
//! Scheduling policy and runner implementations are registered adapters. The
//! scheduler itself only composes the generic timer service and exposes the
//! small compatibility façade used by the existing REST/update callers.  The
//! native Cron provider is installed through its registration seam; every
//! later schedule computation goes through [`ScheduleRegistry`], so this
//! composition never references concrete schedule types.  No runner is
//! registered here (ADR-13): runners arrive through the extension lifecycle
//! (`TimerRunnerRegistrar` → [`Scheduler::register_extension_runner`]) and
//! disappear with their owning extension.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use tracing::info;

use crate::store::Db;
use crate::timer_core::{
    RegisteredRunner, ScheduleRegistry, TimerCore, TimerRunner, TimerRunnerRegistry,
};

/// Entrypoint 参数 schema 描述失败（P12.3 / 契约 §7）：资源缺失或解析失败
/// （诊断透传给 API 边界映射 404 / 400）。
#[derive(Debug, Clone)]
pub enum EntrypointDescribeError {
    /// entrypoint 指向的资源不存在。
    NotFound { resource: String },
    /// 资源存在但解析失败（结构化诊断数组，与保存期校验同源）。
    Invalid { diagnostics: serde_json::Value },
}

/// Entrypoint 参数 schema 描述器（runner 私有能力）：由注册该 runner 的扩展
/// 在 start 生命周期注入，随 runner 注销一并移除。Core 只按 runner_id 转发
/// 查询并透传 JSON——资源语义（YAML/其他 DSL）全部留在扩展边界内。
pub trait EntrypointDescriber: Send + Sync {
    /// 返回 entrypoint（资源 id，如 `<分区>/<脚本>.yaml[#<函数>]`）的参数
    /// schema 描述（契约 §7 JSON 形态）。
    fn describe(&self, entrypoint: &str) -> Result<serde_json::Value, EntrypointDescribeError>;
}

/// [`Scheduler::describe_entrypoint`] 的查询失败：runner 未注册与描述失败分开
/// （前者 404 runner_not_found，后者按 [`EntrypointDescribeError`] 映射）。
#[derive(Debug, Clone)]
pub enum EntrypointQueryError {
    UnknownRunner,
    Describe(EntrypointDescribeError),
}

pub struct Scheduler {
    core: Arc<TimerCore>,
    runners: Arc<TimerRunnerRegistry>,
    schedules: Arc<ScheduleRegistry>,
    /// runner_id → (owner_extension_id, describer)；生命周期与 runner 注册同步。
    describers: Mutex<HashMap<String, (String, Arc<dyn EntrypointDescriber>)>>,
}

impl Scheduler {
    /// Bare-core composition: no runner is pre-registered.  Tasks targeting a
    /// runner whose extension has not started (yet) are saved normally and
    /// enter `DependencyMissing` at dispatch time (ADR-13).
    pub(crate) fn new(db: Db) -> Self {
        let runners = Arc::new(TimerRunnerRegistry::new());
        let schedules = Arc::new(ScheduleRegistry::new());
        crate::cron_extension::register_builtin(&schedules)
            .expect("the built-in Cron schedule extension must register");
        Self {
            core: TimerCore::new(db),
            runners,
            schedules,
            describers: Mutex::new(HashMap::new()),
        }
    }

    /// ADR-13: register a runner on behalf of its owning extension and resume
    /// the tasks that entered `DependencyMissing` because this runner was
    /// missing (wakeup cursors recomputed through the schedule registry).
    pub async fn register_extension_runner(
        &self,
        runner_id: impl Into<String>,
        owner_extension_id: impl Into<String>,
        runner: Arc<dyn TimerRunner>,
    ) -> anyhow::Result<()> {
        let runner_id = runner_id.into();
        self.runners
            .register_runner(&runner_id, owner_extension_id, runner)?;
        let resumed = self
            .core
            .resume_tasks_missing_runner(&runner_id, self.schedules.as_ref())
            .await?;
        if resumed > 0 {
            tracing::info!(runner = %runner_id, resumed, "runner registered; dependency-missing tasks resumed");
        }
        self.core.notify_changed();
        Ok(())
    }

    /// 同步注册（不触发 dependency-missing 任务恢复）：HTTP 集成测试装配用，
    /// 生产路径一律走 `register_extension_runner`（扩展 start 生命周期）。
    #[cfg(test)]
    pub(crate) fn register_runner_for_tests(
        &self,
        runner_id: &str,
        owner_extension_id: &str,
        runner: Arc<dyn TimerRunner>,
    ) -> anyhow::Result<()> {
        self.runners
            .register_runner(runner_id, owner_extension_id, runner)
    }

    /// ADR-13: unregister every runner owned by `extension_id` (idempotent)
    /// and suspend the still-Active tasks bound to the removed runners into
    /// `DependencyMissing`.  Returns the removed runner ids.
    pub async fn unregister_extension_owner(
        &self,
        owner_extension_id: &str,
    ) -> anyhow::Result<Vec<String>> {
        let removed = self.runners.unregister_owner(owner_extension_id);
        // entrypoint 描述器与 runner 同生命周期：owner 离场即整体移除
        self.describers
            .lock()
            .expect("entrypoint describer registry lock poisoned")
            .retain(|_, (owner, _)| owner != owner_extension_id);
        for runner_id in &removed {
            let suspended = self.core.suspend_tasks_missing_runner(runner_id).await?;
            tracing::info!(runner = %runner_id, suspended, "runner unregistered; active tasks suspended as dependency-missing");
        }
        self.core.notify_changed();
        Ok(removed)
    }

    /// Generic schedule registry access for boundary callers (API 校验/预览)
    /// that must turn an opaque [`crate::timer_core::TaskSchedule`] into
    /// instants without knowing any concrete provider.
    pub fn schedules(&self) -> Arc<ScheduleRegistry> {
        Arc::clone(&self.schedules)
    }

    /// Registered runners with their owning extension (`GET /api/runners`
    /// 数据源），按 runner_id 稳定排序。
    pub fn runners(&self) -> Vec<RegisteredRunner> {
        self.runners.list_runners()
    }

    /// Registered schedule provider ids（`GET /api/schedule-providers` 数据源）。
    pub fn schedule_provider_ids(&self) -> Vec<String> {
        self.schedules.list()
    }

    /// 通用执行分发注册表（`POST /api/runs` 手动运行的 runner 查找源）。
    /// Core 只按 request.runner_id 查找并转发；runner 的 payload 语义属于
    /// 注册它的扩展（ADR-13）。
    pub fn runner_registry(&self) -> Arc<TimerRunnerRegistry> {
        Arc::clone(&self.runners)
    }

    /// P12.3（契约 §7）：注册 runner 名下的 entrypoint 参数 schema 描述器
    /// （扩展 start 生命周期注入；同 runner_id 重复注册原地替换）。
    pub fn register_entrypoint_describer(
        &self,
        runner_id: &str,
        owner_extension_id: &str,
        describer: Arc<dyn EntrypointDescriber>,
    ) {
        self.describers
            .lock()
            .expect("entrypoint describer registry lock poisoned")
            .insert(
                runner_id.to_string(),
                (owner_extension_id.to_string(), describer),
            );
    }

    /// 按 runner 查询 entrypoint 参数 schema：Core 不理解资源内容，描述器
    /// 返回什么就透传什么（前端不为取参数而解析 YAML）。
    pub fn describe_entrypoint(
        &self,
        runner_id: &str,
        entrypoint: &str,
    ) -> Result<serde_json::Value, EntrypointQueryError> {
        let describer = self
            .describers
            .lock()
            .expect("entrypoint describer registry lock poisoned")
            .get(runner_id)
            .map(|(_, describer)| Arc::clone(describer))
            .ok_or(EntrypointQueryError::UnknownRunner)?;
        describer
            .describe(entrypoint)
            .map_err(EntrypointQueryError::Describe)
    }

    pub async fn start(&self) {
        info!("timer core started with registered schedule and runner extensions");
        self.core
            .start(self.schedules.clone(), self.runners.clone());
    }

    /// Submit a generic user task immediately. No task payload or runner
    /// semantics are decoded in this composition layer.
    pub async fn run_now(
        &self,
        task: &crate::timer_core::Task,
    ) -> Result<String, crate::timer_core::TimerRunnerError> {
        self.core
            .submit_now(task.clone(), self.runners.clone())
            .await
            .map(|run| run.run_id)
    }

    pub async fn cancel_task(&self, task_id: &str) -> anyhow::Result<()> {
        self.core.cancel_task(task_id, self.runners.clone()).await
    }

    pub async fn suspend_task(&self, task_id: &str, reason: &str) -> anyhow::Result<()> {
        self.core.suspend_task(task_id, reason).await
    }

    pub async fn resume_task(&self, task_id: &str) -> anyhow::Result<()> {
        self.core
            .resume_task(task_id, self.schedules.as_ref())
            .await
    }

    pub async fn on_app_package_uninstalled(&self, package: &str) -> anyhow::Result<usize> {
        self.core.on_app_package_uninstalled(package).await
    }

    /// 下次唤醒时间查询（诊断/编排预读用；调度循环自身不经过它）。
    #[allow(dead_code)]
    pub fn next_wakeup_at(&self) -> Option<DateTime<Utc>> {
        self.core.next_wakeup_at()
    }

    /// 距 TimerCore 下一次待执行触发的秒数（update 安装门禁等空闲判定用）。
    /// 直接读 TimerCore 持久化的唤醒游标（与调度循环睡眠同源，不重复逐任务
    /// 计算）；`0` = 已到期，`None` = 无待执行任务。
    pub fn next_wakeup_in_secs(&self) -> Option<i64> {
        self.core.next_wakeup_in(Utc::now())
    }

    pub fn notify_tasks_changed(&self) {
        self.core.notify_changed();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn scheduler_composes_timer_core_without_runner_or_payload_knowledge() {
        let source = include_str!("scheduler.rs");
        assert!(!source.contains(&["Script", "Store"].concat()));
        assert!(!source.contains(&["timer_", "yaml"].concat()));
        assert!(!source.contains(&["Run", "Target"].concat()));
    }

    /// 架构收口（Phase 1 / ADR-01）源码自检：schedule 计算只允许经过
    /// ScheduleRegistry 抽象。scheduler/timer_core/api 的任务触发路径不得
    /// 引用 cron 具体类型/函数——唯一例外是 scheduler 组装层的注册缝。
    #[test]
    fn schedule_computation_is_locked_to_the_registry_abstraction() {
        let scheduler_source = include_str!("scheduler.rs");
        let cron_module = ["cron_", "extension"].concat();
        // 注册缝是 scheduler 允许的唯一 cron 引用，剥离后再断言
        let registration_seam = ["cron_", "extension::register_builtin"].concat();
        let without_seam = scheduler_source.replace(&registration_seam, "");
        assert!(
            !without_seam.contains(&cron_module),
            "scheduler 只允许通过注册缝引用 cron provider"
        );
        assert!(
            !scheduler_source.contains(&["next_enabled_trigger_", "in_secs"].concat()),
            "scheduler 不得保留逐任务 cron 直算入口"
        );
        for (path, source) in [
            ("timer_core.rs", include_str!("timer_core.rs")),
            ("api/tasks.rs", include_str!("api/tasks.rs")),
        ] {
            assert!(
                !source.contains(&cron_module),
                "{path} 的任务触发路径不得引用具体 cron 实现"
            );
        }
    }
}
