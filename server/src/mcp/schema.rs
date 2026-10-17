//! Narrow offline adapter around the maintained JSON Schema implementation.
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, OnceLock},
};

use super::McpError;

pub(super) fn compile(schema: &Value) -> Result<Arc<jsonschema::Validator>, McpError> {
    static CACHE: OnceLock<Mutex<BTreeMap<String, Arc<jsonschema::Validator>>>> = OnceLock::new();
    let bytes = serde_json::to_vec(schema).map_err(|_| McpError::invalid("Invalid tool schema"))?;
    if bytes.len() > 512 * 1024 {
        return Err(McpError::invalid("Tool schema exceeds its size limit"));
    }
    let key = format!("{:x}", Sha256::digest(&bytes));
    let cache = CACHE.get_or_init(|| Mutex::new(BTreeMap::new()));
    if let Some(validator) = cache.lock().unwrap().get(&key).cloned() {
        return Ok(validator);
    }
    reject_external_references(schema, 0)?;
    let validator = Arc::new(jsonschema::draft202012::options().build(schema).map_err(
        |error| {
            McpError::invalid(format!(
                "Invalid tool schema at {} ({})",
                error.instance_path(),
                error.kind().keyword()
            ))
        },
    )?);
    let mut cache = cache.lock().unwrap();
    if cache.len() >= 128 {
        if let Some(first) = cache.keys().next().cloned() {
            cache.remove(&first);
        }
    }
    cache.insert(key, validator.clone());
    Ok(validator)
}

pub(super) fn validate(schema: &Value, instance: &Value) -> Result<(), McpError> {
    let validator = compile(schema)?;
    let errors: Vec<_> = validator
        .iter_errors(instance)
        .take(6)
        .map(|error| {
            format!(
                "{} failed {} at schema {}",
                error.instance_path(),
                error.kind().keyword(),
                error.schema_path()
            )
        })
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(McpError::invalid(errors.join("; ")))
    }
}

fn reject_external_references(value: &Value, depth: usize) -> Result<(), McpError> {
    if depth > 64 {
        return Err(McpError::invalid(
            "Tool JSON Schema exceeds its nesting limit",
        ));
    }
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if matches!(key.as_str(), "$ref" | "$dynamicRef" | "$recursiveRef") {
                    let reference = value
                        .as_str()
                        .ok_or_else(|| McpError::invalid("Schema references must be strings"))?;
                    if !reference.starts_with('#') {
                        return Err(McpError::invalid(
                            "Only local JSON Schema references are allowed",
                        ));
                    }
                }
                // A nested base URI could redirect a nominally local fragment to
                // another document. Tools need no external schema identifier.
                if key == "$id"
                    && value
                        .as_str()
                        .is_some_and(|id| !id.is_empty() && !id.starts_with('#'))
                {
                    return Err(McpError::invalid(
                        "External schema identifiers are not allowed",
                    ));
                }
                reject_external_references(value, depth + 1)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                reject_external_references(value, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn local_definitions_validate_and_network_file_references_are_rejected() {
        let schema = json!({"type":"object","$defs":{"id":{"type":"string","minLength":1}},"properties":{"id":{"$ref":"#/$defs/id"}},"required":["id"],"additionalProperties":false});
        validate(&schema, &json!({"id":"a"})).unwrap();
        assert!(validate(&schema, &json!({"id":""})).is_err());
        for reference in [
            "https://example.invalid/schema.json",
            "file:///private.json",
            "../schema.json",
        ] {
            assert!(compile(&json!({"$ref":reference})).is_err());
        }
    }
    #[test]
    fn cached_validation_does_not_echo_instance_secrets() {
        let schema = json!({"type":"object","properties":{"text":{"type":"integer"}},"additionalProperties":false});
        let first = compile(&schema).unwrap();
        let second = compile(&schema).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        let error = validate(&schema, &json!({"text":"private-user-input"})).unwrap_err();
        assert!(!error.message.contains("private-user-input"));
        assert!(error.message.contains("/text"));
    }
}
