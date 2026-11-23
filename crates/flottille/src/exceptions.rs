//! The exceptions a blocked agent asks the user for (`docs/design.md` §7.3): a rule of the gate
//! that protects the project's own process, and that the user could break by hand on the forge,
//! lifted for the session that asked until it ends or the user withdraws it. The agent asks with
//! `exception` and its reason on the very call the rule holds; the ask is a request of the user's
//! in every mode, never the judge's, answered at once as any held call is. What bare mode, the
//! logins, the protected branches and the gate itself keep is never asked: such a call is refused
//! with no ask.
//!
//! An exception lifts only its rule's reasons: what else the gate says of the call stays, and the
//! session's mode decides it as before (a forced push of the agent's own branch is then judged as
//! any push is).

use serde::Deserialize;
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::gate::{self, Ask, Class, Verdict};
use crate::mcp::Grant;
use crate::place::Place;
use crate::projects::Failure;

/// A rule an exception lifts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rule {
    /// A forced push of the agent's own branch (`git push --force origin <its branch>`), never of
    /// a protected one: base, and refused by the project's never list.
    ForcePush,
    /// A merge of the workspace's pull request whose checks fail or whose branch protection
    /// blocks it: merged with the protection bypassed (`gh pr merge --admin`).
    MergeBlocked,
    /// Closing, reopening or relabelling an issue or a pull request: to judge.
    ForgeState,
}

impl Rule {
    pub const ALL: [Rule; 3] = [Rule::ForcePush, Rule::MergeBlocked, Rule::ForgeState];

    /// Its name, as the user and the protocol give it.
    pub fn name(self) -> &'static str {
        match self {
            Rule::ForcePush => "force-push",
            Rule::MergeBlocked => "merge-blocked",
            Rule::ForgeState => "forge-state",
        }
    }

    /// What it lets the session do, in words.
    pub fn said(self) -> &'static str {
        match self {
            Rule::ForcePush => "force a push of its own branch",
            Rule::MergeBlocked => {
                "merge its pull request while its checks fail or its branch protection blocks \
                 it, the protection bypassed"
            }
            Rule::ForgeState => "close, reopen or relabel an issue or a pull request",
        }
    }

    pub fn parse(name: &str) -> Option<Rule> {
        Rule::ALL.into_iter().find(|rule| rule.name() == name)
    }
}

/// How the reason of an ask starts, on the card and in `flottille approvals`.
pub const ASKS: &str = "it asks an exception for the rest of this session";

/// What can be asked for, said where an ask is refused.
const ASKABLE: &str = "an exception is asked only for a forced push of the agent's own branch, a \
    merge of its pull request past failing checks or a branch protection, or closing, reopening \
    or relabelling an issue or a pull request";

/// Branches no exception ever lets an agent force a push to, whatever the project.
const PROTECTED: [&str; 5] = ["main", "master", "dev", "develop", "trunk"];
const PROTECTED_LINES: [&str; 3] = ["stable/", "next/", "release/"];

/// The options a forced push of the agent's own branch may take, and those of them that force.
const PUSH_OPTIONS: [&str; 7] = [
    "--force",
    "-f",
    "--force-with-lease",
    "--force-if-includes",
    "--quiet",
    "-q",
    "--set-upstream",
];
const FORCING: [&str; 3] = ["--force", "-f", "--force-with-lease"];

/// The `exception` of a call taken out of its arguments, with the agent's reason: what the tool
/// itself is given is the call without it. A merge asked for this way is waived with that reason.
/// The reason is `Err` with the refusal when it is not one.
pub fn split(tool: &str, mut args: Value) -> (Value, Result<Option<String>, String>) {
    let Some(asked) = args.as_object_mut().and_then(|map| map.remove("exception")) else {
        return (args, Ok(None));
    };
    if asked.is_null() {
        return (args, Ok(None));
    }
    let Some(why) = asked.as_str().map(str::trim).filter(|why| !why.is_empty()) else {
        return (
            args,
            Err(
                "refused: exception is the reason the agent gives the user for an exception: \
                 say exactly what the rule refuses and why it is wrong in this case"
                    .into(),
            ),
        );
    };
    if tool == "forge" && args["action"] == "merge" && args.get("waive").is_none_or(Value::is_null)
    {
        args["waive"] = json!(why);
    }
    (args, Ok(Some(why.to_owned())))
}

/// The rule an exception would lift for this call, by its shape alone: `None` for a call no
/// exception can ever cover.
pub fn rule_of(place: &Place, tool: &str, args: &Value) -> Option<Rule> {
    match tool {
        "run" => args["line"]
            .as_str()
            .filter(|_| keys_within(args, &["line", "repository", "timeout", "why"]))
            .filter(|line| forces_own_branch(place, line))
            .map(|_| Rule::ForcePush),
        "forge" if args["action"] == "merge" => Some(Rule::MergeBlocked),
        "forge" | "tickets" if changes_state(tool, args) => Some(Rule::ForgeState),
        _ => None,
    }
}

/// Whether the call sets no argument but `keys`.
fn keys_within(args: &Value, keys: &[&str]) -> bool {
    args.as_object().is_some_and(|map| {
        map.iter()
            .all(|(key, value)| value.is_null() || keys.contains(&key.as_str()))
    })
}

/// Whether a forge or tickets call closes, reopens or relabels, and does nothing else.
fn changes_state(tool: &str, args: &Value) -> bool {
    let labels: &[&str] = match tool {
        "forge" => &["labels_add", "labels_remove"],
        _ => &["labels", "remove_labels"],
    };
    let target: &[&str] = match tool {
        "forge" => &["action", "repository", "workspace", "number"],
        _ => &["action", "ticket", "workspace"],
    };
    match args["action"].as_str() {
        Some("close" | "reopen") => {
            let mut keys = target.to_vec();
            keys.push("comment");
            if tool == "tickets" {
                keys.push("reason");
            }
            keys_within(args, &keys)
        }
        Some("edit") => {
            let keys: Vec<&str> = target.iter().chain(labels).copied().collect();
            keys_within(args, &keys) && labels.iter().any(|key| !args[*key].is_null())
        }
        _ => false,
    }
}

/// Whether a line is a forced push of the agent's own branch to a named remote, and nothing
/// else: `git push` with forcing options, a remote and one refspec whose destination is the
/// workspace's branch, never a protected one, never a deletion.
fn forces_own_branch(place: &Place, line: &str) -> bool {
    let own = place.branch.as_str();
    if place.workspace.is_none() || own.is_empty() || protected(own) {
        return false;
    }
    let plain = |c: char| c.is_ascii_alphanumeric() || "-_./:+=@^~ ".contains(c);
    if !line.chars().all(plain) {
        return false;
    }
    let words: Vec<&str> = line.split_whitespace().collect();
    let ["git", "push", rest @ ..] = words.as_slice() else {
        return false;
    };
    let (options, named): (Vec<&str>, Vec<&str>) =
        rest.iter().partition(|word| word.starts_with('-'));
    let known =
        |option: &&str| PUSH_OPTIONS.contains(option) || option.starts_with("--force-with-lease=");
    if !options.iter().all(known) {
        return false;
    }
    let [remote, target] = named.as_slice() else {
        return false;
    };
    let remote_plain = remote
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    let plus = target.starts_with('+');
    let pair = target.trim_start_matches('+');
    let (source, destination) = pair.split_once(':').unwrap_or((pair, pair));
    let destination = destination
        .strip_prefix("refs/heads/")
        .unwrap_or(destination);
    let forced = plus
        || options
            .iter()
            .any(|option| FORCING.contains(option) || option.starts_with("--force-with-lease="));
    remote_plain && forced && !source.is_empty() && destination == own
}

/// Whether no exception may ever let an agent rewrite `branch`.
fn protected(branch: &str) -> bool {
    PROTECTED.contains(&branch) || PROTECTED_LINES.iter().any(|line| branch.starts_with(line))
}

/// The verdict once `rule` is lifted: its reasons gone, what else the gate said kept, and the
/// class taken again from what is left.
fn lift(rule: Rule, ask: &Ask) -> Verdict {
    let (base, judge) = ask.reasons.split_at(ask.base.min(ask.reasons.len()));
    let mut base: Vec<String> = base.to_vec();
    let mut judge: Vec<String> = judge.to_vec();
    match rule {
        Rule::ForcePush => {
            let lifted = |reason: &String| {
                reason.contains(gate::FORCES)
                    || reason.starts_with(&format!("{} git push", gate::NEVER_HOLDS))
            };
            let before = base.len();
            base.retain(|reason| !lifted(reason));
            judge.retain(|reason| !lifted(reason));
            // Lifted, it is the push any push is.
            if base.len() != before && !judge.iter().any(|reason| reason == gate::PUSH) {
                judge.push(gate::PUSH.into());
            }
        }
        Rule::MergeBlocked => base.retain(|reason| !reason.starts_with(gate::WAIVED)),
        Rule::ForgeState => judge.clear(),
    }
    let count = base.len();
    let class = match (base.is_empty(), judge.is_empty()) {
        (true, true) => return Verdict::Allow,
        (true, false) => Class::Judge,
        (false, _) => Class::Base,
    };
    base.extend(judge);
    Verdict::Ask(Ask {
        reasons: base,
        base: count,
        class,
        kind: ask.kind,
    })
}

/// Whether the session holds an exception of `rule`.
fn holds(daemon: &Daemon, agent: &str, rule: Rule) -> bool {
    daemon
        .read(|store| store.has_exception(agent, rule.name()))
        .unwrap_or(false)
}

/// Whether the session `agent` holds the exception that lets it merge past failing checks or a
/// branch protection.
pub fn bypasses(daemon: &Daemon, agent: &str) -> bool {
    holds(daemon, agent, Rule::MergeBlocked)
}

/// The gate's verdict once the exceptions the session holds are lifted.
pub fn lifted(
    daemon: &Daemon,
    grant: &Grant,
    tool: &str,
    args: &Value,
    verdict: Verdict,
) -> Verdict {
    let Verdict::Ask(ask) = &verdict else {
        return verdict;
    };
    match rule_of(&grant.place, tool, args) {
        Some(rule) if holds(daemon, &grant.agent, rule) => lift(rule, ask),
        _ => verdict,
    }
}

/// The verdict on a call asked with `exception` (`why`), the session's exceptions already lifted
/// (`verdict`): the ask, a request of the user's in every mode, when an exception can lift what
/// holds the call; the call as it is when the session holds it already; else a refusal, with no
/// ask.
pub fn asked(
    daemon: &Daemon,
    grant: &Grant,
    tool: &str,
    args: &Value,
    verdict: Verdict,
    why: &str,
) -> Verdict {
    let rule = rule_of(&grant.place, tool, args);
    let held = rule.is_some_and(|rule| holds(daemon, &grant.agent, rule));
    match verdict {
        Verdict::Deny(refused) => Verdict::Deny(format!(
            "{refused} (no exception can be asked for it: {ASKABLE})"
        )),
        Verdict::Allow if held => Verdict::Allow,
        Verdict::Allow => Verdict::Deny(
            "refused: this call needs no exception: the gate lets it through as it is; call it \
             again without exception"
                .into(),
        ),
        Verdict::Ask(ask) if held => Verdict::Ask(ask),
        Verdict::Ask(ask) => {
            let Some(rule) = rule else {
                return Verdict::Deny(format!(
                    "refused: no exception can be asked for this call: {ASKABLE}; what holds it \
                     stays: {}",
                    ask.reasons.join("; ")
                ));
            };
            let lifted: Vec<String> = match lift(rule, &ask) {
                Verdict::Ask(lifted) if lifted.reasons == ask.reasons => {
                    return Verdict::Deny(format!(
                        "refused: no exception is needed for this call: the rule an exception \
                         lifts ({}) does not hold it; what holds it: {}",
                        rule.name(),
                        ask.reasons.join("; ")
                    ));
                }
                Verdict::Ask(lifted) => lifted.reasons,
                _ => Vec::new(),
            };
            // What the call becomes once allowed is on the card too: its check again, once the
            // exception is the session's, finds nothing the user did not read.
            let mut reasons = vec![format!("{ASKS}: {} ({}): {why}", rule.name(), rule.said())];
            reasons.extend(ask.reasons.iter().cloned());
            for reason in lifted {
                if !reasons.contains(&reason) {
                    reasons.push(reason);
                }
            }
            Verdict::Ask(Ask {
                base: reasons.len(),
                reasons,
                class: Class::Base,
                kind: ask.kind,
            })
        }
    }
}

/// Whether a held call asks an exception: only the user answers it, in every mode.
pub fn asks(ask: &Ask) -> bool {
    ask.reasons
        .first()
        .is_some_and(|reason| reason.starts_with(ASKS))
}

/// The exception an allowed request grants: its rule and the agent's reason, from its first
/// reason.
pub fn granted_by(reasons: &[String]) -> Option<(Rule, String)> {
    let rest = reasons.first()?.strip_prefix(ASKS)?.strip_prefix(": ")?;
    let (name, rest) = rest.split_once(" (")?;
    let rule = Rule::parse(name)?;
    let why = rest.strip_prefix(rule.said())?.strip_prefix("): ")?;
    Some((rule, why.to_owned()))
}

/// The user accepted the exception of request `number`: the session holds it from now on.
pub fn grant(daemon: &Daemon, agent: &str, reasons: &[String], number: u64) {
    let Some((rule, why)) = granted_by(reasons) else {
        return;
    };
    if let Err(error) =
        daemon.write(|store| store.grant_exception(agent, rule.name(), &why, number))
    {
        eprintln!("flottille daemon: request #{number}: the exception cannot be kept: {error}");
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct List {
    session: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Withdraw {
    session: String,
    rule: String,
}

fn listed(exception: &flottille_core::approval::Exception) -> Value {
    let said = Rule::parse(&exception.rule).map(Rule::said);
    json!({"session": exception.agent, "rule": exception.rule, "said": said,
        "why": exception.why, "request": exception.request, "granted_at": exception.granted_at})
}

/// `exception.list`, what the window shows where a session's rights are, and
/// `exception.withdraw`, the user's way to take one back.
pub fn call(daemon: &Daemon, method: &str, params: Value) -> Result<Value, Failure> {
    let params = if params.is_null() { json!({}) } else { params };
    match method {
        "exception.list" => {
            let List { session } = serde_json::from_value(params)?;
            if daemon.read(|store| store.agent(&session))?.is_none() {
                return Err(Failure::new(
                    "not-found",
                    format!("there is no agent {session}"),
                ));
            }
            let held = daemon.read(|store| store.exceptions_of(&session))?;
            Ok(json!({"exceptions": held.iter().map(listed).collect::<Vec<_>>()}))
        }
        "exception.withdraw" => {
            let Withdraw { session, rule } = serde_json::from_value(params)?;
            match daemon.write(|store| store.withdraw_exception(&session, &rule))? {
                Some(exception) => Ok(json!({"exception": listed(&exception)})),
                None => Err(Failure::new(
                    "not-found",
                    format!("agent {session} holds no exception {rule}"),
                )),
            }
        }
        method => Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn place(workspace: Option<&str>, branch: &str) -> Place {
        Place {
            project: "app".into(),
            workspace: workspace.map(str::to_owned),
            root: PathBuf::from("/work/app"),
            repositories: Vec::new(),
            branch: branch.into(),
            variables: BTreeMap::new(),
            home: None,
        }
    }

    fn run(line: &str) -> Value {
        json!({"line": line})
    }

    #[test]
    fn a_forced_push_of_the_agent_s_own_branch_is_the_only_push_an_exception_covers() {
        let own = place(Some("login"), "app/login");
        for line in [
            "git push --force origin app/login",
            "git push -f origin app/login",
            "git push --force-with-lease origin app/login",
            "git push --force-with-lease=app/login origin HEAD:app/login",
            "git push origin +HEAD:refs/heads/app/login",
            "git push --quiet --force origin main:app/login",
        ] {
            assert_eq!(
                rule_of(&own, "run", &run(line)),
                Some(Rule::ForcePush),
                "{line}"
            );
        }
        for line in [
            // Not forced, not a push, not a plain line.
            "git push origin app/login",
            "git fetch --force origin app/login",
            "bash -c 'git push --force origin app/login'",
            "git push --force origin app/login && true",
            "env A=1 git push --force origin app/login",
            "git -c core.sshCommand=x push --force origin app/login",
            // Another branch, a protected one, a deletion, every branch.
            "git push --force origin app/other",
            "git push --force origin HEAD:main",
            "git push --force origin app/login:stable/1.2",
            "git push --force origin :app/login",
            "git push --delete origin app/login",
            "git push --force --all origin",
            "git push --mirror origin",
            "git push --force origin",
            "git push --force origin app/login app/other",
            // A remote that is a path or an address.
            "git push --force /tmp/elsewhere app/login",
            "git push --force git@host:x/y.git app/login",
            "git push --force --receive-pack=x origin app/login",
        ] {
            assert_eq!(rule_of(&own, "run", &run(line)), None, "{line}");
        }
        // A line that waits for a ready server, or a task, is not one.
        assert_eq!(
            rule_of(
                &own,
                "run",
                &json!({"line": "git push --force origin app/login", "notify": "ready"})
            ),
            None
        );
        assert_eq!(
            rule_of(&own, "run", &json!({"task": "push", "why": "x"})),
            None
        );
        // The project folder, or a workspace on a protected branch, has no own branch to force.
        let line = run("git push --force origin main");
        assert_eq!(rule_of(&place(None, ""), "run", &line), None);
        assert_eq!(rule_of(&place(Some("main"), "main"), "run", &line), None);
        let line = run("git push --force origin next/2.0");
        assert_eq!(rule_of(&place(Some("x"), "next/2.0"), "run", &line), None);
    }

    #[test]
    fn closing_reopening_or_relabelling_and_nothing_else_is_a_change_of_state() {
        let own = place(Some("login"), "app/login");
        for (tool, args) in [
            (
                "forge",
                json!({"action": "close", "repository": ".", "comment": "Done."}),
            ),
            ("forge", json!({"action": "reopen", "number": 3})),
            (
                "forge",
                json!({"action": "edit", "number": 3, "labels_add": ["bug"]}),
            ),
            (
                "tickets",
                json!({"action": "close", "ticket": "#4", "reason": "completed"}),
            ),
            (
                "tickets",
                json!({"action": "edit", "ticket": "#4", "remove_labels": ["bug"]}),
            ),
        ] {
            assert_eq!(
                rule_of(&own, tool, &args),
                Some(Rule::ForgeState),
                "{tool} {args}"
            );
        }
        for (tool, args) in [
            ("forge", json!({"action": "close", "delete_branch": true})),
            (
                "forge",
                json!({"action": "edit", "title": "x", "labels_add": ["bug"]}),
            ),
            ("forge", json!({"action": "edit", "number": 3})),
            ("forge", json!({"action": "comment", "comment": "x"})),
            (
                "tickets",
                json!({"action": "edit", "ticket": "#4", "assignees": ["a"]}),
            ),
            ("tickets", json!({"action": "create", "title": "x"})),
        ] {
            assert_eq!(rule_of(&own, tool, &args), None, "{tool} {args}");
        }
        assert_eq!(
            rule_of(&own, "forge", &json!({"action": "merge"})),
            Some(Rule::MergeBlocked)
        );
    }

    fn ask(base: &[&str], judge: &[&str]) -> Ask {
        let class = match base.is_empty() {
            true => Class::Judge,
            false => Class::Base,
        };
        Ask {
            reasons: base.iter().chain(judge).map(|r| (*r).to_owned()).collect(),
            base: base.len(),
            class,
            kind: flottille_core::approval::Kind::Call,
        }
    }

    #[test]
    fn an_exception_lifts_its_rule_s_reasons_and_keeps_the_others() {
        let forced = format!("git push --force {}", gate::FORCES);
        let never = format!("{} git push", gate::NEVER_HOLDS);
        assert_eq!(
            lift(Rule::ForcePush, &ask(&[&forced, &never], &[])),
            Verdict::Ask(ask(&[], &[gate::PUSH]))
        );
        assert_eq!(
            lift(Rule::ForcePush, &ask(&[&forced, "it reads .env"], &[])),
            Verdict::Ask(ask(&["it reads .env"], &[gate::PUSH]))
        );
        let waived = format!("{}: the user said so", gate::WAIVED);
        assert_eq!(
            lift(Rule::MergeBlocked, &ask(&[&waived], &["it merges"])),
            Verdict::Ask(ask(&[], &["it merges"]))
        );
        assert_eq!(
            lift(Rule::ForgeState, &ask(&[], &["it closes #3"])),
            Verdict::Allow
        );
        assert_eq!(
            lift(Rule::ForgeState, &ask(&["a secret"], &["it closes #3"])),
            Verdict::Ask(ask(&["a secret"], &[]))
        );
    }

    #[test]
    fn the_exception_an_allowed_request_grants_is_read_from_its_first_reason() {
        let reason = format!(
            "{ASKS}: {} ({}): a check refuses: an old commit",
            Rule::ForcePush.name(),
            Rule::ForcePush.said()
        );
        assert_eq!(
            granted_by(&[reason, "other".into()]),
            Some((Rule::ForcePush, "a check refuses: an old commit".into()))
        );
        assert_eq!(granted_by(&["it pushes".into()]), None);
        assert_eq!(granted_by(&[]), None);
    }

    #[test]
    fn the_exception_is_taken_out_of_the_call_and_a_merge_is_waived_with_it() {
        let (args, why) = split("run", json!({"line": "x", "exception": " why "}));
        assert_eq!(args, json!({"line": "x"}));
        assert_eq!(why, Ok(Some("why".into())));
        let (args, _) = split("forge", json!({"action": "merge", "exception": "why"}));
        assert_eq!(args, json!({"action": "merge", "waive": "why"}));
        let (args, why) = split("run", json!({"line": "x"}));
        assert_eq!((args, why), (json!({"line": "x"}), Ok(None)));
        assert!(
            split("run", json!({"line": "x", "exception": ""}))
                .1
                .is_err()
        );
        assert!(
            split("run", json!({"line": "x", "exception": 3}))
                .1
                .is_err()
        );
    }
}
