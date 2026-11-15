//! A project's Progress page (#801, `docs/tickets.md`, "Progress"): the open milestones of its
//! tracker, a row each with its stacked bar and its counts, which unfolds into its tickets
//! grouped, each with chips of what Flottille knows of it (the session on it, its pull request,
//! its checks); the closed ones behind a "Closed" fold. Everything comes from `project.progress`,
//! read again once the `item.*` events of the project go quiet, as the items' lists are, and the
//! sessions come from the shell's own read: the window never asks a forge or a tracker itself.
//!
//! What moves: the rows enter one after another, a milestone that comes or goes unfolds or folds
//! away with its line (`Block::present` of the kit's `Card`), a row unfolds and folds
//! (`Unfolding`), a ticket
//! that changes group folds away from one and unfolds in the other (`Grouped`), a group that gets
//! its first ticket unfolds in and one that loses its last folds away (`Group`), the parts of a
//! bar ease to their new share (`eased`), and the Closed fold opens and closes (`Disclosure`).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_design_system::color::mix;
use gpui_design_system::motion::{Motion, PANEL};
use gpui_design_system::{
    Block, Card, Chip, Disclosure, EmptyLine, Group, Grouped, Icon, Item, Listing, PageHeader,
    Skeleton, Type, Unfolding, awaiting, enter, hoverable, palette, presence_of, space,
    tipped_when_cut,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, ElementId, InteractiveElement, IntoElement, ParentElement, Pixels,
    Render, ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled, Task,
    TestSupportExt as _, WeakEntity, Window, div, px, relative,
};
use serde_json::Value;

use crate::actions::Command;
use crate::commands::perform_later;
use crate::glyphs;
use crate::model::Snapshot;
use crate::shell::{Selection, Shell};
use crate::state::AgentState;

/// How long the `item.*` events must stop before the progress is read again: a round says many
/// at once, and one read answers them all.
const QUIET: Duration = Duration::from_millis(300);

/// The most an unfolded milestone's tickets are tall: about ten lines. A longer list scrolls
/// inside it, so that the page never becomes meters long.
pub const TICKETS_HEIGHT: Pixels = px(400.);

/// The height of a milestone's bar.
const BAR: Pixels = px(6.);

/// A pull request on a ticket.
#[derive(Clone, Debug, PartialEq)]
pub struct Pull {
    pub repository: String,
    pub key: String,
    /// `open`, `draft`, `merged` or `closed`.
    pub state: String,
    /// Its checks as one mark: `passing`, `failing` or `pending`; none without.
    pub ci: Option<String>,
    pub url: Option<String>,
}

/// A ticket of a milestone, and what ties it to the work on it.
#[derive(Clone, Debug, PartialEq)]
pub struct Ticket {
    pub key: String,
    pub title: String,
    pub url: Option<String>,
    /// The workspaces linked to it: their sessions are the ones on it.
    pub workspaces: Vec<String>,
    pub pulls: Vec<Pull>,
}

/// A group of a milestone's tickets: a status of Jira's, or one of GitHub's four.
#[derive(Clone, Debug, PartialEq)]
pub struct TicketGroup {
    pub name: String,
    /// `new`, `indeterminate`, `review`, `done` or `other`.
    pub category: String,
    pub tickets: Vec<Ticket>,
}

/// How many tickets a milestone holds, by where they stand.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Counts {
    pub done: usize,
    pub review: usize,
    pub progress: usize,
    pub todo: usize,
    pub other: usize,
    pub total: usize,
}

/// A milestone as `project.progress` gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct Milestone {
    /// Unique in the project: `github-1`, `jira-ABC-10001`.
    pub id: String,
    pub name: String,
    /// `github` or `jira`.
    pub source: String,
    pub jira_project: Option<String>,
    pub open: bool,
    pub due: Option<String>,
    /// The day the tracker says it was closed (GitHub).
    pub closed_at: Option<String>,
    /// Whether its tickets were read: a released Jira version's are not.
    pub read: bool,
    /// Why they could not be read, for a version not released.
    pub problem: Option<String>,
    pub counts: Counts,
    pub groups: Vec<TicketGroup>,
}

impl Milestone {
    fn of(value: &Value) -> Self {
        let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
        let count = |key: &str| value["counts"][key].as_u64().unwrap_or(0) as usize;
        let groups = value["groups"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|group| TicketGroup {
                name: text(&group["name"]),
                category: text(&group["category"]),
                tickets: group["tickets"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|ticket| Ticket {
                        key: text(&ticket["key"]),
                        title: text(&ticket["title"]),
                        url: ticket["url"].as_str().map(str::to_owned),
                        workspaces: ticket["workspaces"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|w| w.as_str().map(str::to_owned))
                            .collect(),
                        pulls: ticket["pulls"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|pull| Pull {
                                repository: text(&pull["repository"]),
                                key: text(&pull["key"]),
                                state: text(&pull["state"]),
                                ci: pull["ci"].as_str().map(str::to_owned),
                                url: pull["url"].as_str().map(str::to_owned),
                            })
                            .collect(),
                    })
                    .collect(),
            })
            .collect();
        Self {
            id: text(&value["id"]),
            name: text(&value["name"]),
            source: text(&value["source"]),
            jira_project: value["jira_project"].as_str().map(str::to_owned),
            open: value["state"] != "closed",
            due: value["due"].as_str().map(str::to_owned),
            closed_at: value["closed_at"].as_str().map(str::to_owned),
            read: value["read"] != false,
            problem: value["problem"].as_str().map(str::to_owned),
            counts: Counts {
                done: count("done"),
                review: count("review"),
                progress: count("progress"),
                todo: count("todo"),
                other: count("other"),
                total: count("total"),
            },
            groups,
        }
    }

    fn jira(&self) -> bool {
        self.source == "jira"
    }

    /// Where it comes from and when it is due: `GitHub milestone · due 2026-12-05`.
    fn meta(&self) -> String {
        let from = match &self.jira_project {
            Some(project) if self.jira() => format!("Jira version of {project}"),
            _ if self.jira() => "Jira version".to_owned(),
            _ => "GitHub milestone".to_owned(),
        };
        let when = match (&self.due, self.open, self.jira()) {
            (Some(day), false, true) => format!("released {day}"),
            (Some(day), _, _) => format!("due {day}"),
            (None, _, _) => "no due date".to_owned(),
        };
        format!("{from} · {when}")
    }

    /// Its counts in words: `1 done · 1 in review · 1 in progress · 1 to do`; a Jira version has
    /// no review of its own (a pull request open is a chip there), and says its other tickets.
    fn counts_said(&self) -> String {
        let c = self.counts;
        if self.problem.is_some() {
            return "its tickets could not be read".to_owned();
        }
        if !self.read {
            return "its tickets are read while it is not released".to_owned();
        }
        let mut said = match self.jira() {
            true => format!(
                "{} done · {} in progress · {} to do",
                c.done, c.progress, c.todo
            ),
            false => format!(
                "{} done · {} in review · {} in progress · {} to do",
                c.done, c.review, c.progress, c.todo
            ),
        };
        if c.other > 0 {
            said.push_str(&format!(" · {} other", c.other));
        }
        said
    }
}

/// What `project.progress` answered.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Progress {
    pub tracker: String,
    pub milestones: Vec<Milestone>,
    /// Why the milestones could not be read, when they could not.
    pub problem: Option<String>,
}

impl Progress {
    pub fn of(value: &Value) -> Self {
        let mut milestones: Vec<Milestone> = value["milestones"]
            .as_array()
            .into_iter()
            .flatten()
            .map(Milestone::of)
            .collect();
        // The open ones the soonest due first, those with no due date after them; the closed
        // ones the last closed first: by the day the tracker says it closed them, else by their
        // due date (a Jira version's is its release).
        milestones.sort_by(|a, b| match (a.open, b.open) {
            (true, true) => (a.due.is_none(), &a.due).cmp(&(b.due.is_none(), &b.due)),
            (false, false) => b
                .closed_at
                .as_ref()
                .or(b.due.as_ref())
                .cmp(&a.closed_at.as_ref().or(a.due.as_ref())),
            (open, _) => open.cmp(&b.open).reverse(),
        });
        Self {
            tracker: value["tracker"].as_str().unwrap_or_default().to_owned(),
            milestones,
            problem: value["problem"].as_str().map(str::to_owned),
        }
    }
}

/// A session working on a ticket, as the shell's read says it.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionChip {
    pub id: String,
    pub title: String,
    /// `running`, `waits for you`, `waiting`, `at rest` or `ended`.
    pub state: &'static str,
    /// When it ended, for the one that ended last.
    pub ended_at: u64,
}

/// The chips a workspace shows: the sessions that have not ended, and of the ended ones the last
/// alone: a ticket worked on for weeks does not list every session it ever had.
pub fn worth_showing(mut chips: Vec<SessionChip>) -> Vec<SessionChip> {
    let last_ended = chips
        .iter()
        .filter(|c| c.state == "ended")
        .map(|c| c.ended_at)
        .max();
    let mut kept_ended = false;
    chips.retain(|chip| match chip.state {
        "ended" if kept_ended || Some(chip.ended_at) != last_ended => false,
        "ended" => {
            kept_ended = true;
            true
        }
        _ => true,
    });
    chips
}

/// The sessions of `project` by the workspace they work in.
pub fn sessions_of(snapshot: &Snapshot, project: &str) -> HashMap<String, Vec<SessionChip>> {
    let mut by_place: HashMap<String, Vec<SessionChip>> = HashMap::new();
    for session in snapshot.sessions.iter().filter(|s| s.project == project) {
        let Some(place) = &session.place else {
            continue;
        };
        let state = match (session.ended, session.mark) {
            (true, _) => "ended",
            (_, Some(AgentState::Running)) => "running",
            (_, Some(AgentState::NeedsYou)) => "waits for you",
            (_, Some(AgentState::Waiting)) => "waiting",
            _ => "at rest",
        };
        by_place
            .entry(place.clone())
            .or_default()
            .push(SessionChip {
                id: session.id.clone(),
                title: session.title.clone(),
                state,
                ended_at: session.ended_at,
            });
    }
    by_place
        .into_iter()
        .map(|(place, chips)| (place, worth_showing(chips)))
        .collect()
}

/// The workspaces of `project`, by name: one linked to a ticket, or unlinked, is read again.
pub fn workspaces_of(snapshot: &Snapshot, project: &str) -> Vec<String> {
    snapshot
        .workspaces
        .iter()
        .filter(|w| w.project == project)
        .map(|w| w.name.clone())
        .collect()
}

pub struct ProgressPage {
    shell: WeakEntity<Shell>,
    project: String,
    asked: u64,
    listed_at: u64,
    read: Option<Result<Progress, String>>,
    /// The milestones unfolded, by id, and since when the others fold away.
    opened: HashSet<String>,
    folding: HashMap<String, Instant>,
    closed_open: bool,
    /// The milestones the lists were synced with once, and those that arrived since: a card's
    /// block that is first drawn present is simply there, so an arrival is drawn absent for one
    /// frame first and unfolds.
    drawn: bool,
    known: HashSet<String>,
    arriving: HashSet<String>,
    open_rows: Listing<()>,
    closed_rows: Listing<()>,
    /// The tickets of each milestone in their groups, by its id.
    grouped: HashMap<String, Grouped<String, Ticket>>,
    scrolls: HashMap<String, ScrollHandle>,
    sessions: HashMap<String, Vec<SessionChip>>,
    workspaces: Option<Vec<String>>,
    stale: bool,
    quiet: Option<Task<()>>,
    narrow: bool,
}

impl ProgressPage {
    pub fn new(shell: WeakEntity<Shell>, project: String, _: &mut Context<Self>) -> Self {
        Self {
            shell,
            project,
            asked: 0,
            listed_at: 0,
            read: None,
            opened: HashSet::new(),
            folding: HashMap::new(),
            closed_open: false,
            drawn: false,
            known: HashSet::new(),
            arriving: HashSet::new(),
            open_rows: Listing::default(),
            closed_rows: Listing::default(),
            grouped: HashMap::new(),
            scrolls: HashMap::new(),
            sessions: HashMap::new(),
            workspaces: None,
            stale: false,
            quiet: None,
            narrow: false,
        }
    }

    pub fn project(&self) -> &str {
        &self.project
    }

    pub fn set_narrow(&mut self, narrow: bool, cx: &mut Context<Self>) {
        if self.narrow != narrow {
            self.narrow = narrow;
            cx.notify();
        }
    }

    /// Asks the daemon for the progress.
    pub fn read(&mut self, cx: &mut App) {
        self.asked += 1;
        let (project, asked) = (self.project.clone(), self.asked);
        perform_later(&self.shell, Command::Progress { project, asked }, cx);
    }

    /// An event says the project's items changed: read again once the events go quiet.
    pub fn changed(&mut self, cx: &mut Context<Self>) {
        self.stale = true;
        self.quiet = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(QUIET).await;
            this.update(cx, |this, cx| {
                this.quiet = None;
                if std::mem::take(&mut this.stale) {
                    this.read(cx);
                }
            })
            .ok();
        }));
    }

    /// What the daemon answered to the read `asked`: dropped when a later one was applied.
    pub fn listed(&mut self, asked: u64, read: Result<Progress, String>, cx: &mut Context<Self>) {
        if asked < self.listed_at {
            return;
        }
        self.listed_at = asked;
        if self.read.as_ref() != Some(&read) {
            self.read = Some(read);
            cx.notify();
        }
    }

    /// The sessions of the project by workspace, and its workspaces, as the shell read them: a
    /// workspace made or removed may tie a ticket to new work, so the progress is read again.
    pub fn set_sessions(
        &mut self,
        sessions: HashMap<String, Vec<SessionChip>>,
        workspaces: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        if self.workspaces.as_ref().is_some_and(|w| *w != workspaces) {
            self.changed(cx);
        }
        self.workspaces = Some(workspaces);
        if self.sessions != sessions {
            self.sessions = sessions;
            cx.notify();
        }
    }

    pub fn is_read(&self) -> bool {
        self.read.is_some()
    }

    fn milestones(&self) -> &[Milestone] {
        match &self.read {
            Some(Ok(progress)) => &progress.milestones,
            _ => &[],
        }
    }

    /// The names of the open milestones, in the order shown.
    pub fn open_names(&self) -> Vec<String> {
        self.milestones()
            .iter()
            .filter(|m| m.open)
            .map(|m| m.name.clone())
            .collect()
    }

    /// The names of the closed milestones, in the order shown in their fold.
    pub fn closed_names(&self) -> Vec<String> {
        self.milestones()
            .iter()
            .filter(|m| !m.open)
            .map(|m| m.name.clone())
            .collect()
    }

    /// The groups of the milestone `id` and the keys of their tickets, as last read.
    pub fn groups_of(&self, id: &str) -> Vec<(String, Vec<String>)> {
        self.milestones()
            .iter()
            .find(|m| m.id == id)
            .map(|m| {
                m.groups
                    .iter()
                    .filter(|g| !g.tickets.is_empty())
                    .map(|g| {
                        (
                            g.name.clone(),
                            g.tickets.iter().map(|t| t.key.clone()).collect(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// How many milestones the page keeps tickets' groups and a scroll for.
    pub fn kept(&self) -> (usize, usize) {
        (self.grouped.len(), self.scrolls.len())
    }

    fn toggle(&mut self, id: String, open: bool, now: Instant, cx: &mut Context<Self>) {
        match open {
            true => {
                self.folding.remove(&id);
                self.opened.insert(id);
            }
            false => {
                self.opened.remove(&id);
                self.folding.insert(id, now);
            }
        }
        cx.notify();
    }

    /// The groups of each milestone drawn are brought to what was read.
    fn sync(&mut self, now: Instant, reduce: bool) {
        let milestones: Vec<Milestone> = self.milestones().to_vec();
        let ids: HashSet<String> = milestones.iter().map(|m| m.id.clone()).collect();
        // What the tracker no longer lists leaves nothing of its own behind.
        self.grouped.retain(|id, _| ids.contains(id));
        self.scrolls.retain(|id, _| ids.contains(id));
        self.opened.retain(|id| ids.contains(id));
        self.folding.retain(|id, _| ids.contains(id));
        if self.read.as_ref().is_some_and(Result::is_ok) {
            if self.drawn {
                self.arriving
                    .extend(ids.iter().filter(|id| !self.known.contains(*id)).cloned());
            }
            self.known = ids;
        }
        let open = milestones
            .iter()
            .filter(|m| m.open)
            .map(|m| (m.id.clone(), ()));
        self.open_rows.sync(open, now, reduce);
        let closed = milestones
            .iter()
            .filter(|m| !m.open)
            .map(|m| (m.id.clone(), ()));
        self.closed_rows.sync(closed, now, reduce);
        for milestone in &milestones {
            let grouped = self
                .grouped
                .entry(milestone.id.clone())
                .or_insert_with(|| Grouped::new(format!("progress-{}", milestone.id)));
            let names: Vec<String> = milestone.groups.iter().map(|g| g.name.clone()).collect();
            let rows: Vec<(String, String, Ticket)> = milestone
                .groups
                .iter()
                .flat_map(|g| {
                    g.tickets
                        .iter()
                        .map(|t| (t.key.clone(), g.name.clone(), t.clone()))
                })
                .collect();
            grouped.sync(names, rows, now, reduce);
        }
        self.folding
            .retain(|_, since| !reduce && now.saturating_duration_since(*since) < PANEL + PANEL);
    }

    /// One milestone's row: its line, which unfolds into its tickets.
    fn milestone_row(
        &self,
        milestone: &Milestone,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = palette(cx);
        let id = format!("progress-milestone-{}", milestone.id);
        let part = |name: &str| SharedString::from(format!("{id}-{name}"));
        let name = div()
            .id(part("name"))
            .aria_label(SharedString::from(milestone.name.clone()))
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .medium()
            .text_color(p.text)
            .child(SharedString::from(milestone.name.clone()));
        let meta = milestone.meta();
        let line = div()
            .w_full()
            .flex()
            .items_center()
            .gap(space::MD)
            .py(space::SM)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(tipped_when_cut(
                        name,
                        SharedString::from(milestone.name.clone()),
                        window,
                        cx,
                    ))
                    .child(
                        div()
                            .id(part("meta"))
                            .aria_label(SharedString::from(meta.clone()))
                            .text_size(px(12.))
                            .text_color(p.faint)
                            .child(SharedString::from(meta))
                            .test_support(),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .w(space::XL * 7.)
                    .child(bar(&id, milestone, window, cx)),
            )
            .child(
                div()
                    .id(part("counts"))
                    .flex_none()
                    .w(space::XL * 6.)
                    .aria_label(SharedString::from(milestone.counts_said()))
                    .text_size(px(12.))
                    .text_color(p.mute)
                    .whitespace_normal()
                    .child(SharedString::from(milestone.counts_said()))
                    .test_support(),
            );
        let open = self.opened.contains(&milestone.id);
        let drawn = open || self.folding.contains_key(&milestone.id);
        let body = match drawn {
            true => self.tickets(milestone, window, cx),
            false => div().into_any_element(),
        };
        let this = cx.entity().downgrade();
        let key = milestone.id.clone();
        Unfolding::new(SharedString::from(id), open)
            .line(line)
            .body(body)
            .on_toggle(move |open, _, cx| {
                let (key, now) = (key.clone(), cx.background_executor().now());
                this.update(cx, |this, cx| this.toggle(key, open, now, cx))
                    .ok();
            })
            .into_any_element()
    }

    /// The tickets of a milestone in their groups, in a frame that scrolls past its height.
    fn tickets(
        &self,
        milestone: &Milestone,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = palette(cx);
        let id = format!("progress-milestone-{}", milestone.id);
        let Some(grouped) = self.grouped.get(&milestone.id) else {
            return div().into_any_element();
        };
        let mut groups: Vec<AnyElement> = Vec::new();
        for (index, rows) in grouped.groups().enumerate() {
            let category = milestone
                .groups
                .iter()
                .find(|g| &g.name == rows.group)
                .map(|g| g.category.clone())
                .unwrap_or_default();
            let lines: Vec<AnyElement> = rows
                .rows()
                .map(|row| {
                    let line = self.ticket_line(&row.id(), row.value, window, cx);
                    Item::new(row.id(), row.placement, line).into_any_element()
                })
                .collect();
            let mut group = Group::new(
                SharedString::from(format!("progress-{}-group-{index}", milestone.id)),
                rows.group.clone(),
                rows.len(),
            );
            if category == "other" {
                group = group.sentence("Jira gave no status category for these");
            }
            groups.push(group.children(lines).into_any_element());
        }
        let empty = (milestone.counts.total == 0).then(|| match &milestone.problem {
            Some(why) => SharedString::from(format!("Its tickets could not be read: {why}")),
            None => SharedString::from(match milestone.read {
                true => "No ticket in it yet.",
                false => "Its tickets are not read once it is released.",
            }),
        });
        let scroll = self.scrolls.get(&milestone.id).cloned().unwrap_or_default();
        div()
            .id(SharedString::from(format!("{id}-tickets")))
            .max_h(TICKETS_HEIGHT)
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .flex()
            .flex_col()
            .pb(space::SM)
            .children(groups)
            .child(presence_of(
                SharedString::from(format!("{id}-empty")),
                empty,
                move |said| {
                    div()
                        .px(space::SM)
                        .py(space::XS)
                        .text_size(px(12.))
                        .text_color(p.faint)
                        .child(said)
                        .into_any_element()
                },
            ))
            .test_support()
            .into_any_element()
    }

    /// A ticket's line: its key, its title and the chips of what Flottille knows of it.
    fn ticket_line(
        &self,
        row: &ElementId,
        ticket: &Ticket,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = palette(cx);
        let part = |name: &str| -> SharedString {
            match row {
                ElementId::Name(text) => format!("{text}-{name}").into(),
                _ => name.to_owned().into(),
            }
        };
        let key = match ticket.key.parse::<u64>() {
            Ok(_) => format!("#{}", ticket.key),
            Err(_) => ticket.key.clone(),
        };
        // The key opens the ticket where the tracker keeps it, as the chips open theirs.
        let (shell, url) = (self.shell.clone(), ticket.url.clone());
        let opens = url.is_some();
        let key_cell = div()
            .id(part("key"))
            .flex_none()
            .w(space::XXL + space::LG)
            .aria_label(SharedString::from(key.clone()))
            .mono(11.5)
            .when(opens, |d| d.cursor_pointer())
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                if let (Some(shell), Some(url)) = (shell.upgrade(), &url) {
                    shell.read(cx).open_link(url);
                }
            })
            .child(SharedString::from(key));
        let key_cell = hoverable(key_cell, move |d, t| {
            d.text_color(mix(p.faint, if opens { p.text } else { p.faint }, t))
        });
        let title = div()
            .id(part("title"))
            .aria_label(SharedString::from(ticket.title.clone()))
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_color(p.text)
            .child(SharedString::from(ticket.title.clone()));
        let mut chips: Vec<AnyElement> = Vec::new();
        for workspace in &ticket.workspaces {
            for session in self.sessions.get(workspace).into_iter().flatten() {
                // Through the shell alone: the page is not being updated while the shell opens
                // another one in its place.
                let (shell, id) = (self.shell.clone(), session.id.clone());
                chips.push(
                    Chip::new(
                        part(&format!("session-{}", session.id)),
                        glyphs::AGENTS,
                        SharedString::from(format!("Session {}", session.title)),
                    )
                    .faint(SharedString::from(format!("· {}", session.state)))
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        let selection = Selection::Session(id.clone());
                        shell
                            .update(cx, |shell, cx| shell.open(selection, window, cx))
                            .ok();
                    })
                    .into_any_element(),
                );
            }
        }
        for pull in &ticket.pulls {
            let named = format!("{}-{}", pull.repository, pull.key);
            let (said, part_name) = match pull.state.as_str() {
                "merged" => (format!("PR #{} merged", pull.key), "merged"),
                "closed" => (format!("PR #{} closed", pull.key), "pull"),
                "draft" => (format!("PR #{} draft", pull.key), "pull"),
                _ => (format!("PR #{}", pull.key), "pull"),
            };
            let glyph = match pull.state.as_str() {
                "merged" => Icon::Check,
                _ => glyphs::PULL_REQUEST,
            };
            let (shell, url) = (self.shell.clone(), pull.url.clone());
            chips.push(
                Chip::new(
                    part(&format!("{part_name}-{named}")),
                    glyph,
                    SharedString::from(said),
                )
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    if let (Some(shell), Some(url)) = (shell.upgrade(), &url) {
                        shell.read(cx).open_link(url);
                    }
                })
                .into_any_element(),
            );
            let open = matches!(pull.state.as_str(), "open" | "draft");
            if let (true, Some(ci)) = (open, &pull.ci) {
                let (glyph, said) = match ci.as_str() {
                    "failing" => (Icon::Warning, "checks failed"),
                    "pending" => (Icon::Running, "checks running"),
                    _ => (Icon::Check, "checks passed"),
                };
                let (shell, url) = (self.shell.clone(), pull.url.clone());
                chips.push(
                    Chip::new(part(&format!("checks-{named}")), glyph, said)
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            if let (Some(shell), Some(url)) = (shell.upgrade(), &url) {
                                shell.read(cx).open_link(&format!("{url}/checks"));
                            }
                        })
                        .into_any_element(),
                );
            }
        }
        div()
            .id(part("line"))
            .w_full()
            .flex()
            .items_center()
            .gap(space::SM)
            .px(space::SM)
            .py(space::XS)
            .border_t_1()
            .border_color(p.line)
            .child(key_cell)
            .child(div().flex_1().min_w_0().child(tipped_when_cut(
                title,
                SharedString::from(ticket.title.clone()),
                window,
                cx,
            )))
            .child(
                div()
                    .flex_none()
                    .max_w(relative(0.5))
                    .flex()
                    .flex_wrap()
                    .justify_end()
                    .gap(space::XS)
                    .children(chips),
            )
            .test_support()
            .into_any_element()
    }

    /// The parts of the bar and what they stand for.
    fn legend(&self, jira: bool, other: bool, cx: &App) -> AnyElement {
        let p = palette(cx);
        let mut parts = vec![("done", "done", p.ok)];
        if !jira {
            parts.push(("review", "in review", p.pend));
        }
        parts.push(("progress", "in progress", p.run));
        if other {
            parts.push(("other", "other", p.faint));
        }
        parts.push(("todo", "to do", p.card2));
        div()
            .id("progress-legend")
            .flex()
            .flex_wrap()
            .gap(space::MD)
            .text_size(px(12.))
            .text_color(p.faint)
            .children(parts.into_iter().map(|(key, said, colour)| {
                div()
                    .id(SharedString::from(format!("progress-legend-{key}")))
                    .aria_label(said)
                    .flex()
                    .items_center()
                    .gap(space::XS)
                    .child(div().size(px(8.)).rounded(px(2.)).bg(colour))
                    .child(said)
                    .test_support()
            }))
            .test_support()
            .into_any_element()
    }

    /// The rows of `rows`, in one card of the kit: a block each, which unfolds when its milestone
    /// comes and folds away when it goes, with the line above it, drawn only between two blocks.
    fn card(
        &self,
        id: &str,
        rows: Vec<(String, gpui_design_system::Placement)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let milestones: Vec<Milestone> = self.milestones().to_vec();
        let mut card = Card::new().id(SharedString::from(id.to_owned()));
        for (key, placement) in rows {
            let Some(milestone) = milestones.iter().find(|m| m.id == key) else {
                continue;
            };
            let present = !placement.is_gone() && !self.arriving.contains(&key);
            let block = Block::new()
                .present(SharedString::from(format!("progress-item-{key}")), present)
                .child(
                    div()
                        .px(space::XS)
                        .child(self.milestone_row(milestone, window, cx)),
                );
            card = card.block(block);
        }
        div()
            .id(SharedString::from(id.to_owned()))
            .w_full()
            .child(card)
            .test_support()
            .into_any_element()
    }
}

/// A milestone's stacked bar: done, in review, in progress, each its colour, over the track that
/// stands for what is still to do; each part eases to its new share when a ticket moves.
fn bar(id: &str, milestone: &Milestone, window: &mut Window, cx: &mut App) -> AnyElement {
    let p = palette(cx);
    let c = milestone.counts;
    let share = |n: usize| match c.total {
        0 => 0.,
        total => n as f32 / total as f32,
    };
    let mut parts = vec![
        ("done", share(c.done), p.ok),
        ("review", share(c.review), p.pend),
        ("progress", share(c.progress), p.run),
    ];
    // Jira's tickets whose status category Flottille cannot read: a colour of their own, not the
    // track's (to do).
    if milestone.jira() {
        parts.push(("other", share(c.other), p.faint));
    }
    let mut track = div()
        .id(SharedString::from(format!("{id}-bar")))
        .w_full()
        .h(BAR)
        .rounded(BAR / 2.)
        .bg(p.card2)
        .overflow_hidden()
        .flex();
    for (part, target, colour) in parts {
        let part_id = ElementId::Name(format!("{id}-bar-{part}").into());
        let now = eased(part_id.clone(), target, window, cx);
        track = track.child(
            div()
                .id(part_id)
                .flex_none()
                .h_full()
                .w(relative(now))
                .bg(colour)
                .test_support(),
        );
    }
    track.test_support().into_any_element()
}

/// A number that moves to `target` over a panel's time whenever it changes: `id` keeps it, and a
/// first value is simply there (the page's entrance shows it).
fn eased(id: ElementId, target: f32, window: &mut Window, cx: &mut App) -> f32 {
    let key = ElementId::NamedChild(Arc::new(id), "eased".into());
    let state = window.use_keyed_state(key, cx, move |_, _| Motion::settled(target));
    let now = cx.background_executor().now();
    let reduce = cx.reduce_motion();
    let motion = state.update(cx, |motion, _| {
        if motion.target() != target {
            motion.animate(target, PANEL, now, reduce);
        }
        *motion
    });
    if motion.is_running(now) {
        window.request_animation_frame();
    }
    motion.value(now)
}

impl Render for ProgressPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let (now, reduce) = (cx.background_executor().now(), cx.reduce_motion());
        self.sync(now, reduce);
        for milestone in self.milestones().to_vec() {
            self.scrolls.entry(milestone.id.clone()).or_default();
        }
        // Rows that arrived are drawn absent this frame and unfold on the next.
        if !self.arriving.is_empty() {
            window.request_animation_frame();
        }
        let pad = if self.narrow { space::LG } else { space::XXL };
        let key = |part: &str| ElementId::Name(format!("progress-{}-{part}", self.project).into());
        let jira = matches!(&self.read, Some(Ok(progress)) if progress.tracker == "jira");
        let other = self
            .milestones()
            .iter()
            .any(|m| m.open && m.counts.other > 0);
        let header = PageHeader::new("progress-header", "Progress").path(match jira {
            true => format!("{} · Jira versions", self.project),
            false => format!("{} · GitHub milestones", self.project),
        });
        let pending = self.read.is_none();
        let (problem, failed) = match &self.read {
            Some(Err(why)) => (Some(SharedString::from(why.clone())), true),
            Some(Ok(progress)) => (progress.problem.clone().map(SharedString::from), false),
            None => (None, false),
        };
        let open_rows: Vec<(String, gpui_design_system::Placement)> = self
            .open_rows
            .rows()
            .map(|row| (row.key.to_owned(), row.placement))
            .collect();
        let closed_rows: Vec<(String, gpui_design_system::Placement)> = self
            .closed_rows
            .rows()
            .map(|row| (row.key.to_owned(), row.placement))
            .collect();
        let closed_count = self.closed_rows.len();
        let open_count = self.open_rows.len();
        let list = match open_count > 0 || !open_rows.is_empty() {
            true => self.card("progress-open", open_rows, window, cx),
            false if pending || failed || problem.is_some() => div().into_any_element(),
            false => EmptyLine::new(
                "progress-none",
                glyphs::MILESTONE,
                "No open milestone: make one on the tracker and put tickets in it, and its \
                 progress shows here.",
            )
            .into_any_element(),
        };
        let closed_list = match closed_rows.is_empty() {
            true => div().into_any_element(),
            false => self.card("progress-closed-list", closed_rows, window, cx),
        };
        let this = cx.entity().downgrade();
        let closed = (closed_count > 0).then(|| {
            Disclosure::new(
                "progress-closed",
                format!("{closed_count} closed"),
                self.closed_open,
            )
            .summary("milestones done or released")
            .body(div().pt(space::SM).child(closed_list))
            .on_toggle(move |open, _, cx| {
                this.update(cx, |this, cx| {
                    this.closed_open = open;
                    cx.notify();
                })
                .ok();
            })
        });
        let content = div()
            .flex()
            .flex_col()
            .gap(space::LG)
            .child(enter(key("legend"), 1, self.legend(jira, other, cx)))
            .child(enter(key("list"), 2, list))
            .children(closed.map(|closed| enter(key("closed"), 3, closed)))
            .child(
                presence_of(key("problem"), problem, move |why| {
                    div()
                        .id("progress-problem")
                        .aria_label(why.clone())
                        .text_size(px(12.5))
                        .text_color(p.fail)
                        .child(why)
                        .test_support()
                        .into_any_element()
                })
                .gap(space::LG),
            );
        let column = div()
            .max_w(space::COLUMN + pad * 2.)
            .w_full()
            .mx_auto()
            .px(pad)
            .pt(space::XXL)
            .pb(space::XXXL)
            .flex()
            .flex_col()
            .gap(space::XXL)
            .child(enter(key("header"), 0, header))
            .child(awaiting(
                key("wait"),
                pending,
                Skeleton::new("progress-skeleton").h(px(56.)),
                content,
            ));
        let page = div()
            .id("progress-page")
            .size_full()
            .overflow_y_scroll()
            .child(column)
            .test_support();
        self.drawn |= self.read.as_ref().is_some_and(Result::is_ok);
        self.arriving.clear();
        page
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chip(id: &str, state: &'static str, ended_at: u64) -> SessionChip {
        SessionChip {
            id: id.to_owned(),
            title: id.to_owned(),
            state,
            ended_at,
        }
    }

    #[test]
    fn a_workspace_shows_its_live_sessions_and_the_last_ended_one_alone() {
        let shown = worth_showing(vec![
            chip("old", "ended", 10),
            chip("running", "running", 0),
            chip("last", "ended", 30),
            chip("waiting", "waiting", 0),
            chip("older", "ended", 20),
            chip("needs", "waits for you", 0),
        ]);
        let ids: Vec<&str> = shown.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["running", "last", "waiting", "needs"]);
    }

    #[test]
    fn a_workspace_whose_sessions_all_ended_shows_the_last_of_them() {
        let shown = worth_showing(vec![chip("a", "ended", 5), chip("b", "ended", 5)]);
        assert_eq!(shown.len(), 1);
    }
}
