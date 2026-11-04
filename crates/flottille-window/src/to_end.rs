//! The round button that floats over a conversation scrolled up, above the composer: a click
//! scrolls smoothly to the end and the conversation follows it again. A small accent dot on it
//! says the agent wrote something since the user scrolled away. It is its own view: the list it
//! moves draws nothing more than what it scrolls.

use flottille_ui::motion::{ENTER, HOVER, Motion, PANEL, RISE};
use flottille_ui::{Button, Icon, fade, palette, space};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Context, ElementId, FollowMode, InteractiveElement as _, IntoElement, ListState,
    ParentElement as _, Render, Styled as _, TestSupportExt as _, WeakEntity, Window, div, px,
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
    /// The end is out of sight.
    away: bool,
    /// The agent wrote something since the user scrolled away.
    fresh: bool,
    /// The scroll to the end under way: how far it is, from where (pixels from the top).
    going: Option<(Motion, f32)>,
}

impl ToEnd {
    pub fn new(list: ListState) -> Self {
        Self {
            list,
            owner: None,
            away: false,
            fresh: false,
            going: None,
        }
    }

    /// The list it moves, drawn by `owner`: told each time the user scrolls it.
    pub fn watch(
        &mut self,
        list: ListState,
        owner: WeakEntity<ConversationView>,
        cx: &mut Context<Self>,
    ) {
        let me = cx.entity().downgrade();
        list.set_scroll_handler(move |_, _, cx| {
            me.update(cx, |_, cx| cx.notify()).ok();
        });
        self.list = list;
        self.owner = Some(owner);
        self.away = false;
        self.fresh = false;
        self.going = None;
        cx.notify();
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

    fn go(&mut self, cx: &mut Context<Self>) {
        let from = f32::from(-self.list.scroll_px_offset_for_scrollbar().y);
        let mut motion = Motion::settled(0.0);
        let now = cx.background_executor().now();
        motion.animate(1.0, PANEL, now, cx.reduce_motion());
        self.going = Some((motion, from));
        cx.notify();
    }

    /// One step of the scroll to the end; at its end, the list follows its end again.
    fn scroll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((motion, from)) = self.going else {
            return;
        };
        let now = cx.background_executor().now();
        let to = f32::from(self.list.max_offset_for_scrollbar().y);
        let at = f32::from(-self.list.scroll_px_offset_for_scrollbar().y);
        let aim = from + (to - from) * motion.value(now);
        self.list.scroll_by(px(aim - at));
        if motion.is_running(now) {
            window.request_animation_frame();
        } else {
            self.going = None;
            self.list.set_follow_mode(FollowMode::Tail);
        }
        if let Some(owner) = &self.owner {
            owner.update(cx, |_, cx| cx.notify()).ok();
        }
    }
}

impl Render for ToEnd {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.scroll(window, cx);
        let offset = -self.list.scroll_px_offset_for_scrollbar().y;
        let end = self.list.max_offset_for_scrollbar().y;
        let at_end =
            self.going.is_some() || self.list.is_following_tail() || offset >= end - px(1.);
        self.away = !at_end;
        if at_end {
            self.fresh = false;
        }
        let shown = fade(name("shown"), self.away, ENTER, window, cx);
        let dot = fade(name("fresh"), self.fresh, HOVER, window, cx);
        if shown == 0.0 {
            return div().into_any_element();
        }
        let p = palette(cx);
        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom(space::LG + px(RISE * (1.0 - shown)))
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
