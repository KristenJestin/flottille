//! What the tool lists of Flottille's agents cost in tokens, and what serving them in a lighter
//! form would save (`docs/research/tool-list-cost.md`).
//!
//! The tokens are OpenAI's `o200k_base`, counted offline: Codex's models use it, and no tokenizer
//! of Claude's runs offline, so the figure for Claude is this one times the range the research
//! note states. Nothing here calls an API.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use serde_json::{Map, Value, json};
use tiktoken_rs::CoreBPE;

/// How a tool is named to the agent: Claude and Codex both prefix it with its server's name.
pub const PREFIX: &str = "mcp__flottille__";

/// Counts tokens.
pub struct Counter(CoreBPE);

impl Counter {
    pub fn new() -> Counter {
        Counter(tiktoken_rs::o200k_base().expect("o200k_base is compiled in"))
    }

    pub fn tokens(&self, text: &str) -> usize {
        self.0.encode_ordinary(text).len()
    }
}

impl Default for Counter {
    fn default() -> Self {
        Counter::new()
    }
}

/// What one tool costs in a list: the name, the description and the schema, as an agent is given
/// them (`title`, annotations and `_meta` are for the client, not the model).
#[derive(Debug, PartialEq)]
pub struct ToolCost {
    pub name: String,
    /// The description alone.
    pub description: usize,
    /// The input schema alone, compact.
    pub schema: usize,
    /// The tool as one compact JSON object: name, description and schema, with their syntax.
    pub total: usize,
    /// The same object's characters.
    pub chars: usize,
}

pub fn tool_cost(counter: &Counter, tool: &Value) -> ToolCost {
    let name = tool["name"].as_str().unwrap_or_default();
    let description = tool["description"].as_str().unwrap_or_default();
    let schema = tool["inputSchema"].to_string();
    let given = json!({
        "name": format!("{PREFIX}{name}"),
        "description": description,
        "input_schema": tool["inputSchema"],
    })
    .to_string();
    ToolCost {
        name: name.to_owned(),
        description: counter.tokens(description),
        schema: counter.tokens(&schema),
        total: counter.tokens(&given),
        chars: given.chars().count(),
    }
}

/// How much of a tool its list keeps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Level {
    /// Today: the whole description and the whole schema.
    Whole,
    /// The whole description, and a schema without the texts of its properties (names, types,
    /// enums and required stay).
    NoSchemaTexts,
    /// The first sentence of the description, and that same schema.
    OneLine,
    /// The first sentence, and a schema that takes an `action` and its `arguments` as they are:
    /// what the tool lists of Connections do.
    Opaque,
}

pub const LEVELS: [Level; 4] = [
    Level::Whole,
    Level::NoSchemaTexts,
    Level::OneLine,
    Level::Opaque,
];

impl Level {
    pub fn label(self) -> &'static str {
        match self {
            Level::Whole => "whole (today)",
            Level::NoSchemaTexts => "whole description, schema without its texts",
            Level::OneLine => "one line, schema without its texts",
            Level::Opaque => "one line, `action` and `arguments` only",
        }
    }
}

/// The first sentence of a description, cut at a word at 160 characters.
pub fn one_line(description: &str) -> String {
    const LIMIT: usize = 160;
    let text = description.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = text.chars().collect();
    let end = chars
        .iter()
        .enumerate()
        .find(|(i, c)| {
            matches!(c, '.' | '!' | '?') && chars.get(i + 1).is_none_or(|next| next.is_whitespace())
        })
        .map_or(chars.len(), |(i, _)| i + 1);
    let sentence: String = chars[..end].iter().collect();
    if sentence.chars().count() <= LIMIT {
        return sentence;
    }
    let cut: String = sentence.chars().take(LIMIT).collect();
    match cut.rfind(' ') {
        Some(space) => cut[..space].trim_end().to_owned(),
        None => cut,
    }
}

/// The schema without the `description` of itself and of its properties, items and branches.
fn without_texts(schema: &Value) -> Value {
    let Value::Object(map) = schema else {
        return schema.clone();
    };
    let mut kept = Map::new();
    for (key, value) in map {
        match (key.as_str(), value) {
            ("description", Value::String(_)) => {}
            ("properties", Value::Object(properties)) => {
                let properties = properties
                    .iter()
                    .map(|(name, property)| (name.clone(), without_texts(property)))
                    .collect();
                kept.insert(key.clone(), Value::Object(properties));
            }
            ("items" | "additionalProperties", value) => {
                kept.insert(key.clone(), without_texts(value));
            }
            ("anyOf" | "oneOf" | "allOf", Value::Array(branches)) => {
                kept.insert(
                    key.clone(),
                    Value::Array(branches.iter().map(without_texts).collect()),
                );
            }
            _ => {
                kept.insert(key.clone(), value.clone());
            }
        }
    }
    Value::Object(kept)
}

/// The tool as its list would give it at `level`.
pub fn slimmed(tool: &Value, level: Level) -> Value {
    let mut slim = tool.clone();
    match level {
        Level::Whole => {}
        Level::NoSchemaTexts => slim["inputSchema"] = without_texts(&tool["inputSchema"]),
        Level::OneLine => {
            slim["description"] = one_line(tool["description"].as_str().unwrap_or_default()).into();
            slim["inputSchema"] = without_texts(&tool["inputSchema"]);
        }
        Level::Opaque => {
            slim["description"] = one_line(tool["description"].as_str().unwrap_or_default()).into();
            slim["inputSchema"] = json!({
                "type": "object",
                "properties": {"action": {"type": "string"}, "arguments": {"type": "object"}},
            });
        }
    }
    slim
}

/// The `help` tool a lighter list needs: it names the tools it can describe.
pub fn help_tool(names: &[String]) -> Value {
    json!({
        "name": "help",
        "description": "Describes a tool before its first use: its whole description and its \
            arguments. Give the tool, and the action when you want that action's arguments only.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "tool": {"type": "string", "enum": names},
                "action": {"type": "string"},
            },
            "required": ["tool"],
        },
    })
}

/// What a list of `tools` costs when the tools not in `keep` are given at `level`, `help` included.
pub fn list_cost(counter: &Counter, tools: &[Value], keep: &[&str], level: Level) -> usize {
    let mut slimmed_names = Vec::new();
    let mut total = 0;
    for tool in tools {
        let name = tool["name"].as_str().unwrap_or_default();
        if level == Level::Whole || keep.contains(&name) {
            total += tool_cost(counter, tool).total;
        } else {
            slimmed_names.push(name.to_owned());
            total += tool_cost(counter, &slimmed(tool, level)).total;
        }
    }
    if !slimmed_names.is_empty() {
        total += tool_cost(counter, &help_tool(&slimmed_names)).total;
    }
    total
}

fn kinds(dump: &Value) -> &[Value] {
    dump["kinds"].as_array().map_or(&[], Vec::as_slice)
}

fn tools_of(kind: &Value) -> &[Value] {
    kind["tools"].as_array().map_or(&[], Vec::as_slice)
}

/// A prompt without the user's rules for all projects (`# How to work`, between two rules), which
/// `roles::instructions` puts after the enforced text when there are any.
pub fn without_rules(prompt: &str) -> String {
    const OPEN: &str = "\n\n---\n\n# How to work\n\n";
    const RULE: &str = "\n\n---\n\n";
    let Some(start) = prompt.find(OPEN) else {
        return prompt.to_owned();
    };
    let after = start + OPEN.len();
    match prompt[after..].find(RULE) {
        Some(end) => format!("{}{}", &prompt[..start], &prompt[after + end..]),
        None => prompt.to_owned(),
    }
}

/// What a session starts with, per kind: the prompt it is given (with the default rules, and
/// without any rules of the user's) and the list, whose tokens ride in the first request.
fn start_report(counter: &Counter, dump: &Value) -> String {
    let mut out = String::from(
        "## What a session starts with\n\n\
         | Kind | Project | Prompt | Prompt without the rules | List | Start | Start without the rules |\n\
         |---|---|---|---|---|---|---|\n",
    );
    for kind in kinds(dump) {
        let Some(prompt) = kind["instructions"].as_str() else {
            continue;
        };
        let with = counter.tokens(prompt);
        let bare = counter.tokens(&without_rules(prompt));
        let list: usize = tools_of(kind)
            .iter()
            .map(|tool| tool_cost(counter, tool).total)
            .sum();
        let _ = writeln!(
            out,
            "| {} | {} | {with} | {bare} | {list} | {} | {} |",
            kind["kind"].as_str().unwrap_or_default(),
            kind["project"].as_str().unwrap_or_default(),
            with + list,
            bare + list,
        );
    }
    out.push('\n');
    out
}

/// The report of a dump, in Markdown; with `slim`, what each kind would save were those tools
/// alone given at each lighter level.
pub fn report(counter: &Counter, dump: &Value, slim: &[&str]) -> String {
    let mut out = String::new();
    if kinds(dump)
        .iter()
        .any(|kind| kind["instructions"].is_string())
    {
        out.push_str(&start_report(counter, dump));
    }
    out.push_str("## The list of each kind of agent\n\n");
    out.push_str(
        "| Kind | Project | Tools | Characters | Tokens (o200k) | of which descriptions | of which schemas |\n\
         |---|---|---|---|---|---|---|\n",
    );
    let mut biggest: Option<(usize, &Value)> = None;
    for kind in kinds(dump) {
        let costs: Vec<ToolCost> = tools_of(kind)
            .iter()
            .map(|tool| tool_cost(counter, tool))
            .collect();
        let total: usize = costs.iter().map(|c| c.total).sum();
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} | {} | {} |",
            kind["kind"].as_str().unwrap_or_default(),
            kind["project"].as_str().unwrap_or_default(),
            costs.len(),
            costs.iter().map(|c| c.chars).sum::<usize>(),
            total,
            costs.iter().map(|c| c.description).sum::<usize>(),
            costs.iter().map(|c| c.schema).sum::<usize>(),
        );
        if biggest.is_none_or(|(most, _)| total > most) {
            biggest = Some((total, kind));
        }
    }
    if !slim.is_empty() {
        out.push_str(&proposal(counter, dump, slim));
    }
    let Some((total, kind)) = biggest else {
        return out;
    };
    let tools = tools_of(kind);
    let _ = write!(
        out,
        "\n## Per tool, in the biggest list ({} in {}, {total} tokens)\n\n\
         | Tool | Whole | Description | Schema | Share | No schema texts | One line | `action` and `arguments` |\n\
         |---|---|---|---|---|---|---|---|\n",
        kind["kind"].as_str().unwrap_or_default(),
        kind["project"].as_str().unwrap_or_default(),
    );
    for tool in tools {
        let cost = tool_cost(counter, tool);
        let at = |level| tool_cost(counter, &slimmed(tool, level)).total;
        let _ = writeln!(
            out,
            "| `{}` | {} | {} | {} | {:.1}% | {} | {} | {} |",
            cost.name,
            cost.total,
            cost.description,
            cost.schema,
            100.0 * cost.total as f64 / total.max(1) as f64,
            at(Level::NoSchemaTexts),
            at(Level::OneLine),
            at(Level::Opaque),
        );
    }
    out.push_str(
        "\n## The same list with every tool slimmed to each level (`help` included)\n\n\
         | Level | Tokens | Saved | Saved share |\n|---|---|---|---|\n",
    );
    let whole = list_cost(counter, tools, &[], Level::Whole);
    for level in LEVELS {
        let cost = list_cost(counter, tools, &[], level);
        let saved = whole as i64 - cost as i64;
        let _ = writeln!(
            out,
            "| {} | {cost} | {saved} | {:.1}% |",
            level.label(),
            100.0 * saved as f64 / whole.max(1) as f64
        );
    }
    out
}

/// Whether the answer names everything the call should have said.
fn names_it(misuse: &Value) -> bool {
    let text = misuse["text"].as_str().unwrap_or_default();
    misuse["expect"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .all(|name| text.contains(name))
}

/// The report of the answers to misuse, in Markdown.
pub fn misuse_report(counter: &Counter, dump: &Value) -> String {
    // The calls that were a mistake: a tool with nothing required is not misused by no arguments.
    let misuses: Vec<&Value> = dump["misuses"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["expect"].as_array().is_some_and(|e| !e.is_empty()))
        .collect();
    let mut by_mistake: BTreeMap<&str, Vec<&Value>> = BTreeMap::new();
    for misuse in &misuses {
        by_mistake
            .entry(misuse["mistake"].as_str().unwrap_or_default())
            .or_default()
            .push(misuse);
    }
    let mut out = String::from(
        "| Mistake | Calls | Refused | The answer names what to do | Mean answer, tokens |\n\
         |---|---|---|---|---|\n",
    );
    for (mistake, calls) in &by_mistake {
        let tokens: usize = calls
            .iter()
            .map(|m| counter.tokens(m["text"].as_str().unwrap_or_default()))
            .sum();
        let _ = writeln!(
            out,
            "| {mistake} | {} | {} | {} | {} |",
            calls.len(),
            calls.iter().filter(|m| m["error"] == true).count(),
            calls.iter().filter(|m| names_it(m)).count(),
            tokens / calls.len().max(1),
        );
    }
    out.push_str("\nAnswers that do not name what the call should have been:\n\n");
    let mut none = true;
    for misuse in misuses.iter().filter(|m| !names_it(m)) {
        none = false;
        let _ = writeln!(
            out,
            "- `{}` ({})",
            misuse["tool"].as_str().unwrap_or_default(),
            misuse["mistake"].as_str().unwrap_or_default()
        );
    }
    if none {
        out.push_str("- none\n");
    }
    out
}

#[derive(Default)]
struct Usage {
    calls: usize,
    refused: usize,
    sessions: BTreeSet<String>,
    /// The sessions whose first call of the tool was refused.
    first_refused: BTreeSet<String>,
}

/// What the `agent.tool_served` events of `flottille watch --after 0` say of which tools are
/// called, in Markdown. Names and counts only: no argument is read.
pub fn usage_report(events: &str) -> String {
    let mut roots = BTreeSet::new();
    let mut children = BTreeSet::new();
    let mut usage: BTreeMap<String, Usage> = BTreeMap::new();
    for line in events.lines() {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let data = &event["data"];
        let Some(agent) = data["agent"].as_str() else {
            continue;
        };
        match event["name"].as_str() {
            Some("agent.created") => {
                let set = if data["parent"].is_null() {
                    &mut roots
                } else {
                    &mut children
                };
                set.insert(agent.to_owned());
            }
            Some("agent.tool_served") => {
                let Some(tool) = data["tool"].as_str() else {
                    continue;
                };
                let tool = usage.entry(tool.to_owned()).or_default();
                tool.calls += 1;
                let refused = data["outcome"] == "refused";
                tool.refused += usize::from(refused);
                if tool.sessions.insert(agent.to_owned()) && refused {
                    tool.first_refused.insert(agent.to_owned());
                }
            }
            _ => {}
        }
    }
    let known = roots.len() + children.len();
    let mut out = format!(
        "{known} agents ({} roots, {} children)\n\n\
         | Tool | Calls | Sessions | Root sessions | Child sessions | Refused calls | First call refused | Share of sessions |\n\
         |---|---|---|---|---|---|---|---|\n",
        roots.len(),
        children.len()
    );
    let mut rows: Vec<(&String, &Usage)> = usage.iter().collect();
    rows.sort_by(|a, b| {
        b.1.sessions
            .len()
            .cmp(&a.1.sessions.len())
            .then(a.0.cmp(b.0))
    });
    for (tool, usage) in rows {
        let _ = writeln!(
            out,
            "| `{tool}` | {} | {} | {} | {} | {} | {} | {:.0}% |",
            usage.calls,
            usage.sessions.len(),
            usage.sessions.iter().filter(|a| roots.contains(*a)).count(),
            usage
                .sessions
                .iter()
                .filter(|a| children.contains(*a))
                .count(),
            usage.refused,
            usage.first_refused.len(),
            100.0 * usage.sessions.len() as f64 / known.max(1) as f64,
        );
    }
    out
}

/// What each kind of list saves when only `slim` is given at each level, the rest whole.
fn proposal(counter: &Counter, dump: &Value, slim: &[&str]) -> String {
    let mut out = format!(
        "\n## Slimming {}\n\n| Kind | Project | Whole | {} |\n|---|---|---|{}\n",
        slim.iter()
            .map(|name| format!("`{name}`"))
            .collect::<Vec<_>>()
            .join(", "),
        LEVELS[1..]
            .iter()
            .map(|level| format!("Saved: {}", level.label()))
            .collect::<Vec<_>>()
            .join(" | "),
        "---|".repeat(LEVELS.len() - 1),
    );
    for kind in kinds(dump) {
        let tools = tools_of(kind);
        let keep: Vec<&str> = tools
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .filter(|name| !slim.contains(name))
            .collect();
        let whole = list_cost(counter, tools, &[], Level::Whole);
        let saved: Vec<String> = LEVELS[1..]
            .iter()
            .map(|level| {
                (whole as i64 - list_cost(counter, tools, &keep, *level) as i64).to_string()
            })
            .collect();
        let _ = writeln!(
            out,
            "| {} | {} | {whole} | {} |",
            kind["kind"].as_str().unwrap_or_default(),
            kind["project"].as_str().unwrap_or_default(),
            saved.join(" | ")
        );
    }
    out
}
