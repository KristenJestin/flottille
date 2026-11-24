//! The rail's "Waiting for you" section, pinned at the rail's bottom: each request that reached
//! the user is a compact card with Details, Deny and Allow, and so is each request of an agent for
//! a vault entry (`vault_ask.rs`), with Details, Dismiss and New entry. The conversation keeps one
//! neutral line per request, which lights its card here; an answered request leaves the rail and
//! its line says the outcome. The section folds away when nothing waits: it is where the page
//! says, once, that the user is needed.
//!
//! The text of a card is what the daemon gave (`says`, the class, the judge's reason, the reasons
//! it was held, who asked); the window only joins the reasons into sentences.
//!
//! A held launch that needs files the receiving machine lacks (its `needs`) lists them on its
//! card, one step each, then the approval: Allow makes each file a vault entry on the caller's
//! machine and shares it, one after the other, then allows the launch (`docs/remote.md`, "What
//! travels"). Each step's mark and word move as it goes; a step that fails says which file and why
//! and the launch stays held, Try again resuming at that step.

use crate::state::{AgentState, StateIcon, Status};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use std::f32::consts::PI;

use gpui_design_system::color::mix;
use gpui_design_system::motion::{Motion, PANEL};
use gpui_design_system::{
    Button, Icon, Item, Listing, Palette, Placement, Type, changed, fade, hoverable, icon, palette,
    presence, space, turning,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, ElementId, Entity, InteractiveElement, IntoElement,
    ParentElement, Render, ScrollHandle, SharedString, StatefulInteractiveElement, Styled, Task,
    TestSupportExt as _, Transformation, WeakEntity, Window, div, point, px, radians,
};

use crate::actions::Answer;
use crate::commands::spaced;
use crate::conversation::VaultAsk;
use crate::runs::rail_label;
use crate::session::{ApprovalCard, SessionPage};
use crate::steps::{StepRow, StepTone};

/// What waits for the user, as the section lists it: a request held for them, or an agent's
/// request for a vault entry with who asked it.
#[derive(Clone, Debug, PartialEq)]
enum Held {
    Request(ApprovalCard),
    Vault(VaultAsk, String),
}

/// How long a card the user was sent to stays lit.
const POINTED: Duration = Duration::from_millis(1600);

/// How long a card whose Allow made and shared its files shows its marks before it folds away.
const LINGER: Duration = Duration::from_millis(900);

/// What a held launch needs shared before it is allowed (its request's `needs`): files of the
/// caller's project `project` on the machine `from`, each made a vault entry there and shared with
/// the machine `to`, where the launch is held.
#[derive(Clone, Debug, PartialEq)]
pub struct Needs {
    pub from: String,
    pub to: String,
    pub project: String,
    pub files: Vec<NeededFile>,
}

/// A file a held launch needs, and the vault entry it is made into.
#[derive(Clone, Debug, PartialEq)]
pub struct NeededFile {
    pub path: String,
    pub entry: String,
}

impl Needs {
    /// The `needs` of a request's call, when it has some.
    pub fn of(needs: &serde_json::Value) -> Option<Needs> {
        let text = |value: &serde_json::Value| value.as_str().map(str::to_owned);
        let files: Vec<NeededFile> = needs["files"]
            .as_array()?
            .iter()
            .filter_map(|file| {
                Some(NeededFile {
                    path: text(&file["path"])?,
                    entry: text(&file["entry"])?,
                })
            })
            .collect();
        (!files.is_empty()).then_some(())?;
        Some(Needs {
            from: text(&needs["from"])?,
            to: text(&needs["to"])?,
            project: text(&needs["project"])?,
            files,
        })
    }
}

/// Where a step of an Allow that shares files is: a file's, or the approval's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Waiting,
    Making,
    Sharing,
    Shared,
    NotMade,
    NotShared,
    Allowing,
    Allowed,
    NotAllowed,
}

impl Step {
    /// The one word its row says.
    pub fn word(self) -> &'static str {
        match self {
            Step::Waiting => "Waiting",
            Step::Making => "Making…",
            Step::Sharing => "Sharing…",
            Step::Shared => "Shared",
            Step::NotMade => "Not made",
            Step::NotShared => "Not shared",
            Step::Allowing => "Allowing…",
            Step::Allowed => "Allowed",
            Step::NotAllowed => "Not allowed",
        }
    }

    fn failed(self) -> bool {
        matches!(self, Step::NotMade | Step::NotShared | Step::NotAllowed)
    }

    /// Its mark: the window's state mark, a quiet ring, the spinner, a check or a cross.
    fn state(self) -> AgentState {
        match self {
            Step::Waiting => AgentState::Idle,
            Step::Making | Step::Sharing | Step::Allowing => AgentState::Running,
            Step::Shared | Step::Allowed => AgentState::Completed,
            Step::NotMade | Step::NotShared | Step::NotAllowed => AgentState::Failed,
        }
    }

    fn tone(self) -> StepTone {
        match self.state() {
            AgentState::Running => StepTone::Now,
            AgentState::Completed => StepTone::Done,
            AgentState::Failed => StepTone::Failed,
            _ => StepTone::Todo,
        }
    }
}

/// The steps of a card that needs files: each file's, then the approval's, each with its mark.
struct Run {
    /// The needs these steps were made for: another list of files is another run.
    needs: Needs,
    /// Allow was clicked on these steps: what an earlier run's Allow still says is not theirs.
    started: bool,
    steps: Vec<Step>,
    marks: Vec<(Entity<Status>, Entity<StateIcon>)>,
}

impl Run {
    fn new(needs: &Needs, cx: &mut Context<Waiting>) -> Run {
        let files = needs.files.len();
        let marks = (0..=files)
            .map(|_| {
                let status = cx.new(|_| Status::new(AgentState::Idle));
                let icon = {
                    let status = status.clone();
                    cx.new(|cx| StateIcon::new(status, cx))
                };
                (status, icon)
            })
            .collect();
        Run {
            needs: needs.clone(),
            started: false,
            steps: vec![Step::Waiting; files + 1],
            marks,
        }
    }

    fn approval(&self) -> Step {
        self.steps[self.steps.len() - 1]
    }

    /// The step `at` (`None`: the approval) is now `step`; its mark follows.
    fn set(&mut self, at: Option<usize>, step: Step, cx: &mut Context<Waiting>) {
        let at = at.unwrap_or(self.steps.len() - 1).min(self.steps.len() - 1);
        self.steps[at] = step;
        self.marks[at]
            .0
            .update(cx, |status, cx| status.set(step.state(), cx));
    }

    fn failed(&self) -> bool {
        self.steps.iter().any(|step| step.failed())
    }
}

pub use flottille_protocol::words::sentences;

pub struct Waiting {
    page: WeakEntity<SessionPage>,
    /// The requests that wait, in the order they were asked.
    cards: Vec<ApprovalCard>,
    /// The requests for a vault entry that wait, each with who asked it, in the order they were
    /// asked, after the requests.
    asks: Vec<(VaultAsk, String)>,
    list: Listing<Held>,
    /// Answers sent and not yet taken.
    busy: HashSet<u64>,
    /// What the daemon said when it refused an answer.
    errors: HashMap<u64, SharedString>,
    /// The card the user was sent to from the conversation: lit for a moment.
    pointed: Option<u64>,
    /// The cards whose details are unfolded.
    open: HashSet<u64>,
    /// The requests for a vault entry whose details are unfolded.
    open_asks: HashSet<u64>,
    /// The steps of the cards that need files, made with the card.
    runs: HashMap<u64, Run>,
    /// The cards allowed with their files, which show their marks a moment before they fold away.
    lingering: HashSet<u64>,
    /// The pulsing dot of a request to judge.
    dot: Entity<StateIcon>,
    scroll: ScrollHandle,
    /// The card whose error and buttons the section is to scroll to, and whether its error was
    /// drawn: the section measures the card once it is, never before.
    reveal: Option<(u64, bool)>,
    /// The section's scroll on its way to a card's buttons, over `PANEL`.
    scrolling: Option<Motion>,
    _lit: Option<Task<()>>,
    renders: usize,
}

impl Waiting {
    pub fn new(page: WeakEntity<SessionPage>, cx: &mut Context<Self>) -> Self {
        let status = cx.new(|_| Status::new(AgentState::NeedsYou));
        Self {
            page,
            cards: Vec::new(),
            asks: Vec::new(),
            list: Listing::default(),
            busy: HashSet::new(),
            errors: HashMap::new(),
            pointed: None,
            open: HashSet::new(),
            open_asks: HashSet::new(),
            runs: HashMap::new(),
            lingering: HashSet::new(),
            dot: cx.new(|cx| StateIcon::new(status, cx)),
            scroll: ScrollHandle::new(),
            reveal: None,
            scrolling: None,
            _lit: None,
            renders: 0,
        }
    }

    /// The requests that wait for the user.
    pub fn cards(&self) -> &[ApprovalCard] {
        &self.cards
    }

    /// The requests for a vault entry that wait for the user.
    pub fn asks(&self) -> impl Iterator<Item = &VaultAsk> {
        self.asks.iter().map(|(ask, _)| ask)
    }

    pub fn renders(&self) -> usize {
        self.renders
    }

    /// How far the section's cards are scrolled, as a distance down from their top.
    pub fn scrolled(&self) -> gpui_kit::Pixels {
        -self.scroll.offset().y
    }

    /// The card the user was sent to, while it is lit.
    pub fn pointed(&self) -> Option<u64> {
        self.pointed
    }

    /// The requests that wait now; nothing is drawn again when they are what was drawn. A card
    /// whose Allow shared its files stays a moment, its marks seen, before it folds away.
    pub fn set(&mut self, mut cards: Vec<ApprovalCard>, cx: &mut Context<Self>) {
        for (at, card) in self.cards.iter().enumerate() {
            let number = card.number;
            let allowed = self
                .runs
                .get(&number)
                .is_some_and(|run| matches!(run.approval(), Step::Allowing | Step::Allowed));
            if cards.iter().any(|c| c.number == number) || !allowed {
                continue;
            }
            cards.insert(at.min(cards.len()), card.clone());
            if self.lingering.insert(number) {
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(LINGER).await;
                    this.update(cx, |this, cx| {
                        this.lingering.remove(&number);
                        this.runs.remove(&number);
                        this.cards.retain(|card| card.number != number);
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
            }
        }
        for card in &cards {
            // A daemon may list other files for the same card: its steps begin again, with none of
            // the marks of the files it listed before.
            match &card.needs {
                Some(needs)
                    if self
                        .runs
                        .get(&card.number)
                        .is_none_or(|run| run.needs != *needs) =>
                {
                    let run = Run::new(needs, cx);
                    self.runs.insert(card.number, run);
                }
                None => {
                    self.runs.remove(&card.number);
                }
                _ => {}
            }
        }
        self.runs
            .retain(|n, _| cards.iter().any(|c| c.number == *n));
        if self.cards == cards {
            return;
        }
        self.busy.retain(|n| cards.iter().any(|c| c.number == *n));
        self.errors
            .retain(|n, _| cards.iter().any(|c| c.number == *n));
        self.open.retain(|n| cards.iter().any(|c| c.number == *n));
        self.cards = cards;
        cx.notify();
    }

    /// The requests for a vault entry that wait now, each with who asked it; one answered leaves.
    pub fn set_asks(&mut self, asks: Vec<(VaultAsk, String)>, cx: &mut Context<Self>) {
        if self.asks == asks {
            return;
        }
        self.open_asks
            .retain(|n| asks.iter().any(|(ask, _)| ask.seq == *n));
        self.asks = asks;
        cx.notify();
    }

    /// The daemon's answer to Allow or Deny on a card: it leaves with the request, or says why not.
    pub fn answered(&mut self, number: u64, refused: Option<String>, cx: &mut Context<Self>) {
        self.busy.remove(&number);
        if let Some(run) = self.runs.get_mut(&number)
            && run.approval() == Step::Allowing
        {
            let step = match refused {
                Some(_) => Step::NotAllowed,
                None => Step::Allowed,
            };
            run.set(None, step, cx);
        }
        match refused {
            Some(why) => {
                self.reveal = Some((number, false));
                self.errors.insert(number, why.into())
            }
            None => self.errors.remove(&number),
        };
        cx.notify();
    }

    /// Lights a card for a moment, scrolled into sight: the user was sent to it from the
    /// conversation.
    pub fn point_at(&mut self, number: u64, cx: &mut Context<Self>) {
        self.pointed = Some(number);
        if let Some(at) = self.cards.iter().position(|c| c.number == number) {
            self.scroll.scroll_to_item(at);
        }
        self._lit = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(POINTED).await;
            this.update(cx, |this, cx| {
                if this.pointed == Some(number) {
                    this.pointed = None;
                    cx.notify();
                }
            })
            .ok();
        }));
        cx.notify();
    }

    /// A step of the Allow of the card `number`: the file `at` (`None`: the approval) is now
    /// `step`; one that failed says why, and the card takes Try again or Deny.
    pub fn allowing(
        &mut self,
        number: u64,
        at: Option<usize>,
        step: Step,
        why: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if let Some(run) = self.runs.get_mut(&number)
            && run.started
        {
            run.set(at, step, cx);
        }
        if let Some(why) = why {
            self.busy.remove(&number);
            self.errors.insert(number, why.into());
            self.reveal = Some((number, false));
        }
        cx.notify();
    }

    /// How many files the card `number` shared, once each one is.
    pub fn shared(&self, number: u64) -> Option<usize> {
        let run = self.runs.get(&number)?;
        let files = &run.steps[..run.steps.len() - 1];
        files
            .iter()
            .all(|step| *step == Step::Shared)
            .then_some(files.len())
    }

    /// Allow on a card that needs files: its steps run from the first not done, those done keep
    /// their mark.
    fn allow_sharing(&mut self, number: u64, needs: Needs, cx: &mut Context<Self>) {
        if !self.busy.insert(number) {
            return;
        }
        self.errors.remove(&number);
        let Some(run) = self.runs.get_mut(&number) else {
            return;
        };
        run.started = true;
        let files = run.steps.len() - 1;
        let from_step = run.steps[..files]
            .iter()
            .position(|step| *step != Step::Shared)
            .unwrap_or(files);
        for at in from_step..=files {
            run.set(Some(at), Step::Waiting, cx);
        }
        self.page
            .update(cx, |page, cx| {
                page.allow_sharing(number, needs, from_step, cx)
            })
            .ok();
        cx.notify();
    }

    fn answer(&mut self, number: u64, answer: Answer, cx: &mut Context<Self>) {
        if !self.busy.insert(number) {
            return;
        }
        self.errors.remove(&number);
        self.page
            .update(cx, |page, cx| page.answer(number, answer, cx))
            .ok();
        cx.notify();
    }

    /// Opens or folds the rest of a card: the judge's words, why it was held, who asked.
    pub fn toggle_details(&mut self, number: u64, cx: &mut Context<Self>) {
        if !self.open.remove(&number) {
            self.open.insert(number);
        }
        cx.notify();
    }

    /// Opens or folds the rest of a request for a vault entry.
    pub fn toggle_ask_details(&mut self, number: u64, cx: &mut Context<Self>) {
        if !self.open_asks.remove(&number) {
            self.open_asks.insert(number);
        }
        cx.notify();
    }

    /// A compact card: what the call does and its number, the call in one line, its class and the
    /// reason in one line, then Details, Deny and Allow. A base request stands apart by its
    /// tag and its still dot, in the same attention tone: it failed nothing.
    fn card(
        &self,
        card: &ApprovalCard,
        p: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let number = card.number;
        let id = |part: &str| ElementId::Name(format!("{part}-{number}").into());
        let busy = self.busy.contains(&number);
        let open = self.open.contains(&number);
        let lit = fade(
            id("approval-lit"),
            self.pointed == Some(number),
            PANEL,
            window,
            cx,
        );
        let turn = turning(id("approval-details"), open, window, cx);
        let button = |part: &str, label: &str, answer: Answer, primary: bool| {
            let button = match primary {
                true => Button::primary(id(part), label.to_owned()),
                false => Button::quiet(id(part), label.to_owned()),
            };
            button
                .disabled(busy)
                .on_click(cx.listener(move |this, _, _, cx| this.answer(number, answer, cx)))
        };
        let tones = held_tones(p);
        let class = match card.base {
            true => "Always asks",
            false => "To judge",
        };
        let ink = tones.ink;
        // The reason in one line: the judge's when one was asked, else the first reason it was
        // held.
        let reason = card
            .judgement
            .clone()
            .or_else(|| {
                card.why.as_deref().map(|why| {
                    why.split_inclusive(". ")
                        .next()
                        .unwrap_or(why)
                        .trim()
                        .to_owned()
                })
            })
            .unwrap_or_default();
        let title = card
            .asked
            .clone()
            .unwrap_or_else(|| format!("Request #{number}"));
        // The call; an action, which changes the project, says it in words instead.
        let (line, from_says) = match (card.kind.as_str(), &card.call) {
            ("call", Some(call)) => (Some(call.clone()), false),
            _ => (card.says.first().cloned(), true),
        };
        let one_line = |d: gpui_kit::Stateful<gpui_kit::Div>| {
            d.min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
        };
        let head = div()
            .flex()
            .items_center()
            .gap(space::SM)
            .child(
                div()
                    .flex_none()
                    .w(px(16.))
                    .flex()
                    .justify_center()
                    .child(self.mark(card.base, p)),
            )
            .child(
                one_line(div().id(id("approval-title")))
                    .flex_1()
                    .text_size(px(13.))
                    .semibold()
                    .aria_label(SharedString::from(title.clone()))
                    .child(SharedString::from(title))
                    .test_support(),
            )
            .child(
                div()
                    .flex_none()
                    .mono(11.)
                    .text_color(p.faint)
                    .child(SharedString::from(format!("#{number}"))),
            );
        let details = div()
            .id(id("approval-more"))
            .pt(space::XS)
            .flex()
            .flex_col()
            .gap(space::SM)
            .text_size(px(12.))
            .text_color(p.mute)
            .children(
                card.says
                    .iter()
                    .enumerate()
                    .skip(usize::from(from_says))
                    .map(|(i, said)| {
                        div()
                            .id(ElementId::Name(
                                format!("approval-says-{number}-{i}").into(),
                            ))
                            .child(SharedString::from(said.clone()))
                            .test_support()
                    }),
            )
            .when(card.base, |d| {
                let says = card.always_says();
                d.child(
                    div()
                        .id(id("approval-base"))
                        .aria_label(says)
                        .text_color(p.fail)
                        .child(says)
                        .test_support(),
                )
            })
            .when_some(card.judgement.clone(), |d, judgement| {
                d.child(
                    div()
                        .id(id("approval-judge"))
                        .aria_label(SharedString::from(judgement.clone()))
                        .child(SharedString::from(judgement))
                        .test_support(),
                )
            })
            .when_some(card.why.clone(), |d, why| {
                d.child(
                    div()
                        .id(id("approval-why"))
                        .aria_label(SharedString::from(why.clone()))
                        .child(SharedString::from(why))
                        .test_support(),
                )
            })
            .when_some(card.needs.as_ref(), |d, needs| {
                let said = format!(
                    "Deny leaves on {} the entries already shared: each is bound to its file \
                     alone.",
                    needs.to
                );
                // Said once a file is shared, and only then: before, Deny leaves nothing.
                let shared = self
                    .runs
                    .get(&number)
                    .is_some_and(|run| run.steps[..run.steps.len() - 1].contains(&Step::Shared));
                let line = div()
                    .id(id("approval-shared-stay"))
                    .aria_label(SharedString::from(said.clone()))
                    .child(SharedString::from(said))
                    .test_support();
                d.child(presence(id("approval-shared-stay-block"), shared, line).space(space::SM))
            })
            .when_some(card.source.clone(), |d, source| {
                d.child(
                    div()
                        .id(id("approval-source"))
                        .mono(11.)
                        .text_color(p.faint)
                        .child(SharedString::from(source))
                        .test_support(),
                )
            });
        let toggle = details_toggle(
            id("approval-details"),
            open,
            turn,
            p,
            cx.listener(move |this, _, _, cx| this.toggle_details(number, cx)),
        );
        let body = div()
            .id(id("approval-card"))
            .flex()
            .flex_col()
            .px(space::MD)
            .py(space::SM)
            .rounded(px(12.))
            .bg(p.card)
            .border_1()
            .border_color(tones.card_edge)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(space::XS)
                    .child(head)
                    // The whole call, wrapping over as many lines as it takes: what is allowed is
                    // read before it is allowed, never guessed from its start.
                    .children(line.map(|line| {
                        div()
                            .id(match from_says {
                                true => ElementId::Name(format!("approval-says-{number}-0").into()),
                                false => id("approval-call"),
                            })
                            .min_w_0()
                            .whitespace_normal()
                            .mono(11.5)
                            .text_color(p.mute)
                            .aria_label(SharedString::from(line.clone()))
                            .child(SharedString::from(line))
                            .test_support()
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(space::SM)
                            .min_w_0()
                            .child(
                                div()
                                    .id(id("approval-class"))
                                    .flex_none()
                                    .aria_label(class)
                                    .mono(10.)
                                    .medium()
                                    .text_color(ink)
                                    .child(class.to_uppercase())
                                    .test_support(),
                            )
                            .child(
                                one_line(div().id(id("approval-reason")))
                                    .text_size(px(12.))
                                    .text_color(p.faint)
                                    .aria_label(SharedString::from(reason.clone()))
                                    .child(SharedString::from(reason))
                                    .test_support(),
                            ),
                    ),
            )
            .children(
                card.needs
                    .as_ref()
                    .zip(self.runs.get(&number))
                    .filter(|(needs, run)| run.needs == **needs)
                    .map(|(needs, run)| {
                        // Other files are another block: it unfolds and fades in as one does.
                        presence(
                            id(&format!("allow-files-block{}", files_key(needs))),
                            true,
                            files_block(number, needs, run, p),
                        )
                        .space(space::XS)
                    }),
            )
            .child(presence(id("approval-more-block"), open, details).space(space::XS))
            .children(self.errors.get(&number).cloned().map(|error| {
                spaced(
                    space::XS,
                    div()
                        .id(id("approval-error"))
                        .aria_label(error.clone())
                        .text_size(px(12.))
                        .text_color(p.fail)
                        .child(error)
                        .test_support(),
                )
            }))
            .child(spaced(
                space::XS,
                div()
                    .pt(space::XS)
                    .flex()
                    .items_center()
                    .gap(space::SM)
                    .child(div().flex_1().child(toggle))
                    .child(button("deny", "Deny", Answer::Deny, false))
                    .child(match card.proposes_a_connection() {
                        // What it would write is read in its dialog before it is accepted.
                        true => Button::primary(id("open-request"), "Open")
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.page
                                    .update(cx, |page, cx| page.open_connection_request(number, cx))
                                    .ok();
                            }))
                            .into_any_element(),
                        false => match card.needs.clone() {
                            Some(needs) => self.allow_button(number, needs, busy, window, cx),
                            None => button("allow", "Allow", Answer::Once, true).into_any_element(),
                        },
                    }),
            ));
        // A glow round the card the user was sent to, which fades in and out.
        div()
            .id(id("approval"))
            .p(space::XS)
            .rounded(px(14.))
            .bg(p.wait.opacity(0.28 * lit))
            .child(body)
            .test_support()
            .into_any_element()
    }

    /// The card of a request for a vault entry: its entry, where the secret goes, its project and
    /// its reason on lines, "Details" for the rest, then Dismiss and New entry in the place of
    /// Deny and Allow.
    fn vault_card(
        &self,
        ask: &VaultAsk,
        source: &str,
        p: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let number = ask.seq;
        let open = self.open_asks.contains(&number);
        let turn = turning(
            ElementId::Name(format!("vault-ask-details-{number}").into()),
            open,
            window,
            cx,
        );
        let Some(shell) = self.page.upgrade().map(|page| page.read(cx).shell()) else {
            return div().into_any_element();
        };
        let buttons = crate::vault_ask::buttons(number, ask, shell, cx);
        let title = match ask.proposes() {
            true => "Proposes a vault edit",
            false => "Asks for a vault entry",
        };
        let head = div()
            .flex()
            .items_center()
            .gap(space::SM)
            .child(
                div()
                    .flex_none()
                    .w(px(16.))
                    .flex()
                    .justify_center()
                    .child(self.mark(false, p)),
            )
            .child(
                div()
                    .id(crate::vault_ask::part(number, "title"))
                    .flex_1()
                    .min_w_0()
                    .text_size(px(13.))
                    .semibold()
                    .aria_label(title)
                    .child(title)
                    .test_support(),
            )
            .child(
                div()
                    .flex_none()
                    .mono(11.)
                    .text_color(p.faint)
                    .child(SharedString::from(format!("#{number}"))),
            );
        let details = div()
            .id(crate::vault_ask::part(number, "more"))
            .pt(space::XS)
            .flex()
            .flex_col()
            .gap(space::SM)
            .text_size(px(12.))
            .text_color(p.mute)
            .children(crate::vault_ask::details(
                number,
                ask,
                format!("asked by {source} · #{number}"),
                p,
            ));
        let toggle = details_toggle(
            crate::vault_ask::part(number, "details"),
            open,
            turn,
            p,
            cx.listener(move |this, _, _, cx| this.toggle_ask_details(number, cx)),
        );
        let body = div()
            .id(crate::vault_ask::part(number, "card"))
            .flex()
            .flex_col()
            .px(space::MD)
            .py(space::SM)
            .rounded(px(12.))
            .bg(p.card)
            .border_1()
            .border_color(held_tones(p).card_edge)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(space::XS)
                    .child(head)
                    .children(crate::vault_ask::lines(number, ask, p)),
            )
            .child(
                presence(
                    ElementId::Name(format!("vault-ask-more-block-{number}").into()),
                    open,
                    details,
                )
                .space(space::XS),
            )
            .child(spaced(
                space::XS,
                match ask.proposes() {
                    // Three buttons and Details do not fit on one line of the rail: Details
                    // stands on a line of its own above them.
                    true => div()
                        .pt(space::XS)
                        .flex()
                        .flex_col()
                        .gap(space::SM)
                        .child(toggle)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_end()
                                .gap(space::SM)
                                .children(buttons),
                        ),
                    false => div()
                        .pt(space::XS)
                        .flex()
                        .items_center()
                        .gap(space::SM)
                        .child(div().flex_1().child(toggle))
                        .children(buttons),
                },
            ));
        div()
            .id(ElementId::Name(format!("vault-ask-{number}").into()))
            .p(space::XS)
            .rounded(px(14.))
            .child(body)
            .test_support()
            .into_any_element()
    }

    /// Allow on a card that needs files: Try again once a step failed, its word fading as it
    /// changes. A device that may only view signs no share: it is off, and says why.
    fn allow_button(
        &self,
        number: u64,
        needs: Needs,
        busy: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = |part: &str| ElementId::Name(format!("{part}-{number}").into());
        let label: SharedString = match self.runs.get(&number).is_some_and(Run::failed) {
            true => "Try again".into(),
            false => "Allow".into(),
        };
        let shown = changed(&id("allow-label"), &label, window, cx);
        // The shares are signed for `from`: its rights are what count, whichever machine is in
        // front.
        let viewer = self
            .page
            .upgrade()
            .map(|page| page.read(cx).shell())
            .and_then(|shell| shell.upgrade())
            .and_then(|shell| {
                let machines = shell.read(cx).machines().read(cx).machines();
                crate::machines::sharing_held(&needs.from, machines)
            });
        let mut button = Button::primary(id("allow"), label)
            .disabled(busy || viewer.is_some())
            .on_click(
                cx.listener(move |this, _, _, cx| this.allow_sharing(number, needs.clone(), cx)),
            );
        if let Some(viewer) = viewer {
            button = button.tooltip(viewer.line);
        }
        div().opacity(shown).child(button).into_any_element()
    }

    /// A step failed: the section scrolls, once, to the card's error and buttons, over `PANEL`.
    /// The error is drawn first and the card measured on the frame after, so the section moves to
    /// where the buttons are; what the user scrolls afterwards is left alone.
    fn reveal_failure(
        &mut self,
        listed: &[(String, Held, Placement)],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(motion) = self.scrolling {
            let now = cx.background_executor().now();
            let offset = self.scroll.offset();
            self.scroll
                .set_offset(point(offset.x, px(motion.value(now))));
            match motion.is_running(now) {
                true => window.request_animation_frame(),
                false => self.scrolling = None,
            }
        }
        match self.reveal {
            Some((number, false)) => {
                self.reveal = Some((number, true));
                window.request_animation_frame();
            }
            Some((number, true)) => {
                self.reveal = None;
                let key = number.to_string();
                let card = listed
                    .iter()
                    .position(|(listed, _, _)| *listed == key)
                    .and_then(|at| self.scroll.bounds_for_item(at));
                let Some(card) = card else { return };
                let frame = self.scroll.bounds();
                let offset = self.scroll.offset();
                let below = card.bottom() - frame.bottom();
                if below <= px(0.) {
                    return;
                }
                let target = (offset.y - below - space::SM)
                    .min(px(0.))
                    .max(-self.scroll.max_offset().y);
                let now = cx.background_executor().now();
                let mut motion = Motion::settled(f32::from(offset.y));
                motion.animate(f32::from(target), PANEL, now, cx.reduce_motion());
                self.scrolling = Some(motion);
                window.request_animation_frame();
            }
            None => {}
        }
    }

    /// The card's mark: the pulsing dot of what waits for the user, or the still one of a base request.
    fn mark(&self, base: bool, p: Palette) -> AnyElement {
        match base {
            true => div()
                .size(px(8.))
                .rounded_full()
                .bg(held_tones(p).ink)
                .into_any_element(),
            false => self.dot.clone().into_any_element(),
        }
    }
}

impl Render for Waiting {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let at = cx.background_executor().now();
        self.list.sync(
            self.cards
                .iter()
                .map(|c| (c.number.to_string(), Held::Request(c.clone())))
                .chain(self.asks.iter().map(|(ask, source)| {
                    (
                        format!("vault-{}", ask.seq),
                        Held::Vault(ask.clone(), source.clone()),
                    )
                })),
            at,
            cx.reduce_motion(),
        );
        let p = palette(cx);
        let present = !self.list.is_empty();
        let listed: Vec<(String, Held, Placement)> = self
            .list
            .rows()
            .map(|row| (row.key.to_owned(), row.value.clone(), row.placement))
            .collect();
        self.reveal_failure(&listed, window, cx);
        let cards: Vec<AnyElement> = listed
            .into_iter()
            .map(|(key, held, placement)| {
                let content = match &held {
                    Held::Request(card) => self.card(card, p, window, cx),
                    Held::Vault(ask, source) => self.vault_card(ask, source, p, window, cx),
                };
                Item::new(
                    ElementId::Name(format!("waiting-item-{key}").into()),
                    placement,
                    content,
                )
                .rising()
                .into_any_element()
            })
            .collect();
        let count = self.cards.len() + self.asks.len();
        let section = div()
            .id("rail-waiting")
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(p.line)
            .pt(space::SM)
            .child(
                div()
                    .id("rail-waiting-title")
                    .aria_label("Waiting for you")
                    .child(rail_label(
                        "Waiting for you",
                        (count > 0).then(|| {
                            div()
                                .mono(11.)
                                .medium()
                                .text_color(p.wait)
                                .child(SharedString::from(count.to_string()))
                                .into_any_element()
                        }),
                        cx,
                    ))
                    .test_support(),
            )
            .child(
                div()
                    .id("rail-waiting-cards")
                    .max_h(px(WAITING_HEIGHT))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .flex()
                    .flex_col()
                    .children(cards),
            )
            .test_support();
        presence("rail-waiting-presence", present, section)
    }
}

/// What tells one list of files from another, for the id of their block.
fn files_key(needs: &Needs) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    needs
        .files
        .iter()
        .for_each(|file| (&file.path, &file.entry).hash(&mut hasher));
    format!("-{:x}", hasher.finish())
}

/// The files a held launch needs, on its card: where they go, what Allow does with them, then one
/// step each and the approval's, each with its mark and its word.
fn files_block(number: u64, needs: &Needs, run: &Run, p: Palette) -> impl IntoElement {
    let id = |part: &str| ElementId::Name(format!("{part}-{number}").into());
    let count = match needs.files.len() {
        1 => "1 file".to_owned(),
        files => format!("{files} files"),
    };
    let head = format!(
        "From {} → to {} · {} · {count}",
        needs.from, needs.to, needs.project
    );
    let consent = format!(
        "Allow makes each entry on {} from its file of {} and shares it with {}, then allows the \
         launch.",
        needs.from, needs.project, needs.to
    );
    let row = |at: String, step: usize, verb: String, object: Option<String>| {
        let state = run.steps[step];
        let row = StepRow::new(
            ElementId::Name(format!("allow-step-{number}-{at}").into()),
            run.marks[step].1.clone(),
            verb,
            state.tone(),
            ElementId::Name(format!("allow-word-{number}-{at}").into()),
        )
        .wrapped(true)
        .time(state.word());
        match object {
            Some(object) => row.object(object),
            None => row,
        }
    };
    let files = needs.files.len();
    div()
        .id(id("allow-files"))
        .aria_label(SharedString::from(head.clone()))
        .pt(space::XS)
        .flex()
        .flex_col()
        .gap(space::XS)
        .child(
            div()
                .mono(11.)
                .text_color(p.faint)
                .child(SharedString::from(head)),
        )
        .child(
            div()
                .id(id("approval-sharing"))
                .text_size(px(12.))
                .text_color(p.mute)
                .aria_label(SharedString::from(consent.clone()))
                .child(SharedString::from(consent))
                .test_support(),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .children(needs.files.iter().enumerate().map(|(at, file)| {
                    row(
                        at.to_string(),
                        at,
                        file.path.clone(),
                        Some(file.entry.clone()),
                    )
                }))
                .child(div().border_t_1().border_color(p.line).child(row(
                    "approve".to_owned(),
                    files,
                    format!("Allow on {}", needs.to),
                    None,
                ))),
        )
        .test_support()
}

/// The "Details" row of a card: its chevron turns as it opens, its ink fades on the hover.
fn details_toggle(
    id: ElementId,
    open: bool,
    turn: f32,
    p: Palette,
    on_click: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
) -> impl IntoElement {
    let row = div()
        .id(id)
        .aria_expanded(open)
        .aria_label("Details")
        .flex()
        .items_center()
        .gap(space::XS)
        .text_size(px(12.))
        .cursor_pointer()
        .child("Details")
        .child(
            icon(Icon::Chevron)
                .size(px(11.))
                .text_color(p.faint)
                .with_transformation(Transformation::rotate(radians(PI * (1.0 - turn)))),
        )
        .on_click(on_click);
    hoverable(row, move |d, t| d.text_color(mix(p.faint, p.text, t)))
}

/// The most the cards take of the rail's height before they scroll.
const WAITING_HEIGHT: f32 = 420.;

/// The tones of a request held for the user, on the rail's card and on the conversation's line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HeldTones {
    /// The dot and the class tag.
    pub ink: gpui_kit::Hsla,
    /// The card's border, over the card.
    pub card_edge: gpui_kit::Hsla,
    /// The conversation line's border once pointed at.
    pub line_edge: gpui_kit::Hsla,
}

/// A request that waits for the user is attention, a base one too: nothing failed. The error
/// ink is for what was refused or failed.
pub(crate) fn held_tones(p: Palette) -> HeldTones {
    HeldTones {
        ink: p.wait,
        card_edge: mix(p.wait, p.card, 0.7),
        line_edge: p.wait_line,
    }
}

/// The ink of the one-line record a decided request folds into: a denial is a refusal.
pub(crate) fn record_ink(decision: Option<&str>, p: Palette) -> gpui_kit::Hsla {
    match decision {
        Some("deny") => p.fail,
        _ => p.faint,
    }
}

#[cfg(test)]
mod tests {
    use super::{HeldTones, NeededFile, Needs, Step, held_tones, record_ink, sentences};
    use gpui_design_system::{Palette, theme::Shade};

    fn palettes() -> [Palette; 2] {
        [Palette::own(Shade::Light), Palette::own(Shade::Dark)]
    }

    #[test]
    fn a_held_launch_s_needs_are_read_with_their_files_and_none_without() {
        let needs = serde_json::json!({"from": "pc", "to": "server", "project": "app",
            "files": [{"path": "config/local.toml", "entry": "app-config-local.toml",
                "bound": "file:app/config/local.toml"}]});
        assert_eq!(
            Needs::of(&needs),
            Some(Needs {
                from: "pc".into(),
                to: "server".into(),
                project: "app".into(),
                files: vec![NeededFile {
                    path: "config/local.toml".into(),
                    entry: "app-config-local.toml".into(),
                }],
            })
        );
        assert_eq!(Needs::of(&serde_json::Value::Null), None);
        let none = serde_json::json!({"from": "pc", "to": "server", "project": "app", "files": []});
        assert_eq!(Needs::of(&none), None);
    }

    #[test]
    fn each_step_says_one_word_and_a_failed_one_is_drawn_as_failed() {
        let words: Vec<&str> = [
            Step::Waiting,
            Step::Making,
            Step::Sharing,
            Step::Shared,
            Step::NotMade,
            Step::NotShared,
        ]
        .map(Step::word)
        .to_vec();
        assert_eq!(
            words,
            [
                "Waiting",
                "Making…",
                "Sharing…",
                "Shared",
                "Not made",
                "Not shared"
            ]
        );
        for step in [Step::NotMade, Step::NotShared, Step::NotAllowed] {
            assert!(step.failed());
            assert_eq!(step.tone(), crate::steps::StepTone::Failed);
        }
        assert!(!Step::Shared.failed());
    }

    #[test]
    fn a_request_waiting_for_the_user_is_not_drawn_as_an_error() {
        for p in palettes() {
            let HeldTones {
                ink,
                card_edge,
                line_edge,
            } = held_tones(p);
            assert_eq!(ink, p.wait, "the dot and the tag");
            assert_ne!(ink, p.fail);
            assert_ne!(
                card_edge,
                gpui_design_system::color::mix(p.fail, p.card, 0.6)
            );
            assert_ne!(line_edge, p.fail, "the line's border");
        }
    }

    #[test]
    fn the_rail_card_and_the_conversation_line_agree() {
        for p in palettes() {
            assert_eq!(held_tones(p).line_edge, p.wait_line);
            assert_eq!(held_tones(p).ink, p.wait);
        }
    }

    #[test]
    fn a_denied_request_is_drawn_as_refused() {
        for p in palettes() {
            assert_eq!(record_ink(Some("deny"), p), p.fail);
            assert_ne!(record_ink(Some("once"), p), p.fail);
            assert_ne!(record_ink(None, p), p.fail);
        }
    }

    #[test]
    fn two_reasons_are_two_sentences_with_no_doubled_word() {
        let said = sentences([
            "it copies .env into every workspace, which matches a sensitive name (.env): only the \
             user chooses to spread it",
            "it changes the setup that prepares the project's new workspaces",
        ]);
        assert_eq!(
            said,
            "It copies .env into every workspace, which matches a sensitive name (.env): only the \
             user chooses to spread it. It changes the setup that prepares the project's new \
             workspaces."
        );
        assert!(!said.contains("it it"));
    }

    #[test]
    fn a_reason_that_ends_a_sentence_keeps_its_mark_and_an_empty_one_is_left_out() {
        assert_eq!(
            sentences([
                "it is the line it sets, pnpm test: not a command.",
                "  ",
                "why?"
            ]),
            "It is the line it sets, pnpm test: not a command. Why?"
        );
        assert_eq!(sentences(Vec::<&str>::new()), "");
        assert_eq!(sentences(["émile asked"]), "Émile asked.");
    }
}
