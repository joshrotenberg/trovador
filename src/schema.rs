//! JSON schemas for what each command returns, published as MCP `outputSchema` and printed
//! by `trovador schema`.
//!
//! `required` is stripped throughout. A caller can leave fields out with `fields` (see
//! [`crate::fields`]), and a client that validates `structuredContent` against the
//! published schema would otherwise reject the result it asked for.

use schemars::JsonSchema;
use serde_json::{Map, Value};

use crate::sessions::{ListResult, SearchResult, SessionDetail, TranscriptResult};
use crate::text::SessionText;

/// The commands with a published schema, in the order `trovador schema` lists them.
pub const NAMES: &[&str] = &["list", "show", "transcript", "text", "search"];

/// The schema a command's result follows, or None for a name not in [`NAMES`].
pub fn of(name: &str) -> Option<Value> {
    Some(match name {
        "list" => published::<ListResult>(),
        "show" => published::<SessionDetail>(),
        "transcript" => published::<TranscriptResult>(),
        "text" => published::<SessionText>(),
        "search" => published::<SearchResult>(),
        _ => return None,
    })
}

/// The `sessions` tool returns a different result per `action`: one of the four, with
/// their definitions gathered at the root where each `$ref` points.
pub fn sessions_tool() -> Value {
    let mut defs = Map::new();
    let mut one_of = Vec::new();
    for name in ["list", "show", "transcript", "text"] {
        let mut s = of(name).expect("a published name");
        if let Some(Value::Object(d)) = s.as_object_mut().and_then(|o| o.remove("$defs")) {
            defs.extend(d);
        }
        if let Some(o) = s.as_object_mut() {
            o.remove("$schema");
        }
        one_of.push(s);
    }
    let mut root = Map::new();
    root.insert("type".into(), "object".into());
    root.insert("oneOf".into(), Value::Array(one_of));
    if !defs.is_empty() {
        root.insert("$defs".into(), Value::Object(defs));
    }
    Value::Object(root)
}

fn published<T: JsonSchema>() -> Value {
    let mut v = schemars::schema_for!(T).to_value();
    strip_required(&mut v);
    v
}

fn strip_required(v: &mut Value) {
    match v {
        Value::Object(o) => {
            if o.get("required").is_some_and(Value::is_array) {
                o.remove("required");
            }
            o.values_mut().for_each(strip_required);
        }
        Value::Array(a) => a.iter_mut().for_each(strip_required),
        _ => {}
    }
}

/// The property names of `T` as serialized, flattened fields included.
pub(crate) fn property_names<T: JsonSchema>() -> Vec<String> {
    let v = schemars::schema_for!(T).to_value();
    let mut names: Vec<String> = v
        .get("properties")
        .and_then(Value::as_object)
        .map(|p| p.keys().cloned().collect())
        .unwrap_or_default();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has_required(v: &Value) -> bool {
        match v {
            Value::Object(o) => {
                o.get("required").is_some_and(Value::is_array) || o.values().any(has_required)
            }
            Value::Array(a) => a.iter().any(has_required),
            _ => false,
        }
    }

    #[test]
    fn every_published_schema_has_no_required_list() {
        for name in NAMES {
            let s = of(name).unwrap();
            assert!(!has_required(&s), "{name}");
            assert!(s.get("properties").is_some(), "{name}");
        }
        assert!(!has_required(&sessions_tool()));
    }

    #[test]
    fn the_sessions_schema_resolves_every_reference_at_its_root() {
        let s = sessions_tool();
        let defs = s["$defs"].as_object().unwrap();
        let text = s.to_string();
        for r in text.split("\"$ref\":\"#/$defs/").skip(1) {
            let name = &r[..r.find('"').unwrap()];
            assert!(defs.contains_key(name), "unresolved {name}");
        }
        assert_eq!(s["oneOf"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn row_properties_include_flattened_metadata() {
        let names = property_names::<crate::sessions::SessionRow>();
        for n in ["session_id", "modified", "title", "snippet", "resumable"] {
            assert!(names.iter().any(|x| x == n), "{n} in {names:?}");
        }
        assert!(!names.iter().any(|x| x == "last_record_ns"));
    }
}
