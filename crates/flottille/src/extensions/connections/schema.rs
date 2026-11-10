//! Arguments checked against an action's pinned schema before the server sees them: the part of
//! JSON Schema that says what a value is (its type, its properties, the required ones, the values
//! it takes, its items, its bounds). A keyword not checked here leaves the value to the server.

use serde_json::{Map, Value};

/// Whether `value` fits `schema`; `Err` says where it does not, `at` naming the value.
pub fn fits(schema: &Value, value: &Value, at: &str) -> Result<(), String> {
    let Some(schema) = schema.as_object() else {
        // `true`, or a schema that is no object: anything fits; `false`: nothing.
        return match schema {
            Value::Bool(false) => Err(format!("{at} is not taken")),
            _ => Ok(()),
        };
    };
    if let Some(kind) = schema.get("type") {
        let kinds: Vec<&str> = match kind {
            Value::String(kind) => vec![kind.as_str()],
            Value::Array(kinds) => kinds.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        if !kinds.is_empty() && !kinds.iter().any(|kind| is(kind, value)) {
            return Err(format!(
                "{at} must be {}, not {}",
                kinds.join(" or "),
                kind_of(value)
            ));
        }
    }
    if let Some(Value::Array(values)) = schema.get("enum")
        && !values.contains(value)
    {
        let taken: Vec<String> = values.iter().map(Value::to_string).collect();
        return Err(format!(
            "{at} must be one of {}, not {value}",
            taken.join(", ")
        ));
    }
    if let Some(constant) = schema.get("const")
        && constant != value
    {
        return Err(format!("{at} must be {constant}, not {value}"));
    }
    for (keyword, any) in [("anyOf", true), ("oneOf", true), ("allOf", false)] {
        if let Some(Value::Array(schemas)) = schema.get(keyword) {
            let fitting = schemas
                .iter()
                .filter(|schema| fits(schema, value, at).is_ok())
                .count();
            if (any && fitting == 0) || (!any && fitting != schemas.len()) {
                return Err(format!("{at} fits none of the shapes it may take"));
            }
        }
    }
    match value {
        Value::Object(object) => object_fits(schema, object, at),
        Value::Array(items) => {
            if let Some(item) = schema.get("items") {
                for (n, value) in items.iter().enumerate() {
                    fits(item, value, &format!("{at}[{n}]"))?;
                }
            }
            bounded(schema, "minItems", "maxItems", items.len(), at, "items")
        }
        Value::String(text) => bounded(
            schema,
            "minLength",
            "maxLength",
            text.chars().count(),
            at,
            "characters",
        ),
        Value::Number(number) => {
            let number = number.as_f64().unwrap_or_default();
            if let Some(minimum) = schema.get("minimum").and_then(Value::as_f64)
                && number < minimum
            {
                return Err(format!("{at} must be {minimum} at least"));
            }
            if let Some(maximum) = schema.get("maximum").and_then(Value::as_f64)
                && number > maximum
            {
                return Err(format!("{at} must be {maximum} at most"));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn object_fits(
    schema: &Map<String, Value>,
    object: &Map<String, Value>,
    at: &str,
) -> Result<(), String> {
    let properties = schema.get("properties").and_then(Value::as_object);
    if let Some(Value::Array(required)) = schema.get("required") {
        for key in required.iter().filter_map(Value::as_str) {
            if !object.contains_key(key) {
                return Err(format!("{at}.{key} is required"));
            }
        }
    }
    for (key, value) in object {
        let at = format!("{at}.{key}");
        match properties.and_then(|properties| properties.get(key)) {
            Some(property) => fits(property, value, &at)?,
            None => match schema.get("additionalProperties") {
                Some(Value::Bool(false)) => {
                    let known: Vec<&str> = properties
                        .map(|properties| properties.keys().map(String::as_str).collect())
                        .unwrap_or_default();
                    return Err(format!(
                        "{at} is not an argument it takes: it takes {}",
                        if known.is_empty() {
                            "none".to_owned()
                        } else {
                            known.join(", ")
                        }
                    ));
                }
                Some(other @ Value::Object(_)) => fits(other, value, &at)?,
                _ => {}
            },
        }
    }
    Ok(())
}

fn bounded(
    schema: &Map<String, Value>,
    least: &str,
    most: &str,
    count: usize,
    at: &str,
    unit: &str,
) -> Result<(), String> {
    if let Some(least) = schema.get(least).and_then(Value::as_u64)
        && (count as u64) < least
    {
        return Err(format!("{at} must hold {least} {unit} at least"));
    }
    if let Some(most) = schema.get(most).and_then(Value::as_u64)
        && (count as u64) > most
    {
        return Err(format!("{at} must hold {most} {unit} at most"));
    }
    Ok(())
}

fn is(kind: &str, value: &Value) -> bool {
    match kind {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "number" => value.is_number(),
        "integer" => value.is_i64() || value.is_u64(),
        _ => true,
    }
}

fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn arguments_are_checked_against_the_schema() {
        let schema = json!({"type": "object", "properties": {
            "query": {"type": "string", "minLength": 1},
            "limit": {"type": "integer", "maximum": 50},
            "kind": {"enum": ["note", "decision"]},
        }, "required": ["query"], "additionalProperties": false});
        assert!(fits(&schema, &json!({"query": "a", "limit": 3}), "arguments").is_ok());
        let refused = |value: Value| fits(&schema, &value, "arguments").unwrap_err();
        assert_eq!(refused(json!({})), "arguments.query is required");
        assert_eq!(
            refused(json!({"query": 3})),
            "arguments.query must be string, not a number"
        );
        assert!(refused(json!({"query": "a", "limit": 51})).contains("50 at most"));
        assert!(refused(json!({"query": "a", "kind": "x"})).contains("one of"));
        assert!(refused(json!({"query": "a", "other": 1})).contains("not an argument"));
        assert!(refused(json!({"query": "a", "limit": 1.5})).contains("integer"));
    }
}
