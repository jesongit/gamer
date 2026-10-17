use super::*;
use crate::timer_core::{ScheduleExtension, Task, TaskSchedule, TaskState};

impl CoreTools {
    pub(super) fn task_tools(&self) -> Vec<ToolDescriptor> {
        let context = ContextRequirements {
            task_field: Some("task_id".into()),
            ..Default::default()
        };
        vec![
            tool("gamer.core.task_list","List tasks allowed by this token, filtered by their actual nested execution scope.","task.read",ToolEffect::Read,json!({}),&[],Default::default()),
            tool("gamer.core.task_get","Read one authorized task including its schedule, runner and current state.","task.read",ToolEffect::Read,json!({"task_id":string()}),&["task_id"],context.clone()),
            tool("gamer.core.task_save","Create a disabled task or update an explicitly authorized task. Enabled tasks additionally require run.submit. Omit task_id for creation; updates require expected_updated_at.","task.write",ToolEffect::TaskWrite,
                json!({"task_id":string(),"expected_updated_at":string(),"name":{"type":"string","minLength":1,"maxLength":255},"device_id":string(),"package_id":string(),"runner_id":string(),"entrypoint":string(),"payload":{"type":"object"},"schedule":{"type":"object","properties":{"provider_id":string(),"config":{"type":"object"}},"required":["provider_id","config"],"additionalProperties":false},"enabled":{"type":"boolean"}}),
                &["name","device_id","package_id","runner_id","entrypoint","schedule"],ContextRequirements{device_field:Some("device_id".into()),package_field:Some("package_id".into()),runner_field:Some("runner_id".into()),..Default::default()}),
            tool("gamer.core.task_control","Control your task or an explicitly authorized task. Running/enabling/resuming additionally requires run.submit for its actual target, Package, plugin, runner, entrypoint and payload.","task.control",ToolEffect::TaskWrite,
                json!({"task_id":string(),"action":{"enum":["run","enable","resume","disable","suspend","cancel"]}}),&["task_id","action"],context),
        ]
    }

    pub(super) fn authorize_task_scope(
        &self,
        ctx: &InvocationContext,
        task: &Task,
    ) -> Result<ObjectScope, McpError> {
        ctx.authorize_device(task.app.device_id.as_str())?;
        ctx.authorize_runner(&task.runner_id)?;
        let pkg = task
            .app
            .content_package
            .as_ref()
            .ok_or_else(|| McpError::forbidden("Task has no explicit Package context"))?
            .as_str();
        ctx.authorize_package(pkg)?;
        if task.entrypoint.split(['/', '#']).next() != Some(pkg) {
            return Err(McpError::forbidden(
                "Task entrypoint does not belong to its Package",
            ));
        }
        authorize_payload(ctx, &task.payload, 0)?;
        let owner = self
            .scheduler
            .runner_registry()
            .get_runner(&task.runner_id)
            .map(|r| r.owner_extension_id);
        let owner = if let Some(owner) = owner {
            ctx.authorize_plugin(&owner)?;
            Some(owner)
        } else {
            let frozen = ctx.object_scope(ObjectKind::Task, &task.id)?;
            let owner = frozen
                .filter(|scope| scope.runner_id.as_deref() == Some(task.runner_id.as_str()))
                .and_then(|scope| scope.plugin_id);
            if let Some(owner) = owner {
                ctx.authorize_plugin(&owner)?;
                Some(owner)
            } else {
                if !ctx.principal.allowed.plugins.contains("*") {
                    return Err(McpError::forbidden("The task Runner owner is unavailable; use a frozen owner receipt or an explicit all-plugins range"));
                }
                None
            }
        };
        Ok(ObjectScope {
            device_id: Some(task.app.device_id.as_str().into()),
            package_id: Some(pkg.into()),
            plugin_id: owner,
            runner_id: Some(task.runner_id.clone()),
        })
    }

    fn authorize_task_execution(
        &self,
        ctx: &InvocationContext,
        task: &Task,
    ) -> Result<ObjectScope, McpError> {
        ctx.require_scope("run.submit")?;
        self.authorize_external_runner(&task.runner_id)?;
        let mut scope = self.authorize_task_scope(ctx, task)?;
        let pkg = scope.package_id.as_deref().unwrap();
        scope.plugin_id = Some(self.authorize_entrypoint(
            ctx,
            &task.runner_id,
            pkg,
            &task.entrypoint,
            &task.payload,
        )?);
        // Target identity comes from trusted configuration, never caller JSON.
        let actual = crate::targets::app_context(
            &self.devices,
            task.app.device_id.as_str(),
            task.app.content_package.clone(),
        )
        .map_err(business)?;
        if actual != task.app {
            return Err(McpError::forbidden(
                "Task application binding changed; update the task in Gamer before executing",
            ));
        }
        self.scheduler
            .schedules()
            .probe(&task.schedule)
            .map_err(business)?;
        Ok(scope)
    }

    pub(super) async fn persist_task_snapshot(
        &self,
        ctx: &InvocationContext,
        expected: Option<&Task>,
        task: &Task,
    ) -> Result<(), McpError> {
        let admitted = ctx.clone();
        let check: crate::store::TaskWriteAdmission =
            Arc::new(move || admitted.check_active().map_err(anyhow::Error::new));
        if !self
            .db
            .compare_save_timer_task_async(task, expected, Some(check))
            .await
            .map_err(business)?
        {
            return Err(McpError::new(
                ErrorKind::Conflict,
                "task_changed",
                "The task changed after authorization; re-read it before changing it",
            ));
        }
        self.scheduler.notify_tasks_changed();
        Ok(())
    }
    pub(super) fn authorize_task_run(
        &self,
        ctx: &InvocationContext,
        task: &Task,
        record: &Value,
    ) -> Result<(), McpError> {
        // Task control authorizes its actual child run through task ownership,
        // including scheduled children that have no separate token run receipt.
        self.authorize_task_scope(ctx, task)?;
        let package = task.app.content_package.as_ref().map(|pkg| pkg.as_str());
        if record["task_id"].as_str() != Some(task.id.as_str())
            || record["device_id"].as_str() != Some(task.app.device_id.as_str())
            || record["content_package"].as_str() != package
            || record["runner_id"].as_str() != Some(task.runner_id.as_str())
            || record["entrypoint"].as_str() != Some(task.entrypoint.as_str())
        {
            return Err(McpError::new(
                ErrorKind::Conflict,
                "task_run_changed",
                "The active run no longer matches the authorized task snapshot",
            ));
        }
        Ok(())
    }

    pub(super) async fn call_task(
        &self,
        ctx: &InvocationContext,
        name: &str,
        args: &Value,
    ) -> Result<Value, McpError> {
        match name {
            "gamer.core.task_list" => {
                let tasks = self.db.list_timer_tasks_async().await.map_err(business)?;
                Ok(
                    json!({"tasks":tasks.into_iter().filter(|t|ctx.authorize_task(&t.id).is_ok()&&self.authorize_task_scope(ctx,t).is_ok()).collect::<Vec<_>>()}),
                )
            }
            "gamer.core.task_get" => {
                let id = text(args, "task_id")?;
                ctx.authorize_task(id)?;
                let task = self
                    .db
                    .get_timer_task_async(id)
                    .await
                    .map_err(business)?
                    .ok_or_else(|| not_found("task_not_found"))?;
                self.authorize_task_scope(ctx, &task)?;
                Ok(json!(task))
            }
            "gamer.core.task_save" => {
                let previous = if let Some(id) = args["task_id"].as_str() {
                    ctx.authorize_task(id)?;
                    let task = self
                        .db
                        .get_timer_task_async(id)
                        .await
                        .map_err(business)?
                        .ok_or_else(|| not_found("task_not_found"))?;
                    self.authorize_task_scope(ctx, &task)?;
                    let expected = args["expected_updated_at"]
                        .as_str()
                        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok());
                    if !expected
                        .is_some_and(|value| value.with_timezone(&chrono::Utc) == task.updated_at)
                    {
                        return Err(McpError::new(
                            ErrorKind::Conflict,
                            "task_changed",
                            "Re-read the task and supply its expected_updated_at before saving",
                        ));
                    }
                    Some(task)
                } else {
                    None
                };
                let id = previous
                    .as_ref()
                    .map(|t| t.id.clone())
                    .unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
                let pkg = AppPackageId::new(text(args, "package_id")?).map_err(business)?;
                let app =
                    crate::targets::app_context(&self.devices, text(args, "device_id")?, Some(pkg))
                        .map_err(business)?;
                let payload = args.get("payload").cloned().unwrap_or(json!({}));
                let schedule: TaskSchedule =
                    serde_json::from_value(args["schedule"].clone()).map_err(business)?;
                self.scheduler
                    .schedules()
                    .probe(&schedule)
                    .map_err(business)?;
                let mut task = Task::new(
                    id,
                    text(args, "name")?,
                    app,
                    text(args, "runner_id")?,
                    text(args, "entrypoint")?,
                    payload,
                    schedule,
                )
                .map_err(business)?;
                task.enabled = args["enabled"]
                    .as_bool()
                    .or_else(|| previous.as_ref().map(|t| t.enabled))
                    .unwrap_or(false);
                task.state = if task.enabled {
                    TaskState::Active
                } else {
                    TaskState::Suspended
                };
                task.suspend_reason = (!task.enabled).then(|| "disabled".into());
                if let Some(old) = &previous {
                    task.created_at = old.created_at;
                    task.last_run_at = old.last_run_at;
                    task.last_result = old.last_result.clone();
                    task.extensions = old.extensions.clone();
                    task.preset_id = old.preset_id.clone();
                }
                let scope = if task.enabled {
                    self.authorize_task_execution(ctx, &task)?
                } else {
                    self.authorize_task_scope(ctx, &task)?
                };
                // Even disabled tasks may not hide an unauthorized Runner or a
                // foreign entrypoint for a later privileged enable operation.
                self.authorize_entrypoint(
                    ctx,
                    &task.runner_id,
                    scope.package_id.as_deref().unwrap(),
                    &task.entrypoint,
                    &task.payload,
                )?;
                ctx.check_active()?;
                delegation(ctx)
                    .store_in_task(&mut task.extensions)
                    .map_err(business)?;
                self.persist_task_snapshot(ctx, previous.as_ref(), &task)
                    .await?;
                if previous.is_none() {
                    ctx.record_delegated_object(ObjectKind::Task, &task.id, &scope)?;
                }
                self.scheduler.notify_tasks_changed();
                Ok(json!(task))
            }
            "gamer.core.task_control" => {
                let id = text(args, "task_id")?;
                ctx.authorize_task(id)?;
                let mut task = self
                    .db
                    .get_timer_task_async(id)
                    .await
                    .map_err(business)?
                    .ok_or_else(|| not_found("task_not_found"))?;
                let scope = self.authorize_task_scope(ctx, &task)?;
                let expected = task.clone();
                let action = text(args, "action")?;
                if matches!(action, "run" | "enable" | "resume") {
                    self.authorize_task_execution(ctx, &task)?;
                    delegation(ctx)
                        .store_in_task(&mut task.extensions)
                        .map_err(business)?;
                }
                if action == "run" {
                    ctx.check_active()?;
                    let run_id = self.scheduler.run_now(&task).await.map_err(business)?;
                    ctx.record_delegated_object(ObjectKind::Run, &run_id, &scope)?;
                    return Ok(json!({"run_id":run_id,"task_id":id}));
                }
                let cancel_run = if action == "cancel" {
                    let run = self.scheduler.active_task_run_id(id);
                    if let Some(run) = &run {
                        self.authorize_task_run(ctx, &task, &self.run_record(run).await?)?;
                    }
                    run
                } else {
                    None
                };
                match action {
                    "enable" | "resume" => {
                        if task.state == TaskState::Cancelled {
                            return Err(McpError::invalid("Cancelled tasks cannot be resumed"));
                        }
                        task.next_wakeup = self
                            .scheduler
                            .schedules()
                            .next_after(&task.schedule, chrono::Utc::now())
                            .map_err(business)?;
                        task.state = TaskState::Active;
                        task.enabled = true;
                        task.suspend_reason = None;
                    }
                    "disable" | "suspend" => {
                        task.state = TaskState::Suspended;
                        task.enabled = false;
                        task.next_wakeup = None;
                        task.suspend_reason = Some("disabled_by_external_agent".into());
                    }
                    "cancel" => {
                        task.state = TaskState::Cancelled;
                        task.enabled = false;
                        task.next_wakeup = None;
                        task.suspend_reason = None;
                    }
                    _ => return Err(McpError::invalid("Unknown task action")),
                }
                task.updated_at = chrono::Utc::now();
                ctx.check_active()?;
                self.persist_task_snapshot(ctx, Some(&expected), &task)
                    .await?;
                if let Some(run) = &cancel_run {
                    self.runs.cancel(run);
                    self.runs.wait_terminal(run).await;
                }
                // Return the accepted snapshot. A fresh ID lookup could expose
                // a task moved outside this credential after the CAS commit.
                Ok(json!({"task":task,"cancelled_run_id":cancel_run}))
            }
            _ => Err(not_found("tool_not_found")),
        }
    }
}
