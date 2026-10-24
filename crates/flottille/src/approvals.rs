//! The gate's requests (`docs/design.md` §7, `docs/reference/hemera.md` §4 "Requests"): a held
//! call is written as a request and answered at once with the waiting text; the user denies it,
//! allows it once or for the root's tree; an allowed request is checked again, performed, and its
//! answer stored as the `approval` delivery of the agent that asked, which the deliveries (#7)
//! send.
//!
//! Until the tree, every agent is a root: its requests are the user's. A child's request is held
//! by its parent, which denies it or forwards it up; the user answers only what reaches them.

use std::sync::Arc;

use flottille_core::approval::{self, Decision, Request, RequestState};
use flottille_core::store::Store;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::gate::{self, Verdict};
use crate::mcp::{self, Grant, Outcome};
use crate::projects::Failure;

type Answer = Result<Value, Failure>;

/// The text an agent is answered with, at once, when its call is held for the user.
pub fn waiting(number: u64) -> String {
    format!(
        "Waiting for the user's approval, request #{number}. This action has not happened; do \
         not assume it did. Continue with work that does not depend on it, or end your turn \
         saying you wait. The answer will be handed to you when the user gives it."
    )
}

/// Who holds an agent's requests: the user for a root, which every agent is until the tree.
fn holder(_agent: &str) -> String {
    approval::USER.to_owned()
}

/// The root whose tree a grant is given to: the agent itself until the tree.
fn tree_of(agent: &str) -> String {
    agent.to_owned()
}

/// Whether the user allowed this exact call for the agent's tree.
pub fn granted(daemon: &Daemon, agent: &str, identity: &str) -> bool {
    daemon
        .read(|store| store.tree_granted(&tree_of(agent), identity))
        .unwrap_or(false)
}

/// Writes the request for a held call, or finds the one its call key already made, and answers
/// the agent at once.
pub fn ask(
    daemon: &Daemon,
    grant: &Grant,
    tool: &str,
    args: &Value,
    key: Option<&str>,
    ask: gate::Ask,
    identity: String,
) -> Outcome {
    let asked = Request {
        number: 0,
        agent: grant.agent.clone(),
        call_key: key.map(str::to_owned),
        tool: tool.to_owned(),
        arguments: args.clone(),
        shown: mcp::record_of(args, &grant.place),
        reasons: ask
            .reasons
            .iter()
            .map(|reason| mcp::masked(reason, &grant.place))
            .collect(),
        sensitive: ask.sensitive,
        identity,
        held_by: holder(&grant.agent),
        forwards: Vec::new(),
        state: RequestState::Pending,
        decision: None,
        created_at: 0,
        decided_at: None,
    };
    match daemon.write(|store| store.ask(&asked)) {
        Ok(request) => Outcome::Held(waiting(request.number)),
        Err(error) => {
            eprintln!(
                "flottille daemon: agent {}: a request cannot be written: {error}",
                grant.agent
            );
            Outcome::Refused(
                "refused: the request for the user could not be made, so nothing was done; say \
                 so and go on with what does not depend on it"
                    .into(),
            )
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Approve {
    number: u64,
    #[serde(default)]
    for_tree: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Deny {
    number: u64,
}

/// The `approval.*` methods (`docs/protocol.md`).
pub async fn call(daemon: Arc<Daemon>, method: &str, params: Value) -> Answer {
    let params = if params.is_null() { json!({}) } else { params };
    match method {
        "approval.list" => {
            let Empty {} = serde_json::from_value(params)?;
            let requests = daemon.read(|store| store.pending_requests(approval::USER))?;
            Ok(json!({"requests": requests.iter().map(Request::to_json).collect::<Vec<_>>()}))
        }
        "approval.approve" => {
            let asked: Approve = serde_json::from_value(params)?;
            let decision = if asked.for_tree {
                Decision::Tree
            } else {
                Decision::Once
            };
            decide(&daemon, asked.number, decision)
        }
        "approval.deny" => {
            let asked: Deny = serde_json::from_value(params)?;
            decide(&daemon, asked.number, Decision::Deny)
        }
        method => Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        )),
    }
}

/// The user's decision: refused for a request they do not hold, one already decided, and a
/// grant for the tree of a sensitive place. A denial is answered to the agent now; an allowed
/// request is performed in the background, and the user is answered once it is decided.
fn decide(daemon: &Arc<Daemon>, number: u64, decision: Decision) -> Answer {
    let request = daemon
        .read(|store| store.request(number))?
        .ok_or_else(|| Failure::new("not-found", format!("there is no request #{number}")))?;
    if request.held_by != approval::USER {
        return Err(Failure::refused(format!(
            "request #{number} is held by agent {}: it reaches you only if that agent forwards it",
            request.held_by
        )));
    }
    if request.state != RequestState::Pending {
        return Err(already(&request));
    }
    if decision == Decision::Tree && request.sensitive {
        return Err(Failure::refused(format!(
            "request #{number} reaches a sensitive place: it can be allowed once, never for the \
             tree"
        )));
    }
    let tree = tree_of(&request.agent);
    let decided = daemon.write(|store| {
        Ok(match store.decide(number, decision, &tree)? {
            Some((request, event)) => (Some(request), Some(event)),
            None => (None, None),
        })
    })?;
    let Some(request) = decided else {
        // Decided meanwhile.
        let request = daemon
            .read(|store| store.request(number))?
            .unwrap_or(request);
        return Err(already(&request));
    };
    if decision == Decision::Deny {
        let body = format!(
            "Request #{number} ({}) was refused by the user. Nothing was done.",
            request.tool
        );
        store_answer(daemon, &request, "refused", &body);
    } else {
        tokio::spawn(settle(daemon.clone(), request.clone()));
    }
    Ok(json!({"request": request.to_json()}))
}

fn already(request: &Request) -> Failure {
    Failure::refused(format!(
        "request #{} is already decided: it was {}",
        request.number,
        request.state.as_str()
    ))
}

/// Checks an allowed request again, performs it, and stores its answer.
async fn settle(daemon: Arc<Daemon>, request: Request) {
    let (outcome, body) = performed(&daemon, &request).await;
    store_answer(&daemon, &request, outcome, &body);
}

/// What became of an allowed request: its outcome's word and the answer's text, masked.
async fn performed(daemon: &Arc<Daemon>, request: &Request) -> (&'static str, String) {
    let (number, tool) = (request.number, request.tool.as_str());
    let not_executed = |why: String| {
        (
            "not_executed",
            format!("Request #{number} ({tool}) was not executed: {why}"),
        )
    };
    let agent = match daemon.read(|store| store.agent(&request.agent)) {
        Ok(Some(agent)) => agent,
        Ok(None) => return not_executed("the agent that asked no longer exists.".into()),
        Err(error) => return not_executed(format!("{error}.")),
    };
    if agent.state.is_final() {
        return not_executed("the agent that asked has ended.".into());
    }
    let grant = match mcp::grant_of(daemon, &agent) {
        Ok(grant) => Arc::new(grant),
        Err(why) => return not_executed(format!("{why}.")),
    };
    if !grant.tools.contains(&tool) {
        return not_executed(format!("the agent has no tool {tool}."));
    }
    // The situation may have changed since the request was made.
    let changed = match mcp::judge(daemon, &grant, tool, &request.arguments).0 {
        Verdict::Deny(why) => Some(why),
        Verdict::Ask(ask) => {
            let new: Vec<String> = ask
                .reasons
                .iter()
                .map(|reason| mcp::masked(reason, &grant.place))
                .filter(|reason| !request.reasons.contains(reason))
                .collect();
            (!new.is_empty()).then(|| {
                format!(
                    "the situation changed since you allowed it: {}",
                    new.join("; ")
                )
            })
        }
        Verdict::Allow => None,
    };
    if let Some(why) = changed {
        return not_executed(mcp::masked(&why, &grant.place));
    }
    let outcome = mcp::performed(daemon, &grant, tool, request.arguments.clone(), number).await;
    let (word, body) = match outcome {
        Outcome::Done(text) => (
            "done",
            format!(
                "Request #{number} ({tool}) was approved by the user, and Flottille has now done \
                 it. Its result:\n\n{text}"
            ),
        ),
        Outcome::Failed(why) => (
            "failed",
            format!(
                "Request #{number} ({tool}) was approved by the user, but nothing was done: {why}"
            ),
        ),
        Outcome::Refused(why) | Outcome::Held(why) => not_executed(why),
    };
    (word, mcp::masked(&body, &grant.place))
}

/// Stores the answer as the agent's `approval` delivery, once.
fn store_answer(daemon: &Daemon, request: &Request, outcome: &str, body: &str) {
    if let Err(error) = daemon.write(|store| Ok(((), store.settle(request, outcome, body)?))) {
        eprintln!(
            "flottille daemon: request #{}: its answer cannot be stored: {error}",
            request.number
        );
    }
}

/// At start: a request allowed whose answer was never stored was being performed by a daemon
/// that stopped; the agent is told so, since nobody knows whether it took effect.
pub fn settle_at_start(daemon: &Daemon) {
    let unsettled = match daemon.read(Store::unsettled_requests) {
        Ok(unsettled) => unsettled,
        Err(error) => {
            eprintln!("flottille daemon: reading the requests: {error}");
            return;
        }
    };
    for request in unsettled {
        let body = format!(
            "Request #{} ({}) was approved by the user, but Flottille stopped while it was doing \
             it, and cannot tell whether it took effect: check before asking again.",
            request.number, request.tool
        );
        store_answer(daemon, &request, "interrupted", &body);
    }
}

/// The agents waiting for the user: those with a request the user holds.
pub fn waiting_for_user(daemon: &Daemon) -> flottille_core::Result<usize> {
    let mut agents: Vec<String> = daemon
        .read(|store| store.pending_requests(approval::USER))?
        .into_iter()
        .map(|request| request.agent)
        .collect();
    agents.sort();
    agents.dedup();
    Ok(agents.len())
}
