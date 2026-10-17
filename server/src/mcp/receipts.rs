//! Durable at-most-once admission. A pending receipt is never automatically
//! replayed, including after a crash between a business commit and this store.
use std::{path::Path, sync::Mutex, time::Duration};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::{McpError, ToolResult};

#[derive(Clone, Debug)]
pub(crate) struct ReceiptKey {
    pub token_id: String,
    pub tool_name: String,
    pub object_key: String,
    pub operation_id: String,
}

pub(crate) enum Admission {
    New,
    Completed(ToolResult),
    Pending,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ObjectKind {
    Run,
    Task,
}
impl ObjectKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Task => "task",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ObjectScope {
    pub device_id: Option<String>,
    pub package_id: Option<String>,
    pub plugin_id: Option<String>,
    pub runner_id: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct OwnedObject {
    pub token_id: String,
    pub scope: ObjectScope,
}

pub(crate) struct ReceiptStore {
    connection: Mutex<Connection>,
}
impl ReceiptStore {
    pub(crate) fn open(mcp_directory: &Path) -> Result<Self, McpError> {
        std::fs::create_dir_all(mcp_directory)
            .map_err(|_| McpError::internal("Cannot create MCP receipt directory"))?;
        let connection = Connection::open(mcp_directory.join("operations.sqlite"))
            .map_err(|_| McpError::internal("Cannot open MCP operation receipts"))?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(|_| McpError::internal("Cannot configure MCP operation receipts"))?;
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(|_| McpError::internal("Cannot read MCP receipt format"))?;
        if !(0..=1).contains(&version) {
            return Err(McpError::internal("Unsupported MCP receipt format"));
        }
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS operation_receipts (
                token_id TEXT NOT NULL, tool_name TEXT NOT NULL, object_key TEXT NOT NULL,
                operation_id TEXT NOT NULL, fingerprint TEXT NOT NULL, started_at INTEGER NOT NULL,
                result_json TEXT, finished_at INTEGER,
                PRIMARY KEY(token_id, tool_name, object_key, operation_id));
            CREATE TABLE IF NOT EXISTS object_ownership (
                kind TEXT NOT NULL, object_id TEXT NOT NULL, token_id TEXT NOT NULL,
                scope_json TEXT NOT NULL, created_at INTEGER NOT NULL,
                PRIMARY KEY(kind, object_id)); PRAGMA user_version=1;",
            )
            .map_err(|_| McpError::internal("Cannot initialize MCP operation receipts"))?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }
    pub(crate) fn reserve(
        &self,
        key: &ReceiptKey,
        fingerprint: &str,
    ) -> Result<Admission, McpError> {
        let mut connection = self.connection.lock().unwrap();
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|_| McpError::internal("Cannot reserve MCP operation"))?;
        let previous:Option<(String,Option<String>)>=transaction.query_row(
            "SELECT fingerprint,result_json FROM operation_receipts WHERE token_id=?1 AND tool_name=?2 AND object_key=?3 AND operation_id=?4",
            params![key.token_id,key.tool_name,key.object_key,key.operation_id],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(|_|McpError::internal("Cannot read MCP operation receipt"))?;
        if let Some((previous, result)) = previous {
            if previous != fingerprint {
                return Err(McpError::new(
                    super::ErrorKind::Conflict,
                    "operation_conflict",
                    "The operation_id was already used with different arguments",
                ));
            }
            return match result {
                Some(result) => Ok(Admission::Completed(
                    serde_json::from_str(&result)
                        .map_err(|_| McpError::internal("MCP operation receipt is invalid"))?,
                )),
                None => Ok(Admission::Pending),
            };
        }
        transaction.execute("INSERT INTO operation_receipts(token_id,tool_name,object_key,operation_id,fingerprint,started_at) VALUES(?1,?2,?3,?4,?5,?6)",params![key.token_id,key.tool_name,key.object_key,key.operation_id,fingerprint,chrono::Utc::now().timestamp()]).map_err(|_|McpError::internal("Cannot reserve MCP operation"))?;
        transaction
            .commit()
            .map_err(|_| McpError::internal("Cannot commit MCP operation reservation"))?;
        Ok(Admission::New)
    }
    pub(crate) fn complete(
        &self,
        key: &ReceiptKey,
        fingerprint: &str,
        result: &ToolResult,
    ) -> Result<(), McpError> {
        let result = serde_json::to_string(result).map_err(|_| McpError::unknown_operation())?;
        if result.len() > 32 * 1024 * 1024 {
            return Err(McpError::unknown_operation());
        }
        let connection = self.connection.lock().unwrap();
        let changed=connection.execute("UPDATE operation_receipts SET result_json=?1,finished_at=?2 WHERE token_id=?3 AND tool_name=?4 AND object_key=?5 AND operation_id=?6 AND fingerprint=?7 AND result_json IS NULL",params![result,chrono::Utc::now().timestamp(),key.token_id,key.tool_name,key.object_key,key.operation_id,fingerprint]).map_err(|_|McpError::unknown_operation())?;
        if changed != 1 {
            return Err(McpError::unknown_operation());
        }
        Ok(())
    }
    pub(crate) fn record_object(
        &self,
        kind: ObjectKind,
        id: &str,
        token_id: &str,
        scope: &ObjectScope,
    ) -> Result<(), McpError> {
        if id.is_empty() || id.len() > 1024 {
            return Err(McpError::invalid("Invalid owned object ID"));
        }
        let mut connection = self.connection.lock().unwrap();
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|_| McpError::internal("Cannot record MCP object ownership"))?;
        let previous: Option<(String, String)> = transaction
            .query_row(
                "SELECT token_id,scope_json FROM object_ownership WHERE kind=?1 AND object_id=?2",
                params![kind.as_str(), id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|_| McpError::internal("Cannot read MCP object ownership"))?;
        let scope_json = serde_json::to_string(scope)
            .map_err(|_| McpError::internal("Cannot serialize MCP object scope"))?;
        if let Some((owner, prior_scope)) = previous {
            if owner != token_id || prior_scope != scope_json {
                return Err(McpError::new(
                    super::ErrorKind::Conflict,
                    "ownership_conflict",
                    "Object ownership is already recorded with another identity or scope",
                ));
            }
            return Ok(());
        }
        transaction.execute("INSERT INTO object_ownership(kind,object_id,token_id,scope_json,created_at) VALUES(?1,?2,?3,?4,?5)",params![kind.as_str(),id,token_id,scope_json,chrono::Utc::now().timestamp()]).map_err(|_|McpError::internal("Cannot record MCP object ownership"))?;
        transaction
            .commit()
            .map_err(|_| McpError::internal("Cannot commit MCP object ownership"))?;
        Ok(())
    }
    pub(crate) fn object(
        &self,
        kind: ObjectKind,
        id: &str,
    ) -> Result<Option<OwnedObject>, McpError> {
        let connection = self.connection.lock().unwrap();
        let value: Option<(String, String)> = connection
            .query_row(
                "SELECT token_id,scope_json FROM object_ownership WHERE kind=?1 AND object_id=?2",
                params![kind.as_str(), id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|_| McpError::internal("Cannot read MCP object ownership"))?;
        value
            .map(|(token_id, json)| {
                Ok(OwnedObject {
                    token_id,
                    scope: serde_json::from_str(&json)
                        .map_err(|_| McpError::internal("Invalid MCP owned object scope"))?,
                })
            })
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key() -> ReceiptKey {
        ReceiptKey {
            token_id: "owner".into(),
            tool_name: "gamer.core.submit".into(),
            object_key: "device-a".into(),
            operation_id: "op-1".into(),
        }
    }
    #[test]
    fn pending_survives_restart_without_replay_and_completed_results_are_exact() {
        let root = tempfile::tempdir().unwrap();
        let store = ReceiptStore::open(root.path()).unwrap();
        assert!(matches!(
            store.reserve(&key(), "hash").unwrap(),
            Admission::New
        ));
        drop(store);
        let store = ReceiptStore::open(root.path()).unwrap();
        assert!(matches!(
            store.reserve(&key(), "hash").unwrap(),
            Admission::Pending
        ));
        assert_eq!(
            store.reserve(&key(), "changed").err().unwrap().code,
            "operation_conflict"
        );
        let result = ToolResult::json(serde_json::json!({"run_id":"r"}));
        store.complete(&key(), "hash", &result).unwrap();
        drop(store);
        let store = ReceiptStore::open(root.path()).unwrap();
        assert!(
            matches!(store.reserve(&key(),"hash").unwrap(),Admission::Completed(found) if found==result)
        );
    }
    #[test]
    fn ownership_is_persistent_and_another_token_cannot_claim_existing_objects() {
        let root = tempfile::tempdir().unwrap();
        let store = ReceiptStore::open(root.path()).unwrap();
        let scope = ObjectScope {
            device_id: Some("d".into()),
            ..Default::default()
        };
        store
            .record_object(ObjectKind::Run, "r", "a", &scope)
            .unwrap();
        drop(store);
        let store = ReceiptStore::open(root.path()).unwrap();
        assert_eq!(
            store
                .object(ObjectKind::Run, "r")
                .unwrap()
                .unwrap()
                .token_id,
            "a"
        );
        assert!(store
            .record_object(ObjectKind::Run, "r", "b", &scope)
            .is_err());
    }
}
