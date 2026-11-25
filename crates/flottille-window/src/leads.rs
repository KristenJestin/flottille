//! The Leads section of a pilot's page (`docs/design.md` §10, "A pilot sums up"): one row per
//! session it leads, in launch order, with its mark, its ticket, its workspace (and its machine's
//! icon when it is another machine), its model, its pull request and checks, its runs and what it
//! waits for in the colour of its mark; a launch of the pilot's held for the user is a row until
//! it starts. A click opens the session. Rows unfold, fold away and slide; their marks and words
//! fade as they change.

use std::collections::HashMap;

use gpui_design_system::motion::PANEL;
use gpui_design_system::{
    Icon, ListCard, ListRow, Listing, MachineMark, MarkSize, Type as _, changing, fade, icon,
    palette, presence_of, section_label, space,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, InteractiveElement as _,
    IntoElement, ParentElement as _, ScrollHandle, SharedString, StatefulInteractiveElement as _,
    Styled as _, TestSupportExt as _, WeakEntity, Window, div, px,
};

use crate::machines::shade;
use crate::model::{HeldLaunch, Home, Session};
use crate::session_link::PullRequest;
use crate::shell::{Selection, Shell};
use crate::state::{AgentState, StateIcon, Status};

/// The section's title.
pub const LEADS: &str = "Leads";

/// The element id of the section, labelled `Leads 2`, its rows counted.
pub fn leads_id() -> ElementId {
    ElementId::Name("session-leads".into())
}

/// The element id of a row of Leads: `key` is the led session's row id, or `held-<number>` for a
/// launch held for the user.
pub fn lead_row_id(key: &str) -> ElementId {
    ElementId::Name(format!("session-lead-{key}").into())
}

/// The element id of the place a row unfolds in, folds away from and slides in.
pub fn lead_item_id(key: &str) -> ElementId {
    ElementId::Name(format!("session-lead-item-{key}").into())
}

/// The element id of a row's ticket key, its tag beside the title.
pub fn lead_ticket_id(key: &str) -> ElementId {
    ElementId::Name(format!("{}-tag-0", lead_row_id(key)).into())
}

/// The element id of a row's workspace and model: `login · Sonnet 5.5 · medium`.
pub fn lead_place_id(key: &str) -> ElementId {
    ElementId::Name(format!("session-lead-place-{key}").into())
}

/// The element id of the mark of the machine a led session runs on, when it is another one,
/// labelled with its name.
pub fn lead_machine_id(key: &str) -> ElementId {
    ElementId::Name(format!("session-lead-machine-{key}").into())
}

/// The element id of a row's pull request and its checks: `#12, checks failing`.
pub fn lead_pull_id(key: &str) -> ElementId {
    ElementId::Name(format!("session-lead-pull-{key}").into())
}

/// The element id of the runs a row's session follows: `run 900`.
pub fn lead_runs_id(key: &str) -> ElementId {
    ElementId::Name(format!("session-lead-runs-{key}").into())
}

/// The element id of what a row's session waits for, in the colour of its mark.
pub fn lead_waits_id(key: &str) -> ElementId {
    ElementId::Name(format!("session-lead-waits-{key}").into())
}

/// One row of Leads.
#[derive(Clone, Debug, PartialEq)]
pub struct LedLine {
    /// The led session's row id, or `held-<number>`.
    pub key: String,
    /// The session a click opens; none for a held launch.
    pub session: Option<String>,
    /// Its state: `None` at rest.
    pub mark: Option<AgentState>,
    /// The key of its ticket, beside the title.
    pub ticket: Option<String>,
    /// Its ticket's title, else its own.
    pub title: String,
    /// Where it works and what it runs on: `login · Sonnet 5.5 · medium`, or for a held launch
    /// `launch held on server · 2 files`; `None` when its machine's list does not say it.
    pub place: Option<String>,
    /// The machine it runs on, when it is another one.
    pub machine: Option<Home>,
    /// What its machine says of its place, its pull request and its runs reaches here: a
    /// session of another machine's list says none of them, and its row leaves them unsaid.
    pub known: bool,
    pub pull: Option<PullRequest>,
    pub runs: Vec<String>,
    /// What it waits for: `needs you: a permission`, `waits for run 900`, `working`, `at rest`.
    pub waits: String,
}

/// Whether `session` is one its pilot `pilot` leads, as its summary counts and its Leads lists:
/// live, and neither a child its tree launched on another machine nor one in another project,
/// which sits among that project's rows and says who leads it there.
fn led_by(session: &Session, pilot: &str) -> bool {
    session.pilot.as_deref() == Some(pilot)
        && session.id != pilot
        && !session.ended
        && !session.led_from_elsewhere
        && !session.child
}

/// The rows of Leads of the pilot `pilot`: each live session it leads, in launch order, after
/// its launches held for the user, which come first as the newest.
pub fn led_lines<'a>(
    pilot: &str,
    sessions: impl IntoIterator<Item = &'a Session>,
    held: &[HeldLaunch],
    homes: &[Home],
) -> Vec<LedLine> {
    let mut led: Vec<&Session> = sessions.into_iter().filter(|s| led_by(s, pilot)).collect();
    led.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
    led.dedup_by(|a, b| a.id == b.id);
    let held = held.iter().map(|launch| {
        let mut place = match &launch.machine {
            Some(machine) => format!("launch held on {machine}"),
            None => "launch held".to_owned(),
        };
        match launch.files {
            0 => {}
            1 => place.push_str(" · 1 file"),
            n => place.push_str(&format!(" · {n} files")),
        }
        LedLine {
            key: format!("held-{}", launch.number),
            session: None,
            mark: Some(AgentState::NeedsYou),
            ticket: None,
            title: launch.title.clone(),
            place: Some(place),
            known: true,
            machine: launch
                .machine
                .as_ref()
                .and_then(|machine| homes.iter().find(|home| &home.machine == machine))
                .cloned(),
            pull: None,
            runs: Vec::new(),
            waits: "needs you: allow its launch".to_owned(),
        }
    });
    let led = led.into_iter().map(|session| {
        let subject = session.subject.as_ref().filter(|s| s.kind == "ticket");
        // A session of another machine's list is known here by its title and its state only:
        // its place, its pull request and its runs are left unsaid, never made up.
        let known = session.elsewhere.is_none();
        let place = session.place.clone().unwrap_or_else(|| "main".to_owned());
        let machine = session
            .elsewhere
            .as_ref()
            .filter(|there| !there.here)
            .map(|there| there.machine.as_str())
            .or(session.tag.as_deref());
        let waits = match session.mark {
            Some(AgentState::NeedsYou) => match session.needs {
                Some(what) => format!("needs you: {what}"),
                None => "needs you".to_owned(),
            },
            Some(AgentState::Waiting) if !session.waits.is_empty() => session.waits.clone(),
            Some(AgentState::Waiting) => "waits".to_owned(),
            Some(_) => "working".to_owned(),
            None => "at rest".to_owned(),
        };
        LedLine {
            key: session.id.clone(),
            session: Some(session.id.clone()),
            mark: session.mark,
            ticket: subject.and_then(|s| s.key.clone()),
            title: subject
                .and_then(|s| s.title.clone())
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| session.title.clone()),
            place: known.then(|| match session.runs.is_empty() {
                true => place,
                false => format!("{place} · {}", session.runs),
            }),
            known,
            machine: machine
                .and_then(|machine| homes.iter().find(|home| home.machine == machine))
                .cloned()
                .or_else(|| {
                    machine.map(|machine| Home {
                        machine: machine.to_owned(),
                        here: false,
                        icon: String::new(),
                        colour: String::new(),
                        state: String::new(),
                        offline: false,
                        project: String::new(),
                    })
                }),
            pull: session.subject.as_ref().and_then(|s| s.pull.clone()),
            runs: session.followed.clone(),
            waits,
        }
    });
    held.chain(led).collect()
}

/// What each pilot says of the sessions it leads, by its row id, from the tree's lines alone, as
/// every client says it (`flottille_core::leads`): the same sessions its Leads lists, wherever
/// their rows sit, a check red when the pull request its line says has failing checks. A pilot
/// that leads none has none.
pub fn summaries<'a>(
    sessions: impl IntoIterator<Item = &'a Session>,
) -> HashMap<String, Vec<flottille_core::leads::Part>> {
    let sessions: Vec<&Session> = sessions.into_iter().collect();
    let mut led: HashMap<&str, Vec<&Session>> = HashMap::new();
    for session in &sessions {
        if let Some(pilot) = session.pilot.as_deref()
            && led_by(session, pilot)
            && !led
                .get(pilot)
                .is_some_and(|counted| counted.iter().any(|s| s.id == session.id))
        {
            led.entry(pilot).or_default().push(session);
        }
    }
    sessions
        .iter()
        .filter_map(|pilot| {
            let counted: Vec<flottille_core::leads::Led> = led
                .get(pilot.id.as_str())?
                .iter()
                .map(|session| flottille_core::leads::Led {
                    needs_you: session.mark == Some(AgentState::NeedsYou),
                    red: session
                        .subject
                        .as_ref()
                        .and_then(|subject| subject.pull.as_ref())
                        .is_some_and(|pull| pull.mark.as_deref() == Some("failing")),
                    // Nothing says who blocks a session yet (#1451).
                    blocked_by: Vec::new(),
                })
                .collect();
            let summary = flottille_core::leads::summary(&counted, pilot.held.len())?;
            Some((pilot.id.clone(), summary))
        })
        .collect()
}

/// [`summaries`] of every session of `snapshot`, this machine's and the other machines'.
pub fn summaries_of(
    snapshot: &crate::model::Snapshot,
) -> HashMap<String, Vec<flottille_core::leads::Part>> {
    summaries(snapshot.sessions.iter().chain(&snapshot.elsewhere))
}

/// How high the card of Leads grows before its rows scroll inside it: about four rows, so that
/// a pilot leading many sessions leaves its conversation in view.
const CARD_HEIGHT: f32 = 240.;

/// What the section holds and draws: its rows as they come, fold and slide, the mark of each,
/// which fades from a state to the next, and the last words of what fades out.
#[derive(Default)]
pub struct Leads {
    rows: Listing<LedLine>,
    marks: HashMap<String, Entity<StateIcon>>,
    statuses: HashMap<String, Entity<Status>>,
    shown: usize,
    /// The last count it said, which its title keeps while the section folds away.
    count_shown: usize,
    /// The last runs each row said, drawn while they fade out.
    runs_shown: HashMap<String, Vec<String>>,
    scroll: ScrollHandle,
}

impl Leads {
    /// How many rows it shows: the sessions led and the launches held.
    pub fn len(&self) -> usize {
        self.shown
    }

    pub fn is_empty(&self) -> bool {
        self.shown == 0
    }

    /// Takes the rows as they are now: a new one unfolds, a missing one folds away, one that
    /// moved slides, and each mark goes to its new state.
    pub fn set<V: 'static>(&mut self, lines: Vec<LedLine>, cx: &mut Context<V>) {
        let now = cx.background_executor().now();
        for line in &lines {
            let state = line.mark.unwrap_or(AgentState::Idle);
            match self.statuses.get(&line.key) {
                Some(status) => {
                    if status.read(cx).state() != state {
                        status.update(cx, |status, cx| status.set(state, cx));
                    }
                }
                None => {
                    let status = cx.new(|_| Status::new(state));
                    let mark = cx.new(|cx| StateIcon::new(status.clone(), cx));
                    self.statuses.insert(line.key.clone(), status);
                    self.marks.insert(line.key.clone(), mark);
                }
            }
            if !line.runs.is_empty() {
                self.runs_shown.insert(line.key.clone(), line.runs.clone());
            }
        }
        self.shown = lines.len();
        if self.shown > 0 {
            self.count_shown = self.shown;
        }
        let reduce = cx.reduce_motion();
        self.rows.sync(
            lines.into_iter().map(|line| (line.key.clone(), line)),
            now,
            reduce,
        );
        // A row gone for good takes its mark and its words with it.
        let listed: std::collections::HashSet<String> =
            self.rows.rows().map(|row| row.value.key.clone()).collect();
        self.marks.retain(|key, _| listed.contains(key));
        self.statuses.retain(|key, _| listed.contains(key));
        self.runs_shown.retain(|key, _| listed.contains(key));
    }

    /// The section: its title with its count, then its card of rows, which scrolls past a few.
    /// It unfolds when the pilot first leads something and folds away once it leads nothing,
    /// its title saying the last count meanwhile.
    pub fn section(
        &self,
        shell: WeakEntity<Shell>,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let p = palette(cx);
        let present =
            (self.shown > 0).then(|| SharedString::from(format!("{LEADS} {}", self.shown)));
        let mut card = ListCard::new("session-leads-card").scrolling(&self.scroll, px(CARD_HEIGHT));
        for listed in self.rows.rows() {
            let line = listed.value;
            let key = line.key.as_str();
            let mark = self.marks.get(key).cloned();
            let mut below = div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_x(space::MD)
                .text_size(px(11.5))
                .text_color(p.faint);
            let place = SharedString::from(line.place.clone().unwrap_or_default());
            let machine = line.machine.as_ref().map(|home| {
                MachineMark::new(
                    lead_machine_id(key),
                    crate::glyphs::device(&home.icon),
                    shade(&home.colour),
                    home.presence(),
                    home.said(),
                )
                .size(MarkSize::Small)
            });
            let glyph = line.place.is_some().then(|| {
                icon(match line.session {
                    Some(_) => Icon::Branch,
                    None => Icon::Clock,
                })
                .size(px(11.))
                .text_color(p.faint)
            });
            below = below.child(
                div()
                    .id(lead_place_id(key))
                    .aria_label(place.clone())
                    .flex()
                    .items_center()
                    .gap(space::XS)
                    .min_w_0()
                    .children(machine)
                    .children(glyph)
                    .when(line.place.is_some(), |d| {
                        d.child(changing(
                            ElementId::Name(format!("session-lead-place-change-{key}").into()),
                            place,
                            |place| {
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(place)
                                    .into_any_element()
                            },
                        ))
                    })
                    .test_support(),
            );
            if line.known {
                below = below.child(pull_words(line, &p));
            }
            // What it follows unfolds and fades in after the rest, its words fading as they
            // change, and fades out once it follows nothing.
            let following = fade(
                ElementId::Name(format!("session-lead-runs-fade-{key}").into()),
                !line.runs.is_empty(),
                PANEL,
                window,
                cx,
            );
            let runs = match line.runs.is_empty() {
                false => Some(line.runs.join(" · ")),
                true => self
                    .runs_shown
                    .get(key)
                    .filter(|_| following > 0.0)
                    .map(|runs| runs.join(" · ")),
            };
            if let Some(runs) = runs {
                let runs = SharedString::from(runs);
                below = below.child(
                    div()
                        .id(lead_runs_id(key))
                        .aria_label(runs.clone())
                        .flex()
                        .items_center()
                        .gap(space::XS)
                        .opacity(following)
                        .child(icon(Icon::Running).size(px(11.)).text_color(p.faint))
                        .child(changing(
                            ElementId::Name(format!("session-lead-runs-change-{key}").into()),
                            runs,
                            |runs| div().child(runs).into_any_element(),
                        ))
                        .test_support(),
                );
            }
            let ink = match line.mark {
                Some(state) => state.colors(&p).0,
                None => p.faint,
            };
            let waits = SharedString::from(line.waits.clone());
            let trailing = changing(
                ElementId::Name(format!("session-lead-waits-change-{key}").into()),
                waits.clone(),
                {
                    let id = lead_waits_id(key);
                    move |waits| {
                        div()
                            .id(id)
                            .aria_label(waits.clone())
                            .max_w(px(220.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .mono(11.5)
                            .text_color(ink)
                            .child(waits)
                            .test_support()
                            .into_any_element()
                    }
                },
            );
            let mut row = ListRow::new(lead_row_id(key), SharedString::from(line.title.clone()))
                .below(below)
                .trailing(trailing);
            if let Some(mark) = mark {
                row = row.leading(mark);
            }
            if let Some(ticket) = &line.ticket {
                row = row.tag(SharedString::from(ticket.clone()));
            }
            if let Some(session) = line.session.clone() {
                let shell = shell.clone();
                row = row.on_click(move |_, window, cx| {
                    let session = session.clone();
                    shell
                        .update(cx, |shell, cx| {
                            shell.open(Selection::Session(session), window, cx)
                        })
                        .ok();
                });
            }
            card = card.item(lead_item_id(key), listed.placement, row);
        }
        // The count's words fade as it changes; while the section folds away, it says the last
        // one, as `presence_of` hands back its last words (the kit's label needs the app, which
        // the closure does not have).
        let count = SharedString::from(format!("{LEADS} {}", self.count_shown));
        let label = section_label(&count, cx).into_any_element();
        let title = changing("session-leads-count", count, move |_| label).into_any_element();
        presence_of("session-leads-presence", present, move |said| {
            div()
                .id(leads_id())
                .aria_label(said)
                .flex()
                .flex_col()
                .px(space::XL)
                .py(space::SM)
                .child(title)
                .child(card)
                .test_support()
                .into_any_element()
        })
        .into_any_element()
    }
}

/// A row's pull request and its checks, `#12, checks failing`, or that it has none yet; its
/// words fade as they change.
fn pull_words(line: &LedLine, p: &gpui_design_system::Palette) -> AnyElement {
    let key = line.key.as_str();
    let (said, number, mark, ink) = match &line.pull {
        Some(pull) => {
            let (mark, ink, checks) = match pull.mark.as_deref() {
                Some("passing") => (Some(Icon::Check), p.ok, ", checks passing"),
                Some("failing") => (Some(Icon::Close), p.fail, ", checks failing"),
                Some("stopped") => (Some(Icon::Close), p.faint, ", checks stopped"),
                Some(_) => (Some(Icon::Running), p.run, ", checks pending"),
                None => (None, p.faint, ""),
            };
            (
                format!("#{}{checks}", pull.number),
                format!("#{}", pull.number),
                mark,
                ink,
            )
        }
        None if line.session.is_some() => (
            "no pull request yet".to_owned(),
            "no pull request yet".to_owned(),
            None,
            p.faint,
        ),
        None => (
            "no session yet".to_owned(),
            "no session yet".to_owned(),
            None,
            p.faint,
        ),
    };
    let said = SharedString::from(said);
    let id = lead_pull_id(key);
    changing(
        ElementId::Name(format!("session-lead-pull-change-{key}").into()),
        said.clone(),
        move |said| {
            div()
                .id(id)
                .aria_label(said)
                .flex()
                .items_center()
                .gap(space::XS)
                .child(SharedString::from(number))
                .children(mark.map(|mark| icon(mark).size(px(11.)).text_color(ink)))
                .test_support()
                .into_any_element()
        },
    )
    .into_any_element()
}
