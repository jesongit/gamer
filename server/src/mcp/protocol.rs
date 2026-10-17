//! Bounded JSON-RPC parsing; protocol handling has no plugin dependencies.
use serde_json::{json, Value};

pub(super) enum Request {
    Initialize {
        id: Value,
    },
    Ping {
        id: Value,
    },
    ToolsList {
        id: Value,
        cursor: Option<String>,
    },
    ToolsCall {
        id: Value,
        name: String,
        arguments: Value,
    },
    Notification,
    Error(Value),
}

pub(super) fn error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message.into()}})
}
pub(super) fn success(id: Value, result: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":result})
}
fn invalid() -> Request {
    Request::Error(error(Value::Null, -32600, "invalid JSON-RPC request"))
}

pub(super) fn parse(value: Value) -> Request {
    let Some(object) = value.as_object() else {
        return invalid();
    };
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return invalid();
    }
    let Some(method) = object.get("method").and_then(Value::as_str) else {
        return invalid();
    };
    let id = match object.get("id") {
        Some(Value::String(id)) if id.len() <= 256 => Some(json!(id)),
        Some(Value::Number(id)) if id.is_i64() || id.is_u64() => Some(json!(id)),
        Some(Value::Null) => Some(Value::Null),
        None => None,
        _ => return invalid(),
    };
    let Some(id) = id else {
        return Request::Notification;
    };
    let parameters = match object.get("params") {
        Some(Value::Object(parameters)) => parameters.clone(),
        None => serde_json::Map::new(),
        _ => return Request::Error(error(id, -32602, "params must be an object")),
    };
    if parameters
        .get("_meta")
        .is_some_and(|value| !value.is_object())
    {
        return Request::Error(error(id, -32602, "_meta must be an object"));
    }
    match method {
        "initialize" => {
            let version = parameters.get("protocolVersion").and_then(Value::as_str);
            let valid_client = parameters
                .get("clientInfo")
                .and_then(Value::as_object)
                .is_some_and(|client| {
                    client
                        .get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|name| !name.is_empty() && name.len() <= 256)
                        && client
                            .get("version")
                            .and_then(Value::as_str)
                            .is_some_and(|version| version.len() <= 128)
                });
            if !version.is_some_and(|version| !version.is_empty() && version.len() <= 64)
                || !parameters.get("capabilities").is_some_and(Value::is_object)
                || !valid_client
            {
                return Request::Error(error(id, -32602, "invalid initialize parameters"));
            }
            Request::Initialize { id }
        }
        "ping" if parameters.keys().all(|key| key == "_meta") => Request::Ping { id },
        "tools/list" => {
            if parameters
                .keys()
                .any(|key| !matches!(key.as_str(), "cursor" | "_meta"))
            {
                return Request::Error(error(id, -32602, "unsupported tools/list parameter"));
            }
            let cursor = match parameters.get("cursor") {
                None => None,
                Some(Value::String(value)) if value.len() <= 4096 => Some(value.clone()),
                _ => return Request::Error(error(id, -32602, "invalid tools cursor")),
            };
            Request::ToolsList { id, cursor }
        }
        "tools/call" => {
            if parameters
                .keys()
                .any(|key| !matches!(key.as_str(), "name" | "arguments" | "_meta"))
            {
                return Request::Error(error(id, -32602, "unsupported tools/call parameter"));
            }
            let Some(name) = parameters.get("name").and_then(Value::as_str) else {
                return Request::Error(error(id, -32602, "tool name is required"));
            };
            if !valid_tool_name(name) {
                return Request::Error(error(id, -32602, "invalid tool name"));
            }
            let arguments = parameters
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            if !arguments.is_object() {
                return Request::Error(error(id, -32602, "tool arguments must be an object"));
            }
            Request::ToolsCall {
                id,
                name: name.to_owned(),
                arguments,
            }
        }
        "ping" => Request::Error(error(id, -32602, "ping does not accept parameters")),
        _ => Request::Error(error(id, -32601, "method not found")),
    }
}

pub(super) fn valid_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notifications_never_receive_rpc_responses_and_ids_are_only_correlations() {
        assert!(matches!(
            parse(json!({"jsonrpc":"2.0","method":"notifications/initialized"})),
            Request::Notification
        ));
        assert!(matches!(
            parse(
                json!({"jsonrpc":"2.0","id":"correlation","method":"tools/call","params":{"name":"gamer.core.status","arguments":{}}})
            ),
            Request::ToolsCall { .. }
        ));
        assert!(matches!(
            parse(
                json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"gamer.core.status","arguments":{},"owner":"admin"}})
            ),
            Request::Error(_)
        ));
        assert!(matches!(parse(json!([])), Request::Error(_)));
    }
}
