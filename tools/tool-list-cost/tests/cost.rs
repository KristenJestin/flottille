//! The scenarios of the counter, on invented tools: no Flottille list is needed to run them.

use serde_json::{Value, json};
use tool_list_cost::{
    Counter, LEVELS, Level, help_tool, list_cost, misuse_report, one_line, report, slimmed,
    tool_cost, usage_report,
};

fn notes() -> Value {
    json!({
        "name": "notes",
        "title": "Notes",
        "description": "Reads and writes the project's notes. Use it to keep what the next session needs. \
            A write is judged; a read never is. Example: {\"action\": \"add\", \"text\": \"the plan\"}.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["list", "show", "add", "remove"],
                    "description": "What to do with the notes: list them, show one, add one or remove one."},
                "id": {"type": "integer", "description": "The note's number, for show and remove."},
                "text": {"type": "string", "description": "The note's text, for add. One paragraph at most."}
            },
            "required": ["action"]
        },
        "annotations": {"readOnlyHint": false},
        "_meta": {"anthropic/alwaysLoad": true}
    })
}

fn ping() -> Value {
    json!({
        "name": "ping",
        "title": "Ping",
        "description": "Answers pong.",
        "inputSchema": {"type": "object", "properties": {}}
    })
}

fn dump() -> Value {
    json!({
        "kinds": [
            {"kind": "root", "project": "plain", "tools": [notes(), ping()]},
            {"kind": "worker", "project": "plain", "tools": [ping()]}
        ],
        "misuses": [
            {"tool": "notes", "mistake": "no arguments", "arguments": {},
             "expect": ["action"], "error": true,
             "text": "refused: action is required: list, show, add or remove"},
            {"tool": "notes", "mistake": "an action that does not exist",
             "arguments": {"action": "frobnicate"}, "expect": ["list", "show", "add", "remove"],
             "error": true, "text": "refused: no such action"}
        ]
    })
}

#[test]
fn a_known_text_is_counted_the_way_o200k_counts_it() {
    let counter = Counter::new();
    assert_eq!(counter.tokens(""), 0);
    assert_eq!(counter.tokens("hello world"), 2);
}

#[test]
fn a_tool_costs_what_the_model_is_given_of_it_and_nothing_the_client_keeps() {
    let counter = Counter::new();
    let cost = tool_cost(&counter, &notes());
    assert_eq!(cost.name, "notes");
    assert!(cost.description > 20 && cost.schema > 50, "{cost:?}");
    assert!(cost.total > cost.description + cost.schema, "{cost:?}");

    // The title, the annotations and `_meta` are the client's.
    let mut bare = notes();
    for key in ["title", "annotations", "_meta"] {
        bare.as_object_mut().unwrap().remove(key);
    }
    assert_eq!(tool_cost(&counter, &bare), cost);

    // The prefix the provider puts on the name is paid for.
    let mut renamed = notes();
    renamed["name"] = "a_much_longer_name_than_notes_for_a_tool".into();
    assert!(tool_cost(&counter, &renamed).total > cost.total);
}

#[test]
fn a_description_is_cut_to_its_first_sentence_at_a_word() {
    assert_eq!(
        one_line("Reads the notes. Use it to keep what matters."),
        "Reads the notes."
    );
    assert_eq!(one_line("No full stop here"), "No full stop here");
    let long = "word ".repeat(100);
    let line = one_line(&long);
    assert!(line.chars().count() <= 160, "{line}");
    assert!(line.ends_with("word"), "cut at a word: {line}");
    // A dot inside a name is not the end of a sentence.
    assert_eq!(one_line("Edits a.b and c. Then more."), "Edits a.b and c.");
}

#[test]
fn each_level_costs_less_than_the_one_before() {
    let counter = Counter::new();
    let costs: Vec<usize> = LEVELS
        .iter()
        .map(|level| tool_cost(&counter, &slimmed(&notes(), *level)).total)
        .collect();
    assert!(
        costs.windows(2).all(|pair| pair[0] > pair[1]),
        "{costs:?} for {LEVELS:?}"
    );
}

#[test]
fn a_slimmed_schema_keeps_the_names_the_types_and_the_enums() {
    let slim = slimmed(&notes(), Level::OneLine);
    let properties = &slim["inputSchema"]["properties"];
    assert_eq!(properties["action"]["type"], "string");
    assert_eq!(
        properties["action"]["enum"],
        notes()["inputSchema"]["properties"]["action"]["enum"]
    );
    assert_eq!(properties["id"]["type"], "integer");
    assert!(properties["id"].get("description").is_none());
    assert_eq!(slim["inputSchema"]["required"], json!(["action"]));

    let opaque = slimmed(&notes(), Level::Opaque);
    assert!(opaque["inputSchema"]["properties"].get("id").is_none());
    assert!(opaque["inputSchema"]["properties"].get("action").is_some());
    assert!(
        opaque["inputSchema"]["properties"]
            .get("arguments")
            .is_some()
    );
}

#[test]
fn a_lighter_list_saves_what_its_tools_lose_less_the_help_tool_it_needs() {
    let counter = Counter::new();
    let tools = vec![notes(), ping()];
    let whole = list_cost(&counter, &tools, &[], Level::Whole);
    assert_eq!(
        whole,
        tools
            .iter()
            .map(|t| tool_cost(&counter, t).total)
            .sum::<usize>(),
        "the whole list needs no help"
    );
    let kept = list_cost(&counter, &tools, &["notes", "ping"], Level::Opaque);
    assert_eq!(
        kept, whole,
        "tools kept whole make a list that needs no help"
    );

    let lighter = list_cost(&counter, &tools, &["ping"], Level::Opaque);
    let help = tool_cost(&counter, &help_tool(&["notes".to_owned()])).total;
    let notes_slim = tool_cost(&counter, &slimmed(&notes(), Level::Opaque)).total;
    let ping_whole = tool_cost(&counter, &ping()).total;
    assert_eq!(lighter, ping_whole + notes_slim + help);
}

#[test]
fn the_report_has_a_row_for_each_kind_and_a_row_for_each_tool_of_the_biggest() {
    let report = report(&Counter::new(), &dump(), &[]);
    assert!(report.contains("| root | plain | 2 |"), "{report}");
    assert!(report.contains("| worker | plain | 1 |"), "{report}");
    assert!(report.contains("| `notes` |"), "{report}");
    assert!(report.contains("| `ping` |"), "{report}");
    for level in LEVELS {
        assert!(
            report.contains(level.label()),
            "{}: {report}",
            level.label()
        );
    }
}

#[test]
fn the_misuse_report_says_which_answers_name_what_the_call_should_have_been() {
    let report = misuse_report(&Counter::new(), &dump());
    // The first answer names `action`; the second names none of the four actions.
    assert!(report.contains("| no arguments | 1 | 1 | 1 |"), "{report}");
    assert!(
        report.contains("| an action that does not exist | 1 | 1 | 0 |"),
        "{report}"
    );
    assert!(
        report.contains("`notes` (an action that does not exist)"),
        "{report}"
    );
    assert!(!report.contains("`notes` (no arguments)"), "{report}");
}

fn served(agent: &str, tool: &str, outcome: &str) -> String {
    json!({"seq": 1, "at": 1, "name": "agent.tool_served",
        "data": {"agent": agent, "tool": tool, "outcome": outcome, "arguments": {"secret": "never read"}}})
    .to_string()
}

fn created(agent: &str, parent: Value) -> String {
    json!({"seq": 1, "at": 1, "name": "agent.created",
        "data": {"agent": agent, "role": "lead", "parent": parent, "depth": 0}})
    .to_string()
}

#[test]
fn the_usage_report_counts_the_sessions_that_call_each_tool_by_kind_and_the_first_calls_refused() {
    let events = [
        created("a1", Value::Null),
        created("a2", json!("a1")),
        created("a3", json!("a1")),
        served("a1", "fs_read", "ok"),
        served("a1", "fs_read", "ok"),
        served("a2", "fs_read", "ok"),
        served("a1", "forge", "refused"),
        served("a1", "forge", "ok"),
        served("a3", "forge", "ok"),
        served("a2", "run", "refused"),
        // Not a tool call, and a line that is not JSON at all.
        created("a4", Value::Null),
        "not json".to_owned(),
    ]
    .join("\n");

    let report = usage_report(&events);

    // 4 agents: a1 root and a4 root, a2 and a3 children.
    assert!(
        report.contains("4 agents (2 roots, 2 children)"),
        "{report}"
    );
    // fs_read: 3 calls, called in 2 sessions (1 root, 1 child), no refusal.
    assert!(
        report.contains("| `fs_read` | 3 | 2 | 1 | 1 | 0 | 0 |"),
        "{report}"
    );
    // forge: 3 calls in 2 sessions, 1 refused, and the first call refused in 1 of 2 sessions.
    assert!(
        report.contains("| `forge` | 3 | 2 | 1 | 1 | 1 | 1 |"),
        "{report}"
    );
    // run: only a refused first call.
    assert!(
        report.contains("| `run` | 1 | 1 | 0 | 1 | 1 | 1 |"),
        "{report}"
    );
    assert!(!report.contains("never read"), "{report}");
}

#[test]
fn a_proposal_says_what_each_kind_saves_when_only_the_named_tools_are_slimmed() {
    let counter = Counter::new();
    let report = report(&counter, &dump(), &["notes"]);

    assert!(report.contains("## Slimming `notes`"), "{report}");
    // The root has `notes` and `ping`; the worker only `ping`, which nothing slims.
    let tools = vec![notes(), ping()];
    let whole = list_cost(&counter, &tools, &[], Level::Whole);
    let slim = list_cost(&counter, &tools, &["ping"], Level::NoSchemaTexts);
    let row = format!(
        "| root | plain | {whole} | {} |",
        whole as i64 - slim as i64
    );
    assert!(report.contains(&row), "{row}\n{report}");
    let ping = list_cost(&counter, &[ping()], &[], Level::Whole);
    assert!(
        report.contains(&format!("| worker | plain | {ping} | 0 |")),
        "{report}"
    );
}
