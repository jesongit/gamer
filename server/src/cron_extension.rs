//! Native Cron schedule extension.
//!
//! Cron parsing and the local-time policy live here so the Timer Core only
//! deals in opaque [`ScheduleSpec`] values and wakeup instants.  This adapter
//! is intentionally independent from the YAML runner and from the extension
//! Host; a task can select any registered runner through the generic timer
//! boundary.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Local, Utc};
use cron::Schedule;

use crate::timer_core::{ScheduleExtension, ScheduleSpec, TimerTask};

pub(crate) const CRON_SCHEDULE_KIND: &str = "cron";

/// Normalize standard 5/6-field Cron into the seven-field form accepted by
/// the `cron` crate used by this extension.
pub fn normalize_cron(expr: &str) -> String {
    let expr = expr.trim();
    if expr.starts_with('@') {
        return expr.to_string();
    }
    let parts: Vec<&str> = expr.split_whitespace().collect();
    match parts.len() {
        5 => format!("0 {} *", expr),
        6 => format!("0 {}", expr),
        _ => expr.to_string(),
    }
}

/// Validate a Cron expression at an API boundary without teaching the
/// generic task model about Cron syntax.
pub fn validate_cron(expr: &str) -> bool {
    Schedule::from_str(&normalize_cron(expr)).is_ok()
}

#[derive(Debug, Default)]
pub(crate) struct CronExtension;

impl CronExtension {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self)
    }

    fn parse_schedule(&self, schedule: &ScheduleSpec) -> Result<Schedule, String> {
        let expression = schedule
            .value
            .get("expression")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "cron schedule misses expression".to_string())?;
        Schedule::from_str(&normalize_cron(expression))
            .map_err(|error| format!("invalid cron schedule: {error}"))
    }
}

impl ScheduleExtension for CronExtension {
    fn next_after(
        &self,
        schedule: &ScheduleSpec,
        after: DateTime<Utc>,
    ) -> Result<Option<DateTime<Utc>>, String> {
        if schedule.kind != CRON_SCHEDULE_KIND {
            return Err(format!("unsupported schedule extension: {}", schedule.kind));
        }
        let schedule = self.parse_schedule(schedule)?;
        let local_after = after.with_timezone(&Local);
        Ok(schedule
            .after(&local_after)
            .next()
            .and_then(|value| DateTime::<Utc>::from_timestamp(value.timestamp(), 0)))
    }

    fn latest_due(
        &self,
        schedule: &ScheduleSpec,
        now: DateTime<Utc>,
        lookback: Duration,
    ) -> Result<Option<DateTime<Utc>>, String> {
        if schedule.kind != CRON_SCHEDULE_KIND {
            return Err(format!("unsupported schedule extension: {}", schedule.kind));
        }
        let schedule = self.parse_schedule(schedule)?;
        let local_now = now.with_timezone(&Local);
        let window_start = local_now
            - chrono::Duration::from_std(lookback)
                .map_err(|error| format!("invalid Cron lookback: {error}"))?;
        Ok(schedule
            .after(&window_start)
            .take_while(|value| *value <= local_now)
            .last()
            .and_then(|value| DateTime::<Utc>::from_timestamp(value.timestamp(), 0)))
    }
}

/// Cron preview retained for the legacy task endpoint. The endpoint owns the
/// legacy string shape; this function still keeps parsing in the extension.
pub fn next_run(cron_expr: &str) -> Option<DateTime<Local>> {
    let schedule = Schedule::from_str(&normalize_cron(cron_expr)).ok()?;
    schedule.after(&Local::now()).next()
}

/// Return the nearest enabled generic task wakeup. This is used by the update
/// workload gate and deliberately consumes `TimerTask`, not the legacy task
/// or script model.
pub(crate) fn next_enabled_trigger_in_secs(tasks: &[TimerTask], now: DateTime<Utc>) -> Option<i64> {
    let extension = CronExtension;
    tasks
        .iter()
        .filter(|task| task.is_schedulable())
        .filter_map(|task| extension.next_after(&task.schedule, now).ok().flatten())
        .map(|next| (next - now).num_seconds().max(0))
        .min()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::AppContext;
    use chrono::TimeZone;

    #[test]
    fn parses_five_six_and_seven_field_cron_without_core_knowledge() {
        assert!(validate_cron("*/5 * * * *"));
        assert!(validate_cron("0 */5 * * * *"));
        assert!(validate_cron("0 */5 * * * * *"));
        assert!(!validate_cron("not a cron"));
    }

    #[test]
    fn computes_next_wakeup_from_opaque_schedule() {
        let now = Local
            .with_ymd_and_hms(2026, 8, 31, 10, 0, 20)
            .single()
            .unwrap()
            .with_timezone(&Utc);
        let task = TimerTask::new(
            "task",
            "Task",
            AppContext::from_legacy_package("d1", "com.example").unwrap(),
            "runner.example",
            "entry",
            serde_json::json!({}),
            ScheduleSpec::new(
                CRON_SCHEDULE_KIND,
                serde_json::json!({"expression": "*/5 * * * *"}),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            next_enabled_trigger_in_secs(&[task], now),
            Some(280),
            "10:05:00 is 280 seconds after 10:00:20"
        );
    }

    #[test]
    fn rejects_non_cron_schedule_kind() {
        let schedule = ScheduleSpec::new("other", serde_json::json!({})).unwrap();
        assert!(CronExtension.next_after(&schedule, Utc::now()).is_err());
    }
}
