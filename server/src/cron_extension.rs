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

use crate::timer_core::{ScheduleExtension, ScheduleRegistry, ScheduleSpec};

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

#[derive(Debug, Default)]
pub(crate) struct CronExtension;

/// Composition-root seam: installs the native Cron schedule extension into
/// the generic [`ScheduleRegistry`].  Callers only see the registration; all
/// later schedule computation goes through the registry, never this type.
pub fn register_builtin(registry: &ScheduleRegistry) -> anyhow::Result<()> {
    registry.register(CRON_SCHEDULE_KIND, Arc::new(CronExtension))
}

impl CronExtension {
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn registered_provider_answers_generic_registry_queries() {
        let registry = ScheduleRegistry::new();
        register_builtin(&registry).expect("内置 Cron provider 必须可注册");
        let now = Local
            .with_ymd_and_hms(2026, 8, 31, 10, 0, 20)
            .single()
            .unwrap()
            .with_timezone(&Utc);
        let spec = ScheduleSpec::new(
            CRON_SCHEDULE_KIND,
            serde_json::json!({"expression": "*/5 * * * *"}),
        )
        .unwrap();
        let next = registry
            .next_after(&spec, now)
            .expect("已注册 provider 必须解析 spec")
            .expect("*/5 永远存在下一次触发");
        let expected = Local
            .with_ymd_and_hms(2026, 8, 31, 10, 5, 0)
            .single()
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(next, expected, "10:05:00 local is 280s after 10:00:20");
    }

    #[test]
    fn latest_due_returns_the_most_recent_occurrence_within_lookback() {
        let registry = ScheduleRegistry::new();
        register_builtin(&registry).unwrap();
        let now = Local
            .with_ymd_and_hms(2026, 8, 31, 10, 3, 0)
            .single()
            .unwrap()
            .with_timezone(&Utc);
        let spec = ScheduleSpec::new(
            CRON_SCHEDULE_KIND,
            serde_json::json!({"expression": "*/5 * * * *"}),
        )
        .unwrap();
        let due = registry
            .latest_due(&spec, now, Duration::from_secs(60 * 60))
            .expect("已注册 provider 必须解析 spec")
            .expect("10:00:00 local 在回看窗口内");
        let expected = Local
            .with_ymd_and_hms(2026, 8, 31, 10, 0, 0)
            .single()
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(due, expected);
    }

    #[test]
    fn duplicate_builtin_registration_is_rejected() {
        let registry = ScheduleRegistry::new();
        register_builtin(&registry).unwrap();
        assert!(register_builtin(&registry).is_err());
    }

    #[test]
    fn parses_five_six_and_seven_field_cron_without_core_knowledge() {
        let registry = ScheduleRegistry::new();
        register_builtin(&registry).unwrap();
        let probe = |expression: &str| {
            registry.next_after(
                &ScheduleSpec::new(
                    CRON_SCHEDULE_KIND,
                    serde_json::json!({"expression": expression}),
                )
                .unwrap(),
                Utc::now(),
            )
        };
        assert!(probe("*/5 * * * *").is_ok());
        assert!(probe("0 */5 * * * *").is_ok());
        assert!(probe("0 */5 * * * * *").is_ok());
        assert!(probe("not a cron").is_err());
    }

    #[test]
    fn rejects_non_cron_schedule_kind() {
        let schedule = ScheduleSpec::new("other", serde_json::json!({})).unwrap();
        assert!(CronExtension.next_after(&schedule, Utc::now()).is_err());
    }
}
