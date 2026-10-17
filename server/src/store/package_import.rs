//! Package preset publication uses one existing SQLite worker transaction.
use super::Store;
use crate::timer_core::{package_preset_id, PackagePreset, TaskPreset};
use chrono::Utc;
use rusqlite::OptionalExtension;

impl Store {
    pub(crate) fn publish_package_presets_atomic(
        &self,
        package_id: &str,
        presets: &[PackagePreset],
    ) -> anyhow::Result<usize> {
        let package_id = package_id.to_owned();
        let presets = presets.to_vec();
        self.request(move |connection| {
            let transaction = connection.transaction()?;
            for preset in &presets {
                let id = package_preset_id(&package_id, &preset.name);
                let timestamp = Utc::now();
                let created_at = transaction
                    .query_row("SELECT created_at FROM task_presets WHERE id=?1", [&id], |row| row.get::<_, String>(0))
                    .optional()?
                    .unwrap_or_else(|| timestamp.to_rfc3339());
                let record = TaskPreset {
                    id, app_package: package_id.clone(), name: preset.name.clone(),
                    runner_id: preset.runner_id.clone(), entrypoint: preset.entrypoint.clone(),
                    payload: preset.payload.clone(), schedule: preset.schedule.clone(), created_at: timestamp,
                };
                record.validate()?;
                transaction.execute(
                    "INSERT INTO task_presets (id,app_package,name,runner_id,entrypoint,payload_json,schedule_json,created_at)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
                     ON CONFLICT(id) DO UPDATE SET app_package=?2,name=?3,runner_id=?4,
                     entrypoint=?5,payload_json=?6,schedule_json=?7,created_at=?8",
                    rusqlite::params![record.id, record.app_package, record.name, record.runner_id,
                        record.entrypoint, serde_json::to_string(&record.payload)?,
                        serde_json::to_string(&record.schedule)?, created_at],
                )?;
            }
            transaction.commit()?;
            Ok(presets.len())
        })
    }
}
