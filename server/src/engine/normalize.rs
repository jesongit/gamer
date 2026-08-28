//! Pure top-level YAML normalization.

use serde_yaml::Value;

use super::validate;

/// Normalize the two supported single-section shorthands into the explicit
/// `steps` / `func` document shape used by parsing and execution.
pub(super) fn normalize_top(doc: Value) -> anyhow::Result<Value> {
    match &doc {
        Value::Sequence(_) => {
            let mut mapping = serde_yaml::Mapping::new();
            mapping.insert(Value::String("steps".into()), doc);
            Ok(Value::Mapping(mapping))
        }
        Value::Mapping(mapping) => {
            if validate::validate_top_mapping(mapping)? {
                Ok(doc)
            } else {
                let mut out = serde_yaml::Mapping::new();
                out.insert(Value::String("func".into()), doc);
                Ok(Value::Mapping(out))
            }
        }
        _ => Ok(doc),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_only_the_two_unambiguous_shorthands() {
        let steps = normalize_top(serde_yaml::from_str("- log: ok").unwrap()).unwrap();
        assert_eq!(steps["steps"][0]["log"], "ok");

        let funcs = normalize_top(serde_yaml::from_str("f1:\n  - return: true").unwrap()).unwrap();
        assert!(funcs["func"].get("f1").is_some());

        let explicit =
            normalize_top(serde_yaml::from_str("config: {}\nsteps: []").unwrap()).unwrap();
        assert!(explicit.get("config").is_some());
        assert!(explicit.get("steps").is_some());
    }
}
