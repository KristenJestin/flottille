//! The round button that floats over a conversation scrolled up, above the composer: a click
//! scrolls smoothly to the end and the conversation follows it again. A small accent dot on it
//! says the agent wrote something since the user scrolled away. It is its own view: the list it
//! moves draws nothing more than what it scrolls.

use std::time::Duration;

use flottille_ui::motion::{ENTER, HOVER, Motion, PANEL, RISE};
use flottille_ui::{Button, Icon, fade, palette, space};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Context, ElementId, FollowMode, InteractiveElement as _, IntoElement, ListState,
    ParentElement as _, Render, Styled as _, Task, TestSupportExt as _, WeakEntity, Window, div,
    px,
};

use crate::session::ConversationView;

/// The id of the button, and `<id>-dot` of its dot.
pub const TO_END: &str = "conversation-to-end";

fn name(part: &str) -> ElementId {
    ElementId::Name(format!("{TO_END}-{part}").into())
}

pub struct ToEnd {
    list: ListState,
    /// The view that draws the list, drawn again while the scroll to the end moves it.
    owner: Option<WeakEntity<ConversationView>>,
    /// The end is out of sight, as the last scroll left it.
    away: bool,
    /// The agent wrote something since the user scrolled away.
    fresh: bool,
    /// The scroll to the end under way: how far it is, from where (pixels from the top).
    going: Option<(Motion, f32)>,
    /// What moves the list each frame of the scroll to the end; dropped, it stops.
    steps: Option<Task<()>>,
}

/// How often the scroll to the end moves the list: one frame.
const FRAME: Duration = Duration::from_millis(16);

impl ToEnd {
    pub fn new(list: ListState) -> Self {
        Self {
            list,
            owner: None,
            away: false,
            fresh: false,
            going: None,
            steps: None,
        }
    }

    /// The list it moves, drawn by `owner`: told each time the user scrolls it. A scroll of the
    /// user's stops the scroll to the end under way.
    pub fn watch(
        &mut self,
        list: ListState,
        owner: WeakEntity<ConversationView>,
        cx: &mut Context<Self>,
    ) {
        let me = cx.entity().downgrade();
        list.set_scroll_handler(move |_, _, cx| {
            let me = me.clone();
            // The list is still borrowed while it tells: it is read once the scroll is done.
            cx.defer(move |cx| {
                me.update(cx, |this, cx| this.scrolled(cx)).ok();
            });
        });
        self.list = list;
        self.owner = Some(owner);
        self.away = false;
        self.fresh = false;
        self.going = None;
        self.steps = None;
        cx.notify();
    }

    /// The user scrolled: whether the end is out of sight now, and the conversation may want a
    /// page of earlier events.
    fn scrolled(&mut self, cx: &mut Context<Self>) {
        self.going = None;
        self.steps = None;
        if let Some(owner) = &self.owner {
            owner.update(cx, |view, cx| view.scrolled(cx)).ok();
        }
        let offset = -self.list.scroll_px_offset_for_scrollbar().y;
        let end = self.list.max_offset_for_scrollbar().y;
        let away = !self.list.is_following_tail() && offset < end - px(1.);
        if away != self.away {
            self.away = away;
            if !away {
                self.fresh = false;
            }
            cx.notify();
        }
    }

    /// The agent wrote something: marked while the end is out of sight.
    pub fn wrote(&mut self, cx: &mut Context<Self>) {
        if self.away && !self.fresh {
            self.fresh = true;
            cx.notify();
        }
    }

    pub fn is_away(&self) -> bool {
        self.away
    }

    /// The scroll to the end runs.
    pub fn is_going(&self) -> bool {
        self.going.is_some()
    }

    fn go(&mut self, cx: &mut Context<Self>) {
        let from = f32::from(-self.list.scroll_px_offset_for_scrollbar().y);
        let mut motion = Motion::settled(0.0);
        let now = cx.background_executor().now();
        motion.animate(1.0, PANEL, now, cx.reduce_motion());
        self.going = Some((motion, from));
        self.steps = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(FRAME).await;
                let more = this.update(cx, |this, cx| this.step(cx)).unwrap_or(false);
                if !more {
                    break;
                }
            }
        }));
        self.step(cx);
    }

    /// One step of the scroll to the end; at its end, the list follows its end again. Whether
    /// it goes on.
    fn step(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((motion, from)) = self.going else {
            return false;
        };
        let now = cx.background_executor().now();
        let to = f32::from(self.list.max_offset_for_scrollbar().y);
        let at = f32::from(-self.list.scroll_px_offset_for_scrollbar().y);
        let aim = from + (to - from) * motion.value(now);
        self.list.scroll_by(px(aim - at));
        let more = motion.is_running(now);
        if !more {
            self.going = None;
            self.steps = None;
            self.away = false;
            self.fresh = false;
            self.list.set_follow_mode(FollowMode::Tail);
            cx.notify();
        }
        if let Some(owner) = &self.owner {
            owner.update(cx, |_, cx| cx.notify()).ok();
        }
        more
    }
}

impl Render for ToEnd {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shown = fade(
            name("shown"),
            self.away && self.going.is_none(),
            ENTER,
            window,
            cx,
        );
        let dot = fade(name("fresh"), self.fresh, HOVER, window, cx);
        if shown == 0.0 {
            return div().into_any_element();
        }
        let p = palette(cx);
        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom(space::LG - px(RISE * (1.0 - shown)))
            .flex()
            .justify_center()
            .opacity(shown)
            .child(
                div()
                    .relative()
                    .occlude()
                    .child(
                        Button::float(TO_END, Icon::ToEnd, "Back to the end")
                            .on_click(cx.listener(|this, _, _, cx| this.go(cx))),
                    )
                    .when(dot > 0.0, |d| {
                        d.child(
                            div()
                                .id(name("dot"))
                                .absolute()
                                .top(px(0.))
                                .right(px(0.))
                                .size(px(8.))
                                .rounded_full()
                                .bg(p.acc)
                                .border_1()
                                .border_color(p.bg)
                                .opacity(dot)
                                .test_support(),
                        )
                    }),
            )
            .into_any_element()
    }
}
