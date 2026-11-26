//! The rules a Claude client holds a listed tool to, for every test that lists one: the unit
//! tests of the lists (this file is included by `src/main.rs`) and the scenarios that read a list
//! from a running server.

use serde_json::Value;

/// The tools of a list a Claude client would drop, each with the rules it breaks.
pub fn dropped(tools: &[Value]) -> Vec<(String, Vec<String>)> {
    tools
        .iter()
        .map(|tool| (tool["name"].to_string(), claude_drops(tool)))
        .filter(|(_, broken)| !broken.is_empty())
        .collect()
}

/// Why a Claude client would drop the tool, as it is listed: each rule it breaks. Claude Code
/// drops a tool its API would refuse, silently, and the agent is told "No such tool available"
/// when it calls it (#1509). The API takes a tool whose name matches `^[a-zA-Z0-9_-]{1,64}$`
/// (with the `mcp__flottille__` Claude Code puts before it) and whose `input_schema` is a JSON
/// Schema object at its top, with no `oneOf`, `anyOf` nor `allOf` there.
fn claude_drops(tool: &Value) -> Vec<String> {
    const TYPES: [&str; 7] = [
        "string", "number", "integer", "boolean", "object", "array", "null",
    ];
    fn schema(node: &Value, path: &str, broken: &mut Vec<String>) {
        let Some(keywords) = node.as_object() else {
            broken.push(format!("{path}: a schema is an object, not {node}"));
            return;
        };
        let schemas = |value: &Value| value.as_array().filter(|all| !all.is_empty()).cloned();
        for (keyword, value) in keywords {
            let at = format!("{path}/{keyword}");
            let valid = match keyword.as_str() {
                "type" => match value {
                    Value::String(name) => TYPES.contains(&name.as_str()),
                    Value::Array(names) => {
                        !names.is_empty()
                            && names
                                .iter()
                                .all(|n| n.as_str().is_some_and(|n| TYPES.contains(&n)))
                            && names.iter().collect::<std::collections::HashSet<_>>().len()
                                == names.len()
                    }
                    _ => false,
                },
                "enum" => value.as_array().is_some_and(|all| !all.is_empty()),
                "description" | "title" | "pattern" | "format" => value.is_string(),
                "properties" => match value.as_object() {
                    Some(properties) => {
                        for (name, property) in properties {
                            schema(property, &format!("{at}/{name}"), broken);
                        }
                        true
                    }
                    None => false,
                },
                "items" => {
                    schema(value, &at, broken);
                    true
                }
                "additionalProperties" => {
                    if !value.is_boolean() {
                        schema(value, &at, broken);
                    }
                    true
                }
                "required" => value.as_array().is_some_and(|names| {
                    names.iter().all(|name| {
                        name.as_str()
                            .is_some_and(|name| !node["properties"][name].is_null())
                    })
                }),
                "minItems" | "maxItems" | "minLength" | "maxLength" | "minProperties"
                | "maxProperties" => value.is_u64(),
                "minimum" | "maximum" => value.is_number(),
                "anyOf" | "oneOf" | "allOf" => match schemas(value) {
                    Some(all) => {
                        for (i, one) in all.iter().enumerate() {
                            schema(one, &format!("{at}/{i}"), broken);
                        }
                        true
                    }
                    None => false,
                },
                "default" | "const" | "examples" => true,
                _ => {
                    broken.push(format!("{at}: a keyword this check does not know"));
                    true
                }
            };
            if !valid {
                broken.push(format!("{at}: {value}"));
            }
        }
    }
    let mut broken = Vec::new();
    let name = tool["name"].as_str().unwrap_or_default();
    let named = format!("mcp__flottille__{name}");
    if name.is_empty()
        || named.len() > 64
        || !named
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        broken.push(format!("name: {named}"));
    }
    let input = &tool["inputSchema"];
    if input["type"] != "object" {
        broken.push(format!("inputSchema/type: {}", input["type"]));
    }
    for keyword in ["oneOf", "anyOf", "allOf"] {
        if !input[keyword].is_null() {
            broken.push(format!("inputSchema/{keyword} at the top"));
        }
    }
    schema(input, "inputSchema", &mut broken);
    for text in ["title", "description"] {
        if !tool[text].is_string() {
            broken.push(format!("{text}: {}", tool[text]));
        }
    }
    broken
}
