//! The Leads section of a pilot's page (`docs/design.md` §10, "A pilot sums up"): one row per
//! session it leads, in launch order, with its mark, its ticket, its workspace (and its machine's
//! icon when it is another machine), its model, its pull request and checks, its runs and what it
//! waits for in the colour of its mark; a launch of the pilot's held for the user is a row until
//! it starts. A click opens the session. Rows unfold, fold away and slide; their marks and words
//! fade as they change.

use std::collections::HashMap;

use gpui_design_system::{
    Icon, ListCard, ListRow, Listing, MachineMark, MarkSize, Type as _, changing, icon, palette,
    presence_of, section_label, space,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, InteractiveElement as _,
    IntoElement, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _,
    TestSupportExt as _, WeakEntity, div, px,
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
    /// `launch held on server · 2 files`.
    pub place: String,
    /// The machine it runs on, when it is another one.
    pub machine: Option<Home>,
    pub pull: Option<PullRequest>,
    pub runs: Vec<String>,
    /// What it waits for: `needs you: a permission`, `waits for run 900`, `working`, `at rest`.
    pub waits: String,
}

/// The rows of Leads of the pilot `pilot`: each live session it leads, in launch order, after
/// its launches held for the user, which come first as the newest.
pub fn led_lines<'a>(
    pilot: &str,
    sessions: impl IntoIterator<Item = &'a Session>,
    held: &[HeldLaunch],
    homes: &[Home],
) -> Vec<LedLine> {
    let mut led: Vec<&Session> = sessions
        .into_iter()
        // One it leads in another project sits among that project's rows, which say who leads
        // it, and its pilot's summary does not count it: neither does Leads.
        .filter(|s| {
            s.pilot.as_deref() == Some(pilot) && s.id != pilot && !s.ended && !s.led_from_elsewhere
        })
        .collect();
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
            place,
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
        let place = session.place.clone().unwrap_or_else(|| "main".to_owned());
        let machine = session
            .elsewhere
            .as_ref()
            .filter(|there| !there.here)
            .map(|there| there.machine.as_str())
            .or(session.tag.as_deref());
        let waits = match session.mark {
            Some(AgentState::NeedsYou) => match session.mark_said.strip_prefix("Needs you") {
                Some(what) if !what.trim_start_matches(':').trim().is_empty() => {
                    format!("needs you: {}", what.trim_start_matches(':').trim())
                }
                _ => "needs you".to_owned(),
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
            place: match session.runs.is_empty() {
                true => place,
                false => format!("{place} · {}", session.runs),
            },
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

/// What the section holds and draws: its rows as they come, fold and slide, and the mark of
/// each, which fades from a state to the next.
#[derive(Default)]
pub struct Leads {
    rows: Listing<LedLine>,
    marks: HashMap<String, Entity<StateIcon>>,
    statuses: HashMap<String, Entity<Status>>,
    shown: usize,
}

impl Leads {
    /// How many rows it shows: the sessions led and the launches held.
    pub fn len(&self) -> usize {
        self.shown
    }

    pub fn is_empty(&self) -> bool {
        self.shown == 0
    }

    /// The rows it lists, the ones folding away included.
    pub fn lines(&self) -> Vec<LedLine> {
        self.rows
            .rows()
            .filter(|row| !row.placement.is_gone())
            .map(|row| row.value.clone())
            .collect()
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
        }
        self.shown = lines.len();
        let reduce = cx.reduce_motion();
        self.rows.sync(
            lines.into_iter().map(|line| (line.key.clone(), line)),
            now,
            reduce,
        );
    }

    /// The section: its title with its count, then its card of rows. It unfolds when the pilot
    /// first leads something and folds away once it leads nothing.
    pub fn section(&self, shell: WeakEntity<Shell>, cx: &mut App) -> AnyElement {
        let p = palette(cx);
        let count = self.shown;
        let present = (count > 0).then(|| SharedString::from(format!("{LEADS} {count}")));
        let mut card = ListCard::new("session-leads-card");
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
                .text_color(p.faint)
                .child(
                    div()
                        .id(lead_place_id(key))
                        .aria_label(SharedString::from(line.place.clone()))
                        .flex()
                        .items_center()
                        .gap(space::XS)
                        .min_w_0()
                        .children(line.machine.as_ref().map(|home| {
                            MachineMark::new(
                                lead_machine_id(key),
                                crate::glyphs::device(&home.icon),
                                shade(&home.colour),
                                home.presence(),
                                home.said(),
                            )
                            .size(MarkSize::Small)
                        }))
                        .child(
                            icon(match line.session {
                                Some(_) => Icon::Branch,
                                None => Icon::Clock,
                            })
                            .size(px(11.))
                            .text_color(p.faint),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(SharedString::from(line.place.clone())),
                        )
                        .test_support(),
                );
            let pull = match &line.pull {
                Some(pull) => {
                    let (mark, ink, said) = match pull.mark.as_deref() {
                        Some("passing") => (Some(Icon::Check), p.ok, ", checks passing"),
                        Some("failing") => (Some(Icon::Close), p.fail, ", checks failing"),
                        Some("stopped") => (Some(Icon::Close), p.faint, ", checks stopped"),
                        Some(_) => (Some(Icon::Running), p.run, ", checks pending"),
                        None => (None, p.faint, ""),
                    };
                    let said = SharedString::from(format!("#{}{said}", pull.number));
                    div()
                        .id(lead_pull_id(key))
                        .aria_label(said.clone())
                        .flex()
                        .items_center()
                        .gap(space::XS)
                        .child(SharedString::from(format!("#{}", pull.number)))
                        .child(changing(
                            ElementId::Name(format!("session-lead-checks-{key}").into()),
                            said,
                            move |_| match mark {
                                Some(mark) => {
                                    icon(mark).size(px(11.)).text_color(ink).into_any_element()
                                }
                                None => div().into_any_element(),
                            },
                        ))
                        .test_support()
                        .into_any_element()
                }
                None if line.session.is_some() => div()
                    .id(lead_pull_id(key))
                    .aria_label("no pull request yet")
                    .child("no pull request yet")
                    .test_support()
                    .into_any_element(),
                None => div()
                    .id(lead_pull_id(key))
                    .aria_label("no session yet")
                    .child("no session yet")
                    .test_support()
                    .into_any_element(),
            };
            below = below.child(pull);
            if !line.runs.is_empty() {
                let runs = SharedString::from(line.runs.join(" · "));
                below = below.child(
                    div()
                        .id(lead_runs_id(key))
                        .aria_label(runs.clone())
                        .flex()
                        .items_center()
                        .gap(space::XS)
                        .child(icon(Icon::Running).size(px(11.)).text_color(p.faint))
                        .child(runs)
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
        let title = section_label(&format!("{LEADS} {count}"), cx).into_any_element();
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
