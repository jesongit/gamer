//! Core tools. Plugin semantics are registered separately by their owners.
use std::sync::Arc;

use base64::Engine;
use serde_json::{json, Value};

use crate::{
    core::{AppPackageId, RunPayload, RunRequest},
    external_control::{ExternalControlService, InputAction, SessionBinding},
    mcp::{
        ContextRequirements, ErrorKind, InvocationContext, McpError, ObjectKind, ObjectScope,
        ToolDescriptor, ToolEffect, ToolProvider, ToolResult,
    },
};

#[derive(Clone)]
pub(crate) struct CoreTools {
    pub db: crate::store::Db,
    pub devices: Arc<crate::device::DeviceManager>,
    pub packages: Arc<crate::resources::PackageStore>,
    pub runs: Arc<crate::run_manager::RunManager>,
    pub scheduler: Arc<crate::scheduler::Scheduler>,
    pub extensions: Arc<crate::extensions::ExtensionService>,
    pub control: Arc<ExternalControlService>,
}

/// Pure construction: creates only the independent MCP metadata directory and
/// registers providers. It does not start a device, scheduler or network task.
#[allow(clippy::too_many_arguments)]
pub(crate) fn assemble(
    cfg: &crate::config::Config,
    db: crate::store::Db,
    devices: Arc<crate::device::DeviceManager>,
    packages: Arc<crate::resources::PackageStore>,
    runs: Arc<crate::run_manager::RunManager>,
    scheduler: Arc<crate::scheduler::Scheduler>,
    extensions: Arc<crate::extensions::ExtensionService>,
    capabilities: crate::capabilities::CapabilityRegistry,
) -> anyhow::Result<(Arc<crate::mcp::McpService>, Arc<ExternalControlService>)> {
    let mcp = Arc::new(crate::mcp::McpService::open(
        &cfg.data_dir,
        crate::mcp::ServerInfo {
            name: "Gamer".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
    )?);
    let credentials = mcp.credentials();
    let control = ExternalControlService::new(crate::external_control::Runtime {
        devices: devices.clone(),
        packages: packages.clone(),
        runs: runs.clone(),
        capabilities: capabilities.clone(),
        journal: Some(db.clone()),
        token_is_active: Arc::new(move |id| credentials.is_active(id)),
    });
    if let Some(ocr) = &extensions.core_ocr {
        mcp.register_provider(
            "gamer.ocr",
            Arc::new(crate::ocr::external::ExternalTools::new(
                ocr.clone(),
                capabilities.clone(),
                db.clone(),
                packages.clone(),
                scheduler.runner_registry(),
            )),
        )?;
    }
    mcp.register_provider("gamer.extensions", extensions.external_tools_provider())?;
    mcp.register_provider(
        "gamer.core",
        Arc::new(CoreTools {
            db,
            devices,
            packages,
            runs,
            scheduler,
            extensions,
            control: control.clone(),
        }),
    )?;
    Ok((mcp, control))
}

pub(crate) fn tool(
    name: &str,
    description: &str,
    scope: &str,
    effect: ToolEffect,
    properties: Value,
    required: &[&str],
    context: ContextRequirements,
) -> ToolDescriptor {
    ToolDescriptor {
        name: name.into(),
        description: description.into(),
        input_schema: json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}),
        output_schema: None,
        required_scopes: [scope.into()].into(),
        effect,
        context,
    }
}
pub(crate) fn string() -> Value {
    json!({"type":"string","minLength":1,"maxLength":1024})
}
fn page_limit() -> Value {
    json!({"type":"integer","minimum":1,"maximum":100})
}
fn device_context() -> ContextRequirements {
    ContextRequirements {
        device_field: Some("device_id".into()),
        ..Default::default()
    }
}
fn resource_context() -> ContextRequirements {
    ContextRequirements {
        package_field: Some("package_id".into()),
        plugin_field: Some("plugin_id".into()),
        ..Default::default()
    }
}
fn run_context() -> ContextRequirements {
    ContextRequirements {
        run_field: Some("run_id".into()),
        ..Default::default()
    }
}
fn session_context() -> ContextRequirements {
    ContextRequirements {
        control_session_field: Some("session_id".into()),
        ..Default::default()
    }
}

#[async_trait::async_trait]
impl ToolProvider for CoreTools {
    fn tools(&self) -> Vec<ToolDescriptor> {
        let mut tools = vec![
            tool("gamer.core.guide","Read the Gamer external Agent guide and safety contracts.","observe",ToolEffect::Read,json!({}),&[],Default::default()),
            tool("gamer.core.capabilities","List authorized plugins, runners and schedule providers without enabling anything.","observe",ToolEffect::Read,json!({}),&[],Default::default()),
            tool("gamer.core.targets","List configured authorized targets without connecting or scanning devices.","observe",ToolEffect::Read,json!({}),&[],Default::default()),
            tool("gamer.core.target_status","Read a configured target and its current control/run state.","observe",ToolEffect::Read,json!({"device_id":string()}),&["device_id"],device_context()),
            tool("gamer.core.capture","Capture a real PNG of an already available target. Include session_id and generation to obtain a frame usable for control.","observe",ToolEffect::Read,
                json!({"device_id":string(),"session_id":string(),"generation":{"type":"integer","minimum":1},"max_width":{"type":"integer","minimum":320,"maximum":1920}}),&["device_id"],device_context()),
            tool("gamer.core.packages","List authorized Package manifests. Package IDs are independent of Android applications.","resource.read",ToolEffect::Read,json!({}),&[],Default::default()),
            tool("gamer.core.resource_list","List resources in an authorized Package/plugin directory. Private host data is unavailable.","resource.read",ToolEffect::Read,json!({"package_id":string(),"plugin_id":string(),"prefix":{"type":"string","maxLength":1024}}),&["package_id","plugin_id"],resource_context()),
            tool("gamer.core.resource_read","Read a bounded text resource or PNG; this tool cannot publish or change resources.","resource.read",ToolEffect::Read,json!({"package_id":string(),"plugin_id":string(),"path":string()}),&["package_id","plugin_id","path"],resource_context()),
            tool("gamer.core.runner_parameters","Read the registered runner's entrypoint parameter schema.","run.read",ToolEffect::Read,json!({"runner_id":string(),"entrypoint":string(),"package_id":string()}),&["runner_id","entrypoint","package_id"],ContextRequirements{runner_field:Some("runner_id".into()),package_field:Some("package_id".into()),..Default::default()}),
            tool("gamer.core.run_submit","Submit a published entrypoint to its real Runner. Release direct control first. An accepted formal run continues independently of this HTTP request and token lifetime.","run.submit",ToolEffect::RunSubmit,
                json!({"device_id":string(),"package_id":string(),"runner_id":string(),"entrypoint":string(),"payload":{"type":"object"}}),&["device_id","package_id","runner_id","entrypoint"],ContextRequirements{device_field:Some("device_id".into()),package_field:Some("package_id".into()),runner_field:Some("runner_id".into()),..Default::default()}),
            tool("gamer.core.run_status","Read the specified run, including historical runs whose Runner is unavailable.","run.read",ToolEffect::Read,json!({"run_id":string()}),&["run_id"],run_context()),
            tool("gamer.core.run_wait","Wait up to 20 seconds for this run to finish; always returns this run's latest state.","run.read",ToolEffect::Read,json!({"run_id":string(),"timeout_seconds":{"type":"integer","minimum":0,"maximum":20}}),&["run_id"],run_context()),
            tool("gamer.core.run_cancel","Request cancellation and wait up to 10 seconds for input/activity cleanup. cleanup_pending means cleanup has not finished; poll this run. Other tokens' runs need explicit object authorization.","run.cancel",ToolEffect::RunSubmit,json!({"run_id":string()}),&["run_id"],run_context()),
            tool("gamer.core.run_history","Read bounded history for one authorized target. Pass next_before as before to continue, even when scope filtering returns no runs. A cursor does not grant access to its run.","run.read",ToolEffect::Read,json!({"device_id":string(),"before":string()}),&["device_id"],device_context()),
            tool("gamer.core.run_events","Read this run's journal after an event cursor; never substitutes a global latest run.","run.read",ToolEffect::Read,json!({"run_id":string(),"after":{"type":"integer","minimum":0}}),&["run_id"],run_context()),
            tool("gamer.core.run_trace","Read this run's trace index and frozen source snapshot, including explicit evidence gaps.","run.read",ToolEffect::Read,json!({"run_id":string(),"after":{"type":"integer","minimum":0},"limit":page_limit()}),&["run_id"],run_context()),
            tool("gamer.core.run_image","Read a real judgment-frame PNG by run_id and image_id. A call-stack frame_id is not an image_id.","run.read",ToolEffect::Read,json!({"run_id":string(),"image_id":string(),"template":{"type":"boolean"}}),&["run_id","image_id"],run_context()),
        ];
        for (action, description, effect) in [
            ("status","Read your control session without extending its TTL.",ToolEffect::Read),
            ("keepalive","Extend the TTL of your running control session. This never resumes a paused session.",ToolEffect::Input),
            ("pause","Pause your session, drain in-flight input and permit manual control while retaining the run slot.",ToolEffect::Input),
            ("release","End your session and wait for all input and activity cleanup before releasing the run slot.",ToolEffect::Input),
        ] {
            let required=if action=="status" {vec!["session_id"]} else {vec!["session_id","generation"]};
            tools.push(tool(&format!("gamer.core.control_{action}"),description,"control",effect,
                json!({"session_id":string(),"generation":{"type":"integer","minimum":1}}),&required,session_context()));
        }
        tools.push(tool("gamer.core.control_input","Perform one bounded input using your most recent session/generation/frame. Capture again after each action. User must start or resume control in Gamer.","control",ToolEffect::Input,
            json!({"session_id":string(),"generation":{"type":"integer","minimum":1},"frame_id":string(),"input":{"oneOf":[
                input_schema("tap",json!({"x":{"type":"number"},"y":{"type":"number"}}),&["x","y"]),
                input_schema("hover",json!({"x":{"type":"number"},"y":{"type":"number"}}),&["x","y"]),
                input_schema("press",json!({"x":{"type":"number"},"y":{"type":"number"},"duration_ms":{"type":"integer","minimum":60,"maximum":3000}}),&["x","y","duration_ms"]),
                input_schema("swipe",json!({"x1":{"type":"number"},"y1":{"type":"number"},"x2":{"type":"number"},"y2":{"type":"number"},"duration_ms":{"type":"integer","minimum":100,"maximum":3000}}),&["x1","y1","x2","y2","duration_ms"]),
                input_schema("key",json!({"key":{"type":"string","minLength":1,"maxLength":80},"duration_ms":{"type":"integer","minimum":0,"maximum":3000}}),&["key"]),
                input_schema("text",json!({"text":{"type":"string","maxLength":2000}}),&["text"]),
                input_schema("app_launch",json!({}),&[]),input_schema("app_stop",json!({}),&[])
            ]}}),&["session_id","generation","frame_id","input"],session_context()));
        tools.extend(self.task_tools());
        tools
    }

    async fn authorize(
        &self,
        ctx: &InvocationContext,
        name: &str,
        args: &Value,
    ) -> Result<(), McpError> {
        ctx.check_active()?;
        if name.starts_with("gamer.core.control_") {
            let id = text(args, "session_id")?;
            self.authorize_session(ctx, id)?;
            if name != "gamer.core.control_status" {
                let current = self
                    .control
                    .status(&ctx.principal.token_id, id)
                    .map_err(business)?;
                if args["generation"].as_u64() != Some(current.generation) {
                    return Err(McpError::new(
                        ErrorKind::Conflict,
                        "stale_generation",
                        "Observe the current control session again",
                    ));
                }
            }
        }
        if let Some(id) = args["run_id"].as_str() {
            self.authorize_record(ctx, &self.run_record(id).await?)?;
        }
        if let Some(id) = args["task_id"].as_str() {
            ctx.authorize_task(id)?;
            let task = self
                .db
                .get_timer_task_async(id)
                .await
                .map_err(business)?
                .ok_or_else(|| not_found("task_not_found"))?;
            self.authorize_task_scope(ctx, &task)?;
        }
        Ok(())
    }

    async fn call(
        &self,
        ctx: InvocationContext,
        name: &str,
        args: Value,
    ) -> Result<ToolResult, McpError> {
        ctx.check_active()?;
        let value = match name {
            "gamer.core.guide" => json!({"version":1,"protocol":"2025-11-25","workflow":[
                "Discover authorized capabilities and read Package resources.",
                "Edit YAML through the YAML work-draft tools; saves create a new version. Publication and rollback require the management UI.",
                "For direct control, the user starts a token-bound session in Gamer; observe status, capture a frame, act once, then capture again.",
                "Keepalive extends only your running session. TTL expiry pauses input; the user resumes in Gamer. Release and wait before submitting scripts.",
                "Use unique operation_id values for side effects. Retry the same ID only with identical arguments. operation_status_unknown requires reconciliation, never blind replay.",
                "Submitted formal runs and persistent tasks continue after HTTP disconnect or token expiry. Private trials are cancelled on credential expiry/revocation."
            ],"boundaries":{"generic_resources":"read_only","publish":"management_ui","control_start_resume":"management_ui","tools_list_changed":false},"contexts":["device_id","android_package","package_id","plugin_id"]}),
            "gamer.core.capabilities" => {
                let runners = self
                    .scheduler
                    .runners()
                    .into_iter()
                    .filter(|r| {
                        ctx.authorize_runner(&r.runner_id).is_ok()
                            && ctx.authorize_plugin(&r.owner_extension_id).is_ok()
                    })
                    .map(|r| json!({"runner_id":r.runner_id,"plugin_id":r.owner_extension_id}))
                    .collect::<Vec<_>>();
                let plugins = self
                    .extensions
                    .list()
                    .map_err(business)?
                    .into_iter()
                    .filter_map(|p| {
                        ctx.authorize_plugin(p.id().as_str()).ok()?;
                        Some(json!({"id":p.id().as_str(),"version":p.active_version().as_str(),"state":p.state(),"last_error":p.last_error()}))
                    })
                    .collect::<Vec<_>>();
                json!({"plugins":plugins,"runners":runners,"schedule_providers":self.scheduler.schedule_provider_ids()})
            }
            "gamer.core.targets" => {
                let mut targets=self.devices.list_snapshot().into_iter().filter(|(d,_,_)|ctx.authorize_device(&d.id).is_ok()).map(|(d,s,_)|json!({"device_id":d.id,"name":d.name,"kind":"android","android_package":d.pkg,"status":s})).collect::<Vec<_>>();
                for d in self.db.browser_targets().map_err(business)? {
                    if ctx.authorize_device(&d.id).is_ok() {
                        targets.push(json!({"device_id":d.id,"name":d.name,"kind":"browser","width":d.width,"height":d.height}));
                    }
                }
                json!({"targets":targets})
            }
            "gamer.core.target_status" => {
                let id = text(&args, "device_id")?;
                crate::targets::capabilities(&self.devices, id).map_err(business)?;
                let run = self.runs.active_for_device(id).filter(|r| {
                    self.authorize_record(&ctx, &serde_json::to_value(r).unwrap_or_default())
                        .is_ok()
                });
                json!({"device_id":id,"capabilities":crate::targets::capabilities(&self.devices,id).map_err(business)?,"run":run,"input_control":self.devices.controls.status(id)})
            }
            "gamer.core.capture" => {
                let binding = optional_binding(&args)?;
                if let Some(b) = &binding {
                    self.authorize_session(&ctx, &b.session_id)?;
                }
                let capture = self
                    .control
                    .capture(
                        &ctx.principal.token_id,
                        text(&args, "device_id")?,
                        binding,
                        args["max_width"].as_u64().unwrap_or(1280) as u32,
                    )
                    .await
                    .map_err(business)?;
                let mut result =
                    ToolResult::json(serde_json::to_value(&capture.metadata).map_err(business)?);
                result.content.push(png(capture.png));
                return Ok(result);
            }
            "gamer.core.packages" => {
                json!({"packages":self.packages.list_packages().map_err(business)?.into_iter().filter(|p|ctx.authorize_package(&p.id).is_ok()).map(|p|json!({"id":p.id,"name":p.name,"version":p.version,"author":p.author,"revision":p.revision,"targets":{"android":{"packages":p.android_targets},"web":{"url_prefixes":p.web_url_prefixes}},"plugins":p.plugins.into_iter().filter(|(id,_)|ctx.authorize_plugin(id).is_ok()).map(|(id,dependency)|json!({"id":id,"required":dependency.required})).collect::<Vec<_>>()})).collect::<Vec<_>>()})
            }
            "gamer.core.resource_list" => {
                json!({"resources":self.packages.list(text(&args,"package_id")?,text(&args,"plugin_id")?,args["prefix"].as_str().unwrap_or("")).map_err(business)?})
            }
            "gamer.core.resource_read" => {
                let (pkg, plugin, path) = (
                    text(&args, "package_id")?,
                    text(&args, "plugin_id")?,
                    text(&args, "path")?,
                );
                let disk = self
                    .packages
                    .resource_path(pkg, plugin, path)
                    .map_err(business)?;
                let meta = std::fs::metadata(&disk).map_err(|_| not_found("resource_not_found"))?;
                if meta.len() > 1024 * 1024 {
                    return Err(McpError::invalid("Resource exceeds the 1 MiB inline limit; use the management export interface"));
                }
                if let Some(resource) = self
                    .packages
                    .read_text(pkg, plugin, path)
                    .map_err(business)?
                {
                    serde_json::to_value(resource).map_err(business)?
                } else if path.to_ascii_lowercase().ends_with(".png") {
                    let bytes = self
                        .packages
                        .read_binary(pkg, plugin, path)
                        .map_err(business)?
                        .ok_or_else(|| not_found("resource_not_found"))?;
                    return Ok(ToolResult {
                        content: vec![png(bytes)],
                        structured_content: Some(
                            json!({"package_id":pkg,"plugin_id":plugin,"path":path}),
                        ),
                        is_error: false,
                    });
                } else {
                    return Err(McpError::invalid(
                        "Only bounded UTF-8 text and PNG resources can be returned inline",
                    ));
                }
            }
            "gamer.core.runner_parameters" => {
                self.authorize_entrypoint(
                    &ctx,
                    text(&args, "runner_id")?,
                    text(&args, "package_id")?,
                    text(&args, "entrypoint")?,
                    &json!({}),
                )?;
                self.scheduler
                    .describe_entrypoint(text(&args, "runner_id")?, text(&args, "entrypoint")?)
                    .map_err(|_| not_found("runner_or_entrypoint_unavailable"))?
            }
            "gamer.core.run_submit" => {
                let (device, pkg, runner, entry) = (
                    text(&args, "device_id")?,
                    text(&args, "package_id")?,
                    text(&args, "runner_id")?,
                    text(&args, "entrypoint")?,
                );
                let payload = args.get("payload").cloned().unwrap_or(json!({}));
                let owner = self.authorize_entrypoint(&ctx, runner, pkg, entry, &payload)?;
                self.authorize_external_runner(runner)?;
                let app = crate::targets::app_context(
                    &self.devices,
                    device,
                    Some(AppPackageId::new(pkg).map_err(business)?),
                )
                .map_err(business)?;
                let req = RunRequest::for_app(app, runner, entry, RunPayload::new(payload))
                    .map_err(business)?
                    .with_execution_policy(delegation(&ctx));
                use crate::timer_core::TimerRunner;
                ctx.check_active()?;
                let accepted = self
                    .scheduler
                    .runner_registry()
                    .submit(req, "", None, Arc::new(|_| {}))
                    .await
                    .map_err(business)?;
                ctx.record_delegated_object(
                    ObjectKind::Run,
                    &accepted.run_id,
                    &ObjectScope {
                        device_id: Some(device.into()),
                        package_id: Some(pkg.into()),
                        plugin_id: Some(owner),
                        runner_id: Some(runner.into()),
                    },
                )?;
                json!({"run_id":accepted.run_id,"state":"starting","detail":accepted.detail})
            }
            "gamer.core.run_status"
            | "gamer.core.run_wait"
            | "gamer.core.run_cancel"
            | "gamer.core.run_events"
            | "gamer.core.run_trace"
            | "gamer.core.run_image" => {
                let id = text(&args, "run_id")?;
                let mut record = self.run_record(id).await?;
                self.authorize_record(&ctx, &record)?;
                match name {
                    "gamer.core.run_wait" => {
                        let wait = args["timeout_seconds"].as_u64().unwrap_or(15);
                        let _ = tokio::time::timeout(
                            std::time::Duration::from_secs(wait),
                            self.runs.wait_terminal(id),
                        )
                        .await;
                        record = self.run_record(id).await?;
                        self.authorize_record(&ctx, &record)?;
                        record
                    }
                    "gamer.core.run_cancel" => {
                        let outcome = self.runs.cancel(id);
                        let cleanup_pending = tokio::time::timeout(
                            std::time::Duration::from_secs(10),
                            self.runs.wait_terminal(id),
                        )
                        .await
                        .is_err();
                        json!({"outcome":format!("{outcome:?}"),"cleanup_pending":cleanup_pending,"run":self.run_record(id).await?})
                    }
                    "gamer.core.run_events" => self
                        .db
                        .run_event_page(id.into(), args["after"].as_i64().unwrap_or(0))
                        .await
                        .map_err(business)?,
                    "gamer.core.run_trace" => self.db.trace.page(
                        id,
                        args["after"].as_u64().unwrap_or(0),
                        args["limit"].as_u64().unwrap_or(50) as usize,
                    ),
                    "gamer.core.run_image" => {
                        let bytes = self
                            .db
                            .trace
                            .read_image(
                                id,
                                text(&args, "image_id")?,
                                args["template"].as_bool().unwrap_or(false),
                            )
                            .map_err(business)?
                            .ok_or_else(|| not_found("trace_image_unavailable"))?;
                        return Ok(ToolResult {
                            content: vec![png(bytes)],
                            structured_content: Some(
                                json!({"run_id":id,"image_id":args["image_id"]}),
                            ),
                            is_error: false,
                        });
                    }
                    _ => record,
                }
            }
            "gamer.core.run_history" => {
                let records = self
                    .db
                    .run_history(
                        Some(text(&args, "device_id")?.into()),
                        None,
                        args["before"].as_str().map(str::to_owned),
                        None,
                    )
                    .await
                    .map_err(business)?;
                // Advance over the scanned page, including records hidden by
                // authorization. Otherwise a full page of another token's runs
                // makes every older authorized record unreachable.
                let next_before = records
                    .last()
                    .and_then(|r| r["run_id"].as_str())
                    .map(str::to_owned);
                json!({"runs":records.into_iter().filter(|r|self.authorize_record(&ctx,r).is_ok()).collect::<Vec<_>>(),"next_before":next_before})
            }
            n if n.starts_with("gamer.core.control_") => {
                let id = text(&args, "session_id")?;
                self.authorize_session(&ctx, id)?;
                if n == "gamer.core.control_status" {
                    json!(self
                        .control
                        .status(&ctx.principal.token_id, id)
                        .map_err(business)?)
                } else {
                    let binding = optional_binding(&args)?
                        .ok_or_else(|| McpError::invalid("session_id and generation required"))?;
                    match n {
                        "gamer.core.control_keepalive" => json!(self
                            .control
                            .keepalive(&ctx.principal.token_id, &binding)
                            .map_err(business)?),
                        "gamer.core.control_pause" => json!(self
                            .control
                            .pause(&ctx.principal.token_id, binding)
                            .await
                            .map_err(business)?),
                        "gamer.core.control_release" => json!(self
                            .control
                            .release(&ctx.principal.token_id, &binding)
                            .await
                            .map_err(business)?),
                        "gamer.core.control_input" => self
                            .control
                            .input(
                                &ctx.principal.token_id,
                                binding,
                                ctx.operation_id
                                    .as_deref()
                                    .ok_or_else(|| McpError::invalid("operation_id required"))?,
                                text(&args, "frame_id")?,
                                serde_json::from_value::<InputAction>(args["input"].clone())
                                    .map_err(business)?,
                            )
                            .await
                            .map_err(business)?,
                        _ => return Err(not_found("tool_not_found")),
                    }
                }
            }
            n if n.starts_with("gamer.core.task_") => self.call_task(&ctx, n, &args).await?,
            _ => return Err(not_found("tool_not_found")),
        };
        Ok(ToolResult::json(value))
    }
}

impl CoreTools {
    pub(super) fn authorize_external_runner(&self, id: &str) -> Result<(), McpError> {
        if !self
            .scheduler
            .runner_registry()
            .get_runner(id)
            .is_some_and(|runner| runner.runner.supports_external_execution(id))
        {
            return Err(McpError::forbidden(
                "This Runner has not declared support for restricted external execution",
            ));
        }
        Ok(())
    }
    fn authorize_session(&self, ctx: &InvocationContext, id: &str) -> Result<(), McpError> {
        ctx.require_scope("control")?;
        let session = self
            .control
            .status(&ctx.principal.token_id, id)
            .map_err(business)?;
        ctx.authorize_device(&session.device_id)?;
        if let Some(pkg) = session.content_package {
            ctx.authorize_package(&pkg)?;
        }
        Ok(())
    }
    fn authorize_entrypoint(
        &self,
        ctx: &InvocationContext,
        runner: &str,
        pkg: &str,
        entry: &str,
        payload: &Value,
    ) -> Result<String, McpError> {
        ctx.authorize_runner(runner)?;
        ctx.authorize_package(pkg)?;
        let registered = self
            .scheduler
            .runner_registry()
            .get_runner(runner)
            .ok_or_else(|| not_found("runner_unavailable"))?;
        ctx.authorize_plugin(&registered.owner_extension_id)?;
        crate::resources::validate_scope_id("Package", pkg).map_err(business)?;
        if entry.split(['/', '#']).next() != Some(pkg) || !entry.contains(['/', '#']) {
            return Err(McpError::forbidden(
                "Entrypoint must belong to the authorized Package",
            ));
        }
        self.packages.manifest(pkg).map_err(business)?;
        authorize_payload(ctx, payload, 0)?;
        Ok(registered.owner_extension_id)
    }
    async fn run_record(&self, id: &str) -> Result<Value, McpError> {
        if let Some(run) = self.runs.get_run(id) {
            return serde_json::to_value(run).map_err(business);
        }
        self.db
            .stored_run(id.into())
            .await
            .map_err(business)?
            .or_else(|| self.db.trace.retained_record(id))
            .ok_or_else(|| not_found("run_not_found"))
    }
    fn authorize_record(&self, ctx: &InvocationContext, record: &Value) -> Result<(), McpError> {
        let run = text(record, "run_id")?;
        ctx.authorize_run(run)?;
        ctx.authorize_device(text(record, "device_id")?)?;
        if let Some(pkg) = record["content_package"].as_str() {
            ctx.authorize_package(pkg)?;
        }
        let runner = text(record, "runner_id")?;
        ctx.authorize_runner(runner)?;
        if let Some(r) = self.scheduler.runner_registry().get_runner(runner) {
            ctx.authorize_plugin(&r.owner_extension_id)?;
        } else {
            let owner = ctx
                .object_scope(ObjectKind::Run, run)?
                .filter(|scope| scope.runner_id.as_deref() == Some(runner))
                .and_then(|scope| scope.plugin_id);
            if let Some(owner) = owner.filter(|owner| !owner.is_empty()) {
                ctx.authorize_plugin(&owner)?;
            } else if !ctx.principal.allowed.plugins.contains("*") {
                return Err(McpError::forbidden("The historical Runner owner is unavailable. Reading this explicitly granted run requires a matching frozen owner or an all-plugins range."));
            }
        }
        Ok(())
    }
}

fn authorize_payload(ctx: &InvocationContext, value: &Value, depth: usize) -> Result<(), McpError> {
    if depth > 24 {
        return Err(McpError::invalid("Payload nesting exceeds 24 levels"));
    }
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if matches!(
                    key.as_str(),
                    "owner"
                        | "principal"
                        | "human"
                        | "invocation_context"
                        | "session_id"
                        | "generation"
                ) {
                    return Err(McpError::forbidden(
                        "Payload may not supply a control identity",
                    ));
                }
                if matches!(
                    key.as_str(),
                    "device_id" | "package_id" | "content_package" | "plugin_id" | "runner_id"
                ) {
                    let id = value.as_str().ok_or_else(|| {
                        McpError::invalid("Nested object selectors must be strings")
                    })?;
                    match key.as_str() {
                        "device_id" => ctx.authorize_device(id)?,
                        "package_id" | "content_package" => ctx.authorize_package(id)?,
                        "plugin_id" => ctx.authorize_plugin(id)?,
                        "runner_id" => ctx.authorize_runner(id)?,
                        _ => {}
                    }
                }
                authorize_payload(ctx, value, depth + 1)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                authorize_payload(ctx, value, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}
fn input_schema(action: &str, properties: Value, required: &[&str]) -> Value {
    let mut properties = properties.as_object().cloned().unwrap_or_default();
    properties.insert("action".into(), json!({"const":action}));
    let mut required = required.to_vec();
    required.push("action");
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
}
fn optional_binding(args: &Value) -> Result<Option<SessionBinding>, McpError> {
    match (args["session_id"].as_str(), args["generation"].as_u64()) {
        (None, None) => Ok(None),
        (Some(id), Some(generation)) => Ok(Some(SessionBinding {
            session_id: id.into(),
            generation,
        })),
        _ => Err(McpError::invalid(
            "session_id and generation must be supplied together",
        )),
    }
}
pub(crate) fn text<'a>(args: &'a Value, key: &str) -> Result<&'a str, McpError> {
    args[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| McpError::invalid(format!("Required field: {key}")))
}
pub(crate) fn business(error: impl std::fmt::Display) -> McpError {
    McpError::new(
        ErrorKind::InvalidParams,
        "operation_rejected",
        error.to_string(),
    )
}
fn delegation(ctx: &InvocationContext) -> crate::core::ExecutionPolicy {
    crate::core::ExecutionPolicy::external(
        ctx.principal.token_id.clone(),
        ctx.principal.allowed.plugins.clone(),
        ctx.principal.scopes.clone(),
    )
}
fn not_found(code: &str) -> McpError {
    McpError::new(
        ErrorKind::NotFound,
        code,
        "The requested object is unavailable",
    )
}
fn png(bytes: Vec<u8>) -> Value {
    json!({"type":"image","mimeType":"image/png","data":base64::engine::general_purpose::STANDARD.encode(bytes)})
}

#[path = "mcp_core_tasks.rs"]
mod tasks;

#[cfg(test)]
#[path = "mcp_core_tests.rs"]
mod tests;
