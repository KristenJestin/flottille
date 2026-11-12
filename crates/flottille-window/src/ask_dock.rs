//! The dock of a session's asks (`docs/design.md` §10, the session's page): while an ask waits
//! for the user, its card stands above the composer, a little narrower than it, never scrolled
//! away with the conversation. Several waiting, the oldest shows, `1 of 2 asks` above it; the
//! next slides in once it is answered, and the dock folds away with the last.
//!
//! The asks are the root's, and the dock is the session's, not its page's (the shell keeps one
//! per session, as it keeps the composer): what the user chose and wrote stays while a
//! sub-agent's conversation is shown, and when the page is left and opened again.

use flottille_core::question::Question;
use gpui_design_system::{growing, palette, presence, presence_of, sliding, space};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, Focusable as _, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, TestSupportExt as _, WeakEntity, Window, div, px,
};

use crate::question_card::QuestionCard;
use crate::session::SessionPage;

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
                    card.update(cx, |card, cx| {
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
                    cx.defer_in(window, move |_, window, cx| {
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
            .children(shown)
            .test_support();
        presence("ask-dock-open", !self.cards.is_empty(), content)
    }
}
