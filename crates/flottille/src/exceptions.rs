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

use flottille_core::approval::{Exception, Scope};

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
const ASKABLE: &str = "an exception is asked only for a forced push of the agent's own branch, \
    its destination named (git push --force origin HEAD:refs/heads/<its branch>), a merge of its \
    pull request past failing checks or a branch protection, or closing, reopening or relabelling \
    an issue or a pull request, with no comment";

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

/// What an exception's ask says first of the agent's own words, on the card below what Flottille
/// read of the call.
pub const REASON: &str = "the agent's reason:";

/// The `exception` of a call taken out of its arguments, with the agent's reason: what the tool
/// itself is given is the call without it. The reason is `Err` with the refusal when it is not
/// one: one line, no longer than the user is shown whole.
pub fn split(mut args: Value) -> (Value, Result<Option<String>, String>) {
    let Some(asked) = args.as_object_mut().and_then(|map| map.remove("exception")) else {
        return (args, Ok(None));
    };
    if asked.is_null() {
        return (args, Ok(None));
    }
    let refused = |what: &str| {
        Err(format!(
            "refused: exception is the reason the agent gives the user for an exception, {what}: \
             say in one line what the rule refuses and why it is wrong in this case"
        ))
    };
    let Some(why) = asked.as_str().map(str::trim).filter(|why| !why.is_empty()) else {
        return (args, refused("a text"));
    };
    if why.contains(['\n', '\r']) {
        return (args, refused("on one line"));
    }
    if let Some(Verdict::Deny(long)) = gate::too_long("exception", &json!(why)) {
        return (args, Err(long));
    }
    (args, Ok(Some(why.to_owned())))
}

/// What a call asked with `exception` is refused for when another machine answers it.
pub const ELSEWHERE: &str = "refused: an exception is asked of the user of the machine the call \
    runs on, and this call is answered by another machine: exceptions are each machine's own for \
    now; call it without exception, and the other machine's gate holds it as any call";

/// The branches of the place no exception lets an agent rewrite, beside `PROTECTED`: the bases
/// of the project's repositories, and the branches the workspace resumed, which were the user's
/// before it.
pub fn guarded(daemon: &Daemon, place: &Place) -> Vec<String> {
    let mut guarded: Vec<String> = crate::projects::load_project(daemon, &place.project)
        .map(|project| {
            project
                .repositories
                .iter()
                .flat_map(|repository| repository.base.clone())
                .collect()
        })
        .unwrap_or_default();
    let workspace = place.workspace.as_deref().and_then(|name| {
        daemon
            .read(|store| store.workspace(&place.project, name))
            .ok()
            .flatten()
    });
    for repository in workspace.iter().flat_map(|w| &w.repositories) {
        guarded.push(repository.base.branch.clone());
        if repository.resumed {
            // Recorded before its name was kept: the workspace's branch is the one it resumed.
            let resumed = repository.resumed_branch.clone();
            guarded.push(resumed.unwrap_or_else(|| place.branch.clone()));
        }
    }
    guarded
}

/// The rule an exception would lift for this call, by its shape alone: `None` for a call no
/// exception can ever cover. `guarded` are the place's own branches no exception rewrites
/// (`guarded`).
pub fn rule_of(place: &Place, guarded: &[String], tool: &str, args: &Value) -> Option<Rule> {
    match tool {
        "run" => forced_branch(place, guarded, args).map(|_| Rule::ForcePush),
        "forge" if args["action"] == "merge" => Some(Rule::MergeBlocked),
        "forge" | "tickets" if changes_state(tool, args) => Some(Rule::ForgeState),
        _ => None,
    }
}

/// What a forced push of the agent's own branch covers: the repository it is pushed from and
/// the branch; `None` for any other line.
pub fn push_scope(place: &Place, guarded: &[String], args: &Value) -> Option<Scope> {
    let branch = forced_branch(place, guarded, args)?;
    let repository = place.base(args["repository"].as_str()).ok()?.0.to_owned();
    Some(Scope {
        repository,
        branch,
        ..Scope::default()
    })
}

/// The branch a `run` line forces a push of, when it is a forced push of the agent's own branch
/// and nothing else.
fn forced_branch(place: &Place, guarded: &[String], args: &Value) -> Option<String> {
    let line = args["line"]
        .as_str()
        .filter(|_| keys_within(args, &["line", "repository", "timeout", "why"]))?;
    forces_own_branch(place, line)
        .filter(|branch| !guarded.iter().any(|guarded| guarded == branch))
        .map(str::to_owned)
}

/// Whether the call sets no argument but `keys`.
fn keys_within(args: &Value, keys: &[&str]) -> bool {
    args.as_object().is_some_and(|map| {
        map.iter()
            .all(|(key, value)| value.is_null() || keys.contains(&key.as_str()))
    })
}

/// Whether a forge or tickets call closes, reopens or relabels, and does nothing else: a comment's
/// text is judged, whatever exception the session holds.
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

/// The branch a line forces a push of when it is a forced push of the agent's own branch to a
/// named remote, and nothing else: `git push` with forcing options, a remote and one refspec
/// whose destination is the workspace's branch named in full (`:refs/heads/<branch>`), so that no
/// `remote.<name>.push` setting sends it elsewhere; never a protected one, never a deletion.
fn forces_own_branch<'a>(place: &'a Place, line: &str) -> Option<&'a str> {
    let own = place.branch.as_str();
    if place.workspace.is_none() || own.is_empty() || protected(own) {
        return None;
    }
    let plain = |c: char| c.is_ascii_alphanumeric() || "-_./:+=@^~ ".contains(c);
    if !line.chars().all(plain) {
        return None;
    }
    let words: Vec<&str> = line.split_whitespace().collect();
    let ["git", "push", rest @ ..] = words.as_slice() else {
        return None;
    };
    let (options, named): (Vec<&str>, Vec<&str>) =
        rest.iter().partition(|word| word.starts_with('-'));
    let known =
        |option: &&str| PUSH_OPTIONS.contains(option) || option.starts_with("--force-with-lease=");
    if !options.iter().all(known) {
        return None;
    }
    let [remote, target] = named.as_slice() else {
        return None;
    };
    let remote_plain = remote
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    let plus = target.starts_with('+');
    let pair = target.trim_start_matches('+');
    let (source, destination) = pair.split_once(':')?;
    let destination = destination.strip_prefix("refs/heads/")?;
    let forced = plus
        || options
            .iter()
            .any(|option| FORCING.contains(option) || option.starts_with("--force-with-lease="));
    (remote_plain && forced && !source.is_empty() && destination == own).then_some(own)
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
            // The line's own push alone: a service started first says its line's reasons after
            // its name, and stays.
            let lifted = |reason: &String| {
                (reason.starts_with("git push ") && reason.contains(gate::FORCES))
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
        // What blocks a merge is the forge's to say: the merge itself checks the pull request
        // the exception is for (`forge::Excepted`).
        Rule::MergeBlocked => {}
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

/// The exception of `rule` the session holds, if it holds one.
fn held(daemon: &Daemon, agent: &str, rule: Rule) -> Option<Exception> {
    daemon
        .read(|store| store.exception(agent, rule.name()))
        .ok()
        .flatten()
}

/// What the exception the session `agent` holds to merge past what blocks a pull request covers:
/// the merge checks it is that pull request (`forge::Excepted`).
pub fn merge_held(daemon: &Daemon, agent: &str) -> Option<Scope> {
    held(daemon, agent, Rule::MergeBlocked).map(|held| held.scope)
}

/// Whether the session holds the exception of `rule` that covers this call: a forced push of the
/// branch of the repository it was asked for, any change of state.
fn covers(daemon: &Daemon, grant: &Grant, rule: Rule, args: &Value) -> bool {
    let Some(held) = held(daemon, &grant.agent, rule) else {
        return false;
    };
    match rule {
        Rule::ForcePush => {
            let guarded = guarded(daemon, &grant.place);
            push_scope(&grant.place, &guarded, args).is_some_and(|scope| scope == held.scope)
        }
        Rule::MergeBlocked => false,
        Rule::ForgeState => true,
    }
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
    // The place's guarded branches are read only for a session that holds an exception.
    match rule_of(&grant.place, &[], tool, args) {
        Some(rule) if covers(daemon, grant, rule, args) => lift(rule, ask),
        _ => verdict,
    }
}

/// The verdict on a call asked with `exception` (`why`), the session's exceptions already lifted
/// (`verdict`), and what the exception would cover (the request's digest): the ask, a request of
/// the user's in every mode, when an exception can lift what holds the call; the call as it is
/// when the session holds it already; else a refusal, with no ask. The ask says the refused
/// action as Flottille read it, a merge's pull request and what blocks it from the forge, then
/// the agent's reason.
pub async fn asked(
    daemon: &Daemon,
    grant: &Grant,
    tool: &str,
    args: &Value,
    verdict: Verdict,
    why: &str,
) -> (Verdict, Option<String>) {
    let refused = |why: String| (Verdict::Deny(why), None);
    let guarded = guarded(daemon, &grant.place);
    let rule = rule_of(&grant.place, &guarded, tool, args);
    let ask = match verdict {
        Verdict::Deny(refused_for) => {
            return refused(format!(
                "{refused_for} (no exception can be asked for it: {ASKABLE})"
            ));
        }
        Verdict::Allow if rule != Some(Rule::MergeBlocked) => {
            return match rule.is_some_and(|rule| covers(daemon, grant, rule, args)) {
                true => (Verdict::Allow, None),
                false => refused(
                    "refused: this call needs no exception: the gate lets it through as it is; \
                     call it again without exception"
                        .into(),
                ),
            };
        }
        Verdict::Allow => None,
        Verdict::Ask(ask) => Some(ask),
    };
    let Some(rule) = rule else {
        let holding = ask.map(|ask| ask.reasons.join("; ")).unwrap_or_default();
        return refused(format!(
            "refused: no exception can be asked for this call: {ASKABLE}; what holds it stays: \
             {holding}"
        ));
    };
    let (exact, scope, reasons, lifted, kind) = match rule {
        Rule::MergeBlocked => {
            let read = match crate::forge::merge_read(daemon, &grant.place, args).await {
                Ok(read) => read,
                Err(why) => return refused(format!("{why} (no exception was asked)")),
            };
            let Some(blocks) = &read.blocks else {
                return refused(format!(
                    "refused: nothing blocks the pull request #{} of {} in {}: it needs no \
                     exception; merge it without exception",
                    read.number, read.branch, read.slug
                ));
            };
            let scope = Scope {
                repository: read.repository.clone(),
                branch: read.branch.clone(),
                pull: Some(read.number),
                head: read.head.clone(),
                blocks: Some(blocks.clone()),
            };
            if held(daemon, &grant.agent, rule).is_some_and(|held| held.scope == scope) {
                let verdict = ask.map_or(Verdict::Allow, Verdict::Ask);
                return (verdict, None);
            }
            let bypass = match read.bypass {
                true => "its branch protection bypassed (gh pr merge --admin)",
                false => "Flottille's refusal lifted; Bitbucket's own merge checks still answer",
            };
            let at = read.head.as_deref().map_or(String::new(), |head| {
                format!(" at {}", head.chars().take(7).collect::<String>())
            });
            let exact = format!(
                "merge the pull request #{} of {}{at} into {} in {} (repository {}) although \
                 {blocks}, {bypass}; a commit pushed or a blocker come since asks again",
                read.number, read.branch, read.base, read.slug, read.repository
            );
            let (reasons, kind) = match ask {
                Some(ask) => (ask.reasons, ask.kind),
                None => (Vec::new(), flottille_core::approval::Kind::Call),
            };
            (exact, scope, reasons, Vec::new(), kind)
        }
        Rule::ForcePush | Rule::ForgeState => {
            let Some(ask) = ask else {
                unreachable!("an allowed call is answered above")
            };
            if covers(daemon, grant, rule, args) {
                return (Verdict::Ask(ask), None);
            }
            let lifted: Vec<String> = match lift(rule, &ask) {
                Verdict::Ask(lifted) if lifted.reasons == ask.reasons => {
                    return refused(format!(
                        "refused: no exception is needed for this call: the rule an exception \
                         lifts ({}) does not hold it; what holds it: {}",
                        rule.name(),
                        ask.reasons.join("; ")
                    ));
                }
                Verdict::Ask(lifted) => lifted.reasons,
                _ => Vec::new(),
            };
            let (exact, scope) = match push_scope(&grant.place, &guarded, args) {
                Some(scope) if rule == Rule::ForcePush => (
                    format!(
                        "force the push of {} from the repository {}, as the line says",
                        scope.branch, scope.repository
                    ),
                    scope,
                ),
                _ => (
                    "as the call says, then any issue or pull request of the project: this \
                     exception has no scope"
                        .to_owned(),
                    Scope::default(),
                ),
            };
            (exact, scope, ask.reasons, lifted, ask.kind)
        }
    };
    // A session holds one exception of a rule: allowed, this one replaces the one it holds.
    let replaces = match held(daemon, &grant.agent, rule) {
        Some(held) => format!("; it replaces the exception {}", covered(&held.scope)),
        None => String::new(),
    };
    // What the call becomes once allowed is on the card too: its check again, once the
    // exception is the session's, finds nothing the user did not read.
    let mut card = vec![
        format!(
            "{ASKS}: {} ({}): {exact}{replaces}",
            rule.name(),
            rule.said()
        ),
        format!("{REASON} {why}"),
    ];
    for reason in reasons.into_iter().chain(lifted) {
        if !card.contains(&reason) {
            card.push(reason);
        }
    }
    let digest = serde_json::to_string(&scope).ok();
    let ask = Ask {
        base: card.len(),
        reasons: card,
        class: Class::Base,
        kind,
    };
    (Verdict::Ask(ask), digest)
}

/// What a held exception is for, said on the card of the one that replaces it: `for #12 of
/// app/login`, `for app/w in api`.
fn covered(scope: &Scope) -> String {
    match (scope.pull, scope.branch.as_str()) {
        (Some(pull), branch) => format!("for #{pull} of {branch}"),
        (None, "") => "the session holds".to_owned(),
        (None, branch) => format!("for {branch} in {}", scope.repository),
    }
}

/// Whether a held call asks an exception: only the user answers it, in every mode.
pub fn asks(ask: &Ask) -> bool {
    ask.reasons
        .first()
        .is_some_and(|reason| reason.starts_with(ASKS))
}

/// The exception an allowed request grants: its rule from its first reason, and the agent's
/// reason, said after.
pub fn granted_by(reasons: &[String]) -> Option<(Rule, String)> {
    let rest = reasons.first()?.strip_prefix(ASKS)?.strip_prefix(": ")?;
    let (name, rest) = rest.split_once(" (")?;
    let rule = Rule::parse(name)?;
    rest.strip_prefix(rule.said())?.strip_prefix("): ")?;
    let why = reasons
        .iter()
        .find_map(|reason| reason.strip_prefix(REASON))?
        .trim_start();
    Some((rule, why.to_owned()))
}

/// The user accepted the exception of request `number`: the session holds it from now on, for
/// what the request covers (`digest`, its scope).
pub fn grant(daemon: &Daemon, agent: &str, reasons: &[String], digest: Option<&str>, number: u64) {
    let Some((rule, why)) = granted_by(reasons) else {
        return;
    };
    let scope: Scope = match digest.map(serde_json::from_str) {
        Some(Ok(scope)) => scope,
        _ => {
            eprintln!("flottille daemon: request #{number}: the exception covers nothing it says");
            return;
        }
    };
    if let Err(error) =
        daemon.write(|store| store.grant_exception(agent, rule.name(), &why, number, &scope))
    {
        eprintln!("flottille daemon: request #{number}: the exception cannot be kept: {error}");
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct List {
    session: String,
    /// Every exception of the session's tree, the session's own and its sub-agents', each with
    /// its holder: what the window lists.
    #[serde(default)]
    tree: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Withdraw {
    session: String,
    rule: String,
}

fn listed(exception: &Exception, title: Option<&str>) -> Value {
    let said = Rule::parse(&exception.rule).map(Rule::said);
    json!({"session": exception.agent, "title": title, "rule": exception.rule, "said": said,
        "why": exception.why, "request": exception.request, "granted_at": exception.granted_at,
        "scope": exception.scope})
}

/// `exception.list`, what the window shows where a session's rights are, and
/// `exception.withdraw`, the user's way to take one back.
pub fn call(daemon: &Daemon, method: &str, params: Value) -> Result<Value, Failure> {
    let params = if params.is_null() { json!({}) } else { params };
    match method {
        "exception.list" => {
            let List { session, tree } = serde_json::from_value(params)?;
            let Some(agent) = daemon.read(|store| store.agent(&session))? else {
                return Err(Failure::new(
                    "not-found",
                    format!("there is no agent {session}"),
                ));
            };
            let mut holders = vec![agent];
            if tree {
                holders.extend(crate::tree::descendants(daemon, &session));
            }
            let mut held = Vec::new();
            for holder in &holders {
                for exception in daemon.read(|store| store.exceptions_of(&holder.id))? {
                    held.push(listed(&exception, holder.title.as_deref()));
                }
            }
            Ok(json!({"exceptions": held}))
        }
        "exception.withdraw" => {
            let Withdraw { session, rule } = serde_json::from_value(params)?;
            match daemon.write(|store| store.withdraw_exception(&session, &rule))? {
                Some(exception) => Ok(json!({"exception": listed(&exception, None)})),
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
            "git push --force origin app/login:refs/heads/app/login",
            "git push -f origin HEAD:refs/heads/app/login",
            "git push --force-with-lease origin app/login:refs/heads/app/login",
            "git push --force-with-lease=app/login origin HEAD:refs/heads/app/login",
            "git push origin +HEAD:refs/heads/app/login",
            "git push --quiet --force origin main:refs/heads/app/login",
        ] {
            assert_eq!(
                rule_of(&own, &[], "run", &run(line)),
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
            "git push --force --receive-pack=x origin app/login:refs/heads/app/login",
            // A destination left to the remote's push setting, which may send it elsewhere.
            "git push --force origin app/login",
            "git push -f origin HEAD:app/login",
            "git push --force origin +app/login",
        ] {
            assert_eq!(rule_of(&own, &[], "run", &run(line)), None, "{line}");
        }
        // A line that waits for a ready server, or a task, is not one.
        assert_eq!(
            rule_of(
                &own,
                &[],
                "run",
                &json!({"line": "git push --force origin HEAD:refs/heads/app/login", "notify": "ready"})
            ),
            None
        );
        assert_eq!(
            rule_of(&own, &[], "run", &json!({"task": "push", "why": "x"})),
            None
        );
        // The project folder, or a workspace on a protected branch, has no own branch to force.
        let line = run("git push --force origin HEAD:refs/heads/main");
        assert_eq!(rule_of(&place(None, ""), &[], "run", &line), None);
        assert_eq!(
            rule_of(&place(Some("main"), "main"), &[], "run", &line),
            None
        );
        let line = run("git push --force origin HEAD:refs/heads/next/2.0");
        assert_eq!(
            rule_of(&place(Some("x"), "next/2.0"), &[], "run", &line),
            None
        );
    }

    #[test]
    fn closing_reopening_or_relabelling_and_nothing_else_is_a_change_of_state() {
        let own = place(Some("login"), "app/login");
        for (tool, args) in [
            ("forge", json!({"action": "close", "repository": "."})),
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
                rule_of(&own, &[], tool, &args),
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
            // A comment's text is judged, whatever exception the session holds.
            (
                "forge",
                json!({"action": "close", "number": 3, "comment": "Done."}),
            ),
            (
                "tickets",
                json!({"action": "reopen", "ticket": "#4", "comment": "Again."}),
            ),
            (
                "tickets",
                json!({"action": "edit", "ticket": "#4", "assignees": ["a"]}),
            ),
            ("tickets", json!({"action": "create", "title": "x"})),
        ] {
            assert_eq!(rule_of(&own, &[], tool, &args), None, "{tool} {args}");
        }
        assert_eq!(
            rule_of(&own, &[], "forge", &json!({"action": "merge"})),
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
        // A waive stays the user's: the merge itself checks the pull request an exception is for.
        let waived = format!("{}: the user said so", gate::WAIVED);
        assert_eq!(
            lift(Rule::MergeBlocked, &ask(&[&waived], &["it merges"])),
            Verdict::Ask(ask(&[&waived], &["it merges"]))
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
    fn a_forced_push_exception_lifts_the_line_s_own_push_alone_never_a_service_s() {
        let own = format!("git push --force {}: it can rewrite", gate::FORCES);
        let service = format!(
            "the service web, started first, its line `git push --force origin x`: git push \
             --force {}: it can rewrite",
            gate::FORCES
        );
        assert_eq!(
            lift(Rule::ForcePush, &ask(&[&own, &service], &[])),
            Verdict::Ask(ask(&[&service], &[gate::PUSH]))
        );
    }

    #[test]
    fn the_reason_of_an_ask_is_one_line_of_a_bounded_length() {
        let (_, why) = split(json!({"line": "x", "exception": "one\ntwo"}));
        assert!(why.is_err(), "{why:?}");
        let long = "x".repeat(crate::judge::WHOLE_CHARS + 1);
        let (_, why) = split(json!({"line": "x", "exception": long}));
        assert!(why.is_err(), "{why:?}");
        let (_, why) = split(json!({"line": "x", "exception": "one line"}));
        assert_eq!(why, Ok(Some("one line".into())));
    }

    #[test]
    fn the_exception_an_allowed_request_grants_is_read_from_its_first_reason() {
        let reason = format!(
            "{ASKS}: {} ({}): force the push of app/login from the repository .",
            Rule::ForcePush.name(),
            Rule::ForcePush.said()
        );
        let why = format!("{REASON} a check refuses: an old commit");
        assert_eq!(
            granted_by(&[reason.clone(), why, "other".into()]),
            Some((Rule::ForcePush, "a check refuses: an old commit".into()))
        );
        assert_eq!(granted_by(&[reason]), None);
        assert_eq!(granted_by(&["it pushes".into()]), None);
        assert_eq!(granted_by(&[]), None);
    }

    #[test]
    fn a_base_or_resumed_branch_of_the_place_is_never_forced_by_an_exception() {
        let own = place(Some("login"), "app/login");
        let line = run("git push --force origin HEAD:refs/heads/app/login");
        assert_eq!(rule_of(&own, &[], "run", &line), Some(Rule::ForcePush));
        let guarded = ["integration".to_owned(), "app/login".to_owned()];
        assert_eq!(rule_of(&own, &guarded, "run", &line), None);
    }

    #[test]
    fn a_forced_push_covers_its_repository_and_branch() {
        let mut own = place(Some("login"), "app/login");
        own.repositories = vec![
            (".".into(), PathBuf::from("/work/app")),
            ("api".into(), PathBuf::from("/work/app/api")),
        ];
        let scope = |repository: &str| Scope {
            repository: repository.into(),
            branch: "app/login".into(),
            pull: None,
        };
        let line = "git push -f origin HEAD:refs/heads/app/login";
        assert_eq!(
            push_scope(&own, &[], &json!({"line": line, "repository": "api"})),
            Some(scope("api"))
        );
        assert_eq!(
            push_scope(&own, &[], &json!({"line": line})).map(|s| s.branch),
            Some("app/login".into())
        );
        // Switched to another branch, the place's own is that one: a push of it is no other's.
        let switched = place(Some("login"), "app/x");
        assert_eq!(
            push_scope(
                &switched,
                &[],
                &json!({"line": "git push -f origin HEAD:refs/heads/app/x"})
            )
            .map(|s| s.branch),
            Some("app/x".into())
        );
        assert_eq!(push_scope(&switched, &[], &json!({"line": line})), None);
    }

    #[test]
    fn the_exception_is_taken_out_of_the_call_and_a_merge_is_waived_with_it() {
        let (args, why) = split(json!({"line": "x", "exception": " why "}));
        assert_eq!(args, json!({"line": "x"}));
        assert_eq!(why, Ok(Some("why".into())));
        // A merge is never waived with the agent's words: the user reads what blocks it.
        let (args, _) = split(json!({"action": "merge", "exception": "why"}));
        assert_eq!(args, json!({"action": "merge"}));
        let (args, why) = split(json!({"line": "x"}));
        assert_eq!((args, why), (json!({"line": "x"}), Ok(None)));
        assert!(split(json!({"line": "x", "exception": ""})).1.is_err());
        assert!(split(json!({"line": "x", "exception": 3})).1.is_err());
    }
}
