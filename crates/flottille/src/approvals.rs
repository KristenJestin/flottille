//! The gate's requests (`docs/design.md` §7, `docs/reference/hemera.md` §4 "Requests"): a held
//! call is written as a request and answered at once with the waiting text; the user allows it or
//! denies it, once (there is no grant); an allowed request is checked again, performed, and its
//! answer stored as the `approval` delivery of the agent that asked, which the deliveries (#7)
//! send.
//!
//! A root's requests are the user's. A child's request is held by its parent, which is told
//! (`child-asks`) and denies it or forwards it up (`approval_answer`, `tree`); the user answers
//! only what reaches them.

use std::sync::Arc;

use flottille_core::approval::{self, Decision, Judgement, Kind, Request, RequestState};
use flottille_core::store::Store;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::actions;
use crate::daemon::Daemon;
use crate::deliveries;
use crate::gate::{self, Verdict};
use crate::mcp::{self, Grant, Outcome};
use crate::projects::Failure;
use crate::tree;

type Answer = Result<Value, Failure>;

/// The text an agent is answered with, at once, when its call is held for the user.
pub fn waiting(number: u64) -> String {
    format!(
        "Waiting for the user's approval, request #{number}. This action has not happened; do \
         not assume it did. Continue with work that does not depend on it, or end your turn \
         saying you wait. The answer will be handed to you when the user gives it."
    )
}

/// The text a child is answered with, at once, when its call is held for its parent.
fn waiting_on_parent(number: u64) -> String {
    format!(
        "Waiting for approval, request #{number}: your parent decides whether it goes on to the \
         user. This action has not happened; do not assume it did. Continue with work that does \
         not depend on it, or end your turn saying you wait. The answer will be handed to you."
    )
}

/// Who holds an agent's requests: its parent, or the user for a root.
fn holder(daemon: &Daemon, agent: &str) -> String {
    daemon
        .read(|store| store.agent(agent))
        .ok()
        .flatten()
        .and_then(|agent| agent.parent)
        .unwrap_or_else(|| approval::USER.to_owned())
}

/// Writes the request for a held call, or finds the one its call key already made, and answers
/// the agent at once. `says` is what it changes, in words, for the user; `judgement` what the
/// judge said of it, in Auto.
#[allow(clippy::too_many_arguments)]
pub fn ask(
    daemon: &Arc<Daemon>,
    grant: &Grant,
    tool: &str,
    args: &Value,
    key: Option<&str>,
    ask: gate::Ask,
    says: Vec<String>,
    judgement: Option<Judgement>,
) -> (Outcome, Option<u64>) {
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
        base: ask.class == gate::Class::Base,
        kind: ask.kind,
        says: says
            .iter()
            .map(|said| mcp::masked(said, &grant.place))
            .collect(),
        judgement,
        held_by: holder(daemon, &grant.agent),
        forwards: Vec::new(),
        state: RequestState::Pending,
        decision: None,
        created_at: 0,
        decided_at: None,
    };
    match daemon.write(|store| store.ask(&asked)) {
        Ok(request) if request.held_by == approval::USER => {
            (Outcome::Held(waiting(request.number)), Some(request.number))
        }
        Ok(request) => {
            tree::asks_parent(daemon, &request);
            (
                Outcome::Held(waiting_on_parent(request.number)),
                Some(request.number),
            )
        }
        Err(error) => {
            eprintln!(
                "flottille daemon: agent {}: a request cannot be written: {error}",
                grant.agent
            );
            (
                Outcome::Refused(
                    "refused: the request for the user could not be made, so nothing was done; \
                     say so and go on with what does not depend on it"
                        .into(),
                ),
                None,
            )
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct List {
    /// A root, or any agent of its tree: every request of the tree that reached the user.
    #[serde(default)]
    tree: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Approve {
    number: u64,
    /// For a workspace's removal: removed even with what would be lost.
    #[serde(default)]
    discard: bool,
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
            let List { tree } = serde_json::from_value(params)?;
            let requests = match tree {
                None => daemon.read(|store| store.pending_requests(approval::USER))?,
                Some(agent) => {
                    let found = daemon.read(|store| store.agent(&agent))?.ok_or_else(|| {
                        Failure::new("not-found", format!("there is no agent {agent}"))
                    })?;
                    let ids = tree::ids(&daemon, &tree::root_of(&daemon, &found.id))?;
                    daemon.read(|store| store.requests_reaching(approval::USER, &ids))?
                }
            };
            Ok(json!({"requests": requests.iter().map(Request::to_json).collect::<Vec<_>>()}))
        }
        "approval.approve" => {
            let asked: Approve = serde_json::from_value(params)?;
            decide(&daemon, asked.number, Decision::Once, asked.discard)
        }
        "approval.deny" => {
            let asked: Deny = serde_json::from_value(params)?;
            decide(&daemon, asked.number, Decision::Deny, false)
        }
        method => Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        )),
    }
}

/// The user's decision: refused for a request they do not hold, and one already decided. A denial is answered to the agent now; an allowed
/// request is performed in the background, and the user is answered once it is decided. `discard`
/// answers only a workspace's removal.
fn decide(daemon: &Arc<Daemon>, number: u64, decision: Decision, discard: bool) -> Answer {
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
    if discard && !removes_a_workspace(&request) {
        return Err(Failure::refused(format!(
            "discard answers only a request that removes a workspace, and request #{number} \
             ({}) removes none: approve it without discard",
            request.tool
        )));
    }
    let decided = daemon.write(|store| {
        Ok(match store.decide(number, decision, approval::USER)? {
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
        tokio::spawn(settle(daemon.clone(), request.clone(), discard));
    }
    Ok(json!({"request": request.to_json()}))
}

fn removes_a_workspace(request: &Request) -> bool {
    request.tool == "workspace" && request.arguments["action"] == "remove"
}

fn already(request: &Request) -> Failure {
    Failure::refused(format!(
        "request #{} is already decided: it was {}",
        request.number,
        request.state.as_str()
    ))
}

/// Checks an allowed request again, performs it, and stores its answer.
async fn settle(daemon: Arc<Daemon>, request: Request, discard: bool) {
    let (outcome, body) = performed(&daemon, &request, discard).await;
    store_answer(&daemon, &request, outcome, &body);
}

/// What became of an allowed request: its outcome's word and the answer's text, masked.
async fn performed(
    daemon: &Arc<Daemon>,
    request: &Request,
    discard: bool,
) -> (&'static str, String) {
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
    let changed = match mcp::classify(daemon, &grant, tool, &request.arguments) {
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
    // What the user allowed is what they read: an action whose words changed meanwhile (its
    // command was edited, its skill removed) is not done.
    if request.kind == Kind::Action {
        match actions::prepared(daemon, &grant, tool, &request.arguments).await {
            Err(Outcome::Refused(why) | Outcome::Failed(why) | Outcome::Held(why)) => {
                return not_executed(mcp::masked(&why, &grant.place));
            }
            Err(Outcome::Done(_)) => {}
            Ok(says) => {
                let says: Vec<String> = says
                    .iter()
                    .map(|said| mcp::masked(said, &grant.place))
                    .collect();
                if says != request.says {
                    return not_executed(format!(
                        "what it does changed since the user allowed it; it would now: {}",
                        says.join("; ")
                    ));
                }
            }
        }
    }
    // The losses the user discards are the ones they read, up to the removal itself.
    let discarding = match discard {
        true => match actions::approved_losses(daemon, &grant, &request.arguments, &request.says)
            .await
        {
            Ok(losses) => Some(losses),
            Err(Outcome::Refused(why) | Outcome::Failed(why) | Outcome::Held(why)) => {
                return not_executed(mcp::masked(&why, &grant.place));
            }
            Err(Outcome::Done(_)) => None,
        },
        false => None,
    };
    let class = if request.base {
        gate::Class::Base
    } else {
        gate::Class::Judge
    };
    let outcome = mcp::performed(
        daemon,
        &grant,
        tool,
        request.arguments.clone(),
        number,
        class,
        discarding,
    )
    .await;
    let approved = "was approved by the user";
    let (word, body) = match outcome {
        Outcome::Done(text) => (
            "done",
            format!(
                "Request #{number} ({tool}) {approved}, and Flottille has now done it. Its \
                 result:\n\n{text}"
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

/// Stores the answer as the agent's `approval` delivery, once, and rings the agent.
pub fn store_answer(daemon: &Arc<Daemon>, request: &Request, outcome: &str, body: &str) {
    if let Err(error) = daemon.write(|store| Ok(((), store.settle(request, outcome, body)?))) {
        eprintln!(
            "flottille daemon: request #{}: its answer cannot be stored: {error}",
            request.number
        );
        return;
    }
    deliveries::ring(daemon, &request.agent);
}

/// At start: a request allowed whose answer was never stored was being performed by a daemon
/// that stopped; the agent is told so, since nobody knows whether it took effect.
pub fn settle_at_start(daemon: &Arc<Daemon>) {
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

/// The agents waiting for the user: the roots of the trees with a request the user holds, and the
/// roots whose question waits for the user's answer (a child waits on its parent, never on the
/// user).
pub fn waiting_for_user(daemon: &Daemon) -> flottille_core::Result<usize> {
    let mut agents: Vec<String> = daemon
        .read(|store| store.pending_requests(approval::USER))?
        .into_iter()
        .map(|request| tree::root_of(daemon, &request.agent))
        .collect();
    agents.extend(
        daemon
            .read(|store| store.agents())?
            .into_iter()
            .filter(tree::asks_the_user)
            .map(|agent| agent.id),
    );
    agents.sort();
    agents.dedup();
    Ok(agents.len())
}
