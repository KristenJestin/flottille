//! Rules proposed from the failures agents keep making (`docs/projects.md`, "Proposed rules").
//!
//! When a turn ends with a refused call, the refused calls of the project's sessions over the last
//! days are grouped by their tool and their refusal's text, what varies from one call to the next
//! (paths, numbers, ids, quoted values) replaced. A pattern seen often enough, in more than one
//! session, and never proposed before, is put to the judge, which writes one rule; the rule waits
//! for the user, who adds it to the project's rules or dismisses it.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use flottille_core::agent::TurnUsage;
use flottille_core::provider::Judge;
use flottille_core::secrets;
use flottille_core::store::{RuleProposal, now_ms};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::judge;
use crate::projects::{self, Failure};
use crate::rules;
use crate::tree;

/// What the judge is told when it writes a rule: fixed, the user's guidance of judgements has no
/// part in it.
const INSTRUCTIONS: &str = include_str!("../roles/rule-proposal.md");

/// How far back the refused calls are read.
const DAYS: u64 = 14;
/// How many times, and in how many sessions, a pattern is seen before a rule is proposed.
const SEEN: usize = 3;
const SESSIONS: usize = 2;
/// The failures the judge is shown of a pattern, the last ones.
const EXAMPLES: usize = 3;
/// The longest rule taken from the judge: one or two sentences.
const RULE_CHARS: usize = 600;
/// The longest pattern kept: what follows is the detail of one call.
const PATTERN_CHARS: usize = 300;

/// The projects whose patterns are being put to the judge now, and when each last was: the judge
/// is asked at most once an hour per project.
#[derive(Default)]
struct Asked {
    asking: HashSet<String>,
    last: HashMap<String, Instant>,
}

static ASKED: LazyLock<Mutex<Asked>> = LazyLock::new(Mutex::default);

/// How long a project waits between two askings of the judge.
fn every() -> Duration {
    // Tests shorten it, so that a pattern seen again is checked at once.
    std::env::var("FLOTTILLE_RULES_EVERY_MS")
        .ok()
        .and_then(|ms| ms.parse().ok())
        .map(Duration::from_millis)
        .unwrap_or(Duration::from_secs(3600))
}

/// The pattern of a refusal: its first line (what follows is the detail of one call), its quoted
/// values, paths, numbers and ids replaced by placeholders, its spaces made one.
pub fn pattern(text: &str) -> String {
    let first = text.lines().next().unwrap_or_default();
    let words: Vec<String> = unquoted(first).split_whitespace().map(word).collect();
    words.join(" ").chars().take(PATTERN_CHARS).collect()
}

/// The text with each quoted value, in straight or curly quotes or in backticks, replaced by
/// `<q>`. A single quote opens a value only after a space or a bracket, and closes one only before
/// a space or a punctuation mark: an apostrophe is a letter of its word.
fn unquoted(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut at = 0;
    while at < chars.len() {
        let c = chars[at];
        let opens = at == 0 || chars[at - 1].is_whitespace() || "([{:=".contains(chars[at - 1]);
        let close = match c {
            '`' => Some('`'),
            '"' => Some('"'),
            '“' => Some('”'),
            '‘' => Some('’'),
            '\'' if opens => Some('\''),
            _ => None,
        };
        let end = close.and_then(|close| {
            (at + 1..chars.len()).find(|&end| {
                chars[end] == close
                    && (close != '\''
                        || chars
                            .get(end + 1)
                            .is_none_or(|next| !next.is_alphanumeric()))
            })
        });
        match end {
            Some(end) => {
                out.push_str("<q>");
                at = end + 1;
            }
            None => {
                out.push(c);
                at += 1;
            }
        }
    }
    out
}

/// A word of a refusal, or its placeholder: `<path>` for a path or a file's name, `<n>` for a
/// number, `<id>` for a word that holds a digit. The punctuation around it stays.
fn word(word: &str) -> String {
    let core = word.trim_start_matches(['(', '[', '{']);
    let before = &word[..word.len() - core.len()];
    let core = core.trim_end_matches([',', '.', ';', ':', ')', ']', '}', '!', '?']);
    let after = &word[before.len() + core.len()..];
    let placeholder = if core.is_empty() || core == "<q>" {
        None
    } else if core
        .chars()
        .all(|c| c.is_ascii_digit() || ",.:".contains(c))
    {
        Some("<n>")
    } else if core.contains('/') || core.starts_with('~') || file_name(core) {
        Some("<path>")
    } else if let Some(number) = core.strip_prefix('#')
        && !number.is_empty()
        && number.chars().all(|c| c.is_ascii_digit())
    {
        Some("#<n>")
    } else if core.chars().any(|c| c.is_ascii_digit()) {
        Some("<id>")
    } else {
        None
    };
    match placeholder {
        Some(placeholder) => format!("{before}{placeholder}{after}"),
        None => word.to_owned(),
    }
}

/// A file's name: a stem and an extension of a few letters (`README.md`, `app.toml`).
fn file_name(word: &str) -> bool {
    word.rsplit_once('.').is_some_and(|(stem, extension)| {
        !stem.is_empty()
            && (1..=5).contains(&extension.len())
            && extension.chars().all(|c| c.is_ascii_alphanumeric())
            && extension.chars().any(|c| c.is_ascii_lowercase())
    })
}

/// A turn of the agent ended: when a call of it was refused during the turn, the refused calls of
/// its project are looked at, off the agent's way.
pub fn turn_ended(daemon: &Arc<Daemon>, agent: &str, started_at: Option<u64>) {
    let Some(started_at) = started_at else {
        return;
    };
    let refused = daemon
        .read(|store| store.refused_since(agent, started_at))
        .unwrap_or(false);
    let Some(project) = refused
        .then(|| daemon.read(|store| store.agent(agent)).ok().flatten())
        .flatten()
        .map(|agent| agent.project)
    else {
        return;
    };
    let daemon = daemon.clone();
    tokio::spawn(async move { check(&daemon, &project).await });
}

/// The failures of one pattern of a project.
#[derive(Default)]
struct Seen {
    sessions: BTreeSet<String>,
    calls: Vec<Value>,
}

/// The patterns of the project's refused calls of the last days that are worth a rule and were
/// never proposed, by tool and pattern.
fn worth(daemon: &Daemon, project: &str) -> Vec<((String, String), Seen)> {
    let since = now_ms().saturating_sub(DAYS * 24 * 3600 * 1000);
    let Ok(refused) = daemon.read(|store| store.refused_calls(project, since)) else {
        return Vec::new();
    };
    let mut seen: BTreeMap<(String, String), Seen> = BTreeMap::new();
    let mut roots: HashMap<String, String> = HashMap::new();
    for (event, agent) in refused {
        let (Some(tool), Some(error)) = (event.data["tool"].as_str(), event.data["error"].as_str())
        else {
            continue;
        };
        let session = roots
            .entry(agent.clone())
            .or_insert_with(|| tree::root_of(daemon, &agent))
            .clone();
        let one = seen.entry((tool.to_owned(), pattern(error))).or_default();
        one.sessions.insert(session.clone());
        one.calls
            .push(json!({"seq": event.seq, "at": event.at, "agent": agent,
            "session": session, "arguments": event.data["arguments"], "error": error}));
    }
    seen.into_iter()
        .filter(|(_, seen)| seen.calls.len() >= SEEN && seen.sessions.len() >= SESSIONS)
        .filter(|((tool, pattern), _)| {
            !daemon
                .read(|store| store.rule_proposed(project, tool, pattern))
                .unwrap_or(true)
        })
        .collect()
}

/// Puts each pattern of the project worth a rule to the judge, at most once an hour: none while
/// another check of the project asks it, and none without a judge. A judge that fails proposes
/// nothing; the pattern is put to it again at a later check.
async fn check(daemon: &Arc<Daemon>, project: &str) {
    let found = {
        let daemon = daemon.clone();
        let project = project.to_owned();
        tokio::task::spawn_blocking(move || worth(&daemon, &project))
            .await
            .unwrap_or_default()
    };
    if found.is_empty() {
        return;
    }
    {
        let mut asked = ASKED.lock().unwrap();
        let Asked { asking, last } = &mut *asked;
        if asking.contains(project) || last.get(project).is_some_and(|at| at.elapsed() < every()) {
            return;
        }
        asking.insert(project.to_owned());
    }
    if let Some(judge) = judge::chosen(daemon).await {
        ASKED
            .lock()
            .unwrap()
            .last
            .insert(project.to_owned(), Instant::now());
        for ((tool, pattern), seen) in found {
            match propose(daemon, &judge, project, &tool, &pattern, &seen).await {
                Ok(()) => {}
                Err(why) => eprintln!(
                    "flottille daemon: project {project}: no rule proposed for a failure of \
                     {tool}: {why}"
                ),
            }
        }
    }
    ASKED.lock().unwrap().asking.remove(project);
}

/// The judge writes the rule of one pattern, which waits for the user.
async fn propose(
    daemon: &Arc<Daemon>,
    judge: &Judge,
    project: &str,
    tool: &str,
    pattern: &str,
    seen: &Seen,
) -> Result<(), String> {
    let prompt = prompt(daemon, project, tool, pattern, &seen.calls);
    let model = judge::Model {
        provider: &judge.provider,
        model: judge.model.as_deref(),
        effort: None,
    };
    let started_at = now_ms();
    let turned = tokio::time::timeout(
        judge::timeout(),
        judge::one_turn(daemon, &model, "rules", INSTRUCTIONS, &prompt),
    )
    .await
    .map_err(|_| {
        format!(
            "the judge gave no answer within {} seconds",
            judge::timeout().as_secs_f32()
        )
    })??;
    let rows: Vec<TurnUsage> = turned
        .used
        .iter()
        .map(|used| TurnUsage {
            agent: "rules".into(),
            title: None,
            session: "rules".into(),
            project: project.to_owned(),
            workspace: None,
            provider: judge.provider.clone(),
            role: "rules".into(),
            started_at,
            ended_at: now_ms(),
            stop_reason: turned.stop.clone(),
            used: flottille_core::agent::Used {
                model: used.model.clone().or_else(|| judge.model.clone()),
                ..used.clone()
            },
        })
        .collect();
    if let Err(error) = daemon.write(|store| Ok((store.record_turn_usage(&rows)?, None))) {
        eprintln!("flottille daemon: project {project}: recording the judge's usage: {error}");
    }
    if turned.tool {
        return Err("the judge tried to call a tool".into());
    }
    let rule =
        rule(&turned.said).ok_or("the judge's answer was no rule of one or two sentences")?;
    let proposal = RuleProposal {
        project: project.to_owned(),
        tool: tool.to_owned(),
        pattern: pattern.to_owned(),
        rule,
        count: seen.calls.len() as u64,
        sessions: seen.sessions.len() as u64,
        calls: seen.calls.clone(),
        created_at: now_ms(),
        ..RuleProposal::default()
    };
    daemon
        .write(|store| {
            let Some(id) = store.add_rule_proposal(&proposal)? else {
                return Ok(((), None));
            };
            let event = store.append_event(
                "rules.proposed",
                json!({"project": project, "proposal": id, "tool": tool,
                    "rule": proposal.rule, "count": proposal.count,
                    "sessions": proposal.sessions}),
            )?;
            Ok(((), Some(event)))
        })
        .map_err(|error| error.to_string())
}

/// The judge's answer as a rule: trimmed of a fence or quotes around it, masked; none when it is
/// empty or longer than one or two sentences.
fn rule(said: &str) -> Option<String> {
    let text = said
        .trim()
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim()
        .trim_matches(['"', '“', '”'])
        .trim();
    let env = std::env::vars_os().collect();
    let text = secrets::mask(text, &env);
    (!text.is_empty() && text.chars().count() <= RULE_CHARS).then_some(text)
}

/// What the judge is given: the pattern, the last failures behind it and the project's rules as
/// they stand, each in a block of its own, masked whole.
fn prompt(daemon: &Daemon, project: &str, tool: &str, pattern: &str, calls: &[Value]) -> String {
    let own = projects::load_project(daemon, project)
        .map(|project| rules::own(&project).0)
        .unwrap_or_default();
    let mut prompt = format!(
        "The agents of the project keep making the same failing call. The tool: {tool}.\n\
         The failure, what varies from one call to the next replaced:\n\
         <pattern>\n{}\n</pattern>\n\nThe last of them:\n",
        escaped(pattern)
    );
    for call in calls.iter().rev().take(EXAMPLES).rev() {
        prompt.push_str(&format!(
            "<failure>\n<arguments>{}</arguments>\n<refusal>{}</refusal>\n</failure>\n",
            escaped(&call["arguments"].to_string()),
            escaped(call["error"].as_str().unwrap_or_default())
        ));
    }
    prompt.push_str(&format!(
        "\nThe project's rules as they stand:\n<rules>\n{}\n</rules>\n\nWrite the rule.",
        escaped(&own)
    ));
    let env = std::env::vars_os().collect();
    secrets::mask(&prompt, &env)
}

/// A text the judge reads as data: no `<` of it opens or closes a block.
fn escaped(text: &str) -> String {
    text.replace('<', "&lt;")
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Listed {
    project: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Accepted {
    proposal: u64,
    /// The rule as the user edited it; left out: as it was proposed.
    text: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Dismissed {
    proposal: u64,
}

/// Answers `rules.proposals`, `rules.proposal.accept` and `rules.proposal.dismiss`.
pub fn call(daemon: &Daemon, method: &str, params: Value) -> Result<Value, Failure> {
    match method {
        "rules.proposals" => {
            let Listed { project } = match params {
                Value::Null => Listed { project: None },
                params => serde_json::from_value(params)?,
            };
            let proposals = daemon.read(|store| store.rule_proposals(project.as_deref(), true))?;
            Ok(json!({"proposals": proposals.iter().map(shown).collect::<Vec<_>>()}))
        }
        "rules.proposal.accept" => {
            let Accepted { proposal, text } = serde_json::from_value(params)?;
            let open = opened(daemon, proposal)?;
            let rule = text.as_deref().unwrap_or(&open.rule).trim().to_owned();
            if rule.is_empty() {
                return Err(Failure::refused(
                    "the rule is empty: write it, or dismiss it",
                ));
            }
            let project = projects::load_project(daemon, &open.project)?;
            let own = project.rules.clone().unwrap_or_default();
            let text = match own.trim_end() {
                "" => rule.clone(),
                own => format!("{own}\n\n{rule}"),
            };
            rules::set_project(daemon, &open.project, Some(&text), None)?;
            decide(daemon, &open, "accepted", Some(&rule))
        }
        "rules.proposal.dismiss" => {
            let Dismissed { proposal } = serde_json::from_value(params)?;
            let open = opened(daemon, proposal)?;
            decide(daemon, &open, "dismissed", None)
        }
        method => Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        )),
    }
}

/// The proposal, while it waits for the user.
fn opened(daemon: &Daemon, id: u64) -> Result<RuleProposal, Failure> {
    match daemon.read(|store| store.rule_proposal(id))? {
        Some(proposal) if proposal.state == "open" => Ok(proposal),
        Some(proposal) => Err(Failure::refused(format!(
            "proposal {id} was {} already",
            proposal.state
        ))),
        None => Err(Failure::new(
            "not-found",
            format!("there is no proposed rule {id}"),
        )),
    }
}

/// The proposal accepted or dismissed, said by `rules.proposal_accepted` or
/// `rules.proposal_dismissed`; answers it as it is now.
fn decide(
    daemon: &Daemon,
    open: &RuleProposal,
    state: &str,
    rule: Option<&str>,
) -> Result<Value, Failure> {
    let decided = daemon.write(|store| {
        if !store.decide_rule_proposal(open.id, state, rule)? {
            return Ok((false, None));
        }
        let event = store.append_event(
            &format!("rules.proposal_{state}"),
            json!({"project": open.project, "proposal": open.id, "rule": rule}),
        )?;
        Ok((true, Some(event)))
    })?;
    if !decided {
        return Err(Failure::refused(format!(
            "proposal {} was answered already",
            open.id
        )));
    }
    let now = daemon.read(|store| store.rule_proposal(open.id))?;
    Ok(now.as_ref().map(shown).unwrap_or_default())
}

/// A proposal as the protocol says it.
fn shown(proposal: &RuleProposal) -> Value {
    json!({"id": proposal.id, "project": proposal.project, "tool": proposal.tool,
        "pattern": proposal.pattern, "state": proposal.state, "rule": proposal.rule,
        "count": proposal.count, "sessions": proposal.sessions, "calls": proposal.calls,
        "at": proposal.created_at})
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two refusals that are the same failure give the same pattern.
    fn same(a: &str, b: &str) {
        assert_eq!(pattern(a), pattern(b), "\n{a}\n{b}");
    }

    /// Two refusals that are different failures give different patterns.
    fn different(a: &str, b: &str) {
        assert_ne!(pattern(a), pattern(b), "\n{a}\n{b}");
    }

    #[test]
    fn paths_and_ids_do_not_split_a_pattern() {
        // The texts are those of `docs/tools.md` and `docs/reference/hemera.md` §3.
        same(
            "refused: src/main.rs exists and this session has not read it: read it with fs_read \
             first",
            "refused: docs/guide.md exists and this session has not read it: read it with fs_read \
             first",
        );
        same(
            "refused: ~/.ssh/id_ed25519 is outside this agent's place: it works in /work/app",
            "refused: /etc/hosts is outside this agent's place: it works in /work/app-2",
        );
        same(
            "refused: res_1a2b3c4d is no cut result of this session",
            "refused: res_9f8e7d6c is no cut result of this session",
        );
        same(
            "refused: api is on no branch (detached at 1a2b3c4): switch to a branch first",
            "refused: api is on no branch (detached at 9e8d7c6): switch to a branch first",
        );
        same(
            "refused: request #3 already asks the user to let you write to src/a.rs",
            "refused: request #12 already asks the user to let you write to web/index.html",
        );
        same(
            "refused: the anchor of edit 2 appears 3 time(s) in src/lib.rs; it must appear exactly \
             once. Nothing was changed",
            "refused: the anchor of edit 1 appears 0 time(s) in README.md; it must appear exactly \
             once. Nothing was changed",
        );
        // What changed is told after the first line, and differs each time.
        same(
            "refused: src/lib.rs changed since this session read it; read it again before writing. \
             What changed:\n- let a = 1;\n+ let a = 2;",
            "refused: app.toml changed since this session read it; read it again before writing. \
             What changed:\n- name = \"x\"",
        );
        // Quoted values, straight, curly or in backticks; an apostrophe is no quote.
        same(
            "refused: the arguments of fs_read do not read: missing field `path`",
            "refused: the arguments of fs_read do not read: missing field `range`",
        );
        same(
            "Flottille runs a command without a shell, and “$HOME” is shell syntax: a shell would \
             put the value of a variable there and none does here: write the value itself",
            "Flottille runs a command without a shell, and “$PATH” is shell syntax: a shell would \
             put the value of a variable there and none does here: write the value itself",
        );
        same(
            "refused: the task \"lint\" cannot run here",
            "refused: the task 'test:unit' cannot run here",
        );
        same(
            "the judge could not decide: it gave no answer within 30 seconds",
            "the judge could not decide: it gave no answer within 5 seconds",
        );
    }

    #[test]
    fn different_failures_keep_different_patterns() {
        different(
            "refused: this session has ended",
            "refused: this session has been replaced",
        );
        different(
            "refused: tickets comment takes no title: title is for edit and create",
            "refused: forge merge by merge takes no message: the merge commit is the forge's",
        );
        different(
            "refused: src/main.rs exists and this session has not read it: read it with fs_read \
             first",
            "refused: src/main.rs was removed since this session read it; nothing was written",
        );
        different(
            "Flottille runs a command without a shell, and “|” is shell syntax: run starts one \
             program and hands its output to no other",
            "Flottille runs a command without a shell, and “>” is shell syntax: run starts one \
             program and keeps its output for you to read with run_status",
        );
        // The apostrophe of a word does not swallow the text up to the next one.
        different(
            "refused: the agent's place is main: it can't write in the agent's workspace",
            "refused: the agent's place is main: it can't read the user's keys",
        );
    }
}
