//! The dock of a session's asks (`docs/design.md` §10, the session's page): while an ask waits
//! for the user, its card stands above the composer, a little narrower than it, never scrolled
//! away with the conversation. Several waiting, the oldest shows, `1 of 2 asks` above it; the
//! next slides in once it is answered, and the dock folds away with the last.

use flottille_core::question::Question;
use gpui_design_system::{palette, presence, presence_of, sliding, space};
use gpui_kit::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    TestSupportExt as _, WeakEntity, Window, div, px,
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
    pub waiting: Vec<Waiting>,
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
    /// The page was just opened: an ask that waited takes the focus once, as the sidebar's
    /// "needs you" leads here. One asked later never takes it from the composer.
    opening: bool,
}

impl AskDock {
    pub fn new(page: WeakEntity<SessionPage>) -> Self {
        Self {
            page,
            cards: Vec::new(),
            last: None,
            opening: true,
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
}

impl Render for AskDock {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let Some(asks) = self.page.upgrade().map(|page| page.read(cx).asks(cx)) else {
            return div().into_any_element();
        };
        let mut cards = Vec::with_capacity(asks.waiting.len());
        for waiting in &asks.waiting {
            let card = match self.card(waiting.seq, cx) {
                Some(card) => card.clone(),
                None => {
                    let (at, seq, questions) = (waiting.at, waiting.seq, waiting.questions.clone());
                    let session = self
                        .page
                        .upgrade()
                        .map(|page| page.read(cx).id().to_owned())
                        .unwrap_or_default();
                    let page = self.page.clone();
                    cx.new(|cx| QuestionCard::new(at, seq, session, questions, page, window, cx))
                }
            };
            card.update(cx, |card, cx| {
                card.set_answered(waiting.answered);
                card.set_header(&asks.title, waiting.when.clone(), cx);
            });
            cards.push(card);
        }
        self.cards = cards;
        if let Some(first) = self.cards.first().cloned() {
            if std::mem::take(&mut self.opening) && asks.read {
                let card = first.clone();
                cx.defer_in(window, move |_, window, cx| {
                    card.update(cx, |card, cx| card.summon(window, cx));
                });
            }
            self.last = Some(first);
        } else if asks.read {
            self.opening = false;
        }
        let count = (self.cards.len() > 1)
            .then(|| SharedString::from(format!("1 of {} asks", self.cards.len())));
        let shown = self.last.clone().map(|card| {
            let seq = card.read(cx).seq();
            sliding("ask-dock-page", seq as usize, card)
        });
        let content = div()
            .id("ask-dock")
            .pb(space::SM)
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
            .children(shown)
            .test_support();
        presence("ask-dock-open", !self.cards.is_empty(), content).into_any_element()
    }
}
