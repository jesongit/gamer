//! Runner-neutral diagnostic retention, bounded event reduction and export redaction.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub(crate) const POLICY_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DiagnosticsConfig {
    pub summary_retain_days: u32,
    pub success_detail_retain_days: u32,
    pub failure_detail_retain_days: u32,
    pub aggregate_repeated_events: bool,
}

impl Default for DiagnosticsConfig {
    fn default() -> Self {
        Self {
            summary_retain_days: 365,
            success_detail_retain_days: 14,
            failure_detail_retain_days: 30,
            aggregate_repeated_events: true,
        }
    }
}

impl DiagnosticsConfig {
    pub(crate) fn validate(&self) -> Option<String> {
        [
            self.summary_retain_days,
            self.success_detail_retain_days,
            self.failure_detail_retain_days,
        ]
        .into_iter()
        .any(|days| days > 36500)
        .then(|| "diagnostics retention days must be in 0..=36500 (0 disables expiry)".into())
    }
}

pub(crate) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Version metadata is supplied by the producer. Core never parses a script DSL.
pub(crate) fn snapshot_identity(snapshot: &Value) -> Value {
    let producer = snapshot
        .get("_diagnostic_identity")
        .cloned()
        .map(redact)
        .unwrap_or(Value::Null);
    let bytes = producer.to_string();
    let producer = if bytes.len() > 64 * 1024 {
        json!({"sha256":digest(bytes.as_bytes()),"omitted":"identity_exceeds_64_kib"})
    } else {
        producer
    };
    json!({
        "snapshot_sha256": digest(snapshot.to_string().as_bytes()),
        "producer": producer,
    })
}

fn sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase().replace(['-', '_', ' '], "");
    [
        "password",
        "passwd",
        "secret",
        "token",
        "cookie",
        "authorization",
        "apikey",
        "credential",
    ]
    .iter()
    .any(|part| key.contains(part))
}

/// Free-form payloads may contain arbitrary plugin values. Exports redact
/// argument/return bodies and source text, in addition to credential-like keys.
pub(crate) fn redact(value: Value) -> Value {
    match value {
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .map(|(key, value)| {
                    let hidden = sensitive_key(&key)
                        || matches!(
                            key.as_str(),
                            "args"
                                | "arguments"
                                | "payload"
                                | "values"
                                | "value"
                                | "result"
                                | "return_value"
                                | "_source_files"
                                | "source_yaml"
                        );
                    (
                        key,
                        if hidden {
                            json!({"redacted": true})
                        } else {
                            redact(value)
                        },
                    )
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.into_iter().map(redact).collect()),
        Value::String(text) => {
            let lower = text.to_ascii_lowercase();
            if lower.contains("bearer ")
                || text.contains("sk-")
                || text.contains("ghp_")
                || text.contains("github_pat_")
                || text.split_whitespace().any(|word| {
                    word.split_once(['=', ':'])
                        .is_some_and(|(key, _)| !word.contains("://") && sensitive_key(key))
                })
            {
                return Value::String("[redacted sensitive text]".into());
            }
            // Diagnostic messages retain error codes; credentials and URL query
            // values are never included in the default portable export.
            let text = text
                .split_whitespace()
                .map(|word| {
                    if word.contains("://") {
                        let prefix = word.split(['?', '#']).next().unwrap_or(word);
                        if prefix.contains('@') {
                            "[redacted-url]".to_string()
                        } else if prefix.len() != word.len() {
                            format!("{prefix}?[redacted]")
                        } else {
                            prefix.to_string()
                        }
                    } else if word.to_ascii_lowercase().contains("bearer")
                        || word.contains("sk-")
                        || word.contains("ghp_")
                        || word.contains("github_pat_")
                    {
                        "[redacted]".to_string()
                    } else {
                        word.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");
            Value::String(text)
        }
        other => other,
    }
}

/// Aggregate only repeated failed visual observations, never input, decisions,
/// errors, scope boundaries, or opaque producer-owned detail events.
pub(crate) fn observation_key(payload: &Value) -> Option<String> {
    if payload["ev"] != "vision" || payload["found"] != false {
        return None;
    }
    let mut identity = payload.clone();
    identity.as_object_mut()?.remove("score");
    Some(identity.to_string())
}

/// Temporary file + streamed response keep a large support package out of RAM.
pub(crate) fn archive(
    db: &crate::store::Store,
    run_id: &str,
) -> anyhow::Result<tempfile::NamedTempFile> {
    use std::io::Write;
    let record = db
        .diagnostic_record(run_id.to_string())?
        .or_else(|| db.trace.retained_record(run_id))
        .ok_or_else(|| anyhow::anyhow!("run_not_found"))?;
    anyhow::ensure!(record["finished_at"].is_string(), "run_active");
    let file = tempfile::NamedTempFile::new()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    let mut zip = zip::ZipWriter::new(file.reopen()?);
    fn json_file<W: Write + std::io::Seek>(
        zip: &mut zip::ZipWriter<W>,
        name: &str,
        value: &Value,
    ) -> anyhow::Result<()> {
        zip.start_file(
            name,
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated),
        )?;
        serde_json::to_writer_pretty(zip, value)?;
        Ok(())
    }
    let assets = db
        .trace
        .export_assets(run_id, &mut zip, 128 * 1024 * 1024)?;
    json_file(&mut zip, "run.json", &redact(record))?;
    json_file(&mut zip, "trace.json", &assets)?;
    zip.start_file(
        "events.jsonl",
        zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated),
    )?;
    let mut cursor = 0;
    let mut event_bytes = 0;
    let mut truncated = false;
    'pages: loop {
        let rows = db.diagnostic_events(run_id.to_string(), cursor)?;
        if rows.is_empty() {
            break;
        }
        for (id, time, payload) in rows {
            let bytes = serde_json::to_vec(&json!({"id":id,"time":time,"event":redact(payload)}))?;
            if event_bytes + bytes.len() > 16 * 1024 * 1024 {
                truncated = true;
                break 'pages;
            }
            zip.write_all(&bytes)?;
            zip.write_all(b"\n")?;
            event_bytes += bytes.len();
            cursor = id;
        }
    }
    let build = crate::build_info::build_info();
    json_file(
        &mut zip,
        "manifest.json",
        &json!({
            "format":"gamer-diagnostics-v1", "run_id":run_id,"created_at":chrono::Utc::now().to_rfc3339(),
            "exporter":{"version":build.version,"commit":build.commit,"target":build.target},
            "events_truncated":truncated,"assets":assets["files"],"export_gaps":assets["export_gaps"],
            "omitted":["browser_profiles","cookies","private_plugin_settings","credential_keys","raw_arguments","raw_return_values","source_text"],
            "image_note":"Images are original run evidence and may contain account or game information. Missing or sampled evidence is not reconstructed.",
        }),
    )?;
    zip.finish()?.sync_all()?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn observation_reduction_keeps_execution_identity_and_decisions_distinct() {
        let first =
            json!({"ev":"vision","found":false,"template":"a","score":0.1,"trace":{"frame_id":1}});
        let mut changed_score = first.clone();
        changed_score["score"] = json!(0.7);
        assert_eq!(observation_key(&first), observation_key(&changed_score));
        changed_score["trace"]["frame_id"] = json!(2);
        assert_ne!(observation_key(&first), observation_key(&changed_score));
        assert!(observation_key(&json!({"ev":"vision","found":true})).is_none());
        assert!(observation_key(&json!({"ev":"detail","data":{"message":"important"}})).is_none());
    }
    #[test]
    fn portable_export_omits_secrets_argument_values_and_source_bytes() {
        let value = redact(
            json!({"args":{"account":"private"},"api_key":"secret-value","_source_files":{"a":"secret source"},"error":"FIND_TIMEOUT","url":"https://example.test/path?token=hidden"}),
        );
        let text = value.to_string();
        for secret in ["private", "secret-value", "secret source", "hidden"] {
            assert!(!text.contains(secret));
        }
        assert_eq!(value["error"], "FIND_TIMEOUT");
        assert_eq!(value["url"], "https://example.test/path?[redacted]");
        assert!(
            !redact(json!({"error":"Authorization: Bearer very-private-value"}))
                .to_string()
                .contains("very-private-value")
        );
    }
}
