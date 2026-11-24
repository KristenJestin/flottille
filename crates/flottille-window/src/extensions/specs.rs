//! Specs in the window (`docs/extensions.md`, "Specs"): the project's Specs tab, "Write the spec"
//! on a ticket's row, and a spec's page with its questions, each answered in one click. Everything
//! comes from `spec.list` and `spec.show`, read again when a `spec.*` event says a spec changed;
//! every gesture is a `spec.*` method of the daemon, its result a toast. Nothing of it shows in a
//! project where Specs is off.

use crate::glyphs;
use std::collections::{HashMap, HashSet};
use std::f32::consts::FRAC_PI_2;

use gpui_design_system::{
    Button, Confirm, EmptyLine, Field, FilterChip, FilterChips, Icon, ListCard, ListRow, Listing,
    Menu, Modal, PageHeader, RadioChoices, Section, Toast, changing, color, enter, hoverable, icon,
    palette, presence, presence_of, space, toast, turning,
};
use gpui_kit::base::input::{InputEvent, InputState, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, Focusable as _,
    InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, TestSupportExt as _, Transformation, WeakEntity, Window,
    div, px, radians,
};
use serde_json::{Value, json};

use super::{Registry, Then as Asked};
use crate::actions::Command;
use crate::commands::problem_line;
use crate::commands::{menu_of, perform_later, section_blocks, spaced, stack};
use crate::items::ItemRow;
use crate::project_settings::{field_frame, styled_input};
use crate::shell::{Selection, Shell};

/// Its name, as the daemon declares it.
pub const NAME: &str = "specs";

/// What to do with the daemon's answer to a `spec.*` request.
#[derive(Clone, Debug, PartialEq)]
pub enum Then {
    /// `spec.list`, the read `asked`.
    Listed {
        asked: u64,
    },
    /// `spec.show` of `spec` for its page, the read `asked`.
    Shown {
        spec: u64,
        asked: u64,
    },
    /// "New spec" from an idea.
    New,
    /// "Write the spec" of a ticket.
    FromTicket {
        ticket: String,
    },
    /// An answer to a question of the spec.
    Answer {
        spec: u64,
        question: u64,
    },
    Ready {
        spec: u64,
    },
    Draft {
        spec: u64,
    },
    /// The user's choice once the ticket changed: `take` or `overwrite`.
    Ticket {
        spec: u64,
        choice: &'static str,
    },
    /// The departures posted on the spec's tickets.
    Departures {
        spec: u64,
    },
    /// "Launch development".
    Develop {
        spec: u64,
    },
    /// The user's answer to an amendment the development session proposed.
    Amendment {
        spec: u64,
        amendment: u64,
        accept: bool,
    },
    /// `review.list` of the spec: the findings of the reviews of its development.
    Findings {
        spec: u64,
    },
    /// `spec.write` of one section.
    Wrote {
        spec: u64,
    },
    /// `spec.link` or `spec.unlink` of a ticket.
    Linked {
        spec: u64,
        link: bool,
    },
    /// `items.list` of the project: the tickets the picker offers.
    Tickets {
        spec: u64,
    },
}

impl Then {
    /// Whether the open spec's page takes the answer.
    pub fn for_page(&self) -> Option<u64> {
        match self {
            Then::Shown { spec, .. }
            | Then::Answer { spec, .. }
            | Then::Ready { spec }
            | Then::Draft { spec }
            | Then::Ticket { spec, .. }
            | Then::Departures { spec }
            | Then::Develop { spec }
            | Then::Amendment { spec, .. }
            | Then::Findings { spec }
            | Then::Wrote { spec }
            | Then::Linked { spec, .. }
            | Then::Tickets { spec } => Some(*spec),
            Then::Listed { .. } | Then::New | Then::FromTicket { .. } => None,
        }
    }
}

fn ask(shell: &crate::commands::ShellOf, method: &str, params: Value, then: Then, cx: &mut App) {
    let command = Command::Ask {
        method: method.to_owned(),
        params,
        then: Asked::Specs(then),
    };
    perform_later(shell, command, cx);
}

/// A status in words.
fn status_said(status: &str) -> &'static str {
    match status {
        "draft" => "Draft",
        "ready" => "Ready",
        "in_progress" => "In progress",
        "review" => "Review",
        "done" => "Done",
        _ => "Unknown",
    }
}

/// `1 open question`, `2 open questions`.
fn open_said(open: u64) -> String {
    match open {
        1 => "1 open question".into(),
        n => format!("{n} open questions"),
    }
}

/// A spec as `spec.list` gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    pub spec: u64,
    pub project: String,
    pub title: String,
    pub status: String,
    /// The tickets it covers, as the project's tracker writes them (`#5`, `ABC-1`).
    pub tickets: Vec<String>,
    /// Its open questions.
    pub questions: u64,
}

impl Summary {
    fn of(value: &Value) -> Self {
        Self {
            spec: value["spec"].as_u64().unwrap_or_default(),
            project: value["project"].as_str().unwrap_or_default().to_owned(),
            title: value["title"].as_str().unwrap_or_default().to_owned(),
            status: value["status"].as_str().unwrap_or_default().to_owned(),
            tickets: strings(&value["tickets"]),
            questions: value["questions"].as_u64().unwrap_or_default(),
        }
    }

    /// The labelled values of its row: its status, its tickets, its open questions.
    fn facts(&self) -> Vec<(&'static str, String)> {
        let mut facts = vec![("", status_said(&self.status).to_owned())];
        if !self.tickets.is_empty() {
            let label = if self.tickets.len() == 1 {
                "Ticket"
            } else {
                "Tickets"
            };
            facts.push((label, self.tickets.join(", ")));
        }
        if self.questions > 0 {
            facts.push(("", open_said(self.questions)));
        }
        facts
    }
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|text| text.as_str().map(str::to_owned))
        .collect()
}

/// What `spec.list` last answered, for every project where Specs is on.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Specs {
    summaries: Vec<Summary>,
    known: bool,
    asked: u64,
    listed_at: u64,
}

impl Specs {
    /// The specs of `project`, the latest first.
    pub fn of(&self, project: &str) -> Vec<Summary> {
        let mut found: Vec<Summary> = self
            .summaries
            .iter()
            .filter(|summary| summary.project == project)
            .cloned()
            .collect();
        found.sort_by_key(|summary| std::cmp::Reverse(summary.spec));
        found
    }

    /// The spec that covers `ticket` in `project`.
    pub fn of_ticket(&self, project: &str, ticket: &str) -> Option<u64> {
        self.summaries
            .iter()
            .find(|summary| {
                summary.project == project && summary.tickets.iter().any(|t| t == ticket)
            })
            .map(|summary| summary.spec)
    }

    pub fn is_known(&self) -> bool {
        self.known
    }

    /// Specs was turned off: nothing is listed. Whether something was.
    pub(super) fn forget(&mut self) -> bool {
        let had = !self.summaries.is_empty();
        self.summaries.clear();
        had
    }
}

/// Asks the daemon for the specs.
pub(super) fn read(registry: &mut Registry, cx: &mut Context<Registry>) {
    registry.specs.asked += 1;
    let then = Then::Listed {
        asked: registry.specs.asked,
    };
    ask(&registry.shell, "spec.list", json!({}), then, cx);
}

/// What the daemon answered to a request of the registry.
pub(super) fn answered(
    registry: &mut Registry,
    then: Then,
    answer: Result<Value, String>,
    cx: &mut Context<Registry>,
) {
    match then {
        Then::Listed { asked } => {
            let specs = &mut registry.specs;
            if asked < specs.listed_at {
                return;
            }
            specs.listed_at = asked;
            specs.known = true;
            // Refused while Specs is off: nothing is listed.
            let summaries: Vec<Summary> = answer
                .map(|listed| {
                    listed["specs"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(Summary::of)
                        .collect()
                })
                .unwrap_or_default();
            if specs.summaries != summaries {
                specs.summaries = summaries;
                cx.notify();
            }
        }
        Then::New | Then::FromTicket { .. } => match answer {
            Ok(_) => toast(
                Toast::success("Writing the spec").detail(
                    "A session reads the code and asks you what only you can decide; its \
                     questions come to the spec's page.",
                ),
                cx,
            ),
            Err(why) => toast(Toast::failure("The spec was not started").detail(why), cx),
        },
        Then::Develop { .. } => developed(&answer, cx),
        // For a page no longer open.
        Then::Findings { .. } => {}
        // The page's own, when it is no longer open.
        Then::Shown { .. }
        | Then::Answer { .. }
        | Then::Ready { .. }
        | Then::Draft { .. }
        | Then::Ticket { .. }
        | Then::Departures { .. }
        | Then::Wrote { .. }
        | Then::Linked { .. }
        | Then::Tickets { .. }
        | Then::Amendment { .. } => {
            if let Err(why) = answer {
                toast(Toast::failure("The spec was not changed").detail(why), cx);
            }
        }
    }
}

/// "Launch development" of a Ready spec: the user's gesture alone.
fn launch(shell: &crate::commands::ShellOf, spec: u64, cx: &mut App) {
    let then = Then::Develop { spec };
    ask(shell, "spec.develop", json!({"spec": spec}), then, cx);
}

/// The toast of a development launched, or why it was not.
fn developed(answer: &Result<Value, String>, cx: &mut App) {
    match answer {
        Ok(started) => toast(
            Toast::success("Development launched").detail(format!(
                "A session builds the spec in the workspace {} made for it; its first message \
                 is the spec.",
                started["workspace"].as_str().unwrap_or_default()
            )),
            cx,
        ),
        Err(why) => toast(
            Toast::failure("The development was not launched").detail(why.clone()),
            cx,
        ),
    }
}

/// The spec a session writes or develops, as `agent.tree`'s `extensions.specs` says it.
#[derive(Clone, Debug, PartialEq)]
pub struct OfSession {
    pub spec: u64,
    pub title: String,
    /// The session develops the spec, rather than writing it.
    pub developing: bool,
}

impl OfSession {
    pub fn of(extensions: &Value) -> Option<Self> {
        let fact = extensions.get(NAME)?;
        Some(Self {
            spec: fact["spec"].as_u64()?,
            title: fact["title"].as_str().unwrap_or_default().to_owned(),
            developing: fact["work"] == "developing",
        })
    }

    /// What a reader hears of its line.
    pub fn said(&self) -> String {
        let work = match self.developing {
            true => "developed by this session",
            false => "written by this session",
        };
        format!("{} {}, {work}", self.spec, self.title)
    }
}

/// The value of a session's Spec line: the spec's number and title, which open its page.
pub fn session_value(
    shell: WeakEntity<Shell>,
    project: &str,
    of: &OfSession,
    cx: &App,
) -> AnyElement {
    let p = palette(cx);
    let (project, spec) = (project.to_owned(), of.spec);
    let row = div()
        .id("rail-spec")
        .min_w_0()
        .flex()
        .items_center()
        .gap(space::XS)
        .overflow_hidden()
        .whitespace_nowrap()
        .cursor_pointer()
        .aria_label(SharedString::from(of.said()))
        .child(
            div()
                .flex_none()
                .child(SharedString::from(spec.to_string())),
        )
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .child(SharedString::from(of.title.clone())),
        )
        .on_click(move |_, window, cx| open_spec(&shell, &project, spec, window, cx));
    hoverable(row, move |d, t| d.text_color(color::mix(p.mute, p.text, t))).into_any_element()
}

/// The registry of the shell, while it is there.
fn registry_of(shell: &WeakEntity<Shell>, cx: &App) -> Option<Entity<Registry>> {
    shell
        .upgrade()
        .map(|shell| shell.read(cx).extensions().clone())
}

fn open_spec(
    shell: &WeakEntity<Shell>,
    project: &str,
    spec: u64,
    window: &mut Window,
    cx: &mut App,
) {
    let selection = Selection::Spec {
        project: project.to_owned(),
        spec,
    };
    shell
        .update(cx, |shell, cx| shell.open(selection, window, cx))
        .ok();
}

/// The labelled values of a row, drawn as the items' are; `said` is what a reader hears.
fn facts_line(id: String, facts: Vec<(&'static str, String)>, cx: &App) -> impl IntoElement {
    let p = palette(cx);
    let said = facts
        .iter()
        .map(|(label, value)| match label {
            &"" => value.clone(),
            label => format!("{label} {value}"),
        })
        .collect::<Vec<_>>()
        .join(", ");
    let status = facts
        .first()
        .map(|(_, value)| value.clone())
        .unwrap_or_default();
    let rest: Vec<(&'static str, String)> = facts.into_iter().skip(1).collect();
    div()
        .id(SharedString::from(id.clone()))
        .aria_label(SharedString::from(said))
        .flex()
        .flex_wrap()
        .gap_x(space::MD)
        .text_size(px(12.))
        // A status that changes fades back in.
        .child(changing(
            SharedString::from(format!("{id}-status")),
            status,
            move |status| div().text_color(p.mute).child(status).into_any_element(),
        ))
        .children(rest.into_iter().map(|(label, value)| {
            div()
                .flex()
                .gap(space::XS)
                .when(!label.is_empty(), |d| {
                    d.child(div().text_color(p.faint).child(label))
                })
                .child(div().text_color(p.mute).child(SharedString::from(value)))
        }))
        .test_support()
}

/// The project's Specs tab: each spec with its status, its tickets and its open questions, and
/// "New spec".
pub struct SpecsTab {
    shell: crate::commands::ShellOf,
    project: String,
    list: Listing<Summary>,
    /// "New spec" is open.
    new_open: bool,
    idea: Entity<InputState>,
    /// The "⋯" menu of each spec's row, by its key.
    menus: HashMap<String, Entity<Menu>>,
    /// The status the list is cut to (`flottille spec list --status`); all of them when none.
    filter: Option<&'static str>,
}

/// The statuses of a spec, in the order of its life.
const STATUSES: [&str; 5] = ["draft", "ready", "in_progress", "review", "done"];

impl SpecsTab {
    pub fn new(
        shell: crate::commands::ShellOf,
        project: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let idea =
            cx.new(|cx| styled_input("Let users sign in with a link sent by mail", window, cx));
        Self {
            shell,
            project,
            list: Listing::default(),
            new_open: false,
            idea,
            menus: HashMap::new(),
            filter: None,
        }
    }

    /// The chips over the list: All, then each status some spec has (the one chosen stays), each
    /// with its count. The one chosen, chosen again, goes back to all.
    fn chips(&self, specs: &[Summary], cx: &mut Context<Self>) -> FilterChips {
        let count = |status: &str| specs.iter().filter(|s| s.status == status).count();
        let keys: Vec<&'static str> = STATUSES
            .into_iter()
            .filter(|status| count(status) > 0 || self.filter == Some(*status))
            .collect();
        let chosen = self
            .filter
            .and_then(|status| keys.iter().position(|key| *key == status))
            .map_or(0, |at| at + 1);
        let this = cx.weak_entity();
        FilterChips::new(
            "specs-filter",
            "Filter the specs",
            std::iter::once(FilterChip::new("all", "All", specs.len())).chain(
                keys.iter()
                    .map(|key| FilterChip::new(*key, status_said(key), count(key))),
            ),
            chosen,
        )
        .on_choose(move |index, _, cx| {
            this.update(cx, |this, cx| {
                let picked = index.checked_sub(1).and_then(|at| keys.get(at).copied());
                this.filter = if picked == this.filter { None } else { picked };
                cx.notify();
            })
            .ok();
        })
    }

    /// A row's "⋯" menu: open the spec, or launch its development, which the daemon refuses
    /// with its reason unless the spec is Ready.
    fn menu(&mut self, key: &str, summary: &Summary, cx: &mut Context<Self>) -> Entity<Menu> {
        let (open, launching) = (self.shell.clone(), self.shell.clone());
        let (project, spec) = (summary.project.clone(), summary.spec);
        crate::commands::menu_of(&mut self.menus, "specs", key, cx, move |key, menu| {
            menu.entry(format!("{key}-open"), "Open", None, move |window, cx| {
                open_spec(&open, &project, spec, window, cx)
            })
            .entry(
                format!("{key}-develop"),
                "Launch development",
                None,
                move |_, cx| launch(&launching, spec, cx),
            )
        })
    }

    pub fn open_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.new_open = true;
        let focus = self.idea.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn close_new(&mut self, cx: &mut Context<Self>) {
        self.new_open = false;
        cx.notify();
    }

    fn start_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let idea = self.idea.read(cx).value().trim().to_owned();
        if idea.is_empty() {
            return;
        }
        self.idea
            .update(cx, |field, cx| field.set_value("", window, cx));
        self.new_open = false;
        ask(
            &self.shell,
            "spec.new",
            json!({"project": self.project, "idea": idea}),
            Then::New,
            cx,
        );
        cx.notify();
    }

    fn new_modal(&self, cx: &mut Context<Self>) -> Modal {
        let this = cx.weak_entity();
        Modal::new("new-spec", "New spec")
            .sentence(
                "Say in a sentence what to build: a session reads the code, asks you what only \
                 you can decide, and writes the spec.",
            )
            .open(self.new_open)
            .on_close({
                let this = this.clone();
                move |_, cx| {
                    this.update(cx, |this, cx| this.close_new(cx)).ok();
                }
            })
            .action(Button::primary("new-spec-start", "Start writing").on_click(
                move |_, window, cx| {
                    this.update(cx, |this, cx| this.start_new(window, cx)).ok();
                },
            ))
            .child(Field::new(
                "new-spec-idea",
                "What to build",
                field_frame("new-spec-idea-input", &self.idea, cx),
            ))
    }
}

impl Render for SpecsTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let at = cx.background_executor().now();
        let reduce = cx.reduce_motion();
        let all = registry_of(&self.shell, cx)
            .map(|registry| registry.read(cx).specs().of(&self.project))
            .unwrap_or_default();
        let chips = self.chips(&all, cx);
        let shown = self.filter;
        self.list.sync(
            all.into_iter()
                .filter(|summary| shown.is_none_or(|status| summary.status == status))
                .map(|summary| (format!("spec-{}", summary.spec), summary)),
            at,
            reduce,
        );
        let present = !self.list.is_empty();
        let rows: Vec<(String, Summary, gpui_design_system::Placement)> = self
            .list
            .rows()
            .map(|row| (row.key.to_owned(), row.value.clone(), row.placement))
            .collect();
        let menus: Vec<Entity<Menu>> = rows
            .iter()
            .map(|(key, summary, _)| self.menu(key, summary, cx))
            .collect();
        let card = rows.into_iter().zip(menus).fold(
            ListCard::new("project-specs-list"),
            |card, ((key, summary, placement), menu)| {
                let p = palette(cx);
                let facts = facts_line(format!("{key}-facts"), summary.facts(), cx);
                let (shell, project, spec) =
                    (self.shell.clone(), summary.project.clone(), summary.spec);
                let row = ListRow::new(ElementId::Name(key.clone().into()), summary.title.clone())
                    .leading(icon(Icon::File).size(px(14.)).text_color(p.faint))
                    .below(facts)
                    .more(menu)
                    .on_click(move |_, window, cx| open_spec(&shell, &project, spec, window, cx));
                card.item(
                    ElementId::Name(format!("{key}-item").into()),
                    placement,
                    row,
                )
            },
        );
        let this = cx.weak_entity();
        let new = Button::secondary("specs-new", "New spec")
            .leading(Icon::Plus)
            .on_click(move |_, window, cx| {
                this.update(cx, |this, cx| this.open_new(window, cx)).ok();
            });
        let this = cx.weak_entity();
        let empty = EmptyLine::new(
            "project-specs-empty",
            Icon::File,
            "No spec yet: a spec says what an agent needs to build a task without guessing.",
        )
        .action(Button::secondary("specs-new-first", "New spec").on_click(
            move |_, window, cx| {
                this.update(cx, |this, cx| this.open_new(window, cx)).ok();
            },
        ));
        let known = registry_of(&self.shell, cx)
            .is_some_and(|registry| registry.read(cx).specs().is_known());
        div()
            .flex()
            .flex_col()
            .child(presence(
                "project-specs-new-block",
                present,
                div().pb(space::MD).child(new),
            ))
            .child(presence(
                "project-specs-empty-block",
                known && !present && shown.is_none(),
                empty,
            ))
            .child(presence(
                "project-specs-filter-block",
                known && (present || shown.is_some()),
                div().pb(space::MD).child(chips),
            ))
            .child(presence("project-specs-list-block", present, card))
            .child(self.new_modal(cx))
    }
}

/// "Write the spec" of a ticket of the project: the session starts at once, in the project's
/// default mode. "Write the spec as…" asks for the mode of this one spec first.
pub struct WriteSpec {
    shell: crate::commands::ShellOf,
    project: String,
    /// The ticket being written, as the tracker writes it, and its title.
    ticket: Option<(String, String)>,
    open: bool,
    /// The mode chosen: the ticket is the spec, or the spec is Flottille's alone.
    ticket_is_spec: bool,
    /// The "⋯" menu of each ticket's row, made once and kept while the row is drawn, and the
    /// ticket and the title the row shows now: a menu reads them when it is clicked, so that a row
    /// that shows another ticket later never acts on the first.
    menus: HashMap<String, Entity<Menu>>,
    rows: HashMap<String, (String, String)>,
    /// The rows drawn since `begin_draw`.
    seen: HashSet<String>,
}

impl WriteSpec {
    pub fn new(shell: crate::commands::ShellOf, project: String) -> Self {
        Self {
            shell,
            project,
            ticket: None,
            open: false,
            ticket_is_spec: false,
            menus: HashMap::new(),
            rows: HashMap::new(),
            seen: HashSet::new(),
        }
    }

    /// The row's action: opens the spec that covers the ticket, or starts one at once, in the
    /// default mode.
    pub fn act(&mut self, ticket: String, window: &mut Window, cx: &mut Context<Self>) {
        let covering = registry_of(&self.shell, cx)
            .and_then(|registry| registry.read(cx).specs().of_ticket(&self.project, &ticket));
        match covering {
            Some(spec) => open_spec(&self.shell, &self.project, spec, window, cx),
            None => self.start(ticket, None, cx),
        }
    }

    /// "Write the spec as…": the choice of the mode of this one spec, preselected on the default.
    pub fn act_as(&mut self, row: &str, cx: &mut Context<Self>) {
        let Some((ticket, title)) = self.rows.get(row).cloned() else {
            return;
        };
        let default = registry_of(&self.shell, cx)
            .and_then(|registry| registry.read(cx).setting_in(NAME, &self.project, "mode"));
        self.ticket_is_spec = default.is_some_and(|mode| mode == "ticket");
        self.ticket = Some((ticket, title));
        self.open = true;
        cx.notify();
    }

    /// The "⋯" menu of the row of `ticket`, made once: its one entry is "Write the spec as…".
    pub fn menu(
        &mut self,
        row: &str,
        ticket: &str,
        title: &str,
        cx: &mut Context<Self>,
    ) -> Entity<Menu> {
        let this = cx.weak_entity();
        self.rows
            .insert(row.to_owned(), (ticket.to_owned(), title.to_owned()));
        self.seen.insert(row.to_owned());
        menu_of(&mut self.menus, "ticket-spec", row, cx, move |row, menu| {
            let row_of_menu = row.to_owned();
            menu.entry(
                format!("{row}-spec-as"),
                "Write the spec as…",
                Some(Icon::Edit),
                move |_, cx| {
                    this.update(cx, |this, cx| this.act_as(row_of_menu.as_str(), cx))
                        .ok();
                },
            )
        })
    }

    /// A list of rows is about to be drawn: the menus of the rows it no longer draws go at
    /// [`end_draw`](Self::end_draw).
    pub fn begin_draw(&mut self) {
        self.seen.clear();
    }

    /// The rows are drawn: only their menus and tickets are kept.
    pub fn end_draw(&mut self) {
        let seen = &self.seen;
        self.menus.retain(|row, _| seen.contains(row));
        self.rows.retain(|row, _| seen.contains(row));
    }

    /// The rows whose menu is kept.
    pub fn kept_rows(&self) -> usize {
        self.menus.len()
    }

    /// The ticket and the title the choice of a mode is open on, while it is.
    pub fn choosing(&self) -> Option<(&str, &str)> {
        self.ticket
            .as_ref()
            .filter(|_| self.open)
            .map(|(ticket, title)| (ticket.as_str(), title.as_str()))
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        cx.notify();
    }

    /// Starts the session; `mode` is the one chosen for this spec, none to take the default.
    fn start(&mut self, ticket: String, mode: Option<&str>, cx: &mut Context<Self>) {
        self.open = false;
        ask(
            &self.shell,
            "spec.from_ticket",
            json!({"project": self.project, "ticket": ticket, "mode": mode}),
            Then::FromTicket { ticket },
            cx,
        );
        cx.notify();
    }

    /// The choice's button: the spec starts in the mode chosen.
    fn start_chosen(&mut self, cx: &mut Context<Self>) {
        let Some((ticket, _)) = self.ticket.clone() else {
            return;
        };
        let mode = if self.ticket_is_spec {
            "ticket"
        } else {
            "internal"
        };
        self.start(ticket, Some(mode), cx);
    }
}

impl Render for WriteSpec {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.weak_entity();
        let (ticket, title) = self.ticket.clone().unwrap_or_default();
        let modes = RadioChoices::new("write-spec-mode", "Where the spec lives")
            .choice(
                "internal",
                "Kept in Flottille",
                "The ticket is read, never written. At the end, a draft comment lists where the \
                 spec departs from it; you post it or not.",
            )
            .choice(
                "ticket",
                "Written to the ticket",
                "Each Ready writes the spec as the ticket's description, after reading it again.",
            )
            .chosen(usize::from(self.ticket_is_spec))
            .on_choose({
                let this = this.clone();
                move |at, _, cx| {
                    this.update(cx, |this, cx| {
                        this.ticket_is_spec = at == 1;
                        cx.notify();
                    })
                    .ok();
                }
            });
        Modal::new("write-spec", format!("Write the spec of {ticket}"))
            .sentence(format!(
                "{title}. A session reads the ticket and the code, asks you what only you can \
                 decide, and writes the spec."
            ))
            .open(self.open)
            .on_close({
                let this = this.clone();
                move |_, cx| {
                    this.update(cx, |this, cx| this.close(cx)).ok();
                }
            })
            .action(
                Button::primary("write-spec-start", "Write the spec").on_click(move |_, _, cx| {
                    this.update(cx, |this, cx| this.start_chosen(cx)).ok();
                }),
            )
            .child(Field::new(
                "write-spec-mode-field",
                "Where the spec lives",
                modes,
            ))
    }
}

/// The action of a ticket's row: its label, and what it does.
pub fn ticket_action(
    shell: &WeakEntity<Shell>,
    write: &Entity<WriteSpec>,
    project: &str,
    id: String,
    ticket: String,
    cx: &App,
) -> AnyElement {
    let has = registry_of(shell, cx)
        .and_then(|registry| registry.read(cx).specs().of_ticket(project, &ticket))
        .is_some();
    let label = if has {
        "Open the spec"
    } else {
        "Write the spec"
    };
    let write = write.downgrade();
    let button_id = format!("{id}-spec");
    // The words change once the ticket has a spec: they fade back in.
    changing(
        SharedString::from(format!("{id}-spec-words")),
        label,
        move |label| {
            Button::secondary(SharedString::from(button_id), label)
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    let ticket = ticket.clone();
                    write
                        .update(cx, |write, cx| write.act(ticket, window, cx))
                        .ok();
                })
                .into_any_element()
        },
    )
    .into_any_element()
}

/// The "⋯" menu of a ticket's row: "Write the spec as…", while the ticket has no spec.
pub fn ticket_menu(
    shell: &WeakEntity<Shell>,
    write: &Entity<WriteSpec>,
    project: &str,
    id: &str,
    ticket: &str,
    title: &str,
    cx: &mut App,
) -> Option<Entity<Menu>> {
    let has = registry_of(shell, cx)
        .and_then(|registry| registry.read(cx).specs().of_ticket(project, ticket))
        .is_some();
    (!has).then(|| write.update(cx, |write, cx| write.menu(id, ticket, title, cx)))
}

/// A question the writing session asked.
#[derive(Clone, Debug, PartialEq)]
pub struct Question {
    pub question: u64,
    pub text: String,
    pub options: Vec<String>,
    pub recommended: String,
    pub reason: String,
    /// The `D` it became, once answered.
    pub decision: Option<String>,
}

/// A decision, `D<n>`.
#[derive(Clone, Debug, PartialEq)]
pub struct Decision {
    pub id: String,
    pub question: String,
    pub decision: String,
    pub set_aside: Vec<String>,
}

/// An amendment the session developing the spec proposed.
#[derive(Clone, Debug, PartialEq)]
pub struct Amendment {
    pub amendment: u64,
    /// What it does, in words: "C1 changed", "A new D".
    pub what: String,
    /// The item as it would read.
    pub text: String,
    pub why: String,
    /// `pending`, `accepted` or `refused`.
    pub state: String,
}

/// What a development decided of an item: the day of the last amendment accepted on it, and its
/// text before, none for an item an amendment added: a D's says its question, its decision and
/// the options it set aside.
#[derive(Clone, Debug, PartialEq)]
pub struct Decided {
    pub day: String,
    pub was: Option<String>,
}

impl Decided {
    fn of(item: &Value) -> Option<Self> {
        let last = item["amended"].as_array()?.last()?;
        let said = crate::session::said_at(last["at"].as_u64().unwrap_or_default());
        Some(Self {
            day: said.split(", ").next().unwrap_or(&said).to_owned(),
            was: match &last["was"] {
                Value::String(text) => Some(text.clone()),
                Value::Object(d) => {
                    let text = |key: &str| d.get(key).and_then(Value::as_str).unwrap_or_default();
                    let set_aside: Vec<&str> = d
                        .get("set_aside")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .collect();
                    Some(match set_aside.is_empty() {
                        true => format!("{} {}", text("question"), text("decision")),
                        false => format!(
                            "{} {} Set aside: {}.",
                            text("question"),
                            text("decision"),
                            set_aside.join(", ")
                        ),
                    })
                }
                _ => None,
            },
        })
    }

    fn said(&self) -> String {
        format!("Decided during development, {}", self.day)
    }
}

/// A ticket's words in an element's id: `#5` is `5`.
fn ticket_slug(ticket: &str) -> String {
    ticket
        .trim_start_matches('#')
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// How a section is written.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    /// A text: the Problem, the notes, the departures.
    Text,
    /// A list of items, said as the daemon takes it.
    List,
}

/// A section of a spec the page can edit.
#[derive(Clone, Copy, Debug)]
struct Section_ {
    /// Its name for `spec.write`.
    name: &'static str,
    title: &'static str,
    kind: Kind,
    /// The keys of an item of a list, in the order they are said.
    keys: &'static [&'static str],
}

const EDITABLE: &[Section_] = &[
    Section_ {
        name: "problem",
        title: "Problem",
        kind: Kind::Text,
        keys: &[],
    },
    Section_ {
        name: "behaviour",
        title: "Expected behaviour",
        kind: Kind::List,
        keys: &["id", "text"],
    },
    Section_ {
        name: "decisions",
        title: "Decisions",
        kind: Kind::List,
        keys: &["id", "question", "decision", "set_aside"],
    },
    Section_ {
        name: "out_of_scope",
        title: "Out of scope",
        kind: Kind::List,
        keys: &["text", "reason", "ticket"],
    },
    Section_ {
        name: "notes",
        title: "What the agent found",
        kind: Kind::Text,
        keys: &[],
    },
];

fn editable(name: &str) -> Option<&'static Section_> {
    EDITABLE.iter().find(|section| section.name == name)
}

/// A spec as `spec.show` gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    pub spec: u64,
    pub project: String,
    pub title: String,
    pub status: String,
    pub tickets: Vec<String>,
    pub problem: String,
    pub behaviour: Vec<(String, String)>,
    pub decisions: Vec<Decision>,
    pub out_of_scope: Vec<(String, String)>,
    pub notes: String,
    pub verification: Vec<(String, String)>,
    /// Its own open questions, written by the session.
    pub questions: Vec<String>,
    pub asked: Vec<Question>,
    /// The session that writes it.
    pub writer: Option<String>,
    pub mode: String,
    /// Its last write to its ticket: `written`, `changed` or `failed`, and what that says.
    pub ticket_write: Option<(String, String)>,
    /// An internal spec's comment of where it departs from its tickets.
    pub departures: String,
    pub departures_posted: bool,
    pub amendments: Vec<Amendment>,
    /// The items an accepted amendment changed or added, by identifier.
    pub decided: Vec<(String, Decided)>,
    /// What the daemon said, whole: an editor opens a list section on it.
    raw: Value,
}

impl Spec {
    fn of(value: &Value) -> Self {
        let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
        let pairs = |value: &Value| -> Vec<(String, String)> {
            value
                .as_array()
                .into_iter()
                .flatten()
                .map(|item| (text(&item["id"]), text(&item["text"])))
                .collect()
        };
        Self {
            spec: value["spec"].as_u64().unwrap_or_default(),
            project: text(&value["project"]),
            title: text(&value["title"]),
            status: text(&value["status"]),
            tickets: strings(&value["tickets"]),
            problem: text(&value["problem"]),
            behaviour: pairs(&value["behaviour"]),
            decisions: value["decisions"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|d| Decision {
                    id: text(&d["id"]),
                    question: text(&d["question"]),
                    decision: text(&d["decision"]),
                    set_aside: strings(&d["set_aside"]),
                })
                .collect(),
            out_of_scope: value["out_of_scope"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|o| {
                    let why = match (o["reason"].as_str(), o["ticket"].as_str()) {
                        (Some(reason), _) => reason.to_owned(),
                        (None, Some(ticket)) => format!("Taken by {ticket}"),
                        (None, None) => String::new(),
                    };
                    (text(&o["text"]), why)
                })
                .collect(),
            notes: text(&value["notes"]),
            verification: pairs(&value["verification"]),
            questions: strings(&value["questions"]),
            asked: value["asked"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|q| Question {
                    question: q["question"].as_u64().unwrap_or_default(),
                    text: text(&q["text"]),
                    options: strings(&q["options"]),
                    recommended: text(&q["recommended"]),
                    reason: text(&q["reason"]),
                    decision: q["decision"].as_str().map(str::to_owned),
                })
                .collect(),
            writer: value["writer"].as_str().map(str::to_owned),
            mode: text(&value["mode"]),
            ticket_write: value["ticket_write"]["state"]
                .as_str()
                .map(|state| (state.to_owned(), text(&value["ticket_write"]["problem"]))),
            departures: text(&value["departures"]),
            departures_posted: value["departures_posted_at"].is_u64(),
            amendments: value["amendments"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|a| {
                    let content = &a["content"];
                    let behaviour = a["section"] == "behaviour";
                    let what = match (a["item"].as_str(), behaviour) {
                        (Some(item), _) => format!("{item} changed"),
                        (None, true) => "A new C".to_owned(),
                        (None, false) => "A new D".to_owned(),
                    };
                    let proposed = match behaviour {
                        true => text(&content["text"]),
                        false => format!(
                            "{} {}",
                            text(&content["question"]),
                            text(&content["decision"])
                        ),
                    };
                    Amendment {
                        amendment: a["amendment"].as_u64().unwrap_or_default(),
                        what,
                        text: proposed,
                        why: text(&a["why"]),
                        state: text(&a["state"]),
                    }
                })
                .collect(),
            decided: ["behaviour", "decisions"]
                .iter()
                .flat_map(|section| value[*section].as_array().into_iter().flatten())
                .filter_map(|item| Some((text(&item["id"]), Decided::of(item)?)))
                .collect(),
            raw: value.clone(),
        }
    }

    /// What the editor of `section` opens on: the text, or the list as the daemon takes it (an
    /// item keeps its `id` to stay the same item; one with none is a new one).
    fn editable(&self, section: &Section_) -> String {
        match section.kind {
            Kind::Text => self.raw[section.name]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            Kind::List => {
                let keep = |item: &Value| -> Value {
                    let mut kept = serde_json::Map::new();
                    for key in section.keys {
                        if let Some(value) = item.get(*key).filter(|v| !v.is_null()) {
                            kept.insert((*key).to_owned(), value.clone());
                        }
                    }
                    Value::Object(kept)
                };
                let items: Vec<Value> = self.raw[section.name]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(keep)
                    .collect();
                serde_json::to_string_pretty(&items).unwrap_or_else(|_| "[]".to_owned())
            }
        }
    }

    /// What a development decided of the item, if an amendment changed or added it.
    pub fn decided_of(&self, id: &str) -> Option<&Decided> {
        self.decided
            .iter()
            .find(|(item, _)| item == id)
            .map(|(_, decided)| decided)
    }

    /// What its ticket's line says, once a Ready wrote to it or tried.
    pub fn ticket_said(&self) -> Option<String> {
        let (state, said) = self.ticket_write.as_ref()?;
        Some(match state.as_str() {
            "failed" => format!("Not written to its ticket: {said}"),
            "written" => format!("The spec is {said}."),
            _ => format!("{said}."),
        })
    }

    /// Its questions asked and not answered, and its own open ones.
    pub fn open_questions(&self) -> usize {
        self.questions.len() + self.asked.iter().filter(|q| q.decision.is_none()).count()
    }

    /// Why it cannot be made Ready now, while it is a Draft.
    pub fn not_ready(&self) -> Option<String> {
        if self.status != "draft" {
            return None;
        }
        match self.open_questions() {
            0 if self.behaviour.is_empty() => {
                Some("No expected behaviour yet: the session writes it first.".into())
            }
            0 => None,
            1 => Some("1 question is open: answer it first.".into()),
            n => Some(format!("{n} questions are open: answer them first.")),
        }
    }
}

/// A spec's page: the spec as it is, its questions answered in one click, Ready and Back to
/// draft, and the sessions that worked on it.
pub struct SpecPage {
    shell: crate::commands::ShellOf,
    project: String,
    spec: u64,
    shown: Option<Result<Spec, String>>,
    asked: u64,
    shown_at: u64,
    /// The open questions as drawn: one answered folds away.
    questions: Listing<Question>,
    /// The decisions as drawn: one that comes unfolds.
    decisions: Listing<Decision>,
    /// The expected behaviour as drawn: a C an amendment adds unfolds.
    behaviour: Listing<(String, String)>,
    /// The pending amendments as drawn: one answered folds away.
    amendments: Listing<Amendment>,
    /// The amendments whose answer the daemon has not taken yet.
    amending: Vec<u64>,
    /// The items whose old text is unfolded.
    olds: HashSet<String>,
    /// The refused amendments are unfolded.
    refused_open: bool,
    /// The findings of the reviews of its development, which the daemon lists while Review is on
    /// in its project.
    findings: Vec<Value>,
    /// The text answer of each open question.
    texts: HashMap<u64, Entity<InputState>>,
    /// The questions whose answer the daemon has not taken yet.
    answering: Vec<u64>,
    /// Ready or Back to draft was asked, until the daemon answers.
    moving: bool,
    /// The confirmation of Back to draft during a development is open.
    confirming: bool,
    /// The confirmation of overwriting a changed ticket is open.
    overwriting: bool,
    /// A choice on the ticket, or the departures' post, was asked, until the daemon answers.
    writing: bool,
    /// "Launch development" was asked, until the daemon answers.
    launching: bool,
    narrow: bool,
    /// The editor of a section: its field once first opened, the section it is open on, what it
    /// refused, and whether the daemon has the text.
    editor: Option<Entity<TextareaState>>,
    editing: Option<&'static str>,
    edit_error: Option<SharedString>,
    edit_saving: bool,
    /// The picker of a ticket to link: its search once first opened, whether it is open, the
    /// project's tickets once the daemon said them, and the tickets being linked or unlinked.
    link_search: Option<Entity<InputState>>,
    link_open: bool,
    candidates: Option<Result<Vec<ItemRow>, String>>,
    linking: Vec<String>,
    /// The tickets it covers as drawn, and the "⋯" of each row.
    ticket_rows: Listing<String>,
    ticket_menus: HashMap<String, Entity<Menu>>,
    _subscriptions: Vec<gpui_kit::Subscription>,
}

impl SpecPage {
    pub fn new(shell: crate::commands::ShellOf, project: String, spec: u64) -> Self {
        Self {
            shell,
            project,
            spec,
            shown: None,
            asked: 0,
            shown_at: 0,
            questions: Listing::default(),
            decisions: Listing::default(),
            behaviour: Listing::default(),
            amendments: Listing::default(),
            amending: Vec::new(),
            olds: HashSet::new(),
            refused_open: false,
            findings: Vec::new(),
            texts: HashMap::new(),
            answering: Vec::new(),
            moving: false,
            confirming: false,
            overwriting: false,
            writing: false,
            launching: false,
            narrow: false,
            editor: None,
            editing: None,
            edit_error: None,
            edit_saving: false,
            link_search: None,
            link_open: false,
            candidates: None,
            linking: Vec::new(),
            ticket_rows: Listing::default(),
            ticket_menus: HashMap::new(),
            _subscriptions: Vec::new(),
        }
    }

    /// Whether the sections are edited: only a Draft is written, and one write at a time.
    pub fn can_edit(&self) -> bool {
        !self.edit_saving && self.shown().is_some_and(|spec| spec.status == "draft")
    }

    /// Edit on a section: the editor opens on what it says now.
    pub fn open_edit(&mut self, name: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(section), Some(spec)) = (editable(name), self.shown().cloned()) else {
            return;
        };
        if self.editor.is_none() {
            let field = cx.new(|cx| {
                let mut field = TextareaState::new(window, cx).auto_grow(8, 18);
                gpui_design_system::follow_theme(&mut field, cx);
                field
            });
            self._subscriptions
                .push(cx.subscribe(&field, |this, _, event, cx| {
                    if matches!(event, InputEvent::Change) && this.edit_error.take().is_some() {
                        cx.notify();
                    }
                }));
            self.editor = Some(field);
        }
        let text = spec.editable(section);
        if let Some(editor) = &self.editor {
            editor.update(cx, |field, cx| field.set_value(&text, window, cx));
            let focus = editor.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
        }
        self.editing = Some(name);
        self.edit_error = None;
        cx.notify();
    }

    fn close_edit(&mut self, cx: &mut Context<Self>) {
        self.editing = None;
        cx.notify();
    }

    /// Save: the section is written whole, as the command line writes it.
    fn save_edit(&mut self, cx: &mut Context<Self>) {
        let (Some(name), Some(editor)) = (self.editing, self.editor.clone()) else {
            return;
        };
        let Some(section) = editable(name) else {
            return;
        };
        if self.edit_saving {
            return;
        }
        let typed = editor.read(cx).value().to_string();
        let content = match section.kind {
            Kind::Text => Value::String(typed),
            Kind::List => match serde_json::from_str::<Value>(&typed) {
                Ok(list) if list.is_array() => list,
                Ok(_) => {
                    self.edit_error = Some("Say a list: items between [ and ].".into());
                    return cx.notify();
                }
                Err(why) => {
                    self.edit_error =
                        Some(format!("That is not a list the spec can take: {why}").into());
                    return cx.notify();
                }
            },
        };
        self.edit_saving = true;
        self.edit_error = None;
        ask(
            &self.shell,
            "spec.write",
            json!({"spec": self.spec, "section": name, "content": content}),
            Then::Wrote { spec: self.spec },
            cx,
        );
        cx.notify();
    }

    /// Link a ticket…: the picker opens, and the project's tickets are asked.
    pub fn open_link(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.link_search.is_none() {
            let search = cx.new(|cx| styled_input("Search the tickets", window, cx));
            self._subscriptions
                .push(cx.subscribe(&search, |_, _, event, cx| {
                    if matches!(event, InputEvent::Change) {
                        cx.notify();
                    }
                }));
            self.link_search = Some(search);
        }
        if let Some(search) = &self.link_search {
            search.update(cx, |field, cx| field.set_value("", window, cx));
            let focus = search.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
        }
        self.link_open = true;
        self.candidates = None;
        ask(
            &self.shell,
            "items.list",
            json!({"project": self.project}),
            Then::Tickets { spec: self.spec },
            cx,
        );
        cx.notify();
    }

    fn close_link(&mut self, cx: &mut Context<Self>) {
        self.link_open = false;
        cx.notify();
    }

    /// The "⋯" of the row of `ticket`: Unlink.
    fn ticket_menu(&mut self, ticket: &str, cx: &mut Context<Self>) -> Entity<Menu> {
        let this = cx.weak_entity();
        let slug = ticket_slug(ticket);
        let name = ticket.to_owned();
        if let Some(menu) = self.ticket_menus.get(&slug) {
            return menu.clone();
        }
        let id = ElementId::Name(format!("spec-ticket-row-{slug}-more").into());
        let entry = format!("spec-ticket-row-{slug}-unlink");
        let menu = cx.new(|cx| {
            Menu::new(id, "More", cx).entry(entry, "Unlink", Some(Icon::Close), move |_, cx| {
                this.update(cx, |this, cx| this.link(name.clone(), false, cx))
                    .ok();
            })
        });
        self.ticket_menus.insert(slug, menu.clone());
        menu
    }

    /// The tickets the spec covers, each with its ⋯, and Link a ticket…
    fn tickets_section(&mut self, spec: &Spec, cx: &mut Context<Self>) -> Section {
        let p = palette(cx);
        let drawn: Vec<(String, gpui_design_system::Placement, String)> = self
            .ticket_rows
            .rows()
            .map(|row| (row.key.to_owned(), row.placement, row.value.clone()))
            .collect();
        let mut list = ListCard::new("spec-tickets-list");
        for (_, placement, ticket) in drawn {
            let slug = ticket_slug(&ticket);
            let menu = self.ticket_menu(&ticket, cx);
            let row = ListRow::new(
                ElementId::Name(format!("spec-ticket-row-{slug}").into()),
                ticket.clone(),
            )
            .mono()
            .leading(icon(Icon::File).size(px(14.)).text_color(p.faint))
            .more(menu);
            list = list.item(
                ElementId::Name(format!("spec-ticket-item-{slug}").into()),
                placement,
                row,
            );
        }
        let this = cx.weak_entity();
        let link = Button::secondary("spec-link-ticket", "Link a ticket…")
            .leading(Icon::Plus)
            .disabled(spec.status != "draft")
            .on_click(move |_, window, cx| {
                this.update(cx, |this, cx| this.open_link(window, cx)).ok();
            });
        Section::new("spec-tickets", "Tickets")
            .count(self.ticket_rows.len().to_string())
            .sentence("The tickets this spec covers.")
            .action(link)
            .child(list)
    }

    /// The editor of a section: its text in a box, and what the daemon refused under it.
    fn edit_modal(&self, cx: &mut Context<Self>) -> Modal {
        let this = cx.weak_entity();
        let section = self.editing.and_then(editable);
        let title = section.map_or("Edit".to_owned(), |s| {
            format!("Edit {}", s.title.to_lowercase())
        });
        let sentence = match section.map(|s| s.kind) {
            Some(Kind::List) => {
                "One entry per item, between [ and ]. Keep an item's id to keep the item; leave it out for a new one."
            }
            _ => "The section is written whole when you save.",
        };
        let p = palette(cx);
        let editor = self.editor.clone();
        let focusing = editor.clone();
        let text = div()
            .id("spec-edit-text")
            .min_h(px(160.))
            .px(space::MD)
            .py(space::MD)
            .rounded(px(10.))
            .bg(p.field)
            .border_1()
            .border_color(p.line2)
            .text_size(px(13.))
            .line_height(px(20.))
            .cursor_text()
            .on_click(move |_, window, cx| {
                if let Some(editor) = &focusing {
                    let focus = editor.read(cx).focus_handle(cx);
                    window.focus(&focus, cx);
                }
            })
            .children(editor)
            .test_support();
        let error = problem_line("spec-edit-error", self.edit_error.clone(), px(0.), cx);
        Modal::new("spec-edit-modal", title)
            .wide()
            .sentence(sentence)
            .open(self.editing.is_some())
            .on_close({
                let this = this.clone();
                move |_, cx| {
                    this.update(cx, |this, cx| this.close_edit(cx)).ok();
                }
            })
            .action(
                Button::primary("spec-edit-save", "Save")
                    .disabled(self.edit_saving)
                    .on_click(move |_, _, cx| {
                        this.update(cx, |this, cx| this.save_edit(cx)).ok();
                    }),
            )
            .child(Field::new("spec-edit-field", "Text", text))
            .kept(error)
    }

    /// The picker of a ticket to link: a search among the project's tickets, those the spec
    /// covers left out.
    fn link_modal(&self, cx: &mut Context<Self>) -> Modal {
        let this = cx.weak_entity();
        let p = palette(cx);
        let query = self
            .link_search
            .as_ref()
            .map(|search| search.read(cx).value().to_string())
            .unwrap_or_default();
        let covered: Vec<String> = self.shown().map(|s| s.tickets.clone()).unwrap_or_default();
        let mut list = ListCard::new("spec-link-list");
        let mut count = 0;
        let said = match &self.candidates {
            None => Some("Reading the tickets…".to_owned()),
            Some(Err(why)) => Some(format!("The tickets cannot be read: {why}")),
            Some(Ok(rows)) => {
                let mut found: Vec<(u32, &ItemRow)> = rows
                    .iter()
                    .filter(|row| !covered.contains(&row.label()))
                    .filter_map(|row| {
                        gpui_design_system::fuzzy::find(&query, &row.title, &row.label())
                            .map(|found| (found.score, row))
                    })
                    .collect();
                found.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
                for (_, row) in found {
                    count += 1;
                    let (slug, ticket) = (ticket_slug(&row.label()), row.label());
                    let this = this.clone();
                    list = list.row(
                        ListRow::new(
                            ElementId::Name(format!("spec-link-pick-{slug}").into()),
                            row.title.clone(),
                        )
                        .line(ticket.clone())
                        .on_click(move |_, _, cx| {
                            let ticket = ticket.clone();
                            this.update(cx, |this, cx| this.link(ticket, true, cx)).ok();
                        }),
                    );
                }
                (count == 0).then(|| "No ticket to link.".to_owned())
            }
        };
        let search = self.link_search.clone();
        let body = div()
            .flex()
            .flex_col()
            .gap(space::MD)
            .children(search.map(|search| {
                Field::new(
                    "spec-link-search-field",
                    "Search the tickets",
                    field_frame("spec-link-search", &search, cx),
                )
            }))
            .child(list)
            .children(said.map(|said| {
                div()
                    .id("spec-link-said")
                    .text_size(px(12.5))
                    .text_color(p.mute)
                    .child(SharedString::from(said))
            }));
        Modal::new("spec-link-modal", "Link a ticket")
            .sentence("The spec covers the ticket you pick, as the ones it has.")
            .open(self.link_open)
            .on_close(move |_, cx| {
                this.update(cx, |this, cx| this.close_link(cx)).ok();
            })
            .child(body)
    }

    /// Covers `ticket` too (`link`), or no more.
    fn link(&mut self, ticket: String, link: bool, cx: &mut Context<Self>) {
        if self.linking.contains(&ticket) {
            return;
        }
        self.linking.push(ticket.clone());
        self.link_open = false;
        ask(
            &self.shell,
            if link { "spec.link" } else { "spec.unlink" },
            json!({"spec": self.spec, "ticket": ticket}),
            Then::Linked {
                spec: self.spec,
                link,
            },
            cx,
        );
        cx.notify();
    }

    pub fn spec(&self) -> u64 {
        self.spec
    }

    pub fn project(&self) -> &str {
        &self.project
    }

    /// Whether the daemon answered a read.
    pub fn is_read(&self) -> bool {
        matches!(self.shown, Some(Ok(_)))
    }

    pub fn shown(&self) -> Option<&Spec> {
        self.shown.as_ref().and_then(|shown| shown.as_ref().ok())
    }

    /// Whether Ready can be clicked: a Draft with no open question and one expected behaviour.
    pub fn can_ready(&self) -> bool {
        !self.moving
            && self
                .shown()
                .is_some_and(|spec| spec.status == "draft" && spec.not_ready().is_none())
    }

    pub fn set_narrow(&mut self, narrow: bool, cx: &mut Context<Self>) {
        if self.narrow != narrow {
            self.narrow = narrow;
            cx.notify();
        }
    }

    /// Asks the daemon for the spec.
    pub fn read(&mut self, cx: &mut App) {
        self.asked += 1;
        let then = Then::Shown {
            spec: self.spec,
            asked: self.asked,
        };
        ask(
            &self.shell,
            "spec.show",
            json!({"spec": self.spec}),
            then,
            cx,
        );
        // The daemon lists no review of a project where Review is off.
        let then = Then::Findings { spec: self.spec };
        ask(
            &self.shell,
            "review.list",
            json!({"spec": self.spec}),
            then,
            cx,
        );
    }

    /// The findings its page shows.
    pub fn findings(&self) -> &[Value] {
        &self.findings
    }

    /// What the daemon answered to something the page asked.
    pub fn answered(&mut self, then: Then, answer: Result<Value, String>, cx: &mut Context<Self>) {
        match then {
            Then::Findings { .. } => {
                let findings: Vec<Value> = answer
                    .map(|listed| {
                        listed["reviews"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .flat_map(|review| {
                                review["findings"].as_array().cloned().unwrap_or_default()
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if self.findings != findings {
                    self.findings = findings;
                    cx.notify();
                }
            }
            Then::Shown { asked, .. } => {
                if asked < self.shown_at {
                    return;
                }
                self.shown_at = asked;
                let shown = answer.map(|spec| Spec::of(&spec));
                if self.shown.as_ref() != Some(&shown) {
                    self.shown = Some(shown);
                    cx.notify();
                }
            }
            Then::Answer { question, .. } => {
                self.answering.retain(|q| *q != question);
                match answer {
                    Ok(spec) => {
                        let spec = Spec::of(&spec);
                        let became = spec
                            .asked
                            .iter()
                            .find(|q| q.question == question)
                            .and_then(|q| q.decision.clone())
                            .unwrap_or_default();
                        toast(
                            Toast::success("Answer sent").detail(format!(
                                "It is now {became}; the session writing the spec is told."
                            )),
                            cx,
                        );
                        self.shown = Some(Ok(spec));
                    }
                    Err(why) => toast(Toast::failure("The answer was not taken").detail(why), cx),
                }
                cx.notify();
            }
            Then::Ready { .. } | Then::Draft { .. } => {
                self.moving = false;
                let ready = matches!(then, Then::Ready { .. });
                match answer {
                    Ok(spec) => {
                        let spec = Spec::of(&spec);
                        let state = spec.ticket_write.as_ref().map(|(state, _)| state.as_str());
                        match (ready, state) {
                            // Nothing reached its ticket, whether it failed or someone changed it.
                            (true, Some("failed" | "changed")) => toast(
                                Toast::failure("The ticket was not written")
                                    .detail(spec.ticket_said().unwrap_or_default()),
                                cx,
                            ),
                            (true, Some(_)) => toast(
                                Toast::success("Spec ready")
                                    .detail(spec.ticket_said().unwrap_or_default()),
                                cx,
                            ),
                            (true, None) => toast(
                                Toast::success("Spec ready").detail("No agent writes it any more."),
                                cx,
                            ),
                            (false, _) => toast(
                                Toast::success("Spec back to Draft").detail(
                                    "The session writing it is told; it may write it again.",
                                ),
                                cx,
                            ),
                        }
                        self.shown = Some(Ok(spec));
                    }
                    Err(why) => {
                        let title = match ready {
                            true => "The spec was not made Ready",
                            false => "The spec was not sent back to Draft",
                        };
                        toast(Toast::failure(title).detail(why), cx);
                    }
                }
                cx.notify();
            }
            Then::Ticket { choice, .. } => {
                self.writing = false;
                match answer {
                    Ok(spec) => {
                        let spec = Spec::of(&spec);
                        let (title, detail) = match choice {
                            "take" => (
                                "Change taken",
                                "The spec is a Draft again; the session writing it is told the \
                                 ticket's new text."
                                    .to_owned(),
                            ),
                            _ => ("Ticket overwritten", spec.ticket_said().unwrap_or_default()),
                        };
                        match spec.ticket_write.as_ref().map(|(state, _)| state.as_str()) {
                            Some("failed") => toast(
                                Toast::failure("The ticket was not written")
                                    .detail(spec.ticket_said().unwrap_or_default()),
                                cx,
                            ),
                            _ => toast(Toast::success(title).detail(detail), cx),
                        }
                        self.shown = Some(Ok(spec));
                    }
                    Err(why) => toast(Toast::failure("The ticket was not changed").detail(why), cx),
                }
                cx.notify();
            }
            Then::Departures { .. } => {
                self.writing = false;
                match answer {
                    Ok(_) => {
                        toast(
                            Toast::success("Departures posted")
                                .detail("Posted as a comment on the spec's tickets."),
                            cx,
                        );
                        self.read(cx);
                    }
                    Err(why) => toast(Toast::failure("The comment was not posted").detail(why), cx),
                }
                cx.notify();
            }
            Then::Develop { .. } => {
                self.launching = false;
                developed(&answer, cx);
                if let Ok(started) = answer {
                    self.shown = Some(Ok(Spec::of(&started["spec"])));
                }
                cx.notify();
            }
            Then::Amendment {
                amendment, accept, ..
            } => {
                self.amending.retain(|a| *a != amendment);
                match answer {
                    Ok(spec) => {
                        let spec = Spec::of(&spec);
                        let (title, detail) = match accept {
                            true => (
                                "Amendment accepted",
                                "It is part of the spec now; the session developing it is told.",
                            ),
                            false => (
                                "Amendment refused",
                                "The spec is unchanged; the session developing it is told.",
                            ),
                        };
                        toast(Toast::success(title).detail(detail), cx);
                        self.shown = Some(Ok(spec));
                    }
                    Err(why) => toast(
                        Toast::failure("The amendment was not answered").detail(why),
                        cx,
                    ),
                }
                cx.notify();
            }
            Then::Wrote { .. } => {
                self.edit_saving = false;
                match answer {
                    Ok(spec) => {
                        self.shown = Some(Ok(Spec::of(&spec)));
                        self.editing = None;
                        toast(Toast::success("Section saved"), cx);
                    }
                    Err(why) => self.edit_error = Some(why.into()),
                }
                cx.notify();
            }
            Then::Linked { link, .. } => {
                self.linking.clear();
                match answer {
                    Ok(spec) => {
                        self.shown = Some(Ok(Spec::of(&spec)));
                        toast(
                            Toast::success(if link {
                                "Ticket linked"
                            } else {
                                "Ticket unlinked"
                            }),
                            cx,
                        );
                    }
                    Err(why) => toast(
                        Toast::failure(if link {
                            "The ticket was not linked"
                        } else {
                            "The ticket was not unlinked"
                        })
                        .detail(why),
                        cx,
                    ),
                }
                cx.notify();
            }
            Then::Tickets { .. } => {
                self.candidates = Some(answer.map(|listed| {
                    listed["items"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(ItemRow::of)
                        .filter(|row| row.kind == "ticket")
                        .collect()
                }));
                cx.notify();
            }
            Then::Listed { .. } | Then::New | Then::FromTicket { .. } => {}
        }
    }

    /// Accepts the amendment into the spec, or refuses it.
    pub fn answer_amendment(&mut self, amendment: u64, accept: bool, cx: &mut Context<Self>) {
        if self.amending.contains(&amendment) {
            return;
        }
        self.amending.push(amendment);
        let then = Then::Amendment {
            spec: self.spec,
            amendment,
            accept,
        };
        let answer = if accept { "accept" } else { "refuse" };
        ask(
            &self.shell,
            "spec.amendment",
            json!({"spec": self.spec, "amendment": amendment, "answer": answer}),
            then,
            cx,
        );
        cx.notify();
    }

    /// A line that unfolds what it holds when clicked, its chevron turning.
    fn toggle(
        &self,
        id: String,
        said: String,
        open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
        flip: impl Fn(&mut Self) + 'static,
    ) -> AnyElement {
        let p = palette(cx);
        let turn = turning(
            SharedString::from(format!("{id}-chevron")),
            open,
            window,
            cx,
        );
        hoverable(
            div()
                .id(SharedString::from(id))
                .flex()
                .items_center()
                .gap(space::SM)
                .cursor_pointer()
                .text_size(px(12.5))
                .aria_label(SharedString::from(said.clone()))
                .aria_expanded(open)
                .child(SharedString::from(said))
                .child(
                    icon(Icon::Chevron)
                        .size(px(12.))
                        .with_transformation(Transformation::rotate(radians(-FRAC_PI_2 * turn))),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    flip(this);
                    cx.notify();
                })),
            move |d, t| d.text_color(color::mix(p.faint, p.mute, t)),
        )
        .into_any_element()
    }

    /// Under an item an amendment changed or added: "Decided during development" with its day,
    /// and its old text folded, which unfolds on a click. It unfolds when the item gets it.
    fn decided(
        &self,
        key: &str,
        decided: Option<&Decided>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = palette(cx);
        let content = match decided {
            None => div().into_any_element(),
            Some(decided) => {
                let open = self.olds.contains(key);
                let old = decided
                    .was
                    .clone()
                    .map(|was| {
                        let item = key.to_owned();
                        let toggle = self.toggle(
                            format!("{key}-was-toggle"),
                            "Old text".to_owned(),
                            open,
                            window,
                            cx,
                            move |this| {
                                if !this.olds.remove(&item) {
                                    this.olds.insert(item.clone());
                                }
                            },
                        );
                        div().flex().flex_col().child(toggle).child(presence(
                            SharedString::from(format!("{key}-was")),
                            open,
                            div()
                                .id(SharedString::from(format!("{key}-was-text")))
                                .aria_label(SharedString::from(was.clone()))
                                .pt(space::XS)
                                .text_size(px(12.5))
                                .text_color(p.faint)
                                .child(SharedString::from(was))
                                .test_support(),
                        ))
                    })
                    .map(IntoElement::into_any_element);
                // Under the item's text, past the column of its number, as `numbered` draws it.
                div()
                    .pt(space::XS)
                    .flex()
                    .gap(space::MD)
                    .child(div().flex_none().w(px(32.)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(space::XS)
                            .child(
                                div()
                                    .id(SharedString::from(format!("{key}-decided")))
                                    .aria_label(SharedString::from(decided.said()))
                                    .text_size(px(12.))
                                    .text_color(p.mute)
                                    .child(SharedString::from(decided.said()))
                                    .test_support(),
                            )
                            .children(old),
                    )
                    .into_any_element()
            }
        };
        presence(
            SharedString::from(format!("{key}-decided-block")),
            decided.is_some(),
            content,
        )
        .into_any_element()
    }

    /// A pending amendment: what it proposes, why, and Accept or Refuse.
    fn amendment_card(&self, amendment: &Amendment, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let n = amendment.amendment;
        let id = format!("spec-amendment-{n}");
        let busy = self.amending.contains(&n);
        let this = cx.weak_entity();
        let accept = Button::primary(SharedString::from(format!("{id}-accept")), "Accept")
            .disabled(busy)
            .on_click({
                let this = this.clone();
                move |_, _, cx| {
                    this.update(cx, |this, cx| this.answer_amendment(n, true, cx))
                        .ok();
                }
            });
        let refuse = Button::secondary(SharedString::from(format!("{id}-refuse")), "Refuse")
            .disabled(busy)
            .on_click(move |_, _, cx| {
                this.update(cx, |this, cx| this.answer_amendment(n, false, cx))
                    .ok();
            });
        div()
            .id(SharedString::from(id.clone()))
            .aria_label(SharedString::from(format!(
                "Amendment {n}: {}",
                amendment.what
            )))
            .p(space::LG)
            .flex()
            .flex_col()
            .gap(space::MD)
            .rounded(px(12.))
            .bg(p.card)
            .border_1()
            .border_color(p.line)
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(p.faint)
                    .child(SharedString::from(format!(
                        "Amendment {n} · {}",
                        amendment.what
                    ))),
            )
            .child(
                div()
                    .id(SharedString::from(format!("{id}-text")))
                    .aria_label(SharedString::from(amendment.text.clone()))
                    .text_size(px(13.5))
                    .child(SharedString::from(amendment.text.clone()))
                    .test_support(),
            )
            .child(
                div()
                    .id(SharedString::from(format!("{id}-why")))
                    .aria_label(SharedString::from(amendment.why.clone()))
                    .text_size(px(12.5))
                    .text_color(p.mute)
                    .child(SharedString::from(format!("Why: {}", amendment.why)))
                    .test_support(),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::SM)
                    .child(accept)
                    .child(refuse),
            )
            .test_support()
            .into_any_element()
    }

    /// "Launch development": a session for the user, in a new workspace, builds the spec.
    pub fn develop(&mut self, cx: &mut Context<Self>) {
        let ready = self.shown().is_some_and(|spec| spec.status == "ready");
        if !ready || self.launching {
            return;
        }
        self.launching = true;
        launch(&self.shell, self.spec, cx);
        cx.notify();
    }

    /// Takes the ticket's change into the spec, or (after its confirmation) writes over it.
    pub fn choose(&mut self, choice: &'static str, cx: &mut Context<Self>) {
        self.overwriting = false;
        if self.writing {
            return;
        }
        self.writing = true;
        let then = Then::Ticket {
            spec: self.spec,
            choice,
        };
        ask(
            &self.shell,
            "spec.ticket",
            json!({"spec": self.spec, "choice": choice}),
            then,
            cx,
        );
        cx.notify();
    }

    /// Posts the departures on the spec's tickets: the user's gesture alone.
    pub fn post_departures(&mut self, cx: &mut Context<Self>) {
        if self.writing {
            return;
        }
        self.writing = true;
        let then = Then::Departures { spec: self.spec };
        ask(
            &self.shell,
            "spec.departures",
            json!({"spec": self.spec, "post": true}),
            then,
            cx,
        );
        cx.notify();
    }

    /// Answers `question` with its option `option` (from 1), or with a text.
    pub fn answer(
        &mut self,
        question: u64,
        option: Option<usize>,
        text: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.answering.contains(&question) {
            return;
        }
        self.answering.push(question);
        let mut params = json!({"spec": self.spec, "question": question});
        match (option, text) {
            (Some(option), _) => params["option"] = json!(option),
            (None, Some(text)) => params["text"] = json!(text),
            (None, None) => return,
        }
        let then = Then::Answer {
            spec: self.spec,
            question,
        };
        ask(&self.shell, "spec.answer", params, then, cx);
        cx.notify();
    }

    fn answer_text(&mut self, question: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(field) = self.texts.get(&question).cloned() else {
            return;
        };
        let text = field.read(cx).value().trim().to_owned();
        if text.is_empty() {
            return;
        }
        field.update(cx, |field, cx| field.set_value("", window, cx));
        self.answer(question, None, Some(text), cx);
    }

    pub fn ready(&mut self, cx: &mut Context<Self>) {
        if !self.can_ready() {
            return;
        }
        self.moving = true;
        let then = Then::Ready { spec: self.spec };
        ask(
            &self.shell,
            "spec.ready",
            json!({"spec": self.spec}),
            then,
            cx,
        );
        cx.notify();
    }

    /// Back to draft: from Ready at once; during a development, after its confirmation.
    pub fn back_to_draft(&mut self, cx: &mut Context<Self>) {
        let Some(spec) = self.shown() else { return };
        if self.moving {
            return;
        }
        match spec.status.as_str() {
            "ready" => self.send_back(cx),
            "in_progress" | "review" => {
                self.confirming = true;
                cx.notify();
            }
            _ => {}
        }
    }

    fn send_back(&mut self, cx: &mut Context<Self>) {
        self.confirming = false;
        self.moving = true;
        let then = Then::Draft { spec: self.spec };
        ask(
            &self.shell,
            "spec.draft",
            json!({"spec": self.spec}),
            then,
            cx,
        );
        cx.notify();
    }

    fn question_card(
        &mut self,
        question: &Question,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = palette(cx);
        let id = format!("spec-question-{}", question.question);
        let busy = self.answering.contains(&question.question);
        let field = self
            .texts
            .entry(question.question)
            .or_insert_with(|| cx.new(|cx| styled_input("Another answer", window, cx)))
            .clone();
        let options = question.options.iter().enumerate().map(|(at, option)| {
            let n = at + 1;
            let option_id = format!("{id}-option-{n}");
            let recommended = *option == question.recommended;
            let this = cx.weak_entity();
            let q = question.question;
            hoverable(
                div()
                    .id(SharedString::from(option_id.clone()))
                    .aria_label(SharedString::from(option.clone()))
                    .px(space::MD)
                    .py(space::SM)
                    .rounded(px(8.))
                    .border_1()
                    .border_color(if recommended { p.run } else { p.line })
                    .flex()
                    .items_center()
                    .gap(space::SM)
                    .cursor_pointer()
                    .text_size(px(13.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(SharedString::from(option.clone())),
                    )
                    .when(recommended, {
                        let tag = format!("{option_id}-recommended");
                        move |d| {
                            d.child(
                                div()
                                    .id(SharedString::from(tag))
                                    .aria_label("Recommended")
                                    .flex_none()
                                    .text_size(px(11.5))
                                    .text_color(p.run)
                                    .child("Recommended")
                                    .test_support(),
                            )
                        }
                    })
                    .on_click(move |_, _, cx| {
                        if busy {
                            return;
                        }
                        this.update(cx, |this, cx| this.answer(q, Some(n), None, cx))
                            .ok();
                    }),
                move |d, t| d.bg(p.hover.opacity(t)),
            )
        });
        let this = cx.weak_entity();
        let q = question.question;
        let own = div()
            .flex()
            .items_end()
            .gap(space::SM)
            .child(div().flex_1().min_w_0().child(Field::new(
                SharedString::from(format!("{id}-text")),
                "Or in your own words",
                field_frame(SharedString::from(format!("{id}-text-input")), &field, cx),
            )))
            .child(
                Button::secondary(SharedString::from(format!("{id}-answer")), "Answer")
                    .disabled(busy)
                    .on_click(move |_, window, cx| {
                        this.update(cx, |this, cx| this.answer_text(q, window, cx))
                            .ok();
                    }),
            );
        div()
            .id(SharedString::from(id.clone()))
            .aria_label(SharedString::from(question.text.clone()))
            .p(space::LG)
            .flex()
            .flex_col()
            .gap(space::MD)
            .rounded(px(12.))
            .bg(p.card)
            .border_1()
            .border_color(p.line)
            .child(
                div()
                    .text_size(px(13.5))
                    .child(SharedString::from(question.text.clone())),
            )
            .child(div().flex().flex_col().gap(space::SM).children(options))
            .child(
                div()
                    .id(SharedString::from(format!("{id}-reason")))
                    .aria_label(SharedString::from(question.reason.clone()))
                    .text_size(px(12.5))
                    .text_color(p.mute)
                    .child(SharedString::from(format!(
                        "Recommended because: {}",
                        question.reason
                    )))
                    .test_support(),
            )
            .child(own)
            .test_support()
            .into_any_element()
    }
}

/// A paragraph of the spec, or a faint line saying it is not written yet.
fn paragraph(id: &str, text: &str, cx: &App) -> AnyElement {
    let p = palette(cx);
    let (said, color) = match text.trim() {
        "" => ("Not written yet.".to_owned(), p.faint),
        text => (text.to_owned(), p.text),
    };
    div()
        .id(SharedString::from(id.to_owned()))
        .aria_label(SharedString::from(said.clone()))
        .text_size(px(13.))
        .text_color(color)
        .child(SharedString::from(said))
        .test_support()
        .into_any_element()
}

/// A numbered line: `C1`, then its text.
fn numbered(id: String, number: &str, text: String, below: Option<String>, cx: &App) -> AnyElement {
    let p = palette(cx);
    let said = match &below {
        Some(below) => format!("{number} {text} {below}"),
        None => format!("{number} {text}"),
    };
    div()
        .id(SharedString::from(id))
        .aria_label(SharedString::from(said))
        .flex()
        .gap(space::MD)
        .text_size(px(13.))
        .child(
            div()
                .flex_none()
                .w(px(32.))
                .text_color(p.faint)
                .child(SharedString::from(number.to_owned())),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(space::XS)
                .child(SharedString::from(text))
                .children(below.map(|below| {
                    div()
                        .text_size(px(12.5))
                        .text_color(p.mute)
                        .child(SharedString::from(below))
                })),
        )
        .test_support()
        .into_any_element()
}

fn block(children: Vec<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(space::MD)
        .children(children)
        .into_any_element()
}

impl Render for SpecPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let pad = if self.narrow { space::LG } else { space::XXL };
        let at = cx.background_executor().now();
        let reduce = cx.reduce_motion();
        let spec = self.shown().cloned();
        let open: Vec<Question> = spec
            .as_ref()
            .map(|spec| {
                spec.asked
                    .iter()
                    .filter(|q| q.decision.is_none())
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        self.questions.sync(
            open.into_iter()
                .map(|q| (format!("spec-question-{}", q.question), q)),
            at,
            reduce,
        );
        let decisions = spec
            .as_ref()
            .map(|spec| spec.decisions.clone())
            .unwrap_or_default();
        self.decisions.sync(
            decisions
                .into_iter()
                .map(|d| (format!("spec-decision-{}", d.id), d)),
            at,
            reduce,
        );
        let behaviour = spec
            .as_ref()
            .map(|spec| spec.behaviour.clone())
            .unwrap_or_default();
        self.behaviour.sync(
            behaviour
                .into_iter()
                .map(|(id, text)| (format!("spec-behaviour-{id}"), (id, text))),
            at,
            reduce,
        );
        let covered = spec
            .as_ref()
            .map(|spec| spec.tickets.clone())
            .unwrap_or_default();
        self.ticket_rows
            .sync(covered.into_iter().map(|t| (t.clone(), t)), at, reduce);
        let pending: Vec<Amendment> = spec
            .as_ref()
            .map(|spec| {
                spec.amendments
                    .iter()
                    .filter(|a| a.state == "pending")
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        self.amendments.sync(
            pending
                .into_iter()
                .map(|a| (format!("spec-amendment-{}", a.amendment), a)),
            at,
            reduce,
        );
        let title = spec
            .as_ref()
            .map_or_else(|| format!("Spec {}", self.spec), |spec| spec.title.clone());
        let status = spec.as_ref().map_or("", |spec| status_said(&spec.status));
        let crumb = {
            let (shell, project) = (self.shell.clone(), self.project.clone());
            hoverable(
                div()
                    .id("spec-crumb")
                    .aria_label(SharedString::from(format!("{} /", self.project)))
                    .cursor_pointer()
                    .child(SharedString::from(format!("{} / Specs /", self.project)))
                    .on_click(move |_, window, cx| {
                        let selection = Selection::Project(project.clone());
                        shell
                            .update(cx, |shell, cx| shell.open(selection, window, cx))
                            .ok();
                    }),
                move |d, t| d.text_color(color::mix(p.faint, p.text, t)),
            )
        };
        // The status is said once, under the title; a status that changes fades back in.
        let chip = changing("spec-status-words", status, move |status| {
            div()
                .id("spec-status")
                .aria_label(status.clone())
                .px(space::SM)
                .rounded(px(6.))
                .border_1()
                .border_color(p.line2)
                .text_size(px(12.))
                .text_color(p.mute)
                .child(status)
                .test_support()
                .into_any_element()
        });
        let tickets = spec
            .as_ref()
            .filter(|spec| !spec.tickets.is_empty())
            .map(|spec| SharedString::from(format!("Tickets {}", spec.tickets.join(", "))));
        let header = PageHeader::new("spec-header", title.clone())
            .crumb(crumb)
            .below(
                div()
                    .flex()
                    .items_center()
                    .gap(space::MD)
                    .child(chip)
                    .children(tickets.map(|tickets| {
                        div().text_size(px(12.5)).text_color(p.faint).child(tickets)
                    })),
            );
        let why = spec.as_ref().and_then(Spec::not_ready);
        let draft = spec.as_ref().is_some_and(|spec| spec.status == "draft");
        let sendable = spec
            .as_ref()
            .is_some_and(|spec| matches!(spec.status.as_str(), "ready" | "in_progress" | "review"));
        let this = cx.weak_entity();
        let ready = Button::primary("spec-ready", "Ready")
            .disabled(!self.can_ready())
            .on_click({
                let this = this.clone();
                move |_, _, cx| {
                    this.update(cx, |this, cx| this.ready(cx)).ok();
                }
            });
        let developable = spec.as_ref().is_some_and(|spec| spec.status == "ready");
        let develop = Button::primary("spec-develop", "Launch development")
            .disabled(self.launching)
            .on_click({
                let this = this.clone();
                move |_, _, cx| {
                    this.update(cx, |this, cx| this.develop(cx)).ok();
                }
            });
        let back = Button::secondary("spec-draft", "Back to draft")
            .disabled(self.moving)
            .on_click({
                let this = this.clone();
                move |_, _, cx| {
                    this.update(cx, |this, cx| this.back_to_draft(cx)).ok();
                }
            });
        let gestures = div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::SM)
                    .child(presence("spec-ready-block", draft, ready))
                    .child(presence("spec-develop-block", developable, develop))
                    .child(presence("spec-draft-block", sendable, back)),
            )
            .child(
                presence_of(
                    "spec-ready-why-block",
                    why.map(SharedString::from),
                    move |why| {
                        div()
                            .id("spec-ready-why")
                            .aria_label(why.clone())
                            .text_size(px(12.5))
                            .text_color(p.mute)
                            .child(why)
                            .test_support()
                            .into_any_element()
                    },
                )
                .space(space::SM),
            );
        // The ticket, in the mode "the ticket is the spec": what its last write became, and the
        // user's choice when someone changed it since Flottille last read it.
        let ticket_said = spec.as_ref().and_then(Spec::ticket_said);
        let ticket_failed = spec
            .as_ref()
            .and_then(|spec| spec.ticket_write.as_ref())
            .is_some_and(|(state, _)| state == "failed");
        let changed = spec
            .as_ref()
            .and_then(|spec| spec.ticket_write.as_ref())
            .is_some_and(|(state, _)| state == "changed");
        let take = Button::secondary("spec-ticket-take", "Take the change")
            .disabled(self.writing)
            .on_click({
                let this = this.clone();
                move |_, _, cx| {
                    this.update(cx, |this, cx| this.choose("take", cx)).ok();
                }
            });
        let overwrite = Button::secondary("spec-ticket-overwrite", "Overwrite the ticket")
            .disabled(self.writing)
            .on_click({
                let this = this.clone();
                move |_, _, cx| {
                    this.update(cx, |this, cx| {
                        this.overwriting = true;
                        cx.notify();
                    })
                    .ok();
                }
            });
        let ticket = stack(space::SM)
            .child(
                presence_of(
                    "spec-ticket-said-block",
                    ticket_said.map(SharedString::from),
                    move |said| {
                        changing("spec-ticket-words", said, move |said| {
                            div()
                                .id("spec-ticket-said")
                                .aria_label(said.clone())
                                .text_size(px(12.5))
                                .text_color(if ticket_failed { p.fail } else { p.mute })
                                .child(said)
                                .test_support()
                                .into_any_element()
                        })
                        .into_any_element()
                    },
                )
                .space(space::SM),
            )
            .child(
                presence(
                    "spec-ticket-choice-block",
                    changed,
                    div()
                        .flex()
                        .items_center()
                        .gap(space::SM)
                        .child(take)
                        .child(overwrite),
                )
                .space(space::SM),
            );
        // The questions: the open ones, each answered in one click; the spec's own open ones.
        let listed: Vec<(String, Question, gpui_design_system::Placement)> = self
            .questions
            .rows()
            .map(|row| (row.key.to_owned(), row.value.clone(), row.placement))
            .collect();
        let mut cards = Vec::new();
        for (key, question, placement) in listed {
            let card = self.question_card(&question, window, cx);
            cards.push(
                gpui_design_system::Item::new(
                    ElementId::Name(format!("{key}-item").into()),
                    placement,
                    card,
                )
                .gap(space::MD)
                .into_any_element(),
            );
        }
        let own_questions: Vec<AnyElement> = spec
            .as_ref()
            .map(|spec| {
                spec.questions
                    .iter()
                    .enumerate()
                    .map(|(at, text)| {
                        numbered(format!("spec-open-{at}"), "?", text.clone(), None, cx)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let open = spec.as_ref().map_or(0, Spec::open_questions);
        let questions = Section::new("spec-questions", "Questions")
            .count(open.to_string())
            .sentence("What only you can decide. Each answer becomes a decision of the spec.")
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(div().flex().flex_col().children(cards))
                    .child(
                        presence(
                            "spec-own-questions-block",
                            !own_questions.is_empty(),
                            block(own_questions),
                        )
                        .space(space::MD),
                    ),
            );
        let (editor_page, can_edit) = (cx.weak_entity(), self.can_edit());
        let edit = move |name: &'static str, id: &'static str| {
            let this = editor_page.clone();
            Button::quiet(id, "Edit")
                .leading(Icon::Edit)
                .disabled(!can_edit)
                .on_click(move |_, window, cx| {
                    this.update(cx, |this, cx| this.open_edit(name, window, cx))
                        .ok();
                })
        };
        let problem = Section::new("spec-problem", "Problem")
            .sentence("Why, in a few lines.")
            .action(edit("problem", "spec-problem-edit"))
            .child(paragraph(
                "spec-problem-text",
                spec.as_ref().map_or("", |spec| spec.problem.as_str()),
                cx,
            ));
        let rows: Vec<(String, (String, String), gpui_design_system::Placement)> = self
            .behaviour
            .rows()
            .map(|row| (row.key.to_owned(), row.value.clone(), row.placement))
            .collect();
        let mut lines = Vec::new();
        for (key, (id, text), placement) in rows {
            let decided = spec.as_ref().and_then(|spec| spec.decided_of(&id)).cloned();
            let line = div()
                .flex()
                .flex_col()
                .child(numbered(key.clone(), &id, text, None, cx))
                .child(self.decided(&key, decided.as_ref(), window, cx));
            lines.push(
                gpui_design_system::Item::new(
                    ElementId::Name(format!("{key}-item").into()),
                    placement,
                    line,
                )
                .gap(space::MD)
                .into_any_element(),
            );
        }
        let behaviour_count = self.behaviour.len();
        let behaviour = Section::new("spec-behaviour", "Expected behaviour")
            .count(behaviour_count.to_string())
            .sentence("Each one sentence that can be checked.")
            .action(edit("behaviour", "spec-behaviour-edit"))
            .child(
                stack(space::MD)
                    .child(
                        presence(
                            "spec-behaviour-none-block",
                            spec.as_ref().is_some_and(|spec| spec.behaviour.is_empty()),
                            paragraph("spec-behaviour-none", "", cx),
                        )
                        .space(space::MD),
                    )
                    .child(spaced(space::MD, div().flex().flex_col().children(lines))),
            );
        let rows: Vec<(String, Decision, gpui_design_system::Placement)> = self
            .decisions
            .rows()
            .map(|row| (row.key.to_owned(), row.value.clone(), row.placement))
            .collect();
        let decision_count = self.decisions.len();
        let mut decision_lines = Vec::new();
        for (key, decision, placement) in rows {
            let mut below = decision.question.clone();
            if !decision.set_aside.is_empty() {
                below = format!("{below} Set aside: {}.", decision.set_aside.join("; "));
            }
            let decided = spec
                .as_ref()
                .and_then(|spec| spec.decided_of(&decision.id))
                .cloned();
            let line = div()
                .flex()
                .flex_col()
                .child(numbered(
                    key.clone(),
                    &decision.id,
                    decision.decision.clone(),
                    Some(below),
                    cx,
                ))
                .child(self.decided(&key, decided.as_ref(), window, cx));
            decision_lines.push(
                gpui_design_system::Item::new(
                    ElementId::Name(format!("{key}-item").into()),
                    placement,
                    line,
                )
                .gap(space::MD)
                .into_any_element(),
            );
        }
        let decisions = Section::new("spec-decisions", "Decisions")
            .count(decision_count.to_string())
            .sentence("Each with the question it answered and the options set aside.")
            .child(
                stack(space::MD)
                    .child(
                        presence(
                            "spec-decisions-none-block",
                            spec.as_ref().is_some_and(|spec| spec.decisions.is_empty()),
                            paragraph("spec-decisions-none", "", cx),
                        )
                        .space(space::MD),
                    )
                    .child(spaced(
                        space::MD,
                        div().flex().flex_col().children(decision_lines),
                    )),
            );
        // The amendments: the pending ones, each accepted or refused in one click, folding away
        // once answered; the refused ones folded under their own line.
        let rows: Vec<(String, Amendment, gpui_design_system::Placement)> = self
            .amendments
            .rows()
            .map(|row| (row.key.to_owned(), row.value.clone(), row.placement))
            .collect();
        let cards: Vec<AnyElement> = rows
            .into_iter()
            .map(|(key, amendment, placement)| {
                let card = self.amendment_card(&amendment, cx);
                gpui_design_system::Item::new(
                    ElementId::Name(format!("{key}-item").into()),
                    placement,
                    card,
                )
                .gap(space::MD)
                .into_any_element()
            })
            .collect();
        let refused: Vec<Amendment> = spec
            .as_ref()
            .map(|spec| {
                spec.amendments
                    .iter()
                    .filter(|a| a.state == "refused")
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let refused_toggle = self.toggle(
            "spec-amendments-refused-toggle".to_owned(),
            format!("Refused ({})", refused.len()),
            self.refused_open,
            window,
            cx,
            |this| this.refused_open = !this.refused_open,
        );
        let refused_lines: Vec<AnyElement> = refused
            .iter()
            .map(|a| {
                let why = format!("{} Why: {}", a.what, a.why);
                numbered(
                    format!("spec-amendment-{}-refused", a.amendment),
                    &a.amendment.to_string(),
                    a.text.clone(),
                    Some(why),
                    cx,
                )
            })
            .collect();
        let has_amendments = spec
            .as_ref()
            .is_some_and(|spec| !spec.amendments.is_empty());
        let amendments = Section::new("spec-amendments", "Amendments")
            .count(self.amendments.len().to_string())
            .sentence(
                "What the session developing the spec proposes from what you decided. Accepted, \
                 it is part of the spec, marked as decided during development.",
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(div().flex().flex_col().children(cards))
                    .child(
                        presence(
                            "spec-amendments-refused-block",
                            !refused.is_empty(),
                            div()
                                .flex()
                                .flex_col()
                                .child(refused_toggle)
                                .child(presence(
                                    "spec-amendments-refused",
                                    self.refused_open,
                                    div().pt(space::SM).child(block(refused_lines)),
                                )),
                        )
                        .space(space::MD),
                    ),
            );
        let outside: Vec<AnyElement> = spec
            .as_ref()
            .map(|spec| {
                spec.out_of_scope
                    .iter()
                    .enumerate()
                    .map(|(at, (text, why))| {
                        let why = (!why.is_empty()).then(|| why.clone());
                        numbered(format!("spec-outside-{at}"), "", text.clone(), why, cx)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let has_outside = !outside.is_empty();
        let outside = Section::new("spec-out-of-scope", "Out of scope")
            .sentence("What the spec leaves out, with its reason or the ticket that takes it.")
            .action(edit("out_of_scope", "spec-out-of-scope-edit"))
            .child(block(outside));
        let verification: Vec<AnyElement> = spec
            .as_ref()
            .map(|spec| {
                spec.verification
                    .iter()
                    .map(|(id, text)| {
                        numbered(
                            format!("spec-verification-{id}"),
                            id,
                            text.clone(),
                            None,
                            cx,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let found = Section::new("spec-found", "What the agent found")
            .sentence("Read by you, not validated: its technical notes and how each C is verified.")
            .action(edit("notes", "spec-notes-edit"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(space::MD)
                    .child(paragraph(
                        "spec-notes",
                        spec.as_ref().map_or("", |spec| spec.notes.as_str()),
                        cx,
                    ))
                    .children((!verification.is_empty()).then(|| block(verification))),
            );
        // Where an internal spec departs from its tickets: read by the user, offered at Done, posted
        // by them alone.
        let departures_text = spec
            .as_ref()
            .filter(|spec| spec.mode == "internal")
            .map(|spec| spec.departures.clone())
            .unwrap_or_default();
        let posted = spec.as_ref().is_some_and(|spec| spec.departures_posted);
        let offered = !posted
            && !departures_text.trim().is_empty()
            && spec.as_ref().is_some_and(|spec| spec.status == "done");
        let posted_said = spec
            .as_ref()
            .filter(|_| posted)
            .map(|spec| SharedString::from(format!("Posted on {}.", spec.tickets.join(", "))));
        let has_departures = !departures_text.trim().is_empty();
        let post = Button::secondary("spec-departures-post", "Post as a comment")
            .disabled(self.writing)
            .on_click({
                let this = this.clone();
                move |_, _, cx| {
                    this.update(cx, |this, cx| this.post_departures(cx)).ok();
                }
            });
        let departures = Section::new("spec-departures", "Departures from the tickets")
            .sentence(
                "Read by you, not validated: where the spec departs from its tickets. Offered at \
                 Done, to post as a comment or not.",
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(paragraph("spec-departures-text", &departures_text, cx))
                    .child(presence("spec-departures-post-block", offered, post).space(space::MD))
                    .child(
                        presence_of("spec-departures-posted-block", posted_said, move |said| {
                            div()
                                .id("spec-departures-posted")
                                .aria_label(said.clone())
                                .text_size(px(12.5))
                                .text_color(p.mute)
                                .child(said)
                                .test_support()
                                .into_any_element()
                        })
                        .space(space::MD),
                    ),
            );
        let writer = spec.as_ref().and_then(|spec| spec.writer.clone());
        let sessions = {
            let rows: Vec<ListRow> = writer
                .iter()
                .map(|agent| {
                    let title = self
                        .shell
                        .upgrade()
                        .and_then(|shell| {
                            shell
                                .read(cx)
                                .snapshot()
                                .session(agent)
                                .map(|session| session.title.clone())
                        })
                        .unwrap_or_else(|| format!("agent {agent}"));
                    let (shell, agent) = (self.shell.clone(), agent.clone());
                    ListRow::new(
                        ElementId::Name(format!("spec-session-{agent}").into()),
                        title,
                    )
                    .leading(icon(glyphs::AGENTS).size(px(14.)).text_color(p.faint))
                    .line("Writes the spec")
                    .on_click(move |_, window, cx| {
                        let selection = Selection::Session(agent.clone());
                        shell
                            .update(cx, |shell, cx| shell.open(selection, window, cx))
                            .ok();
                    })
                })
                .collect();
            let count = rows.len();
            let section = Section::new("spec-sessions", "Sessions")
                .count(count.to_string())
                .sentence("The sessions that worked on the spec; each opens its conversation.")
                .child(section_blocks(
                    "spec-sessions-blocks",
                    count > 0,
                    space::MD,
                    stack(space::MD).child(
                        presence(
                            "spec-sessions-block",
                            count > 0,
                            ListCard::new("spec-sessions-list").rows(rows),
                        )
                        .space(space::MD),
                    ),
                ));
            // Its own box, which the tests measure: the section's root is not an element they find.
            div().id("spec-sessions-box").child(section).test_support()
        };
        let overwrite_confirm = Confirm::new(
            "spec-ticket-confirm",
            format!("Write {title} over the change of its ticket?"),
        )
        .sentence(
            "What someone changed on the ticket since Flottille last read it is lost on the \
             tracker. The spec stays Ready.",
        )
        .action("Overwrite the ticket")
        .open(self.overwriting)
        .on_confirm({
            let this = this.clone();
            move |_, cx| {
                this.update(cx, |this, cx| this.choose("overwrite", cx))
                    .ok();
            }
        })
        .on_cancel({
            let this = this.clone();
            move |_, cx| {
                this.update(cx, |this, cx| {
                    this.overwriting = false;
                    cx.notify();
                })
                .ok();
            }
        });
        let confirm = Confirm::new("spec-draft-confirm", format!("Send {title} back to Draft?"))
            .sentence(
                "Its development session is told, and no amendment is taken while it is a Draft.",
            )
            .action("Back to draft")
            .open(self.confirming)
            .on_confirm({
                let this = this.clone();
                move |_, cx| {
                    this.update(cx, |this, cx| this.send_back(cx)).ok();
                }
            })
            .on_cancel(move |_, cx| {
                this.update(cx, |this, cx| {
                    this.confirming = false;
                    cx.notify();
                })
                .ok();
            });
        let failed = match &self.shown {
            Some(Err(why)) => Some(SharedString::from(format!(
                "The spec cannot be read: {why}"
            ))),
            _ => None,
        };
        let tickets = spec.as_ref().map(|spec| self.tickets_section(spec, cx));
        let edit_modal = self.edit_modal(cx);
        let link_modal = self.link_modal(cx);
        let key = |part: &str| ElementId::Name(format!("enter-spec-{}-{part}", self.spec).into());
        let read = spec.is_some();
        let column = div()
            .max_w(space::COLUMN + pad * 2.)
            .w_full()
            .mx_auto()
            .px(pad)
            .pt(space::XXL)
            .pb(space::XXXL)
            .flex()
            .flex_col()
            .child(enter(
                key("header"),
                0,
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(space::LG)
                            .child(header)
                            .child(gestures)
                            .child(ticket),
                    )
                    .child(
                        presence_of("spec-failed-block", failed, move |why| {
                            div()
                                .id("spec-failed")
                                .aria_label(why.clone())
                                .text_size(px(12.5))
                                .text_color(p.fail)
                                .child(why)
                                .test_support()
                                .into_any_element()
                        })
                        .space(space::LG),
                    ),
            ))
            .child(
                presence(
                    "spec-questions-block",
                    open > 0 || !self.questions.is_empty(),
                    enter(key("questions"), 1, questions),
                )
                .space(space::XXL),
            )
            .child(
                presence(
                    "spec-amendments-block",
                    has_amendments,
                    enter(key("amendments"), 1, amendments),
                )
                .space(space::XXL),
            )
            .child(
                presence(
                    "spec-findings-block",
                    !self.findings.is_empty(),
                    enter(key("findings"), 1, findings_section(&self.findings, p)),
                )
                .space(space::XXL),
            )
            .child(
                presence(
                    "spec-body-block",
                    read,
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(space::XXL)
                                .child(enter(key("problem"), 2, problem))
                                .child(enter(key("behaviour"), 3, behaviour))
                                .child(enter(key("decisions"), 4, decisions))
                                .children(tickets.map(|tickets| enter(key("tickets"), 5, tickets))),
                        )
                        .child(
                            presence(
                                "spec-out-of-scope-block",
                                has_outside,
                                enter(key("out-of-scope"), 5, outside),
                            )
                            .space(space::XXL),
                        )
                        .child(spaced(space::XXL, enter(key("found"), 6, found)))
                        .child(
                            presence(
                                "spec-departures-section-block",
                                has_departures,
                                enter(key("departures"), 6, departures),
                            )
                            .space(space::XXL),
                        )
                        .child(spaced(space::XXL, enter(key("sessions"), 7, sessions))),
                )
                .space(space::XXL),
            );
        div()
            .id("spec-page")
            .size_full()
            .text_color(p.text)
            .overflow_y_scroll()
            .child(column)
            .child(confirm)
            .child(overwrite_confirm)
            .child(edit_modal)
            .child(link_modal)
    }
}

/// The findings of the reviews of a spec's development, under its amendments: each its
/// identifier, severity and confidence, the C or D it is about (or the code), its place, what is
/// wrong and its status, which fades to a new one; each enters as it comes. Findings block
/// nothing: they are said here, never asked about.
fn findings_section(findings: &[Value], p: gpui_design_system::Palette) -> Section {
    let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
    let rows = findings.iter().enumerate().map(|(at, finding)| {
        let id = text(&finding["id"]);
        let review = finding["review"].as_u64().unwrap_or_default();
        let place = match (finding["file"].as_str(), finding["lines"].as_str()) {
            (Some(file), Some(lines)) => format!(" · {file}:{lines}"),
            (Some(file), None) => format!(" · {file}"),
            _ => String::new(),
        };
        let concerns = match finding["quote"].as_str() {
            Some(quote) => format!("{} (\u{201c}{quote}\u{201d})", text(&finding["concerns"])),
            None => text(&finding["concerns"]),
        };
        let head = format!(
            "{id} · {} · {} · {concerns}{place}",
            text(&finding["severity"]),
            text(&finding["confidence"])
        );
        let said = text(&finding["text"]);
        let status = match finding["status"].as_str() {
            Some("settle") => "to settle with you".to_owned(),
            Some(status) => status.to_owned(),
            None => String::new(),
        };
        let key = format!("{review}-{id}");
        enter(
            SharedString::from(format!("spec-finding-{key}")),
            at,
            div()
                .id(SharedString::from(format!("spec-finding-row-{key}")))
                .py(space::SM)
                .flex()
                .flex_col()
                .gap(space::XS)
                .aria_label(SharedString::from(format!("{head} · {status}: {said}")))
                .child(
                    div()
                        .flex()
                        .gap(space::SM)
                        .text_size(px(12.))
                        .text_color(p.faint)
                        .child(head)
                        .child(changing(
                            SharedString::from(format!("spec-finding-status-{key}")),
                            status,
                            |status| div().child(status).into_any_element(),
                        )),
                )
                .child(div().text_size(px(13.)).text_color(p.text).child(said))
                .test_support(),
        )
    });
    Section::new("spec-findings", "Review findings")
        .count(findings.len().to_string())
        .sentence(
            "What the reviews of its development found, each with the C or D it is about. They \
             block nothing: the session was told those that matter.",
        )
        .child(div().flex().flex_col().children(rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_says_why_it_cannot_be_clicked() {
        let spec = |questions: usize, behaviour: bool| {
            Spec::of(&json!({
                "status": "draft",
                "questions": vec!["?"; questions],
                "behaviour": if behaviour { json!([{"id": "C1", "text": "x"}]) } else { json!([]) },
            }))
        };
        assert_eq!(
            spec(1, true).not_ready().as_deref(),
            Some("1 question is open: answer it first.")
        );
        assert_eq!(
            spec(2, true).not_ready().as_deref(),
            Some("2 questions are open: answer them first.")
        );
        assert!(spec(0, false).not_ready().is_some());
        assert_eq!(spec(0, true).not_ready(), None);
    }

    #[test]
    fn a_changed_decision_shows_its_old_question_and_set_aside_options() {
        let d = json!({"id": "D1", "amended": [{"at": 0, "was": {
            "question": "Where do the filters go?", "decision": "Above the list.",
            "set_aside": ["In a side panel"]}}]});

        assert_eq!(
            Decided::of(&d).unwrap().was.as_deref(),
            Some("Where do the filters go? Above the list. Set aside: In a side panel.")
        );
    }
}
