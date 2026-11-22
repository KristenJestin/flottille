//! A project's Routines tab (#1123, `docs/design.md`, "Routines"): the project's routines, each
//! with its trigger, its mode and its last run; a routine's page with its history; a run's result;
//! and the form that creates and edits a routine. This file holds what the daemon says
//! (`routine.list`, `routine.show`, `docs/protocol.md`, "The routine methods") and the words the
//! window says it in; the tab (`tab.rs`) draws it, the form (`form.rs`) edits it.

mod form;
mod tab;

pub use form::RoutineForm;
pub use tab::{RoutinesTab, Then};

use serde_json::Value;

use crate::session::{local_offset, said_at};
use crate::session_link::PullRequest;

/// A routine as the daemon gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct Routine {
    pub id: String,
    pub name: String,
    pub project: String,
    pub prompt: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// `weekdays 08:30`; none for a routine that runs on an event.
    pub schedule: Option<String>,
    pub zone: Option<String>,
    /// The event it runs on, instead of a schedule.
    pub event: Option<Event>,
    /// `read_only`, `ask` or `auto`.
    pub mode: String,
    /// `something`, `new` (only when an item is new) or `never`.
    pub rings: String,
    pub paused: bool,
    pub next_at: Option<u64>,
    pub delayed: Option<String>,
    /// How many results with something to say were not opened.
    pub unread: u64,
    /// Its last run, from `routine.list`.
    pub last: Option<Run>,
}

/// The event a routine runs on: its kind, and the fields of its kind (`docs/protocol.md`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Event {
    /// `workflow_run`, `review_requested`, `mentioned` or `assigned`.
    pub kind: String,
    pub repository: Option<String>,
    pub workflow: Option<String>,
    pub branch: Option<String>,
    /// `schedule`, `push`, `manual` or `any`.
    pub started_by: String,
    /// `failed` or `any`.
    pub outcome: String,
    pub tracker_project: Option<String>,
    pub ticket_type: Option<String>,
    /// `HH:MM`.
    pub not_before: Option<String>,
    /// `day` or `hour`.
    pub at_most_once: Option<String>,
    pub new_session: bool,
}

/// One run of a routine.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub id: u64,
    pub started_at: u64,
    pub ended_at: Option<u64>,
    pub session: Option<String>,
    /// `running`, `something`, `nothing`, `goes_on`, `skipped` or `failed`.
    pub outcome: String,
    pub summary: Option<String>,
    pub why: Option<String>,
    pub opened: bool,
    /// The list its result gave, in the agent's order.
    pub items: Vec<Item>,
    /// The ids of what the last run's list had that this one has not.
    pub left: Vec<String>,
    /// Markdown the result gave beside its items.
    pub body: Option<String>,
}

/// One item of a run's list: what the agent said, which comes from outside, so it is shown with
/// no control character.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: String,
    pub title: String,
    /// Opened by the system's handler when clicked, never fetched by the window.
    pub link: Option<String>,
    pub text: Option<String>,
    pub tags: Vec<String>,
    pub group: Option<String>,
    /// Not in the last run's list.
    pub new: bool,
}

impl Item {
    /// Where the link goes (its host and port), said on the row so that a title cannot pass for
    /// another place.
    pub fn host(&self) -> Option<&str> {
        self.link
            .as_deref()
            .and_then(flottille_core::address::web_authority)
    }
}

fn text(value: &Value) -> Option<String> {
    value.as_str().filter(|s| !s.is_empty()).map(str::to_owned)
}

/// What the agent or the daemon said at length, without an invisible character: a remote daemon
/// built before the daemon refused them could still send one, and a bidirectional override would
/// make the report read as another text. Its lines and its markdown stay.
fn unhidden(value: &Value) -> Option<String> {
    text(value)
        .map(|said| {
            said.chars()
                .filter(|c| !flottille_link::hidden(*c))
                .collect()
        })
        .filter(|said: &String| !said.is_empty())
}

impl Event {
    fn of(value: &Value) -> Option<Event> {
        let kind = text(&value["kind"])?;
        Some(Event {
            kind,
            repository: text(&value["repository"]),
            workflow: text(&value["workflow"]),
            branch: text(&value["branch"]),
            started_by: text(&value["started_by"]).unwrap_or_else(|| "any".into()),
            outcome: text(&value["outcome"]).unwrap_or_else(|| "any".into()),
            tracker_project: text(&value["tracker_project"]),
            ticket_type: text(&value["type"]),
            not_before: text(&value["not_before"]),
            at_most_once: text(&value["at_most_once"]),
            new_session: value["new_session"] == true,
        })
    }

    /// The object `routine.add` and `routine.edit` take: the fields of the kind, and those every
    /// kind has. A field left out is its default.
    pub fn to_json(&self) -> Value {
        let mut event = serde_json::Map::new();
        event.insert("kind".into(), self.kind.clone().into());
        let mut set = |key: &str, value: &Option<String>| {
            if let Some(value) = value {
                event.insert(key.into(), value.clone().into());
            }
        };
        match self.kind.as_str() {
            "workflow_run" => {
                set("repository", &self.repository);
                set("workflow", &self.workflow);
                set("branch", &self.branch);
                set("started_by", &Some(self.started_by.clone()));
                set("outcome", &Some(self.outcome.clone()));
            }
            "review_requested" => set("repository", &self.repository),
            "mentioned" => set("tracker_project", &self.tracker_project),
            "assigned" => {
                set("tracker_project", &self.tracker_project);
                set("type", &self.ticket_type);
            }
            _ => {}
        }
        set("not_before", &self.not_before);
        set("at_most_once", &self.at_most_once);
        if self.new_session {
            event.insert("new_session".into(), true.into());
        }
        Value::Object(event)
    }

    /// What starts the routine, in a line.
    pub fn said(&self) -> String {
        let detail: Vec<String> = match self.kind.as_str() {
            "workflow_run" => [
                self.workflow.clone(),
                self.branch.clone(),
                match self.started_by.as_str() {
                    "any" => None,
                    by => Some(format!("started by {by}")),
                },
            ]
            .into_iter()
            .flatten()
            .collect(),
            "review_requested" => self.repository.clone().into_iter().collect(),
            "mentioned" | "assigned" => [self.tracker_project.clone(), self.ticket_type.clone()]
                .into_iter()
                .flatten()
                .collect(),
            _ => Vec::new(),
        };
        let what = match (self.kind.as_str(), self.outcome.as_str()) {
            ("workflow_run", "failed") => "A workflow run fails",
            ("workflow_run", _) => "A workflow run ends",
            ("review_requested", _) => "A review is asked of you",
            ("mentioned", _) => "You are mentioned",
            ("assigned", _) => "A ticket is assigned to you",
            _ => "An event",
        };
        let mut line = what.to_owned();
        if !detail.is_empty() {
            line.push_str(&format!(" ({})", detail.join(", ")));
        }
        if let Some(hour) = &self.not_before {
            line.push_str(&format!(", not before {hour}"));
        }
        if let Some(per) = &self.at_most_once {
            line.push_str(&format!(", at most once a {per}"));
        }
        line
    }
}

/// A line of the agent's, with no control and no invisible character (a bidirectional override
/// would make it read as another text).
fn plain(value: &Value) -> Option<String> {
    text(value)
        .map(|line| {
            line.chars()
                .filter(|c| !c.is_control() && !flottille_link::hidden(*c))
                .collect::<String>()
        })
        .filter(|line| !line.trim().is_empty())
}

impl Item {
    fn of(value: &Value) -> Option<Item> {
        Some(Item {
            id: plain(&value["id"])?,
            title: plain(&value["title"])?,
            link: text(&value["link"]).filter(|link| {
                flottille_core::address::web_authority(link).is_some()
                    && !link
                        .chars()
                        .any(|c| c.is_control() || c.is_whitespace() || flottille_link::hidden(c))
            }),
            text: plain(&value["text"]),
            tags: value["tags"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(plain)
                .collect(),
            group: plain(&value["group"]),
            new: value["new"] == true,
        })
    }
}

impl Run {
    /// The items under their group headings, in the agent's order: a group stands where its first
    /// item does, and the items without one are listed as the first group is, with no heading.
    pub fn sections(&self) -> Vec<(Option<&str>, Vec<&Item>)> {
        let mut sections: Vec<(Option<&str>, Vec<&Item>)> = Vec::new();
        for item in &self.items {
            let group = item.group.as_deref();
            match sections.iter_mut().find(|(name, _)| *name == group) {
                Some((_, items)) => items.push(item),
                None => sections.push((group, vec![item])),
            }
        }
        sections
    }

    pub fn of(value: &Value) -> Option<Run> {
        Some(Run {
            id: value["id"].as_u64()?,
            started_at: value["started_at"].as_u64().unwrap_or_default(),
            ended_at: value["ended_at"].as_u64(),
            session: text(&value["session"]),
            outcome: text(&value["outcome"])?,
            summary: unhidden(&value["summary"]),
            why: unhidden(&value["why"]),
            opened: value["opened"] == true,
            items: value["items"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Item::of)
                .collect(),
            left: value["left"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|left| plain(&left["id"]))
                .collect(),
            body: unhidden(&value["body"]),
        })
    }

    /// Whether its result has something to say that was not opened.
    pub fn unread(&self) -> bool {
        matches!(self.outcome.as_str(), "something" | "goes_on") && !self.opened
    }

    /// Its outcome, said: `4 to review`, `Nothing`, `Skipped: why`, `Failed: why`, `Under way`.
    pub fn said(&self) -> String {
        let with = |word: &str, why: &Option<String>| match why {
            Some(why) => format!("{word}: {why}"),
            None => word.to_owned(),
        };
        match self.outcome.as_str() {
            "running" => "Under way".into(),
            "nothing" => "Nothing".into(),
            "skipped" => with("Skipped", &self.why),
            "failed" => with("Failed", &self.why),
            _ => self
                .summary
                .clone()
                .unwrap_or_else(|| "Something to say".into()),
        }
    }

    /// Whether its session is still its own: under way, or work that goes on.
    pub fn goes_on(&self) -> bool {
        self.outcome == "goes_on"
    }
}

impl Routine {
    pub fn of(value: &Value) -> Option<Routine> {
        Some(Routine {
            id: text(&value["id"])?,
            name: text(&value["name"])?,
            project: text(&value["project"]).unwrap_or_default(),
            prompt: text(&value["prompt"]).unwrap_or_default(),
            provider: text(&value["agent"]["provider"]),
            model: text(&value["agent"]["model"]),
            effort: text(&value["agent"]["effort"]),
            schedule: text(&value["schedule"]),
            zone: text(&value["zone"]),
            event: Event::of(&value["event"]),
            mode: text(&value["mode"]).unwrap_or_else(|| "read_only".into()),
            rings: text(&value["rings"]).unwrap_or_else(|| "something".into()),
            paused: value["paused"] == true,
            next_at: value["next_at"].as_u64(),
            delayed: text(&value["delayed"]),
            unread: value["unread"].as_u64().unwrap_or_default(),
            last: Run::of(&value["last"]),
        })
    }

    /// What starts it, in a line: its schedule, or its event.
    pub fn trigger(&self) -> String {
        match (&self.schedule, &self.event) {
            (Some(schedule), _) => schedule.clone(),
            (None, Some(event)) => event.said(),
            (None, None) => "No trigger".into(),
        }
    }

    /// The agent it runs on, in a line.
    pub fn agent(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        parts.push(match &self.provider {
            Some(provider) => provider.clone(),
            None => "The provider of a new session".into(),
        });
        parts.extend(self.model.clone());
        parts.extend(self.effort.clone());
        parts.join(" · ")
    }

    /// The sentence that says when it runs next.
    pub fn next(&self, now: u64) -> String {
        match (&self.event, self.paused, self.next_at) {
            (_, true, _) => "Paused: it does not run.".into(),
            (Some(event), false, _) => format!("Runs when it happens: {}.", event.said()),
            (None, false, Some(at)) => match &self.delayed {
                Some(why) => format!("Next: {} ({why})", next_said(at, now)),
                None => format!("Next: {}", next_said(at, now)),
            },
            (None, false, None) => "Not scheduled.".into(),
        }
    }
}

/// What Flottille knows of the session of a run whose work goes on, read live: its state, the
/// branch it works on, its pull requests with their checks, the requests that wait for the
/// user and what the agent last said.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    /// `Running`, `Waiting for you` or `Ended`.
    pub state: String,
    pub branch: Option<String>,
    pub pulls: Vec<PullRequest>,
    pub requests: usize,
    /// The agent's last message, one or two lines of it.
    pub last: Option<String>,
}

pub fn mode_said(mode: &str) -> &'static str {
    match mode {
        "ask" => "Ask",
        "auto" => "Auto",
        _ => "Read only",
    }
}

pub fn mode_line(mode: &str) -> &'static str {
    match mode {
        "ask" => "Every write is asked of you, as in a session in Ask mode.",
        "auto" => "The session decides what it may write, as in Auto mode.",
        _ => "Reads go through, every write is refused.",
    }
}

const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

/// The machine's day (days since the epoch) and second of the day at `ms`.
fn local_day(ms: u64) -> (i64, i64) {
    let seconds = (ms / 1000) as i64 + local_offset(ms);
    (seconds.div_euclid(86_400), seconds.rem_euclid(86_400))
}

/// 0 for Sunday: the 1st of January 1970 was a Thursday.
fn weekday_of(day: i64) -> usize {
    (day + 4).rem_euclid(7) as usize
}

/// When a next run is due, as `Monday 08:30` within the week and `21 Nov 08:30` beyond it, on the
/// machine's clock.
pub fn next_said(at: u64, now: u64) -> String {
    let (day, seconds) = local_day(at);
    let (today, _) = local_day(now);
    let clock = format!("{:02}:{:02}", seconds / 3600, seconds % 3600 / 60);
    match day - today {
        0..7 => format!("{} {clock}", WEEKDAYS[weekday_of(day)]),
        _ => {
            let said = said_at(at);
            let date = said.split(',').next().unwrap_or_default();
            let date: Vec<&str> = date.split(' ').take(2).collect();
            format!("{} {clock}", date.join(" "))
        }
    }
}

/// The days a schedule can name: its words as `routine.add` reads them, and as the form says
/// them.
pub const DAYS: [(&str, &str); 10] = [
    ("every day", "Every day"),
    ("weekdays", "Weekdays"),
    ("weekends", "Weekends"),
    ("mondays", "Mondays"),
    ("tuesdays", "Tuesdays"),
    ("wednesdays", "Wednesdays"),
    ("thursdays", "Thursdays"),
    ("fridays", "Fridays"),
    ("saturdays", "Saturdays"),
    ("sundays", "Sundays"),
];

fn day_allows(days: &str, weekday: usize) -> bool {
    match days {
        "every day" => true,
        "weekdays" => (1..=5).contains(&weekday),
        "weekends" => weekday == 0 || weekday == 6,
        other => WEEKDAYS
            .iter()
            .position(|name| other.eq_ignore_ascii_case(&format!("{}s", name.to_lowercase())))
            .is_some_and(|at| at == weekday),
    }
}

/// `08:30` read as an hour and a minute; none for anything else.
pub fn hour_of(typed: &str) -> Option<(i64, i64)> {
    let (hour, minute) = typed.trim().split_once(':')?;
    if hour.is_empty() || hour.len() > 2 || minute.len() != 2 {
        return None;
    }
    let (hour, minute) = (hour.parse::<i64>().ok()?, minute.parse::<i64>().ok()?);
    ((0..24).contains(&hour) && (0..60).contains(&minute)).then_some((hour, minute))
}

/// When `days` at `hour` next comes after `now`, on the machine's clock: what the form says
/// before the daemon keeps the routine (which says it again, in the routine's own zone).
pub fn next_run(days: &str, hour: (i64, i64), now: u64) -> Option<u64> {
    let (today, _) = local_day(now);
    (0..=7).find_map(|ahead| {
        let day = today + ahead;
        if !day_allows(days, weekday_of(day)) {
            return None;
        }
        let local = day * 86_400 + hour.0 * 3600 + hour.1 * 60;
        let guess = (local - local_offset(now)) * 1000;
        let at = (local - local_offset(guess.max(0) as u64)) * 1000;
        (at > now as i64).then_some(at as u64)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_routine_is_read_with_its_last_run() {
        let routine = Routine::of(&json!({
            "id": "a1", "name": "Tickets to review", "project": "app", "prompt": "List them",
            "agent": {"provider": null, "model": "opus", "effort": "high"},
            "schedule": "weekdays 08:30", "zone": null, "event": null, "mode": "read_only",
            "rings": "something", "paused": false, "next_at": 5, "delayed": null, "unread": 2,
            "last": {"id": 3, "started_at": 1, "ended_at": 2, "session": "s", "outcome": "something",
                "summary": "4 to review", "why": null, "opened": false},
        }))
        .unwrap();
        assert_eq!(routine.trigger(), "weekdays 08:30");
        assert_eq!(routine.unread, 2);
        assert_eq!(
            routine.agent(),
            "The provider of a new session · opus · high"
        );
        let last = routine.last.unwrap();
        assert_eq!(last.said(), "4 to review");
        assert!(last.unread());
    }

    #[test]
    fn an_event_is_said_and_sent_as_its_kind_has_it() {
        let event = Event::of(&json!({"kind": "workflow_run", "workflow": "nightly.yml",
            "branch": "dev", "started_by": "schedule", "outcome": "failed",
            "not_before": "08:00", "at_most_once": "day", "new_session": false}))
        .unwrap();
        assert_eq!(
            event.said(),
            "A workflow run fails (nightly.yml, dev, started by schedule), not before 08:00, \
             at most once a day"
        );
        assert_eq!(
            event.to_json(),
            json!({"kind": "workflow_run", "workflow": "nightly.yml", "branch": "dev",
                "started_by": "schedule", "outcome": "failed", "not_before": "08:00",
                "at_most_once": "day"})
        );
        // A field that is not the kind's is never sent.
        let review = Event {
            kind: "review_requested".into(),
            workflow: Some("nightly.yml".into()),
            ..Event::default()
        };
        assert_eq!(review.to_json(), json!({"kind": "review_requested"}));
    }

    #[test]
    fn a_run_says_what_came_of_it() {
        let run = |outcome: &str, why: Option<&str>| {
            Run::of(
                &json!({"id": 1, "started_at": 0, "outcome": outcome, "why": why,
                "summary": null, "opened": true}),
            )
            .unwrap()
        };
        assert_eq!(run("running", None).said(), "Under way");
        assert_eq!(run("nothing", None).said(), "Nothing");
        assert_eq!(
            run("skipped", Some("the last run still runs")).said(),
            "Skipped: the last run still runs"
        );
        assert_eq!(run("failed", Some("no result")).said(), "Failed: no result");
        assert!(!run("something", None).unread(), "an opened result is read");
    }

    #[test]
    fn the_items_of_a_result_are_listed_under_their_groups_in_the_agents_order() {
        let run = Run::of(&json!({"id": 1, "started_at": 0, "outcome": "something",
            "items": [
                {"id": "a", "title": "A", "group": "Blocking", "new": true},
                {"id": "b", "title": "B", "group": "Later"},
                {"id": "c", "title": "C"},
                {"id": "d", "title": "D", "group": "Blocking",
                 "link": "javascript:alert(1)", "text": "one\u{1b}[31m"},
            ],
            "left": [{"id": "z", "title": "Z"}]}))
        .unwrap();

        let sections = run.sections();

        let names: Vec<Option<&str>> = sections.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, [Some("Blocking"), Some("Later"), None]);
        let blocking: Vec<&str> = sections[0].1.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(blocking, ["a", "d"]);
        assert!(sections[0].1[0].new && !sections[0].1[1].new);
        assert_eq!(run.left, ["z"]);
        // What comes from outside: no link but the web's, no control character.
        assert_eq!(sections[0].1[1].link, None);
        assert_eq!(sections[0].1[1].text.as_deref(), Some("one[31m"));
    }

    #[test]
    fn what_an_agent_said_shows_no_invisible_character() {
        let item = Item::of(&json!({
            "id": "a\u{200B}b", "title": "ok \u{202E}gnp.exe", "text": "one\u{2028}two",
            "tags": ["\u{2067}x"], "group": "G\u{FEFF}",
        }))
        .unwrap();
        assert_eq!(item.id, "ab");
        assert_eq!(item.title, "ok gnp.exe");
        assert_eq!(item.text.as_deref(), Some("onetwo"));
        assert_eq!(item.tags, ["x"]);
        assert_eq!(item.group.as_deref(), Some("G"));
    }

    #[test]
    fn what_a_run_says_shows_no_invisible_character() {
        // A remote daemon built before the daemon refused them may still send one.
        let run = Run::of(&json!({
            "id": 1, "outcome": "failed", "summary": "done \u{202E}gnp.exe",
            "why": "no\u{2066}pe", "body": "# Title\n\u{2067}text\u{FE0D}",
        }))
        .unwrap();
        assert_eq!(run.summary.as_deref(), Some("done gnp.exe"));
        assert_eq!(run.why.as_deref(), Some("nope"));
        assert_eq!(run.body.as_deref(), Some("# Title\ntext"));
        assert_eq!(run.said(), "Failed: nope");
    }

    #[test]
    fn an_emoji_keeps_its_presentation_selector() {
        let said = "\u{26A0}\u{FE0F} 2 failing \u{2764}\u{FE0F} \u{2714}\u{FE0E}";
        let run = Run::of(&json!({
            "id": 1, "outcome": "something", "summary": said, "body": said,
            "items": [{"id": "a", "title": said, "tags": [said], "group": said}],
        }))
        .unwrap();
        assert_eq!(run.summary.as_deref(), Some(said));
        assert_eq!(run.body.as_deref(), Some(said));
        assert_eq!(run.items[0].title, said);
        assert_eq!(run.items[0].tags, [said]);
        assert_eq!(run.items[0].group.as_deref(), Some(said));
    }

    #[test]
    fn a_link_with_userinfo_is_no_link_and_a_row_says_the_host() {
        let of = |link: &str| Item::of(&json!({"id": "a", "title": "A", "link": link})).unwrap();
        assert_eq!(of("https://github.com@evil.example/x").link, None);
        assert_eq!(of("http://user:pw@host.example/").link, None);
        assert_eq!(of("https://a\u{202E}b.example/").link, None);
        let linked = of("https://github.com/org/repo/issues/1");
        assert_eq!(linked.host(), Some("github.com"));
        assert_eq!(
            of("https://host.example:8443/x").host(),
            Some("host.example:8443")
        );
        assert_eq!(
            Item::of(&json!({"id": "a", "title": "A"})).unwrap().host(),
            None
        );
    }

    #[test]
    fn an_hour_is_hours_and_minutes() {
        assert_eq!(hour_of("08:30"), Some((8, 30)));
        assert_eq!(hour_of("8:05"), Some((8, 5)));
        assert_eq!(hour_of("25:00"), None);
        assert_eq!(hour_of("08:60"), None);
        assert_eq!(hour_of("0830"), None);
    }

    #[test]
    fn the_next_run_follows_the_days_and_the_hour() {
        // Monday the 23rd of November 2026, 10:00 on the machine's clock, whatever its zone.
        let (days, _) = local_day(1_795_000_000_000);
        let monday = (0..7)
            .map(|d| days + d)
            .find(|d| weekday_of(*d) == 1)
            .unwrap();
        let now = ((monday * 86_400 + 10 * 3600) - local_offset(1_795_000_000_000)) as u64 * 1000;
        let said =
            |days: &str, hour: (i64, i64)| next_said(next_run(days, hour, now).unwrap(), now);
        assert_eq!(said("weekdays", (8, 30)), "Tuesday 08:30");
        assert_eq!(said("weekdays", (11, 0)), "Monday 11:00");
        assert_eq!(said("every day", (7, 0)), "Tuesday 07:00");
        // A week ahead is a date: the weekday alone would read as today.
        assert!(said("mondays", (9, 0)).starts_with(|c: char| c.is_ascii_digit()));
        assert_eq!(said("weekends", (10, 0)), "Saturday 10:00");
    }
}
