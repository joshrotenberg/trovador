use serde_json::Value;

use crate::types::RunOutput;

const MAX_JSON_BYTES: usize = 64_000;

pub(crate) fn compile_schema(schema: &Value) -> Result<jsonschema::Validator, String> {
    if !schema.is_object() || schema.get("type").and_then(Value::as_str) != Some("object") {
        return Err("output_schema must be a JSON Schema object with root type object".into());
    }
    check_size("output_schema", schema)?;
    // Do not fetch referenced schemas from the network or local filesystem.
    jsonschema::options()
        .offline()
        .build(schema)
        .map_err(|e| format!("invalid output_schema: {e}"))
}

pub(crate) fn validate_result(
    schema: Option<&Value>,
    mut output: RunOutput,
) -> Result<RunOutput, String> {
    let Some(schema) = schema else {
        output.structured_result = None;
        return Ok(output);
    };
    let value = output
        .structured_result
        .as_ref()
        .ok_or("provider returned no structured_result for the requested output_schema")?;
    check_size("structured_result", value)?;
    compile_schema(schema)?
        .validate(value)
        .map_err(|e| format!("structured_result does not match output_schema: {e}"))?;
    Ok(output)
}

fn check_size(name: &str, value: &Value) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|e| format!("cannot serialize {name}: {e}"))?;
    if bytes.len() > MAX_JSON_BYTES {
        return Err(format!("{name} exceeds {MAX_JSON_BYTES} serialized bytes"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::compile_schema;

    #[test]
    fn schemas_can_use_local_definitions_but_never_fetch_external_references() {
        let schema = json!({
            "type": "object", "properties": {"answer": {"$ref": "#/$defs/answer"}},
            "$defs": {"answer": {"type": "integer"}}, "required": ["answer"]
        });
        let validator = compile_schema(&schema).unwrap();
        assert!(validator.is_valid(&json!({"answer": 42})));
        assert!(!validator.is_valid(&json!({"answer": "42"})));
        for reference in ["https://example.com/schema.json", "file:///tmp/schema.json"] {
            assert!(compile_schema(&json!({"type": "object", "$ref": reference})).is_err());
        }
    }
}
