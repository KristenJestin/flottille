//! A project's pull requests and tickets (`docs/items.md`), as the daemon keeps them: the project
//! page's "Pull requests and tickets", two tabs, each a list with its filters (open, closed or
//! all, the user's own, a repository), and the detail of one in a dialog. Everything comes from
//! `items.list` and `items.show`, read again when an `item.*` event says the project's changed:
//! the window never asks a forge or a tracker itself. A row unfolds when it comes, folds away when
//! it goes (a filter, a round), and moves to its place.

use std::collections::HashMap;

use flottille_ui::{
    AgentState, Button, EmptyLine, Icon, LabelledLine, ListCard, ListRow, Listing, Modal, Section,
    Segmented, Select, SelectOption, StateIcon, Status, Switch, Tabs, Type, enter, hoverable, icon,
    palette, presence, presence_of, space,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, TestSupportExt as _,
    WeakEntity, Window, div, px,
};
use serde_json::Value;

use crate::actions::Command;
use crate::commands::perform_later;
use crate::model::ago;
use crate::project::repository_label;
use crate::project_settings::Reveal;
use crate::session::MarkdownText;
use crate::shell::{Shell, now_ms};

/// Which list a tab shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Pulls,
    Tickets,
}

impl Kind {
    /// The kind of an item, as the daemon says it.
    fn word(self) -> &'static str {
        match self {
            Kind::Pulls => "pull",
            Kind::Tickets => "ticket",
        }
    }

    fn index(self) -> usize {
        match self {
            Kind::Pulls => 0,
            Kind::Tickets => 1,
        }
    }
}

/// Which states the lists show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    Open,
    Closed,
    All,
}

const SHOWN: [(&str, &str); 3] = [("open", "Open"), ("closed", "Closed"), ("all", "All")];

/// A pull request or a ticket, as `items.list` gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct ItemRow {
    /// `pull` or `ticket`.
    pub kind: String,
    pub repository: String,
    /// Its number, or its Jira key.
    pub key: String,
    pub title: String,
    /// `open`, `draft`, `merged` or `closed`.
    pub state: String,
    /// A Jira work item's own status (`In Progress`).
    pub status: Option<String>,
    pub author: Option<String>,
    pub assignees: Vec<String>,
    pub url: Option<String>,
    /// When the forge says it last changed, RFC 3339 in UTC.
    pub updated_at: Option<String>,
    pub mine: bool,
    /// A pull request's checks as one mark: `passing`, `failing` or `pending`; none without.
    pub ci: Option<String>,
}

impl ItemRow {
    pub fn of(value: &Value) -> Self {
        let text = |key: &str| value[key].as_str().unwrap_or_default().to_owned();
        let names = |key: &str| {
            value[key]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|name| name.as_str().map(str::to_owned))
                .collect()
        };
        Self {
            kind: text("kind"),
            repository: text("repository"),
            key: text("key"),
            title: text("title"),
            state: text("state"),
            status: value["details"]["status"].as_str().map(str::to_owned),
            author: value["author"].as_str().map(str::to_owned),
            assignees: names("assignees"),
            url: value["url"].as_str().map(str::to_owned),
            updated_at: value["updated_at"].as_str().map(str::to_owned),
            mine: value["mine"] == true,
            ci: ci_of(&value["checks"]),
        }
    }

    /// Its row's element id, the same while it is the same item.
    pub fn id(&self) -> String {
        let slug: String = format!("{}-{}-{}", self.kind, self.repository, self.key)
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        format!("item-{slug}")
    }

    /// `#12`, or a Jira key as it is.
    pub fn label(&self) -> String {
        match self.key.bytes().all(|b| b.is_ascii_digit()) {
            true => format!("#{}", self.key),
            false => self.key.clone(),
        }
    }

    fn closed(&self) -> bool {
        matches!(self.state.as_str(), "merged" | "closed")
    }

    /// Its state in words: a Jira status when it has one.
    fn state_said(&self) -> String {
        self.status.clone().unwrap_or_else(|| self.state.clone())
    }

    /// The labelled values of its row: its number, its state, who opened it, who it is assigned
    /// to, its repository when the project has several, and when it last changed.
    fn facts(&self, project: &str, repositories: usize, now: u64) -> Vec<(&'static str, String)> {
        let mut facts = vec![("", self.label()), ("State", self.state_said())];
        if let Some(author) = &self.author {
            facts.push(("Author", author.clone()));
        }
        if !self.assignees.is_empty() {
            facts.push(("Assignee", self.assignees.join(", ")));
        }
        if repositories > 1 && !self.repository.is_empty() {
            facts.push(("Repository", repository_label(project, &self.repository)));
        }
        if let Some(at) = self.updated_at.as_deref().and_then(utc_ms) {
            facts.push(("Updated", ago_said(now, at)));
        }
        facts
    }
}

/// The checks as one mark: `failing` when one fails, else `pending` while one runs, else
/// `passing`; none without checks.
fn ci_of(checks: &Value) -> Option<String> {
    let states: Vec<&str> = checks
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|check| check["state"].as_str())
        .collect();
    let any = |state: &str| states.contains(&state);
    match () {
        _ if states.is_empty() => None,
        _ if any("failing") => Some("failing".into()),
        _ if any("pending") => Some("pending".into()),
        _ => Some("passing".into()),
    }
}

/// The state that draws a CI mark: a spinner, a check, a cross.
fn ci_state(ci: &str) -> AgentState {
    match ci {
        "failing" => AgentState::Failed,
        "pending" => AgentState::Running,
        _ => AgentState::Completed,
    }
}

/// `3h ago`, `now`.
fn ago_said(now: u64, then: u64) -> String {
    match ago(now, then) {
        now if now == "now" => now,
        ago => format!("{ago} ago"),
    }
}

/// An RFC 3339 time in UTC, as the daemon stores `updated_at` (`2026-11-01T10:00:00Z`, with its
/// milliseconds when it has some), in milliseconds since 1970.
fn utc_ms(text: &str) -> Option<u64> {
    let (day, clock) = text.strip_suffix('Z')?.split_once('T')?;
    let mut day = day.split('-').map(|n| n.parse::<i64>().ok());
    let (year, month, mday) = (day.next()??, day.next()??, day.next()??);
    let (clock, millis) = match clock.split_once('.') {
        Some((clock, fraction)) => (clock, format!("{fraction:0<3}")[..3].parse::<i64>().ok()?),
        None => (clock, 0),
    };
    let mut clock = clock.split(':').map(|n| n.parse::<i64>().ok());
    let (hours, minutes, seconds) = (clock.next()??, clock.next()??, clock.next()??);
    // Howard Hinnant's `days_from_civil`.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + mday - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    let ms = ((days * 86_400 + hours * 3600 + minutes * 60 + seconds) * 1000) + millis;
    u64::try_from(ms).ok()
}

/// A source of the project's items, as `items.list` says it.
#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    /// `pull` or `ticket`.
    pub kind: String,
    /// A repository's path, or `tracker`.
    pub target: String,
    /// Why its last round failed.
    pub problem: Option<String>,
}

/// What a read of the project's items gave: the items, their sources, and what the project
/// declares (the repositories whose forge is set, whether a tracker is).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Read {
    pub items: Vec<ItemRow>,
    pub sources: Vec<Source>,
    pub forges: Vec<String>,
    pub tracker: bool,
    /// Every repository of the project, by its path.
    pub repositories: Vec<String>,
}

impl Read {
    /// `items.list`'s answer and `project.settings`'.
    pub fn of(listed: &Value, settings: &Value) -> Self {
        Self {
            items: listed["items"]
                .as_array()
                .into_iter()
                .flatten()
                .map(ItemRow::of)
                .collect(),
            sources: listed["sources"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|source| Source {
                    kind: source["kind"].as_str().unwrap_or_default().to_owned(),
                    target: source["target"].as_str().unwrap_or_default().to_owned(),
                    problem: source["problem"].as_str().map(str::to_owned),
                })
                .collect(),
            forges: settings["repositories"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|repository| !repository["forge"].is_null())
                .filter_map(|repository| repository["path"].as_str().map(str::to_owned))
                .collect(),
            tracker: !settings["tracker"].is_null(),
            repositories: settings["repositories"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|repository| repository["path"].as_str().map(str::to_owned))
                .collect(),
        }
    }
}

/// A comment or a review of an item's detail.
pub struct Said {
    /// `dana: Looks good`, what the line says to a reader.
    pub said: String,
    pub author: String,
    /// `changes requested`: a review's verdict.
    pub verdict: Option<String>,
    pub by_agent: bool,
    text: MarkdownText,
}

/// A check of an item's detail.
#[derive(Clone, Debug, PartialEq)]
pub struct CheckLine {
    pub name: String,
    pub state: String,
    pub link: Option<String>,
}

/// What `items.show` gave of one item, and its texts drawn.
pub struct Full {
    pub head: Option<String>,
    pub base: Option<String>,
    pub labels: Vec<String>,
    pub description: Option<(String, MarkdownText)>,
    pub checks: Vec<CheckLine>,
    pub said: Vec<Said>,
}

impl Full {
    fn of(item: &Value, cx: &mut App) -> Self {
        let details = &item["details"];
        let text = |value: &Value| {
            value
                .as_str()
                .filter(|text| !text.trim().is_empty())
                .map(str::to_owned)
        };
        let said = details["discussion"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|said| {
                let author = said["author"].as_str().unwrap_or_default().to_owned();
                let verdict = said["state"].as_str().map(str::to_owned);
                let by_agent = said["by_agent"] == true;
                let body = said["body"].as_str().unwrap_or_default();
                let mut who = author.clone();
                if by_agent {
                    who = format!("{who}, posted by an agent");
                }
                if let Some(verdict) = &verdict {
                    who = format!("{who}, {verdict}");
                }
                Said {
                    said: format!("{who}: {body}"),
                    author,
                    verdict,
                    by_agent,
                    text: MarkdownText::new(body, cx),
                }
            })
            .collect();
        Self {
            head: text(&details["head"]),
            base: text(&details["base"]),
            labels: item["labels"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|label| label.as_str().map(str::to_owned))
                .collect(),
            description: text(&details["description"]).map(|description| {
                let drawn = MarkdownText::new(&description, cx);
                (description, drawn)
            }),
            checks: item["checks"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|check| CheckLine {
                    name: check["name"].as_str().unwrap_or_default().to_owned(),
                    state: check["state"].as_str().unwrap_or_default().to_owned(),
                    link: check["link"].as_str().map(str::to_owned),
                })
                .collect(),
            said,
        }
    }
}

/// The item whose detail is shown: its row at once, the rest once `items.show` answers.
pub struct Detail {
    pub row: ItemRow,
    pub full: Option<Result<Full, String>>,
}

impl Detail {
    /// Whether `items.show` answered.
    pub fn read(&self) -> bool {
        self.full.is_some()
    }
}

/// The project page's "Pull requests and tickets".
pub struct ProjectItems {
    shell: WeakEntity<Shell>,
    project: String,
    read: Option<Result<Read, String>>,
    tab: Kind,
    shown: Shown,
    mine: bool,
    /// The repository whose pull requests alone are listed.
    repository: Option<String>,
    /// The rows as drawn, one list per tab: a row that leaves folds away before it is gone.
    lists: [Listing<ItemRow>; 2],
    /// The CI mark of each pull request that has checks, by its row's id: its own view, so a
    /// spinner turning redraws it alone.
    marks: HashMap<String, (Entity<Status>, Entity<StateIcon>)>,
    detail: Option<Detail>,
    detail_open: bool,
}

impl ProjectItems {
    pub fn new(shell: WeakEntity<Shell>, project: String) -> Self {
        Self {
            shell,
            project,
            read: None,
            tab: Kind::Pulls,
            shown: Shown::Open,
            mine: false,
            repository: None,
            lists: [Listing::default(), Listing::default()],
            marks: HashMap::new(),
            detail: None,
            detail_open: false,
        }
    }

    pub fn project(&self) -> &str {
        &self.project
    }

    /// Asks the daemon for the items.
    pub fn read(&self, cx: &mut App) {
        let project = self.project.clone();
        perform_later(&self.shell, Command::Items { project }, cx);
    }

    /// Whether the daemon answered a read.
    pub fn is_read(&self) -> bool {
        self.read.is_some()
    }

    /// What the daemon answered; nothing is drawn again when it is what was drawn.
    pub fn listed(&mut self, read: Result<Read, String>, cx: &mut Context<Self>) {
        if self.read.as_ref() == Some(&read) {
            return;
        }
        if let Ok(read) = &read {
            for row in &read.items {
                let Some(ci) = &row.ci else { continue };
                let state = ci_state(ci);
                let (status, _) = self.marks.entry(row.id()).or_insert_with(|| {
                    let status = cx.new(|_| Status::new(state));
                    let mark = cx.new(|cx| StateIcon::new(status.clone(), cx));
                    (status, mark)
                });
                status.update(cx, |status, cx| status.set(state, cx));
            }
            self.marks.retain(|id, _| {
                read.items
                    .iter()
                    .any(|row| &row.id() == id && row.ci.is_some())
            });
            if self
                .repository
                .as_ref()
                .is_some_and(|path| !read.forges.contains(path))
            {
                self.repository = None;
            }
        }
        self.read = Some(read);
        cx.notify();
    }

    pub fn tab(&self) -> Kind {
        self.tab
    }

    pub fn choose(&mut self, tab: Kind, cx: &mut Context<Self>) {
        self.tab = tab;
        cx.notify();
    }

    pub fn show(&mut self, shown: Shown, cx: &mut Context<Self>) {
        self.shown = shown;
        cx.notify();
    }

    pub fn toggle_mine(&mut self, cx: &mut Context<Self>) {
        self.mine = !self.mine;
        cx.notify();
    }

    pub fn filter_repository(&mut self, repository: Option<String>, cx: &mut Context<Self>) {
        self.repository = repository;
        cx.notify();
    }

    /// The rows of `kind` the filters keep, the most recently changed first.
    fn kept(&self, kind: Kind) -> Vec<ItemRow> {
        let Some(Ok(read)) = &self.read else {
            return Vec::new();
        };
        read.items
            .iter()
            .filter(|row| row.kind == kind.word())
            .filter(|row| match self.shown {
                Shown::Open => !row.closed(),
                Shown::Closed => row.closed(),
                Shown::All => true,
            })
            .filter(|row| !self.mine || row.mine)
            .filter(|row| {
                kind == Kind::Tickets
                    || self
                        .repository
                        .as_ref()
                        .is_none_or(|path| &row.repository == path)
            })
            .cloned()
            .collect()
    }

    /// The rows the chosen tab lists now.
    pub fn rows(&self) -> Vec<ItemRow> {
        self.kept(self.tab)
    }

    /// The state the CI mark of the row `id` draws.
    pub fn ci_state(&self, id: &str, cx: &App) -> Option<AgentState> {
        self.marks
            .get(id)
            .map(|(status, _)| status.read(cx).state())
    }

    pub fn detail(&self) -> Option<&Detail> {
        self.detail.as_ref()
    }

    pub fn is_detail_open(&self) -> bool {
        self.detail_open
    }

    /// Opens the detail of `row`, and asks the daemon for the rest of it.
    pub fn open_detail(&mut self, row: ItemRow, cx: &mut Context<Self>) {
        perform_later(
            &self.shell,
            Command::Item {
                project: self.project.clone(),
                kind: row.kind.clone(),
                key: row.key.clone(),
                repository: Some(row.repository.clone()).filter(|path| !path.is_empty()),
            },
            cx,
        );
        self.detail = Some(Detail { row, full: None });
        self.detail_open = true;
        cx.notify();
    }

    /// What `items.show` answered of the item `key` of `kind`, when its detail is still the one
    /// shown.
    pub fn shown(
        &mut self,
        kind: &str,
        key: &str,
        shown: Result<Value, String>,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = &self.detail else { return };
        if detail.row.kind != kind || detail.row.key != key {
            return;
        }
        let full = shown.map(|item| Full::of(&item, cx));
        if let Some(detail) = &mut self.detail {
            detail.full = Some(full);
        }
        cx.notify();
    }

    pub fn close_detail(&mut self, cx: &mut Context<Self>) {
        self.detail_open = false;
        cx.notify();
    }

    fn open_link(&self, url: &str, cx: &mut App) {
        if let Some(shell) = self.shell.upgrade() {
            shell.read(cx).open_link(url);
        }
    }

    /// One row: its CI mark (or what it is), its title, its labelled values.
    fn row(&self, row: &ItemRow, repositories: usize, now: u64, cx: &mut Context<Self>) -> ListRow {
        let p = palette(cx);
        let id = row.id();
        let leading: AnyElement = match self.marks.get(&id) {
            Some((_, mark)) => mark.clone().into_any_element(),
            None => icon(match row.kind.as_str() {
                "pull" => Icon::PullRequest,
                _ => Icon::Ticket,
            })
            .size(px(14.))
            .text_color(p.faint)
            .into_any_element(),
        };
        let facts = row.facts(&self.project, repositories, now);
        let said = facts
            .iter()
            .map(|(label, value)| match label {
                &"" => value.clone(),
                label => format!("{label} {value}"),
            })
            .collect::<Vec<_>>()
            .join(", ");
        let below = div()
            .id(SharedString::from(format!("{id}-facts")))
            .aria_label(SharedString::from(said))
            .flex()
            .flex_wrap()
            .gap_x(space::MD)
            .text_size(px(12.))
            .children(facts.into_iter().map(|(label, value)| {
                div()
                    .flex()
                    .gap(space::XS)
                    .when(!label.is_empty(), |d| {
                        d.child(div().text_color(p.faint).child(label))
                    })
                    .child(
                        div()
                            .text_color(p.mute)
                            .when(label.is_empty(), |d| d.mono(11.5))
                            .child(SharedString::from(value)),
                    )
            }))
            .test_support();
        let this = cx.weak_entity();
        let clicked = row.clone();
        ListRow::new(ElementId::Name(id.into()), row.title.clone())
            .leading(leading)
            .below(below)
            .on_click(move |_, _, cx| {
                this.update(cx, |this, cx| this.open_detail(clicked.clone(), cx))
                    .ok();
            })
    }

    /// The filters of the chosen tab: the states, the user's own, and a repository for the pull
    /// requests of a project of several.
    fn filters(&self, forges: &[String], cx: &mut Context<Self>) -> AnyElement {
        let this = cx.weak_entity();
        let chosen = SHOWN
            .iter()
            .position(|(key, _)| {
                *key == match self.shown {
                    Shown::Open => "open",
                    Shown::Closed => "closed",
                    Shown::All => "all",
                }
            })
            .unwrap_or(0);
        let states = Segmented::new("project-items-state", "Show", SHOWN, chosen).on_choose({
            let this = this.clone();
            move |at, _, cx| {
                let shown = [Shown::Open, Shown::Closed, Shown::All][at];
                this.update(cx, |this, cx| this.show(shown, cx)).ok();
            }
        });
        let mine = Switch::new("project-items-mine", self.mine, "Mine only").on_click({
            let this = this.clone();
            move |_, _, cx| {
                this.update(cx, |this, cx| this.toggle_mine(cx)).ok();
            }
        });
        let repository = (self.tab == Kind::Pulls && forges.len() > 1).then(|| {
            let options = std::iter::once(SelectOption::new("all", "All repositories")).chain(
                forges.iter().map(|path| {
                    SelectOption::new(path.clone(), repository_label(&self.project, path))
                }),
            );
            let chosen = self
                .repository
                .as_ref()
                .and_then(|path| forges.iter().position(|p| p == path))
                .map_or(0, |at| at + 1);
            let forges = forges.to_vec();
            Select::new("project-items-repository", options, chosen)
                .fit()
                .on_choose(move |at, _, cx| {
                    let repository = at.checked_sub(1).map(|at| forges[at].clone());
                    this.update(cx, |this, cx| this.filter_repository(repository, cx))
                        .ok();
                })
        });
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(space::LG)
            .child(states)
            .child(mine)
            .children(repository)
            .into_any_element()
    }

    /// The detail of the item clicked, in a dialog; drawn while it closes too.
    fn detail_modal(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let detail = self.detail.as_ref()?;
        let p = palette(cx);
        let row = &detail.row;
        let this = cx.weak_entity();
        let kind = match row.kind.as_str() {
            "pull" => "Pull request",
            _ => "Ticket",
        };
        let mut sentence = kind.to_owned();
        if !row.repository.is_empty() {
            sentence = format!(
                "{sentence} of {}",
                repository_label(&self.project, &row.repository)
            );
        }
        let mut modal = Modal::new("item-detail", format!("{} {}", row.label(), row.title))
            .sentence(format!("{sentence}, {}.", row.state_said()))
            .wide()
            .takes_focus()
            .cancel_label("Close")
            .open(self.detail_open)
            .on_close({
                let this = this.clone();
                move |_, cx| {
                    this.update(cx, |this, cx| this.close_detail(cx)).ok();
                }
            });
        if let Some(url) = row.url.clone() {
            let this = this.clone();
            modal = modal.action(
                Button::primary("item-detail-browser", "Open in browser")
                    .leading(Icon::OpenInEditor)
                    .on_click(move |_, _, cx| {
                        this.update(cx, |this, cx| this.open_link(&url, cx)).ok();
                    }),
            );
        }
        let full = detail.full.as_ref().and_then(|full| full.as_ref().ok());
        let mut facts: Vec<AnyElement> = Vec::new();
        let mut fact = |id: &str, glyph: Icon, label: &str, value: String| {
            facts.push(
                LabelledLine::new(
                    SharedString::from(id.to_owned()),
                    glyph,
                    SharedString::from(label.to_owned()),
                    SharedString::from(value.clone()),
                )
                .child(SharedString::from(value))
                .into_any_element(),
            );
        };
        if let Some(author) = &row.author {
            fact("item-detail-author", Icon::Agents, "Author", author.clone());
        }
        if !row.assignees.is_empty() {
            fact(
                "item-detail-assignees",
                Icon::Agents,
                "Assignees",
                row.assignees.join(", "),
            );
        }
        if let Some(full) = full {
            if let (Some(head), Some(base)) = (&full.head, &full.base) {
                fact(
                    "item-detail-branches",
                    Icon::Branch,
                    "Branches",
                    format!("{head} into {base}"),
                );
            }
            if !full.labels.is_empty() {
                fact(
                    "item-detail-labels",
                    Icon::Text,
                    "Labels",
                    full.labels.join(", "),
                );
            }
        }
        let facts = div().flex().flex_col().children(facts);
        let heading = |text: &'static str| {
            div()
                .text_size(px(12.))
                .semibold()
                .text_color(p.mute)
                .child(text)
        };
        let description = full.map(|full| match &full.description {
            Some((text, drawn)) => div()
                .id("item-detail-description")
                .aria_label(SharedString::from(text.clone()))
                .text_size(px(13.))
                .child(drawn.view("item-detail-description-text"))
                .test_support()
                .into_any_element(),
            None => div()
                .id("item-detail-description")
                .aria_label("No description.")
                .text_size(px(13.))
                .text_color(p.faint)
                .child("No description.")
                .test_support()
                .into_any_element(),
        });
        let checks = full.filter(|full| !full.checks.is_empty()).map(|full| {
            div()
                .flex()
                .flex_col()
                .gap(space::XS)
                .child(heading("Checks"))
                .children(full.checks.iter().map(|check| {
                    let (glyph, color) = match check.state.as_str() {
                        "passing" => ("✓", p.ok),
                        "failing" => ("✗", p.fail),
                        "pending" => ("●", p.run),
                        _ => ("–", p.faint),
                    };
                    let line = div()
                        .id(SharedString::from(format!(
                            "item-detail-check-{}",
                            check.name
                        )))
                        .aria_label(SharedString::from(format!(
                            "{}: {}",
                            check.name, check.state
                        )))
                        .flex()
                        .items_center()
                        .gap(space::SM)
                        .text_size(px(13.))
                        .child(div().mono(12.).text_color(color).child(glyph))
                        .child(SharedString::from(check.name.clone()))
                        .child(
                            div()
                                .text_color(p.faint)
                                .child(SharedString::from(check.state.clone())),
                        );
                    match check.link.clone() {
                        Some(link) => {
                            let this = this.clone();
                            hoverable(
                                line.cursor_pointer().on_click(move |_, _, cx| {
                                    this.update(cx, |this, cx| this.open_link(&link, cx)).ok();
                                }),
                                move |d, t| d.bg(p.hover.opacity(t)),
                            )
                            .into_any_element()
                        }
                        None => line.test_support().into_any_element(),
                    }
                }))
        });
        let discussion = full.filter(|full| !full.said.is_empty()).map(|full| {
            div()
                .flex()
                .flex_col()
                .gap(space::MD)
                .child(heading("Reviews and comments"))
                .children(full.said.iter().enumerate().map(|(at, said)| {
                    let mut who = vec![
                        div()
                            .medium()
                            .child(SharedString::from(said.author.clone()))
                            .into_any_element(),
                    ];
                    if said.by_agent {
                        who.push(
                            div()
                                .text_color(p.faint)
                                .child("posted by an agent")
                                .into_any_element(),
                        );
                    }
                    if let Some(verdict) = &said.verdict {
                        who.push(
                            div()
                                .text_color(p.mute)
                                .child(SharedString::from(verdict.clone()))
                                .into_any_element(),
                        );
                    }
                    div()
                        .id(SharedString::from(format!("item-detail-said-{at}")))
                        .aria_label(SharedString::from(said.said.clone()))
                        .flex()
                        .flex_col()
                        .gap(space::XS)
                        .p(space::MD)
                        .rounded(px(10.))
                        .bg(p.card2)
                        .text_size(px(13.))
                        .child(div().flex().gap(space::SM).text_size(px(12.)).children(who))
                        .child(said.text.view(format!("item-detail-said-{at}-text")))
                        .test_support()
                }))
        });
        let failed = detail
            .full
            .as_ref()
            .and_then(|full| full.as_ref().err())
            .map(|why| SharedString::from(why.clone()));
        let refused = presence_of("item-detail-refused-block", failed, move |why| {
            div()
                .id("item-detail-refused")
                .aria_label(why.clone())
                .text_size(px(12.5))
                .text_color(p.fail)
                .child(why)
                .test_support()
                .into_any_element()
        });
        let body = div()
            .flex()
            .flex_col()
            .gap(space::LG)
            .child(facts)
            .children(description.map(|d| enter("item-detail-enter-description", 0, d)))
            .children(checks.map(|c| enter("item-detail-enter-checks", 1, c)))
            .children(discussion.map(|d| enter("item-detail-enter-discussion", 2, d)));
        Some(modal.child(body).kept(refused).into_any_element())
    }
}

impl Render for ProjectItems {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let now = now_ms();
        let at = cx.background_executor().now();
        let reduce = cx.reduce_motion();
        let tab = self.tab;
        let kept = self.kept(tab);
        let counts = [self.kept(Kind::Pulls).len(), self.kept(Kind::Tickets).len()];
        self.lists[tab.index()].sync(
            kept.iter()
                .map(|row| (row.id(), row.clone()))
                .collect::<Vec<_>>(),
            at,
            reduce,
        );
        let (read, failed) = match &self.read {
            Some(Ok(read)) => (Some(read.clone()), None),
            Some(Err(why)) => (None, Some(why.clone())),
            None => (None, None),
        };
        let forges = read
            .as_ref()
            .map(|read| read.forges.clone())
            .unwrap_or_default();
        let set = read.as_ref().map(|read| match tab {
            Kind::Pulls => !read.forges.is_empty(),
            Kind::Tickets => read.tracker,
        });
        let problems: Vec<String> = read
            .as_ref()
            .map(|read| {
                read.sources
                    .iter()
                    .filter(|source| source.kind == tab.word())
                    .filter_map(|source| {
                        let problem = source.problem.as_ref()?;
                        Some(match source.target.as_str() {
                            "tracker" => format!("The tracker cannot be read: {problem}"),
                            path => format!(
                                "{} cannot be read: {problem}",
                                repository_label(&self.project, path)
                            ),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let problem = failed
            .or_else(|| (!problems.is_empty()).then(|| problems.join("\n")))
            .map(SharedString::from);
        let listed: Vec<(String, ItemRow, flottille_ui::Placement)> = self.lists[tab.index()]
            .rows()
            .map(|listed| {
                (
                    listed.key.to_owned(),
                    listed.value.clone(),
                    listed.placement,
                )
            })
            .collect();
        let present = !self.lists[tab.index()].is_empty();
        let card = listed.into_iter().fold(
            ListCard::new("project-items-list"),
            |card, (key, row, placement)| {
                let drawn = self.row(&row, forges.len(), now, cx);
                card.item(
                    ElementId::Name(format!("{key}-item").into()),
                    placement,
                    drawn,
                )
            },
        );
        let this = cx.weak_entity();
        let tabs = Tabs::new("project-items-tabs", "Lists", tab.index())
            .counted("pulls", "Pull requests", counts[0].to_string())
            .counted("tickets", "Tickets", counts[1].to_string())
            .on_choose({
                let this = this.clone();
                move |at, _, cx| {
                    let tab = [Kind::Pulls, Kind::Tickets][at];
                    this.update(cx, |this, cx| this.choose(tab, cx)).ok();
                }
            });
        let not_set = {
            let (shell, project) = (self.shell.clone(), self.project.clone());
            let first = read
                .as_ref()
                .and_then(|read| read.repositories.first().cloned())
                .unwrap_or_default();
            let (glyph, text) = match tab {
                Kind::Pulls => (
                    Icon::PullRequest,
                    "No forge is set for the project's repositories: set one in its Settings to \
                     list their pull requests.",
                ),
                Kind::Tickets => (
                    Icon::Ticket,
                    "No tracker is set for the project: set one in its Settings to list its \
                     tickets.",
                ),
            };
            EmptyLine::new("project-items-not-set", glyph, text).action(
                Button::secondary("project-items-settings", "Open Settings").on_click(
                    move |_, window, cx| {
                        let at = match tab {
                            Kind::Pulls => Reveal::Repository(first.clone()),
                            Kind::Tickets => Reveal::Tracker,
                        };
                        shell
                            .update(cx, |shell, cx| {
                                shell.open_project_settings_at(&project, at, window, cx)
                            })
                            .ok();
                    },
                ),
            )
        };
        let empty_said = match (tab, self.shown) {
            (Kind::Pulls, Shown::Open) => "No open pull request.",
            (Kind::Pulls, Shown::Closed) => "No closed or merged pull request.",
            (Kind::Pulls, Shown::All) => "No pull request.",
            (Kind::Tickets, Shown::Open) => "No open ticket.",
            (Kind::Tickets, Shown::Closed) => "No closed ticket.",
            (Kind::Tickets, Shown::All) => "No ticket.",
        };
        let empty = set == Some(true) && !present;
        let body = div()
            .flex()
            .flex_col()
            .child(presence(
                "project-items-filters-block",
                set == Some(true),
                {
                    let filters = self.filters(&forges, cx);
                    div().pb(space::MD).child(filters)
                },
            ))
            .child(presence_of(
                "project-items-problem-block",
                problem,
                move |problem| {
                    div()
                        .id("project-items-problem")
                        .pb(space::MD)
                        .aria_label(problem.clone())
                        .flex()
                        .items_start()
                        .gap(space::SM)
                        .text_size(px(12.5))
                        .text_color(p.fail)
                        .child(icon(Icon::Warning).size(px(14.)).text_color(p.fail))
                        .child(div().flex_1().min_w_0().child(problem))
                        .test_support()
                        .into_any_element()
                },
            ))
            .child(presence(
                "project-items-not-set-block",
                set == Some(false),
                not_set,
            ))
            .child(presence(
                "project-items-empty-block",
                empty,
                EmptyLine::new(
                    "project-items-empty",
                    match tab {
                        Kind::Pulls => Icon::PullRequest,
                        Kind::Tickets => Icon::Ticket,
                    },
                    empty_said,
                ),
            ))
            .child(presence("project-items-list-block", present, card));
        let shown_key = match tab {
            Kind::Pulls => "pulls",
            Kind::Tickets => "tickets",
        };
        let modal = self.detail_modal(cx);
        Section::new("project-items", "Pull requests and tickets")
            .sentence(
                "What Flottille keeps of the project's forges and tracker, read again every \
                 minute while the window is open.",
            )
            .child(tabs)
            .child(enter(
                ElementId::Name(format!("enter-items-{}-{shown_key}", self.project).into()),
                0,
                body,
            ))
            .children(modal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_checks_say_one_mark_failing_before_pending_before_passing() {
        let checks = |states: &[&str]| {
            ci_of(&Value::Array(
                states.iter().map(|s| json!({"state": s})).collect(),
            ))
        };
        assert_eq!(checks(&[]), None);
        assert_eq!(checks(&["passing", "skipped"]).as_deref(), Some("passing"));
        assert_eq!(checks(&["passing", "pending"]).as_deref(), Some("pending"));
        assert_eq!(
            checks(&["pending", "failing", "passing"]).as_deref(),
            Some("failing")
        );
    }

    #[test]
    fn an_updated_at_in_utc_is_read_with_its_milliseconds() {
        assert_eq!(utc_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(utc_ms("1970-01-02T00:00:01.5Z"), Some(86_401_500));
        assert_eq!(utc_ms("2026-11-01T10:00:00Z"), Some(1_793_527_200_000));
        assert_eq!(utc_ms("2026-11-01"), None);
    }

    #[test]
    fn a_number_is_said_with_a_hash_and_a_jira_key_as_it_is() {
        let row = |key: &str| ItemRow::of(&json!({"kind": "ticket", "key": key}));
        assert_eq!(row("12").label(), "#12");
        assert_eq!(row("ABC-1").label(), "ABC-1");
        assert_eq!(row("ABC-1").id(), "item-ticket--ABC-1");
    }
}
