//! 包内 `presets/` 任务预设声明（Phase 9「包提供任务预设」）。
//!
//! 每个预设是包内 `presets/` 下的一个 YAML 文件：
//!
//! ```yaml
//! name: 每日领取
//! runner_id: gamer.yaml
//! entrypoint: run
//! payload: {}
//! schedule:
//!   kind: cron
//!   value:
//!     expression: "0 8 * * *"
//! ```
//!
//! `app_package` 由 manifest 提供不在文件内重复声明；发布 id 由
//! 「来源包 + 预设名」确定性生成（见 timer_core::package_preset_id），
//! 同包同名重复安装/激活为更新，不产生第二行。payload/schedule 保持
//! 中性 JSON 值，由 hook 适配层转换为 timer_core 类型。

use std::path::Path;

use serde::Deserialize;

use super::error::{AppPackageError, AppPackageResult};

/// 一个包内预设声明的中性结构（未绑定来源包）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PresetDeclaration {
    pub(crate) name: String,
    pub(crate) runner_id: String,
    pub(crate) entrypoint: String,
    pub(crate) payload: serde_json::Value,
    pub(crate) schedule: serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPreset {
    name: String,
    runner_id: String,
    entrypoint: String,
    #[serde(default)]
    payload: serde_json::Value,
    schedule: serde_json::Value,
}

pub(crate) fn parse_preset(bytes: &[u8], source: &str) -> AppPackageResult<PresetDeclaration> {
    let text = std::str::from_utf8(bytes).map_err(|error| {
        AppPackageError::InvalidPreset(format!("{source}: 必须是 UTF-8 ({error})"))
    })?;
    let raw: RawPreset = serde_yaml::from_str(text)
        .map_err(|error| AppPackageError::InvalidPreset(format!("{source}: {error}")))?;
    for (field, value) in [
        ("name", raw.name.as_str()),
        ("runner_id", raw.runner_id.as_str()),
        ("entrypoint", raw.entrypoint.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(AppPackageError::InvalidPreset(format!(
                "{source}: {field} 不能为空"
            )));
        }
    }
    if raw
        .schedule
        .get("kind")
        .and_then(|kind| kind.as_str())
        .is_none()
    {
        return Err(AppPackageError::InvalidPreset(format!(
            "{source}: schedule.kind 必须是非空字符串"
        )));
    }
    Ok(PresetDeclaration {
        name: raw.name.trim().to_string(),
        runner_id: raw.runner_id.trim().to_string(),
        entrypoint: raw.entrypoint.trim().to_string(),
        payload: raw.payload,
        schedule: raw.schedule,
    })
}

/// 读取一个已安装版本目录下的全部预设（`presets/*.yaml`，按文件名排序；
/// 目录缺失视为零预设）。单个文件非法即整体失败，避免半套预设静默生效。
pub(crate) fn read_package_presets(
    version_root: &Path,
) -> AppPackageResult<Vec<PresetDeclaration>> {
    let dir = version_root.join("presets");
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut files: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            let lower = path
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.to_ascii_lowercase());
            path.is_file()
                && matches!(lower.as_deref(), Some(name) if name.ends_with(".yaml") || name.ends_with(".yml"))
        })
        .collect();
    files.sort();
    let mut presets = Vec::with_capacity(files.len());
    for path in files {
        let source = format!(
            "presets/{}",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("?")
        );
        let bytes = std::fs::read(&path)?;
        presets.push(parse_preset(&bytes, &source)?);
    }
    Ok(presets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_parse_requires_identity_and_schedule_kind() {
        let valid = parse_preset(
            br#"name: daily
runner_id: gamer.yaml
entrypoint: run
schedule:
  kind: cron
  value:
    expression: "0 8 * * *"
"#,
            "presets/daily.yaml",
        )
        .unwrap();
        assert_eq!(valid.name, "daily");
        assert_eq!(valid.runner_id, "gamer.yaml");
        assert_eq!(valid.schedule["value"]["expression"], "0 8 * * *");

        let empty_name = parse_preset(
            br#"name: " "
runner_id: r
entrypoint: e
schedule: {kind: cron}
"#,
            "presets/bad.yaml",
        )
        .unwrap_err();
        assert!(matches!(empty_name, AppPackageError::InvalidPreset(_)));

        let missing_kind = parse_preset(
            br#"name: a
runner_id: r
entrypoint: e
schedule: {value: {}}
"#,
            "presets/bad.yaml",
        )
        .unwrap_err();
        assert!(matches!(missing_kind, AppPackageError::InvalidPreset(_)));

        let unknown_field = parse_preset(
            br#"name: a
runner_id: r
entrypoint: e
surprise: 1
schedule: {kind: cron}
"#,
            "presets/bad.yaml",
        )
        .unwrap_err();
        assert!(matches!(unknown_field, AppPackageError::InvalidPreset(_)));
    }

    #[test]
    fn read_package_presets_is_sorted_and_tolerates_missing_dir() {
        let temp = tempfile::TempDir::new().unwrap();
        assert!(read_package_presets(temp.path()).unwrap().is_empty());

        let presets = temp.path().join("presets");
        std::fs::create_dir_all(&presets).unwrap();
        std::fs::write(
            presets.join("b.yaml"),
            br#"name: b
runner_id: r
entrypoint: e
schedule: {kind: cron}
"#,
        )
        .unwrap();
        std::fs::write(
            presets.join("a.yaml"),
            br#"name: a
runner_id: r
entrypoint: e
schedule: {kind: cron}
"#,
        )
        .unwrap();
        std::fs::write(presets.join("notes.txt"), b"ignored").unwrap();

        let presets = read_package_presets(temp.path()).unwrap();
        assert_eq!(presets.len(), 2);
        assert_eq!(presets[0].name, "a");
        assert_eq!(presets[1].name, "b");
    }
}
