//! Read-only discovery for media and completed teaching recordings.
use super::*;
use crate::media::{FrameRequest, MediaId, MediaMetadata};
use crate::recording::{RecordingId, RecordingSessionMeta, RecordingState};

fn paging() -> Value {
    json!({"name":{"type":"string","maxLength":255},"offset":{"type":"integer","minimum":0,"maximum":1000000},"limit":page_limit()})
}
fn media_context() -> ContextRequirements {
    ContextRequirements {
        media_field: Some("media_id".into()),
        ..Default::default()
    }
}
fn read_error(error: anyhow::Error) -> McpError {
    if let Some(error) = error.downcast_ref::<crate::media::MediaError>() {
        use crate::media::MediaErrorKind;
        if matches!(
            error.kind(),
            MediaErrorKind::NotFound | MediaErrorKind::FrameNotFound
        ) {
            return not_found("media_not_found");
        }
    }
    if error
        .downcast_ref::<crate::recording::RecordingFailure>()
        .is_some_and(|e| e.kind == crate::recording::FailureKind::NotFound)
    {
        return not_found("recording_not_found");
    }
    let code = if error.to_string().contains("recording_events_too_large") {
        "recording_events_too_large"
    } else {
        "media_read_failed"
    };
    // Native errors may contain absolute paths, tool commands or stderr.
    McpError::new(
        ErrorKind::Unavailable,
        code,
        "Media evidence could not be read. Check the recording in Gamer.",
    )
}
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> Result<T, McpError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| McpError::internal("Media read worker failed"))?
        .map_err(read_error)
}
fn page(items: Vec<Value>, args: &Value) -> Result<Value, McpError> {
    let offset = args["offset"].as_u64().unwrap_or(0) as usize;
    let limit = args["limit"].as_u64().unwrap_or(20) as usize;
    if offset > items.len() {
        return Err(McpError::invalid(
            "Offset outside the authorized collection",
        ));
    }
    let end = offset.saturating_add(limit).min(items.len());
    Ok(
        json!({"items":&items[offset..end],"total":items.len(),"next_offset":if end < items.len() { Some(end) } else { None },"complete":end==items.len()}),
    )
}
fn matches_name(name: &str, args: &Value) -> bool {
    let query = args["name"].as_str().unwrap_or("").trim().to_lowercase();
    name.to_lowercase().contains(&query)
}
fn authorize_recording(
    ctx: &InvocationContext,
    record: &RecordingSessionMeta,
) -> Result<(), McpError> {
    ctx.authorize_device(&record.device_id)?;
    // A failed recording can have events but no media segments. Without a
    // stable media identity, only an explicit all-media grant can read it.
    if record.segments.is_empty() {
        ctx.authorize_media("*")?;
    }
    for segment in &record.segments {
        ctx.authorize_media(&segment.media_id.0)?;
    }
    Ok(())
}
fn media_value(ctx: &InvocationContext, mut meta: MediaMetadata) -> Value {
    meta.refs.retain(|r| {
        ctx.authorize_package(&r.package_id).is_ok() && ctx.authorize_plugin(&r.plugin_id).is_ok()
    });
    serde_json::to_value(meta).expect("media metadata serializes")
}
fn recording_value(
    record: RecordingSessionMeta,
    names: &std::collections::BTreeMap<String, String>,
) -> Value {
    let mut value = serde_json::to_value(&record).expect("recording metadata serializes");
    value["name"] = record
        .segments
        .iter()
        .find_map(|s| names.get(&s.media_id.0))
        .cloned()
        .unwrap_or_else(|| record.id.0.clone())
        .into();
    if record.error.is_some() {
        value["error"] = "recording_failed; inspect details in Gamer".into();
    }
    value["evidence_issues"] = json!(record
        .evidence_issues
        .iter()
        .map(|s| s
            .split(':')
            .next()
            .filter(|code| code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
            .unwrap_or("recording_evidence_issue"))
        .collect::<Vec<_>>());
    if let Some(segments) = value["segments"].as_array_mut() {
        for segment in segments {
            if let Some(name) = segment["media_id"].as_str().and_then(|id| names.get(id)) {
                segment["name"] = name.clone().into();
            }
        }
    }
    value
}

impl CoreTools {
    pub(super) fn media_tools(&self) -> Vec<ToolDescriptor> {
        vec![
            tool("gamer.core.media_list","Find authorized media by display name (case-insensitive substring), newest first. Returns paginated metadata, never video bytes or local paths.","media.read",ToolEffect::Read,paging(),&[],Default::default()),
            tool("gamer.core.media_get","Read one authorized media item's identity, duration and original geometry.","media.read",ToolEffect::Read,json!({"media_id":string()}),&["media_id"],media_context()),
            tool("gamer.core.media_frames","Read true presentation-frame count and PTS bounds; optionally locate the first frame at or after pts_us.","media.read",ToolEffect::Read,json!({"media_id":string(),"pts_us":{"type":"integer","minimum":0}}),&["media_id"],media_context()),
            {
                let mut t=tool("gamer.core.media_frame","Read a deterministic original-video PNG by exact index or PTS. Returned geometry identifies any display scaling. This does not control or connect a target.","media.read",ToolEffect::Read,json!({"media_id":string(),"index":{"type":"integer","minimum":0,"maximum":4294967295u64},"pts_us":{"type":"integer","minimum":0},"max_width":{"type":"integer","minimum":320,"maximum":1920}}),&["media_id"],media_context());
                t.input_schema["oneOf"]=json!([{"required":["index"],"not":{"required":["pts_us"]}},{"required":["pts_us"],"not":{"required":["index"]}}]);t
            },
            {
                let mut p=paging();p["device_id"]=string();p["state"]=json!({"type":"string","enum":["recording","finalizing","completed","interrupted","failed","cancelled"]});
                tool("gamer.core.recording_list","Find authorized recordings by current media display name, device or state. Includes recording_id (id), segment media IDs and timeline bounds needed by sample_create. All segments and the actual device must be authorized.","media.read",ToolEffect::Read,p,&[],Default::default())
            },
            tool("gamer.core.recording_get","Read one recording's state, exact session timeline, segment identities and evidence issues without loading video or sending input.","media.read",ToolEffect::Read,json!({"recording_id":string()}),&["recording_id"],Default::default()),
            tool("gamer.core.recording_events","Read a bounded page of ended recording events in session timeline order. Supports time/kind filters to locate Alt+Q or other recorded actions. Evidence is never replayed; active, changing, corrupt or oversized recordings fail explicitly.","media.read",ToolEffect::Read,json!({"recording_id":string(),"offset":{"type":"integer","minimum":0,"maximum":1000000},"limit":page_limit(),"start_us":{"type":"integer","minimum":0},"end_us":{"type":"integer","minimum":0},"kind":{"type":"string","minLength":1,"maxLength":40}}),&["recording_id"],Default::default()),
        ]
    }
    pub(super) fn is_media_tool(&self, name: &str) -> bool {
        matches!(
            name,
            "gamer.core.media_list"
                | "gamer.core.media_get"
                | "gamer.core.media_frames"
                | "gamer.core.media_frame"
                | "gamer.core.recording_list"
                | "gamer.core.recording_get"
                | "gamer.core.recording_events"
        )
    }
    pub(super) async fn call_media(
        &self,
        ctx: InvocationContext,
        name: &str,
        args: Value,
    ) -> Result<ToolResult, McpError> {
        ctx.require_scope("media.read")?;
        let result = match name {
            "gamer.core.media_list" => {
                let media = self.media.clone();
                let mut items = blocking(move || media.list()).await?;
                ctx.check_active()?;
                items.sort_by(|a, b| {
                    b.created_at
                        .cmp(&a.created_at)
                        .then_with(|| a.id.0.cmp(&b.id.0))
                });
                let items = items
                    .into_iter()
                    .filter(|m| {
                        ctx.authorize_media(&m.id.0).is_ok() && matches_name(&m.name, &args)
                    })
                    .map(|m| media_value(&ctx, m))
                    .collect();
                ToolResult::json(page(items, &args)?)
            }
            "gamer.core.media_get" | "gamer.core.media_frames" | "gamer.core.media_frame" => {
                let id = MediaId(text(&args, "media_id")?.into());
                ctx.authorize_media(&id.0)?;
                let media = self.media.clone();
                match name {
                    "gamer.core.media_get" => {
                        let meta = blocking(move || media.get(&id)).await?;
                        ctx.check_active()?;
                        ToolResult::json(media_value(&ctx, meta))
                    }
                    "gamer.core.media_frames" => {
                        let pts = args["pts_us"].as_u64();
                        let info = blocking(move || media.frames_info(&id, pts)).await?;
                        ctx.check_active()?;
                        ToolResult::json(serde_json::to_value(info).map_err(business)?)
                    }
                    _ => {
                        let request = FrameRequest {
                            pts_us: args["pts_us"].as_u64(),
                            index: args["index"].as_u64().map(|i| i as u32),
                            max_width: Some(args["max_width"].as_u64().unwrap_or(1280) as u32),
                        };
                        let frame = blocking(move || media.extract_frame(&id, &request)).await?;
                        ctx.check_active()?;
                        let mut result = ToolResult::json(
                            serde_json::to_value(frame.descriptor).map_err(business)?,
                        );
                        result.content.push(png(frame.png));
                        result
                    }
                }
            }
            "gamer.core.recording_list" => {
                let recordings = self.recordings.clone();
                let media = self.media.clone();
                let (records, media) =
                    blocking(move || Ok((recordings.history_metadata()?, media.list()?))).await?;
                ctx.check_active()?;
                let names = media
                    .into_iter()
                    .filter(|m| ctx.authorize_media(&m.id.0).is_ok())
                    .map(|m| (m.id.0, m.name))
                    .collect();
                let mut items = Vec::new();
                for record in records {
                    let record: RecordingSessionMeta = serde_json::from_value(record)
                        .map_err(|_| McpError::internal("Invalid recording metadata"))?;
                    if authorize_recording(&ctx, &record).is_err() {
                        continue;
                    }
                    if args["device_id"]
                        .as_str()
                        .is_some_and(|id| id != record.device_id)
                    {
                        continue;
                    }
                    let value = recording_value(record, &names);
                    if args["state"]
                        .as_str()
                        .is_some_and(|state| value["state"] != state)
                    {
                        continue;
                    }
                    let name_matches = value["segments"].as_array().is_some_and(|segments| {
                        segments
                            .iter()
                            .any(|s| matches_name(s["name"].as_str().unwrap_or(""), &args))
                    });
                    if matches_name(value["name"].as_str().unwrap_or(""), &args) || name_matches {
                        items.push(value);
                    }
                }
                ToolResult::json(page(items, &args)?)
            }
            "gamer.core.recording_get" | "gamer.core.recording_events" => {
                let id = RecordingId(text(&args, "recording_id")?.into());
                let recordings = self.recordings.clone();
                let lookup = id.clone();
                let record = blocking(move || recordings.status_readonly(&lookup)).await?;
                authorize_recording(&ctx, &record)?;
                if name == "gamer.core.recording_get" {
                    let media = self.media.clone();
                    let media = blocking(move || media.list()).await?;
                    ctx.check_active()?;
                    let names = media
                        .into_iter()
                        .filter(|m| ctx.authorize_media(&m.id.0).is_ok())
                        .map(|m| (m.id.0, m.name))
                        .collect();
                    ToolResult::json(recording_value(record, &names))
                } else {
                    if matches!(
                        record.state,
                        RecordingState::Recording | RecordingState::Finalizing
                    ) {
                        return Err(McpError::new(
                            ErrorKind::Conflict,
                            "recording_not_finished",
                            "Finish recording in Gamer before reading its teaching events.",
                        ));
                    }
                    let start = args["start_us"].as_u64().unwrap_or(0);
                    let end = args["end_us"].as_u64().unwrap_or(u64::MAX);
                    if end < start {
                        return Err(McpError::invalid("end_us must be at least start_us"));
                    }
                    let recordings = self.recordings.clone();
                    let (actual, events) = blocking(move || {
                        recordings.events_snapshot_bounded(&id, 100000, 16 * 1024 * 1024)
                    })
                    .await?;
                    authorize_recording(&ctx, &actual)?;
                    if actual != record {
                        return Err(McpError::new(
                            ErrorKind::Conflict,
                            "recording_changed",
                            "Read the recording details again before requesting its events.",
                        ));
                    }
                    ctx.check_active()?;
                    let items = events
                        .into_iter()
                        .filter(|e| {
                            e.timeline_us >= start
                                && e.timeline_us <= end
                                && args["kind"].as_str().is_none_or(|kind| e.kind == kind)
                        })
                        .map(|e| serde_json::to_value(e).expect("event serializes"))
                        .collect();
                    let mut value = page(items, &args)?;
                    value["recording_id"] = args["recording_id"].clone();
                    value["schema_version"] = 1.into();
                    value["time_domain"] = "recording_session_us".into();
                    ToolResult::json(value)
                }
            }
            _ => return Err(not_found("tool_not_found")),
        };
        ctx.check_active()?;
        Ok(result)
    }
}
