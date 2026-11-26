//! The findings an agent files about Flottille itself (`docs/developer.md`, "The findings"):
//! `services` `report` and `find_reports`, offered while the developer mode is on, and the
//! daemon's `developer.reports*` methods that list, mark and export them.
//!
//! A finding is one bug with its occurrences. The agent searches the open findings first and adds
//! an occurrence to the one that describes its bug: the judgement is the agent's, Flottille does
//! no fuzzy matching of its own. Every text is masked as a tool's output is (`mcp::masked`)
//! before it is stored, so neither the store nor the export ever holds a secret.

use flottille_core::finding::{self, Finding, NewOccurrence};
use flottille_core::protocol::Event;
use flottille_core::store::now_ms;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::mcp::{Grant, Outcome, arguments, masked};
use crate::projects::Failure;

/// How many findings a search answers at most.
const FOUND: usize = 10;

/// The actions of `services` this module answers, offered while the developer mode is on.
pub const ACTIONS: [&str; 2] = ["report", "find_reports"];

/// What `services` says of them in its description while they are offered: when to report, which
/// is the guide of the agents, and what is never a finding.
pub const GUIDE: &str = " report and find_reports file a finding about Flottille itself, never \
    about your task or your own mistake: a tool of Flottille that answered wrong, a refusal that \
    makes no sense, a step that hangs. Never report what you did wrong, what the user asked, or \
    a bug of the project's code. Search first: find_reports {\"query\": \"tickets empty\"} lists \
    the open findings whose title or text hold every word. If one describes your bug, add an \
    occurrence to it: {\"action\": \"report\", \"finding\": 3, \"called\": \"what you called\", \
    \"happened\": \"what happened this time\", \"differs\": \"what differs from before\"}; \
    otherwise a new finding: {\"action\": \"report\", \"title\": \"one line\", \"called\": \
    \"what you called\", \"expected\": \"what you expected\", \"happened\": \"what happened\"}. \
    Never put a secret in one. Harmless: it is kept for the maintainer and changes nothing.";

/// Whether a call of `services` is one of these actions.
pub fn is(args: &Value) -> bool {
    args["action"]
        .as_str()
        .is_some_and(|action| ACTIONS.contains(&action))
}

/// `services` as it is listed while the developer mode is on: the two actions, their fields, and
/// the guide that says when to use them.
pub fn offer(tool: &mut rmcp::model::Tool) {
    let mut schema = Value::Object((*tool.input_schema).clone());
    if let Some(actions) = schema["properties"]["action"]["enum"].as_array_mut() {
        actions.extend(ACTIONS.iter().map(|action| json!(action)));
    }
    if let Some(fields) = schema["properties"].as_object_mut() {
        let texts = [
            (
                "title",
                "report, a new finding: one line that says the bug.",
            ),
            ("expected", "report, a new finding: what you expected."),
            ("called", "report: what you called, with its arguments."),
            ("happened", "report: what happened."),
            (
                "differs",
                "report, an occurrence: what differs from the finding's other occurrences.",
            ),
            ("query", "find_reports: the words of your bug."),
        ];
        for (name, about) in texts {
            fields.insert(name.into(), json!({"type": "string", "description": about}));
        }
        fields.insert(
            "finding".into(),
            json!({"type": "integer", "minimum": 1, "description": "report: the open finding that describes your bug, which gets this occurrence."}),
        );
    }
    if let Value::Object(schema) = schema {
        tool.input_schema = std::sync::Arc::new(schema);
    }
    let description = tool.description.take().unwrap_or_default();
    tool.description = Some(format!("{description}{GUIDE}").into());
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Call {
    #[allow(dead_code)]
    action: String,
    finding: Option<u64>,
    title: Option<String>,
    expected: Option<String>,
    called: Option<String>,
    happened: Option<String>,
    differs: Option<String>,
    query: Option<String>,
}

/// A text of the call, cut to its limit: whole, trimmed, never empty.
fn text(name: &str, value: &Option<String>, limit: usize) -> Result<String, Outcome> {
    let value = value.as_deref().map(str::trim).unwrap_or_default();
    if value.is_empty() {
        return Err(Outcome::Invalid(format!(
            "refused: report needs {name}, in a few words"
        )));
    }
    if value.chars().count() > limit {
        return Err(Outcome::Invalid(format!(
            "refused: {name} holds {} characters and at most {limit} are kept: say it shorter",
            value.chars().count()
        )));
    }
    Ok(value.to_owned())
}

pub fn tool(daemon: &Daemon, grant: &Grant, args: Value) -> Outcome {
    if !daemon.developer.on() {
        return Outcome::Refused(
            "refused: services has no action report: it is offered only while the developer \
             mode is on, and it is off"
                .into(),
        );
    }
    let call: Call = match arguments("services", args) {
        Ok(call) => call,
        Err(refused) => return refused,
    };
    match call.action.as_str() {
        "find_reports" => find(daemon, &call),
        _ => report(daemon, grant, &call),
    }
}

fn find(daemon: &Daemon, call: &Call) -> Outcome {
    let query = call.query.as_deref().map(str::trim).unwrap_or_default();
    if query.is_empty() {
        return Outcome::Invalid(
            "refused: find_reports needs a query, the words of your bug".into(),
        );
    }
    let all = match daemon.read(|store| store.findings()) {
        Ok(all) => all,
        Err(error) => return Outcome::Failed(error.to_string()),
    };
    let open: Vec<&Finding> = all
        .iter()
        .filter(|finding| finding.filed.is_none() && finding.matches(query))
        .collect();
    if open.is_empty() {
        return Outcome::Done(format!(
            "No open finding matches {query:?}: report it as a new finding."
        ));
    }
    let mut says = format!(
        "{} open finding{} match{}:\n",
        open.len().min(FOUND),
        if open.len() == 1 { "" } else { "s" },
        if open.len() == 1 { "es" } else { "" }
    );
    for finding in open.iter().take(FOUND) {
        says.push_str(&format!(
            "#{} {} (expected: {}; {} occurrence{}) — if it is your bug, report {{\"finding\": {}, \
             \"called\", \"happened\", \"differs\"}}\n",
            finding.id,
            finding.title,
            finding.expected,
            finding.count,
            if finding.count == 1 { "" } else { "s" },
            finding.id
        ));
    }
    Outcome::Done(says)
}

fn report(daemon: &Daemon, grant: &Grant, call: &Call) -> Outcome {
    let place = &grant.place;
    let mask = |text: String| masked(&text, place);
    let (called, happened) = match (
        text("called", &call.called, finding::TEXT),
        text("happened", &call.happened, finding::TEXT),
    ) {
        (Ok(called), Ok(happened)) => (mask(called), mask(happened)),
        (Err(refused), _) | (_, Err(refused)) => return refused,
    };
    let differs = match call.differs.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(_) => match text("differs", &call.differs, finding::TEXT) {
            Ok(differs) => Some(mask(differs)),
            Err(refused) => return refused,
        },
    };
    let occurrence = NewOccurrence {
        at: now_ms(),
        session: grant.agent.clone(),
        called,
        happened,
        differs,
    };
    let (id, new) = match call.finding {
        Some(id) => {
            if call.title.is_some() || call.expected.is_some() {
                return Outcome::Invalid(
                    "refused: a report that names a finding adds an occurrence to it: leave \
                     title and expected out"
                        .into(),
                );
            }
            let known = daemon.read(|store| store.findings());
            match known.map(|all| all.into_iter().find(|finding| finding.id == id)) {
                Err(error) => return Outcome::Failed(error.to_string()),
                Ok(None) => {
                    return Outcome::Refused(format!(
                        "refused: there is no finding #{id}: find_reports lists the open ones"
                    ));
                }
                Ok(Some(finding)) if finding.filed.is_some() => {
                    return Outcome::Refused(format!(
                        "refused: finding #{id} is filed as an issue already: report the bug as \
                         a new finding"
                    ));
                }
                Ok(Some(finding)) if finding.count >= finding::OCCURRENCES => {
                    return Outcome::Refused(format!(
                        "refused: finding #{id} holds {} occurrences already: it is known, stop \
                         adding to it",
                        finding.count
                    ));
                }
                Ok(Some(_)) => {}
            }
            match daemon.write(|store| Ok((store.add_occurrence(id, &occurrence)?, None::<Event>)))
            {
                Ok(Some(_)) => (id, false),
                Ok(None) => return Outcome::Refused(format!("refused: there is no finding #{id}")),
                Err(error) => return Outcome::Failed(error.to_string()),
            }
        }
        None => {
            let (title, expected) = match (
                text("title", &call.title, finding::TITLE),
                text("expected", &call.expected, finding::TEXT),
            ) {
                (Ok(title), Ok(expected)) => (mask(title), mask(expected)),
                (Err(refused), _) | (_, Err(refused)) => return refused,
            };
            match daemon.read(|store| store.findings()) {
                Ok(all) if all.len() >= finding::FINDINGS => {
                    return Outcome::Refused(format!(
                        "refused: {} findings are kept already: the maintainer exports them \
                         first",
                        all.len()
                    ));
                }
                Err(error) => return Outcome::Failed(error.to_string()),
                Ok(_) => {}
            }
            match daemon.write(|store| {
                Ok((
                    store.add_finding(&title, &expected, &occurrence)?,
                    None::<Event>,
                ))
            }) {
                Ok(id) => (id, true),
                Err(error) => return Outcome::Failed(error.to_string()),
            }
        }
    };
    let _ = daemon.emit("developer.reported", json!({"finding": id, "new": new}));
    Outcome::Done(match new {
        true => format!(
            "Filed as finding #{id}. If you meet the same bug again, add to it: report \
             {{\"finding\": {id}, \"called\", \"happened\", \"differs\"}}."
        ),
        false => format!("Added an occurrence to finding #{id}."),
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Filed {
    id: u64,
    issue: String,
}

/// Answers `developer.reports`, `developer.report_filed` and `developer.reports_export`.
pub fn call(daemon: &Daemon, method: &str, params: Value) -> Result<Value, Failure> {
    let unavailable = |error: flottille_core::Error| Failure::new("unavailable", error.to_string());
    match method {
        "developer.reports" => {
            let findings = daemon.read(|store| store.findings()).map_err(unavailable)?;
            Ok(json!({"findings": findings}))
        }
        "developer.report_filed" => {
            let Filed { id, issue } = serde_json::from_value(params)?;
            let issue = issue.trim();
            if issue.is_empty() {
                return Err(Failure::new(
                    "invalid-params",
                    "the issue's address is empty",
                ));
            }
            let found = daemon
                .write(|store| Ok((store.file_finding(id, issue)?, None::<Event>)))
                .map_err(unavailable)?;
            if !found {
                return Err(Failure::new(
                    "not-found",
                    format!("there is no finding #{id}"),
                ));
            }
            let _ = daemon.emit("developer.reported", json!({"finding": id, "new": false}));
            Ok(json!({"id": id, "filed": issue}))
        }
        "developer.reports_export" => {
            let findings = daemon.read(|store| store.findings()).map_err(unavailable)?;
            let file = daemon.paths.state.join("findings.json");
            let body = serde_json::to_string_pretty(&json!({"findings": findings}))
                .map_err(|e| Failure::new("unavailable", e.to_string()))?;
            std::fs::create_dir_all(&daemon.paths.state)
                .and_then(|()| crate::providers::write_in_place(&file, &format!("{body}\n")))
                .map_err(|e| Failure::new("unavailable", format!("{}: {e}", file.display())))?;
            Ok(json!({"file": file, "count": findings.len()}))
        }
        method => Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        )),
    }
}

/// The findings as `flottille developer reports` prints them: one block each, the open ones
/// first.
pub fn listing(said: &Value) -> String {
    let all = said["findings"].as_array().map_or(&[][..], Vec::as_slice);
    if all.is_empty() {
        return "No finding: agents file them while the developer mode is on.\n".into();
    }
    let mut ordered: Vec<&Value> = all.iter().collect();
    ordered.sort_by_key(|finding| !finding["filed"].is_null());
    let mut out = String::new();
    for finding in ordered {
        let count = finding["count"].as_u64().unwrap_or_default();
        out.push_str(&format!(
            "#{} {} — {count} occurrence{}, {}\n",
            finding["id"],
            finding["title"].as_str().unwrap_or_default(),
            if count == 1 { "" } else { "s" },
            match finding["filed"].as_str() {
                Some(issue) => format!("filed: {issue}"),
                None => "open".to_owned(),
            }
        ));
        out.push_str(&format!(
            "  expected: {}\n",
            finding["expected"].as_str().unwrap_or_default()
        ));
        for occurrence in finding["occurrences"].as_array().into_iter().flatten() {
            out.push_str(&format!(
                "  - {} called {}: {}\n",
                occurrence["session"].as_str().unwrap_or_default(),
                occurrence["called"].as_str().unwrap_or_default(),
                occurrence["happened"].as_str().unwrap_or_default()
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::{Definition, tool_of};

    #[test]
    fn the_findings_offer_adds_no_action_to_a_tool_that_has_none() {
        // Indexing into the schema would add `"action": {"enum": null}`, a schema Claude Code
        // drops its tool for (#1509).
        let schemas: [fn() -> Value; 2] = [
            || json!({"type": "object", "properties": {"name": {"type": "string"}}}),
            || json!({"type": "object"}),
        ];
        for schema in schemas {
            let mut tool = tool_of(
                &Definition {
                    name: "no_action",
                    title: "No action",
                    description: "A tool of a test.",
                    read_only: true,
                    idempotent: true,
                    destructive: false,
                    schema,
                },
                true,
            );
            offer(&mut tool);
            let listed = Value::Object((*tool.input_schema).clone());
            assert_eq!(listed.pointer("/properties/action"), None, "{listed}");
            // Nor a `"properties": null` to a schema that declares none.
            assert_ne!(listed.get("properties"), Some(&Value::Null), "{listed}");
        }
    }
}
