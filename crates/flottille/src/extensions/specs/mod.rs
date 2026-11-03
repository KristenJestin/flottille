//! Specs (`docs/extensions.md`, "Specs"): the internal, technical version of a task to do, kept in
//! the store with its status, its lock, its mode and its tickets.

pub mod command;

use flottille_core::project::Project;
use flottille_core::protocol::Event;
use flottille_core::store::{Space, now_ms};
use flottille_core::{Error, Result};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Extension, Kind, Methods, Place, Scope, Setting};
use crate::daemon::Daemon;
use crate::projects::Failure;
use crate::tickets;

pub const EXTENSION: Extension = Extension {
    name: "specs",
    title: "Specs",
    sentence: "Keeps a spec of each task to build: what an agent needs to build it without \
               guessing. It runs no model on its own.",
    scope: Scope::Project,
    on_by_default: false,
    place: Place::Tab {
        title: "Specs",
        order: 2,
    },
    settings: &[Setting {
        key: "language",
        label: "Language",
        hint: "The language specs are written in; yours by default",
        kind: Kind::Language,
        per_project: true,
    }],
    migrations: &["
    -- A spec: its sections as JSON (`Content`), and the last C and D numbers given, so that
    -- none is ever given twice. AUTOINCREMENT: a spec's number is never given again either.
    CREATE TABLE ext_specs_specs (
        id         INTEGER PRIMARY KEY AUTOINCREMENT,
        project    TEXT NOT NULL,
        title      TEXT NOT NULL,
        mode       TEXT NOT NULL,
        status     TEXT NOT NULL,
        language   TEXT NOT NULL,
        content    TEXT NOT NULL,
        last_c     INTEGER NOT NULL,
        last_d     INTEGER NOT NULL,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL
    ) STRICT;
    -- The tickets of a project's tracker a spec covers: a ticket has at most one spec.
    CREATE TABLE ext_specs_tickets (
        project TEXT NOT NULL,
        ticket  TEXT NOT NULL,
        spec    INTEGER NOT NULL REFERENCES ext_specs_specs (id),
        PRIMARY KEY (project, ticket)
    ) STRICT;
    "],
    methods: Methods {
        prefix: "spec",
        call,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// No ticket: the spec is in Flottille only.
    Alone,
    /// Linked to tickets Flottille reads and never writes.
    Internal,
    /// The ticket's description is the spec.
    Ticket,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Draft,
    Ready,
    /// The three below are read from what Flottille sees (#336): none is set here.
    InProgress,
    Review,
    Done,
}

impl Status {
    fn title(self) -> &'static str {
        match self {
            Status::Draft => "Draft",
            Status::Ready => "Ready",
            Status::InProgress => "In progress",
            Status::Review => "Review",
            Status::Done => "Done",
        }
    }
}

fn word<T: Serialize>(value: T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn from_word<T: serde::de::DeserializeOwned>(word: &str) -> Result<T> {
    Ok(serde_json::from_value(json!(word))?)
}

/// An expected behaviour, `C<n>`: one sentence that can be checked.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Behaviour {
    id: String,
    text: String,
}

/// A decision, `D<n>`, with the question it answered and the options set aside.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Decision {
    id: String,
    question: String,
    decision: String,
    set_aside: Vec<String>,
}

/// What the spec leaves out, with its reason or the ticket that takes it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Outside {
    text: String,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    ticket: Option<String>,
}

/// How one `C` is verified: read by the user, not validated.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Verification {
    id: String,
    text: String,
}

/// The sections of a spec.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Content {
    problem: String,
    behaviour: Vec<Behaviour>,
    decisions: Vec<Decision>,
    out_of_scope: Vec<Outside>,
    notes: String,
    verification: Vec<Verification>,
    questions: Vec<String>,
}

const SECTIONS: &[&str] = &[
    "problem",
    "behaviour",
    "decisions",
    "out_of_scope",
    "notes",
    "verification",
    "questions",
];

/// A spec as the store keeps it.
struct Spec {
    id: u64,
    project: String,
    title: String,
    mode: Mode,
    status: Status,
    language: String,
    content: Content,
    last_c: u64,
    last_d: u64,
    tickets: Vec<String>,
    created_at: u64,
    updated_at: u64,
}

impl Spec {
    fn to_json(&self) -> Value {
        json!({
            "spec": self.id,
            "project": self.project,
            "title": self.title,
            "mode": self.mode,
            "status": self.status,
            "language": self.language,
            "tickets": self.tickets,
            "created_at": self.created_at,
            "updated_at": self.updated_at,
            "problem": self.content.problem,
            "behaviour": self.content.behaviour,
            "decisions": self.content.decisions,
            "out_of_scope": self.content.out_of_scope,
            "notes": self.content.notes,
            "verification": self.content.verification,
            "questions": self.content.questions,
        })
    }

    fn summary(&self) -> Value {
        json!({
            "spec": self.id,
            "project": self.project,
            "title": self.title,
            "mode": self.mode,
            "status": self.status,
            "tickets": self.tickets,
            "questions": self.content.questions.len(),
            "updated_at": self.updated_at,
        })
    }
}

const COLUMNS: &str = "id, project, title, mode, status, language, content, last_c, last_d, \
    created_at, updated_at";

fn tickets_of(space: &Space, id: u64) -> Result<Vec<String>> {
    let mut statement = space
        .tables()
        .prepare("SELECT ticket FROM ext_specs_tickets WHERE spec = ?1 ORDER BY rowid")?;
    let tickets = statement
        .query_map([id], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(tickets)
}

/// A row as it is read, before its words and its JSON are.
struct Raw {
    id: u64,
    project: String,
    title: String,
    mode: String,
    status: String,
    language: String,
    content: String,
    last_c: u64,
    last_d: u64,
    created_at: u64,
    updated_at: u64,
}

fn raw(row: &rusqlite::Row) -> rusqlite::Result<Raw> {
    Ok(Raw {
        id: row.get(0)?,
        project: row.get(1)?,
        title: row.get(2)?,
        mode: row.get(3)?,
        status: row.get(4)?,
        language: row.get(5)?,
        content: row.get(6)?,
        last_c: row.get(7)?,
        last_d: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

fn specs_where(space: &Space, filter: &str, values: &[&dyn rusqlite::ToSql]) -> Result<Vec<Spec>> {
    let mut statement = space.tables().prepare(&format!(
        "SELECT {COLUMNS} FROM ext_specs_specs {filter} ORDER BY id"
    ))?;
    let rows = statement
        .query_map(values, raw)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(|raw| {
            Ok(Spec {
                id: raw.id,
                mode: from_word(&raw.mode)?,
                status: from_word(&raw.status)?,
                content: serde_json::from_str(&raw.content)?,
                tickets: tickets_of(space, raw.id)?,
                project: raw.project,
                title: raw.title,
                language: raw.language,
                last_c: raw.last_c,
                last_d: raw.last_d,
                created_at: raw.created_at,
                updated_at: raw.updated_at,
            })
        })
        .collect()
}

fn spec_of(space: &Space, id: u64) -> Result<Option<Spec>> {
    Ok(specs_where(space, "WHERE id = ?1", &[&id])?.pop())
}

fn not_found(id: u64) -> Failure {
    Failure::new("not-found", format!("no spec {id}"))
}

/// The spec, read in a space of its own; `None` when there is none of that number.
fn read(daemon: &Daemon, id: u64) -> Result<Option<Spec>> {
    daemon.write(|store| {
        let space = store.space()?;
        Ok((spec_of(&space, id)?, None::<Event>))
    })
}

/// The spec, once Specs is active in its project.
fn active_spec(daemon: &Daemon, id: u64) -> std::result::Result<(Spec, Project), Failure> {
    let spec = read(daemon, id)?.ok_or_else(|| not_found(id))?;
    let project = super::active(daemon, &EXTENSION, &spec.project)?;
    Ok((spec, project))
}

/// The ticket that a spec of this project already covers: the spec that has it.
fn holder(space: &Space, project: &str, ticket: &str) -> Result<Option<(u64, String)>> {
    Ok(space
        .tables()
        .query_row(
            "SELECT s.id, s.title FROM ext_specs_tickets t JOIN ext_specs_specs s ON s.id = t.spec \
             WHERE t.project = ?1 AND t.ticket = ?2",
            params![project, ticket],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?)
}

fn link_ticket(space: &Space, spec: &Spec, ticket: &str) -> Result<()> {
    if let Some((id, title)) = holder(space, &spec.project, ticket)? {
        return Err(Error::Refused(if id == spec.id {
            format!("spec {id} already covers ticket {ticket}")
        } else {
            format!(
                "ticket {ticket} already has a spec: spec {id}, \"{title}\"; a ticket has at most \
                 one spec"
            )
        }));
    }
    space.tables().execute(
        "INSERT INTO ext_specs_tickets (project, ticket, spec) VALUES (?1, ?2, ?3)",
        params![spec.project, ticket, spec.id],
    )?;
    Ok(())
}

/// The ticket as the project's tracker writes it, or why it is not one of its tickets.
fn ticket_of(project: &Project, text: &str) -> std::result::Result<String, Failure> {
    tickets::accepted(project, text)
        .map(|key| key.text())
        .map_err(Failure::refused)
}

/// Every write to a spec passes here, whoever asks: past Draft it is locked, the user's door
/// included, until the user sends it back to Draft. `change` edits the spec and says what changed;
/// the spec and `spec.changed` are written together.
fn write_draft(
    daemon: &Daemon,
    id: u64,
    change: impl FnOnce(&Space, &mut Spec) -> Result<&'static str>,
) -> std::result::Result<Value, Failure> {
    active_spec(daemon, id)?;
    let spec = daemon.write(|store| {
        let space = store.space()?;
        let mut spec =
            spec_of(&space, id)?.ok_or_else(|| Error::Refused(format!("no spec {id}")))?;
        if spec.status != Status::Draft {
            return Err(Error::Refused(format!(
                "spec {id} is {}: it is locked; send it back to Draft first (`flottille spec draft \
                 {id}`)",
                spec.status.title()
            )));
        }
        let what = change(&space, &mut spec)?;
        spec.updated_at = now_ms();
        space.tables().execute(
            "UPDATE ext_specs_specs SET content = ?2, last_c = ?3, last_d = ?4, updated_at = ?5 \
             WHERE id = ?1",
            params![
                id,
                serde_json::to_string(&spec.content)?,
                spec.last_c,
                spec.last_d,
                spec.updated_at
            ],
        )?;
        spec.tickets = tickets_of(&space, id)?;
        let event = space.append_event(
            "spec.changed",
            json!({"spec": id, "project": spec.project, "what": what}),
        )?;
        space.commit()?;
        Ok((spec, event))
    })?;
    Ok(spec.to_json())
}

/// The user's gestures: Ready from Draft, Draft from Ready.
fn set_status(daemon: &Daemon, id: u64, to: Status) -> std::result::Result<Value, Failure> {
    active_spec(daemon, id)?;
    let spec = daemon.write(|store| {
        let space = store.space()?;
        let mut spec =
            spec_of(&space, id)?.ok_or_else(|| Error::Refused(format!("no spec {id}")))?;
        let from = match to {
            Status::Draft => Status::Ready,
            _ => Status::Draft,
        };
        if spec.status != from {
            return Err(Error::Refused(format!(
                "spec {id} is {}: only a spec that is {} becomes {}",
                spec.status.title(),
                from.title(),
                to.title()
            )));
        }
        if to == Status::Ready {
            let open = spec.content.questions.len();
            if open > 0 {
                return Err(Error::Refused(format!(
                    "spec {id} has {open} open question{}: none may be left for Ready",
                    if open == 1 { "" } else { "s" }
                )));
            }
            if spec.content.behaviour.is_empty() {
                return Err(Error::Refused(format!(
                    "spec {id} has no expected behaviour (C) yet: Ready needs one at least"
                )));
            }
        }
        spec.status = to;
        spec.updated_at = now_ms();
        space.tables().execute(
            "UPDATE ext_specs_specs SET status = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, word(to), spec.updated_at],
        )?;
        let event = space.append_event(
            "spec.status_changed",
            json!({"spec": id, "project": spec.project, "status": to, "was": from}),
        )?;
        space.commit()?;
        Ok((spec, event))
    })?;
    Ok(spec.to_json())
}

/// The next identifiers of a list written whole: an item that keeps its identifier must name one
/// the spec has now; one without gets the next number. An identifier removed is never given
/// again, nor taken back.
fn numbered(
    letter: char,
    asked: Vec<Option<String>>,
    now: &[&str],
    last: &mut u64,
) -> Result<Vec<String>> {
    let mut given: Vec<String> = Vec::new();
    for id in asked {
        let id = match id {
            Some(id) => {
                let number = id
                    .strip_prefix(letter)
                    .and_then(|number| number.parse::<u64>().ok())
                    .filter(|number| *number >= 1);
                match number {
                    None => {
                        return Err(Error::Refused(format!(
                            "{id} is not an identifier of a {letter}: {letter}1, {letter}2…"
                        )));
                    }
                    Some(number) if number <= *last && !now.contains(&id.as_str()) => {
                        return Err(Error::Refused(format!(
                            "{id} was removed: an identifier is never given again; write the \
                             {letter} without its identifier to get a new one"
                        )));
                    }
                    Some(_) if !now.contains(&id.as_str()) => {
                        return Err(Error::Refused(format!(
                            "{id} is not a {letter} of this spec: a new {letter} is written \
                             without its identifier, and gets the next one"
                        )));
                    }
                    Some(_) => id,
                }
            }
            None => {
                *last += 1;
                format!("{letter}{last}")
            }
        };
        if given.contains(&id) {
            return Err(Error::Refused(format!("{id} is written twice")));
        }
        given.push(id);
    }
    Ok(given)
}

fn said(text: &str, what: &str) -> Result<String> {
    let text = text.trim();
    if text.is_empty() {
        return Err(Error::Refused(format!("{what}: must not be empty")));
    }
    Ok(text.to_owned())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BehaviourAsked {
    #[serde(default)]
    id: Option<String>,
    text: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DecisionAsked {
    #[serde(default)]
    id: Option<String>,
    question: String,
    decision: String,
    #[serde(default)]
    set_aside: Vec<String>,
}

/// One section written whole, as `spec.write` gives it: a text for `problem` and `notes`, a list
/// for the others.
fn write_section(project: &Project, spec: &mut Spec, section: &str, content: Value) -> Result<()> {
    let refuse = |error: serde_json::Error| Error::Refused(format!("{section}: {error}"));
    let text = |content: Value| -> Result<String> {
        content
            .as_str()
            .map(|text| text.trim().to_owned())
            .ok_or_else(|| Error::Refused(format!("{section}: must be a text")))
    };
    let content_of = &mut spec.content;
    match section {
        "problem" => content_of.problem = text(content)?,
        "notes" => content_of.notes = text(content)?,
        "behaviour" => {
            let asked: Vec<BehaviourAsked> = serde_json::from_value(content).map_err(refuse)?;
            let now: Vec<&str> = content_of.behaviour.iter().map(|c| c.id.as_str()).collect();
            let ids = numbered(
                'C',
                asked.iter().map(|c| c.id.clone()).collect(),
                &now,
                &mut spec.last_c,
            )?;
            let behaviour = asked
                .into_iter()
                .zip(ids)
                .map(|(asked, id)| {
                    Ok(Behaviour {
                        text: said(&asked.text, &id)?,
                        id,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            content_of
                .verification
                .retain(|v| behaviour.iter().any(|c| c.id == v.id));
            content_of.behaviour = behaviour;
        }
        "decisions" => {
            let asked: Vec<DecisionAsked> = serde_json::from_value(content).map_err(refuse)?;
            let now: Vec<&str> = content_of.decisions.iter().map(|d| d.id.as_str()).collect();
            let ids = numbered(
                'D',
                asked.iter().map(|d| d.id.clone()).collect(),
                &now,
                &mut spec.last_d,
            )?;
            content_of.decisions = asked
                .into_iter()
                .zip(ids)
                .map(|(asked, id)| {
                    Ok(Decision {
                        question: said(&asked.question, &format!("{id}.question"))?,
                        decision: said(&asked.decision, &format!("{id}.decision"))?,
                        set_aside: asked
                            .set_aside
                            .iter()
                            .map(|option| said(option, &format!("{id}.set_aside")))
                            .collect::<Result<_>>()?,
                        id,
                    })
                })
                .collect::<Result<_>>()?;
        }
        "out_of_scope" => {
            let asked: Vec<Outside> = serde_json::from_value(content).map_err(refuse)?;
            content_of.out_of_scope = asked
                .into_iter()
                .enumerate()
                .map(|(at, outside)| {
                    let at = format!("out_of_scope[{at}]");
                    let reason = outside
                        .reason
                        .as_deref()
                        .map(|reason| said(reason, &format!("{at}.reason")))
                        .transpose()?;
                    let ticket = outside
                        .ticket
                        .as_deref()
                        .map(|ticket| {
                            tickets::accepted(project, ticket)
                                .map(|key| key.text())
                                .map_err(|why| Error::Refused(format!("{at}.ticket: {why}")))
                        })
                        .transpose()?;
                    if reason.is_none() && ticket.is_none() {
                        return Err(Error::Refused(format!(
                            "{at}: says its reason, or the ticket that takes it"
                        )));
                    }
                    Ok(Outside {
                        text: said(&outside.text, &at)?,
                        reason,
                        ticket,
                    })
                })
                .collect::<Result<_>>()?;
        }
        "verification" => {
            let asked: Vec<Verification> = serde_json::from_value(content).map_err(refuse)?;
            let mut verification: Vec<Verification> = Vec::new();
            for entry in asked {
                if !content_of.behaviour.iter().any(|c| c.id == entry.id) {
                    return Err(Error::Refused(format!(
                        "verification: {} is not a C of this spec",
                        entry.id
                    )));
                }
                if verification.iter().any(|v| v.id == entry.id) {
                    return Err(Error::Refused(format!(
                        "verification: {} is written twice",
                        entry.id
                    )));
                }
                verification.push(Verification {
                    text: said(&entry.text, &format!("verification.{}", entry.id))?,
                    id: entry.id,
                });
            }
            content_of.verification = verification;
        }
        "questions" => {
            let asked: Vec<String> = serde_json::from_value(content).map_err(refuse)?;
            content_of.questions = asked
                .iter()
                .map(|question| said(question, "questions"))
                .collect::<Result<_>>()?;
        }
        section => {
            return Err(Error::Refused(format!(
                "{section}: not a section of a spec: the sections are {}",
                SECTIONS.join(", ")
            )));
        }
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct New {
    project: String,
    title: String,
    mode: Option<Mode>,
    #[serde(default)]
    tickets: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Listed {
    project: Option<String>,
    status: Option<Status>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Named {
    spec: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Write {
    spec: u64,
    section: String,
    content: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TicketOf {
    spec: u64,
    ticket: String,
}

/// Answers a `spec.*` request of the user.
fn call(daemon: &Daemon, method: &str, params: Value) -> std::result::Result<Value, Failure> {
    match method {
        "spec.new" => new(daemon, serde_json::from_value(params)?),
        "spec.list" => {
            let asked: Listed = serde_json::from_value(params)?;
            let projects: Vec<String> = match &asked.project {
                Some(project) => vec![super::active(daemon, &EXTENSION, project)?.name],
                None => {
                    super::on(daemon, &EXTENSION)?;
                    crate::projects::all(daemon)
                        .into_iter()
                        .filter(|project| super::active(daemon, &EXTENSION, &project.name).is_ok())
                        .map(|project| project.name)
                        .collect()
                }
            };
            let specs = daemon.write(|store| {
                let space = store.space()?;
                Ok((specs_where(&space, "", &[])?, None::<Event>))
            })?;
            let listed: Vec<Value> = specs
                .iter()
                .filter(|spec| projects.contains(&spec.project))
                .filter(|spec| asked.status.is_none_or(|status| spec.status == status))
                .map(Spec::summary)
                .collect();
            Ok(json!({"specs": listed}))
        }
        "spec.show" => {
            let Named { spec } = serde_json::from_value(params)?;
            Ok(active_spec(daemon, spec)?.0.to_json())
        }
        "spec.write" => {
            let asked: Write = serde_json::from_value(params)?;
            let (_, project) = active_spec(daemon, asked.spec)?;
            let section = asked.section;
            write_draft(daemon, asked.spec, |_, spec| {
                write_section(&project, spec, &section, asked.content)?;
                Ok(SECTIONS
                    .iter()
                    .find(|known| **known == section)
                    .expect("a section written is known"))
            })
        }
        "spec.link" => {
            let asked: TicketOf = serde_json::from_value(params)?;
            let (_, project) = active_spec(daemon, asked.spec)?;
            let ticket = ticket_of(&project, &asked.ticket)?;
            write_draft(daemon, asked.spec, |space, spec| {
                if spec.mode == Mode::Alone {
                    return Err(Error::Refused(format!(
                        "spec {} is alone: it has no ticket",
                        spec.id
                    )));
                }
                link_ticket(space, spec, &ticket)?;
                Ok("tickets")
            })
        }
        "spec.unlink" => {
            let asked: TicketOf = serde_json::from_value(params)?;
            let (_, project) = active_spec(daemon, asked.spec)?;
            let ticket = ticket_of(&project, &asked.ticket)?;
            write_draft(daemon, asked.spec, |space, spec| {
                if !spec.tickets.contains(&ticket) {
                    return Err(Error::Refused(format!(
                        "spec {} does not cover ticket {ticket}",
                        spec.id
                    )));
                }
                if spec.tickets.len() == 1 {
                    return Err(Error::Refused(format!(
                        "ticket {ticket} is the last of spec {}, which is {}: it keeps one ticket \
                         at least",
                        spec.id,
                        word(spec.mode)
                    )));
                }
                space.tables().execute(
                    "DELETE FROM ext_specs_tickets WHERE project = ?1 AND ticket = ?2",
                    params![spec.project, ticket],
                )?;
                Ok("tickets")
            })
        }
        "spec.ready" => {
            let Named { spec } = serde_json::from_value(params)?;
            set_status(daemon, spec, Status::Ready)
        }
        "spec.draft" => {
            let Named { spec } = serde_json::from_value(params)?;
            set_status(daemon, spec, Status::Draft)
        }
        method => Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        )),
    }
}

/// `spec.new`: a Draft in the project's language, alone without tickets and internal with them
/// unless the mode is given.
fn new(daemon: &Daemon, asked: New) -> std::result::Result<Value, Failure> {
    let project = super::active(daemon, &EXTENSION, &asked.project)?;
    let title = said(&asked.title, "title")?;
    let tickets = asked
        .tickets
        .iter()
        .map(|ticket| ticket_of(&project, ticket))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mode = asked.mode.unwrap_or(if tickets.is_empty() {
        Mode::Alone
    } else {
        Mode::Internal
    });
    match mode {
        Mode::Alone if !tickets.is_empty() => {
            return Err(Failure::refused(
                "a spec alone has no ticket: choose --mode internal or ticket to link it",
            ));
        }
        Mode::Internal | Mode::Ticket if tickets.is_empty() => {
            return Err(Failure::refused(format!(
                "a spec in the mode {} covers one ticket at least: name it with --ticket",
                word(mode)
            )));
        }
        _ => {}
    }
    let language = super::value(daemon, &EXTENSION, &project, "language")?
        .as_str()
        .unwrap_or("en")
        .to_owned();
    let at = now_ms();
    let spec = daemon.write(|store| {
        let space = store.space()?;
        space.tables().execute(
            "INSERT INTO ext_specs_specs (project, title, mode, status, language, content, \
             last_c, last_d, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 0, ?7, ?7)",
            params![
                project.name,
                title,
                word(mode),
                word(Status::Draft),
                language,
                serde_json::to_string(&Content::default())?,
                at
            ],
        )?;
        let id = space.tables().last_insert_rowid() as u64;
        let mut spec = spec_of(&space, id)?.expect("the spec just written");
        for ticket in &tickets {
            link_ticket(&space, &spec, ticket)?;
        }
        spec.tickets = tickets_of(&space, id)?;
        let event = space.append_event(
            "spec.created",
            json!({
                "spec": id,
                "project": spec.project,
                "title": spec.title,
                "mode": spec.mode,
                "tickets": spec.tickets,
            }),
        )?;
        space.commit()?;
        Ok((spec, event))
    })?;
    Ok(spec.to_json())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_identifier_follows_the_last_given_and_a_removed_one_is_never_taken_back() {
        let mut last = 0;
        let ids = numbered('C', vec![None, None, None], &[], &mut last).unwrap();
        assert_eq!(ids, ["C1", "C2", "C3"]);
        // C2 removed, a new one added.
        let ids = numbered(
            'C',
            vec![Some("C1".into()), Some("C3".into()), None],
            &["C1", "C2", "C3"],
            &mut last,
        )
        .unwrap();
        assert_eq!(ids, ["C1", "C3", "C4"]);
        let removed = numbered('C', vec![Some("C2".into())], &["C1", "C3", "C4"], &mut last);
        assert!(removed.unwrap_err().to_string().contains("C2 was removed"));
        let unknown = numbered('C', vec![Some("C9".into())], &["C1"], &mut last);
        assert!(
            unknown
                .unwrap_err()
                .to_string()
                .contains("not a C of this spec")
        );
        let twice = numbered(
            'C',
            vec![Some("C1".into()), Some("C1".into())],
            &["C1"],
            &mut last,
        );
        assert!(twice.unwrap_err().to_string().contains("twice"));
        assert_eq!(last, 4);
    }
}
