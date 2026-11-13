//! The dock of a session's asks (`docs/design.md` §10, the session's page): while an ask waits
//! for the user, its card stands above the composer, a little narrower than it, never scrolled
//! away with the conversation. Several waiting, the oldest shows, `1 of 2 asks` above it; the
//! next slides in once it is answered, and the dock folds away with the last.
//!
//! The asks are the root's, and the dock is the session's, not its page's (the shell keeps one
//! per session, as it keeps the composer): what the user chose and wrote stays while a
//! sub-agent's conversation is shown, and when the page is left and opened again.

use std::cell::Cell;
use std::rc::Rc;

use flottille_core::question::Question;
use gpui_design_system::{growing, palette, presence, presence_of, sliding, space};
use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, Context, Entity, Focusable as _,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, TestSupportExt as _, WeakEntity, Window, canvas,
    div, px,
};

use crate::question_card::QuestionCard;
use crate::session::SessionPage;

/// What the conversation keeps while an ask waits: past it, the questions of the card scroll.
pub const CONVERSATION: f32 = 160.;

/// The room the dock may still take on the page, measured as the page is drawn: its column, its
/// conversation and its composer, each by a canvas, read by the next frame. Negative when what
/// the page holds is taller than the page; the card's questions then give up that height.
#[derive(Clone, Default)]
pub struct Room {
    column: Rc<Cell<Bounds<Pixels>>>,
    conversation: Rc<Cell<Bounds<Pixels>>>,
    composer: Rc<Cell<Bounds<Pixels>>>,
    /// The dock's height as it is drawn, easing, and as it will be once it has eased: the page
    /// has room for the second, whatever the first lags by.
    shown: Rc<Cell<f32>>,
    eased: Rc<Cell<f32>>,
    /// What the last frame handed to the dock, to ask for another when this one measures more.
    used: Rc<Cell<f32>>,
}

impl Room {
    fn spare(&self) -> Option<f32> {
        let (column, conversation, composer) = (
            self.column.get(),
            self.conversation.get(),
            self.composer.get(),
        );
        // Not measured yet: the first frame is drawn whole.
        if column.size.height <= px(0.) || composer.size.height <= px(0.) {
            return None;
        }
        // The room of the dock at its eased height: what the card asks of it, once it eased,
        // never depends on how far it has got.
        let lag = self.eased.get() - self.shown.get();
        Some(
            f32::from(column.bottom() - composer.bottom()) + f32::from(conversation.size.height)
                - CONVERSATION
                - lag,
        )
    }

    /// An element that fills its parent and says where it is, as `part` of the page.
    pub fn watch(&self, part: Part) -> AnyElement {
        let room = self.clone();
        let cell = match part {
            Part::Column => self.column.clone(),
            Part::Conversation => self.conversation.clone(),
            Part::Composer => self.composer.clone(),
            Part::Shown | Part::Eased => Rc::default(),
        };
        let height = match part {
            Part::Shown => Some(self.shown.clone()),
            Part::Eased => Some(self.eased.clone()),
            _ => None,
        };
        canvas(
            move |bounds: Bounds<Pixels>, window, _| {
                cell.set(bounds);
                if let Some(height) = &height {
                    height.set(f32::from(bounds.size.height));
                }
                let spare = room.spare().unwrap_or(0.);
                if (spare - room.used.get()).abs() > 0.5 {
                    window.request_animation_frame();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full()
        .into_any_element()
    }

    /// The dock is shut: what it will be eased to is nothing, whatever it was.
    fn shut(&self) {
        self.eased.set(0.);
    }

    /// What the dock may take, as the last frame measured it; the frame draws with it.
    pub fn take(&self) -> Option<f32> {
        let spare = self.spare();
        self.used.set(spare.unwrap_or(0.));
        spare
    }
}

/// A part of the page the room is measured from.
#[derive(Clone, Copy)]
pub enum Part {
    Column,
    Conversation,
    Composer,
    /// The dock as it is drawn (easing), then as it will be.
    Shown,
    Eased,
}

/// An ask that waits, as the conversation holds it.
pub struct Waiting {
    /// Its row in the conversation.
    pub at: usize,
    pub seq: u64,
    pub questions: Vec<Question>,
    /// When it was asked, as the card says it.
    pub when: SharedString,
    /// The history holds the answers given from its card.
    pub answered: bool,
}

/// What the page says about its asks, for the dock.
pub struct Asks {
    /// The asks that wait, oldest first; `None` while the page cannot say them (a sub-agent's
    /// conversation is shown, or the root's is not read yet).
    pub waiting: Option<Vec<Waiting>>,
    /// The numbers of the asks the daemon's tree says wait, once the tree is read.
    pub asking: Option<Vec<u64>>,
    pub session: String,
    pub title: String,
    /// The conversation was read: what waits now waited when the page was opened.
    pub read: bool,
}

pub struct AskDock {
    page: WeakEntity<SessionPage>,
    /// The cards of the asks that wait, oldest first.
    cards: Vec<Entity<QuestionCard>>,
    /// The card last shown, drawn while the dock folds away.
    last: Option<Entity<QuestionCard>>,
    /// A page was just opened on the dock: an ask that waited takes the focus once, as the
    /// sidebar's "needs you" leads here. One asked later never takes it from the composer.
    opening: bool,
    /// What the page can still give the dock.
    room: Option<f32>,
    /// What the page is measured by: the page draws with it, the dock too.
    cells: Room,
}

impl Default for AskDock {
    fn default() -> Self {
        Self::new()
    }
}

impl AskDock {
    pub fn new() -> Self {
        Self {
            page: WeakEntity::new_invalid(),
            cards: Vec::new(),
            last: None,
            opening: true,
            room: None,
            cells: Room::default(),
        }
    }

    /// What the page measures its room by.
    pub fn cells(&self) -> Room {
        self.cells.clone()
    }

    /// The room the page left, set before it is drawn: the cards size their questions to it.
    pub fn set_room(&mut self, room: Option<f32>, cx: &mut Context<Self>) {
        self.room = room;
        for card in &self.cards {
            card.update(cx, |card, cx| card.set_room(room, cx));
        }
    }

    /// A page of the session opens on the dock: the cards answer through it, and an ask that
    /// waits takes the focus once more.
    pub fn attach(&mut self, page: WeakEntity<SessionPage>, cx: &mut Context<Self>) {
        self.page = page.clone();
        self.opening = true;
        for card in &self.cards {
            card.update(cx, |card, _| card.attach(page.clone()));
        }
    }

    /// The card of the ask `seq`, while it waits.
    pub fn card(&self, seq: u64, cx: &App) -> Option<&Entity<QuestionCard>> {
        self.cards.iter().find(|card| card.read(cx).seq() == seq)
    }

    /// The card shown, the oldest ask that waits.
    pub fn shown(&self) -> Option<&Entity<QuestionCard>> {
        self.cards.first()
    }

    /// The cards of the asks that wait, oldest first.
    pub fn cards(&self) -> &[Entity<QuestionCard>] {
        &self.cards
    }

    /// Brings the focus to the card of `seq` and lights it: the line in the conversation asks
    /// for it.
    pub fn summon(&mut self, seq: u64, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(card) = self.card(seq, cx).cloned() {
            card.update(cx, |card, cx| card.summon(window, cx));
        }
    }

    /// Brings the cards up to what the page says waits, before the page is drawn.
    pub fn sync(&mut self, asks: Asks, window: &mut Window, cx: &mut Context<Self>) {
        let before = self.cards.first().cloned();
        // The card shown held the focus as it left: the next one takes it, the user answers on.
        let held = before
            .as_ref()
            .is_some_and(|card| card.focus_handle(cx).contains_focused(window, cx));
        match asks.waiting {
            Some(waiting) => {
                let mut cards = Vec::with_capacity(waiting.len());
                for waiting in &waiting {
                    let card = match self.card(waiting.seq, cx) {
                        Some(card) => card.clone(),
                        None => {
                            let (seq, questions) = (waiting.seq, waiting.questions.clone());
                            let (session, page) = (asks.session.clone(), self.page.clone());
                            cx.new(|cx| {
                                QuestionCard::new(seq, session, questions, page, window, cx)
                            })
                        }
                    };
                    let room = self.room;
                    card.update(cx, |card, cx| {
                        card.set_room(room, cx);
                        card.set_answered(waiting.answered);
                        card.set_header(&asks.title, waiting.when.clone(), cx);
                    });
                    cards.push(card);
                }
                self.cards = cards;
            }
            None => {
                if let Some(asking) = &asks.asking {
                    self.cards
                        .retain(|card| asking.contains(&card.read(cx).seq()));
                }
            }
        }
        match self.cards.first().cloned() {
            Some(first) => {
                let opening = self.opening && asks.read;
                if opening {
                    self.opening = false;
                }
                let moved_on = held && before.as_ref() != Some(&first);
                if opening || moved_on {
                    let card = first.clone();
                    cx.defer_in(window, move |this, window, cx| {
                        // The history is read after the page opens: a user who has begun to
                        // write meanwhile keeps the focus.
                        let typing = opening
                            && this
                                .page
                                .read_with(cx, |page, cx| page.typing(window, cx))
                                .unwrap_or(false);
                        if typing {
                            return;
                        }
                        card.update(cx, |card, cx| match opening {
                            true => card.summon(window, cx),
                            false => window.focus(&card.focus_handle(cx), cx),
                        });
                    });
                }
                self.last = Some(first);
            }
            None if asks.read => self.opening = false,
            None => {}
        }
    }

    /// Above the part that scrolls, so that it stays in sight: `1 of 2 asks`, then the steps of
    /// the card shown (Previous, `2 of 4` and its dots, Next).
    pub fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let count = (self.cards.len() > 1)
            .then(|| SharedString::from(format!("1 of {} asks", self.cards.len())));
        let several = !self.cards.is_empty()
            && self
                .last
                .as_ref()
                .is_some_and(|card| card.read(cx).several());
        // The steps move with their card's page: the next card's come in as it does.
        let steps = match &self.last {
            Some(card) => {
                let seq = card.read(cx).seq();
                let steps = card.update(cx, |card, cx| card.steps(cx));
                sliding("ask-dock-steps-page", seq as usize, steps).into_any_element()
            }
            None => div().into_any_element(),
        };
        div()
            .flex_none()
            .flex()
            .flex_col()
            .child(presence_of("ask-dock-count-line", count, move |said| {
                div()
                    .id("ask-dock-count")
                    .pb(space::SM)
                    .text_size(px(12.))
                    .text_color(p.faint)
                    .aria_label(said.clone())
                    .child(said)
                    .test_support()
                    .into_any_element()
            }))
            .child(presence("ask-dock-steps", several, steps))
            .into_any_element()
    }
}

impl Render for AskDock {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.cards.is_empty() {
            self.cells.shut();
        }
        let (shown_height, eased_height) =
            (self.cells.watch(Part::Shown), self.cells.watch(Part::Eased));
        let shown = self.last.clone().map(|card| {
            let seq = card.read(cx).seq();
            growing(
                "ask-dock-height",
                sliding("ask-dock-page", seq as usize, card),
            )
        });
        let content = div()
            .id("ask-dock")
            .pb(space::SM)
            .flex()
            .flex_col()
            .relative()
            .children(shown)
            .child(eased_height)
            .test_support();
        div()
            .relative()
            .child(presence("ask-dock-open", !self.cards.is_empty(), content))
            .child(shown_height)
    }
}
