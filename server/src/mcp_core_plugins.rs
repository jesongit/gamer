//! Explicit, pinned updates of already-installed plugins; no admin action tunnel.
use crate::{
    extensions::{ExtensionError, ExtensionId, ExtensionInstallContext, ExtensionUpdateAdmission},
    mcp::{
        ContextRequirements, ErrorKind, InvocationContext, McpError, ToolDescriptor, ToolEffect,
    },
    mcp_core::{business, string, text, tool, CoreTools},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, sync::Arc};

fn unavailable() -> McpError {
    McpError::new(
        ErrorKind::Unavailable,
        "plugin_source_unavailable",
        "Check the configured plugin source in Gamer",
    )
}
fn conflict(message: &str) -> McpError {
    McpError::new(ErrorKind::Conflict, "plugin_update_changed", message)
}

impl CoreTools {
    pub(super) fn plugin_update_tools(&self) -> Vec<ToolDescriptor> {
        let fields = json!({"plugin_id":string(),"source_id":string(),"from_version":string(),"to_version":string(),
            "sha256":{"type":"string","pattern":"^[a-f0-9]{64}$"}});
        let context = ContextRequirements {
            plugin_field: Some("plugin_id".into()),
            ..Default::default()
        };
        let update = tool("gamer.core.plugin_update", "Update one installed plugin from its configured source, pinned to old/new versions and archive SHA256. Refuses new permissions, execution-type changes and busy work; retains enabled state and immutable installations.",
            "plugin.update", ToolEffect::ResourceWrite, fields.clone(), &["plugin_id","from_version","to_version","sha256"], context.clone());
        let mut grant_fields = fields;
        grant_fields["grant_permissions"] = json!({"type":"array","maxItems":64,"uniqueItems":true,"items":{"type":"string","minLength":1,"maxLength":128}});
        let mut grant = tool("gamer.core.plugin_update_with_permissions", "Update a pinned installed plugin and explicitly grant exactly the added permissions returned by an inspected plan. Requires the separate plugin.permissions.grant scope; never changes execution type or enables a disabled plugin.",
            "plugin.update", ToolEffect::ResourceWrite, grant_fields, &["plugin_id","from_version","to_version","sha256","grant_permissions"], context);
        grant
            .required_scopes
            .insert("plugin.permissions.grant".into());
        vec![tool("gamer.core.plugin_update_plan", "List newer versions for authorized installed plugins only. inspect=true with plugin_id downloads and checks the real archive, compatibility and exact permission diff; never installs or changes source configuration.",
            "plugin.update", ToolEffect::Read, json!({"plugin_id":string(),"source_id":string(),"refresh":{"type":"boolean"},"inspect":{"type":"boolean"}}), &[], Default::default()), update, grant]
    }

    async fn update_catalog(&self, args: &Value) -> Result<Value, McpError> {
        let market = self.plugin_market.clone();
        let source = args["source_id"].as_str().map(str::to_owned);
        let refresh = args["refresh"].as_bool().unwrap_or(false);
        tokio::task::spawn_blocking(move || market.registry_for(source.as_deref(), refresh))
            .await
            .map_err(|_| unavailable())?
            .map_err(|_| unavailable())
    }

    async fn update_archive(
        &self,
        args: &Value,
        id: &str,
        version: &str,
    ) -> Result<Vec<u8>, McpError> {
        let market = self.plugin_market.clone();
        let source = args["source_id"].as_str().map(str::to_owned);
        let id = id.to_owned();
        let version = version.to_owned();
        tokio::task::spawn_blocking(move || market.archive_for(source.as_deref(), &id, &version))
            .await
            .map_err(|_| unavailable())?
            .map_err(|_| unavailable())
    }

    pub(super) async fn call_plugin_update(
        &self,
        ctx: InvocationContext,
        name: &str,
        args: Value,
    ) -> Result<Value, McpError> {
        ctx.require_scope("plugin.update")?;
        if let Some(id) = args["plugin_id"].as_str() {
            ctx.authorize_plugin(id)?;
        }
        let installed = self.extensions.list().map_err(business)?;
        let allowed: Vec<_> = installed
            .into_iter()
            .filter(|p| {
                ctx.authorize_plugin(p.id().as_str()).is_ok()
                    && args["plugin_id"]
                        .as_str()
                        .is_none_or(|id| p.id().as_str() == id)
            })
            .collect();
        if allowed.is_empty() {
            return if name == "gamer.core.plugin_update_plan" {
                Ok(json!({"updates":[]}))
            } else {
                Err(McpError::invalid(
                    "Only already-installed authorized plugins can be updated",
                ))
            };
        }
        let catalog = self.update_catalog(&args).await?;
        ctx.check_active()?;
        if name == "gamer.core.plugin_update_plan" {
            if args["inspect"] == true && args["plugin_id"].as_str().is_none() {
                return Err(McpError::invalid(
                    "Inspect one explicit plugin_id at a time",
                ));
            }
            let mut updates = vec![];
            for current in allowed {
                let old = semver::Version::parse(&current.active_version().to_string())
                    .map_err(business)?;
                let entry = catalog["plugins"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|p| p["id"] == current.id().as_str())
                    .filter_map(|p| {
                        semver::Version::parse(p["version"].as_str()?)
                            .ok()
                            .map(|v| (v, p))
                    })
                    .filter(|(v, _)| v > &old)
                    .max_by(|a, b| a.0.cmp(&b.0));
                let Some((version, entry)) = entry else {
                    continue;
                };
                let existing = current.manifest().permissions().names();
                let added: Vec<_> = entry["permissions"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .filter(|p| !existing.contains(p))
                    .map(str::to_owned)
                    .collect();
                let compatible = entry["execution"]["host_version"].as_str().is_none_or(|r| {
                    semver::VersionReq::parse(r).is_ok_and(|r| {
                        r.matches(&semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap())
                    })
                });
                let mut plan = json!({"plugin_id":current.id(),"from_version":old.to_string(),"to_version":version.to_string(),
                    "sha256":entry["sha256"],"size":entry["size"],"added_permissions":added,"host_compatible":compatible,"inspected":false});
                if args["inspect"] == true {
                    let bytes = self
                        .update_archive(&args, current.id().as_str(), &version.to_string())
                        .await?;
                    let context = ExtensionInstallContext {
                        expected_sha256: Some(text(entry, "sha256")?.into()),
                        ..Default::default()
                    };
                    let inspection = self
                        .extensions
                        .inspect_with_context(&bytes, &context)
                        .map_err(business)?;
                    if inspection.manifest().id() != current.id()
                        || inspection.manifest().version().to_string() != version.to_string()
                    {
                        return Err(conflict("Catalog and archive identity differ"));
                    }
                    plan["added_permissions"] = json!(inspection.permission_diff().added);
                    plan["execution_change"] = json!(inspection.execution_change());
                    plan["inspected"] = true.into();
                }
                updates.push(plan);
            }
            ctx.check_active()?;
            return Ok(
                json!({"updates":updates,"source_id":args["source_id"],"real_execution":false}),
            );
        }
        let id = ExtensionId::parse(text(&args, "plugin_id")?).map_err(business)?;
        let from = text(&args, "from_version")?.to_owned();
        let to = text(&args, "to_version")?.to_owned();
        if semver::Version::parse(&to).map_err(business)?
            <= semver::Version::parse(&from).map_err(business)?
        {
            return Err(McpError::invalid(
                "Updates must advance the installed version; rollback is not exposed",
            ));
        }
        let entry = catalog["plugins"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|p| p["id"] == id.as_str() && p["version"] == to)
            .ok_or_else(|| conflict("Refresh the plugin update plan"))?;
        let hash = text(&args, "sha256")?.to_owned();
        if entry["sha256"].as_str() != Some(hash.as_str()) {
            return Err(conflict("Catalog SHA256 changed"));
        }
        let bytes = self.update_archive(&args, id.as_str(), &to).await?;
        if format!("{:x}", Sha256::digest(&bytes)) != hash {
            return Err(conflict("Archive SHA256 changed"));
        }
        let grants: BTreeSet<String> = args["grant_permissions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        if name == "gamer.core.plugin_update_with_permissions" || !grants.is_empty() {
            ctx.require_scope("plugin.permissions.grant")?;
        }
        let admitted = ctx.clone();
        let expected_id = id.clone();
        let expected_to = to.clone();
        let expected_from = from.clone();
        let admission: ExtensionUpdateAdmission = Arc::new(move |active, inspection| {
            admitted
                .check_active()
                .map_err(|e| ExtensionError::Runtime(e.to_string()))?;
            if active.to_string() != expected_from
                || inspection.manifest().id() != &expected_id
                || inspection.manifest().version().to_string() != expected_to
                || inspection.execution_change().is_some()
            {
                return Err(ExtensionError::Runtime(
                    "plugin_update_changed: version or execution kind changed".into(),
                ));
            }
            let added: BTreeSet<_> = inspection.permission_diff().added.iter().cloned().collect();
            if added != grants {
                return Err(ExtensionError::PermissionConfirmationRequired(
                    "Inspect and acknowledge the exact added permissions".into(),
                ));
            }
            Ok(())
        });
        ctx.check_active()?;
        let _maintenance = self.runs.try_idle_maintenance().ok_or_else(|| {
            McpError::new(
                ErrorKind::Conflict,
                "runtime_busy",
                "Wait for active runs to finish before updating plugins",
            )
        })?;
        let context = ExtensionInstallContext {
            official: args["source_id"].is_null(),
            permission_confirmed: name == "gamer.core.plugin_update_with_permissions",
            expected_sha256: Some(hash),
        };
        let snapshot = self
            .extensions
            .update_quiescent(&bytes, &context, admission)
            .await
            .map_err(business)?;
        Ok(
            json!({"plugin_id":snapshot.id(),"active_version":snapshot.active_version(),"state":snapshot.state(),
            "installed_versions":snapshot.installed_versions(),"last_error":snapshot.last_error(),"previous_version":from}),
        )
    }
}
