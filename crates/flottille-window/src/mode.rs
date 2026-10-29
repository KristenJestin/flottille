//! The session's mode in the composer's bar: Ask or Auto, which only the user changes
//! (`agent.mode`). In Auto without a judge chosen, calls to judge come to the user as in Ask, and
//! the bar says once where to choose one.

use flottille_ui::color::mix;
use flottille_ui::motion::HOVER;
use flottille_ui::{Switch, enter, fade, palette};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Context, InteractiveElement, IntoElement, ParentElement, Render, SharedString, Styled,
    TestSupportExt as _, WeakEntity, Window, div, px,
};

use crate::actions::Command;
use crate::shell::Shell;

pub const NO_JUDGE: &str = "Choose an Auto judge in Settings";

pub struct ModeSwitch {
    shell: WeakEntity<Shell>,
    /// The root agent, whose mode its whole tree takes at its start.
    session: String,
    /// `ask` or `auto`, as the tree last said it; `None` until it is read.
    mode: Option<String>,
    /// A flip sent and not yet seen in the tree: what it asked for.
    pending: Option<String>,
    judge_chosen: bool,
    refused: Option<String>,
}

impl ModeSwitch {
    pub fn new(shell: WeakEntity<Shell>, session: String) -> Self {
        Self {
            shell,
            session,
            mode: None,
            pending: None,
            // Until the providers are read, no sentence about a judge that may well be chosen.
            judge_chosen: true,
            refused: None,
        }
    }

    /// The mode shown: the one asked for while the daemon has not said it.
    pub fn mode(&self) -> Option<&str> {
        self.pending.as_deref().or(self.mode.as_deref())
    }

    /// The sentence the bar shows beside the switch, if any.
    pub fn said(&self) -> Option<String> {
        if let Some(why) = &self.refused {
            return Some(why.clone());
        }
        (self.mode() == Some("auto") && !self.judge_chosen).then(|| NO_JUDGE.to_owned())
    }

    pub fn set_mode(&mut self, mode: Option<String>, cx: &mut Context<Self>) {
        if self.pending.is_some() && self.pending == mode {
            self.pending = None;
        }
        if self.mode != mode {
            self.mode = mode;
            cx.notify();
        }
    }

    pub fn set_judge_chosen(&mut self, chosen: bool, cx: &mut Context<Self>) {
        if self.judge_chosen != chosen {
            self.judge_chosen = chosen;
            cx.notify();
        }
    }

    /// The daemon's answer to a flip: on a refusal the switch goes back and says why.
    pub fn answered(&mut self, refused: Option<String>, cx: &mut Context<Self>) {
        // Once the daemon took it, the mode is the one asked for, whatever the tree said meanwhile.
        let asked = self.pending.take();
        if refused.is_none() {
            self.mode = asked.or(self.mode.take());
        }
        self.refused = refused;
        cx.notify();
    }

    fn flip(&mut self, cx: &mut Context<Self>) {
        if self.pending.is_some() {
            return;
        }
        let Some(mode) = self.mode.as_deref() else {
            return;
        };
        let wanted = if mode == "auto" { "ask" } else { "auto" };
        self.pending = Some(wanted.to_owned());
        self.refused = None;
        let command = Command::SetMode {
            session: self.session.clone(),
            mode: wanted.to_owned(),
        };
        self.shell
            .update(cx, |shell, _| shell.perform(command))
            .ok();
        cx.notify();
    }
}

impl Render for ModeSwitch {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let auto = self.mode() == Some("auto");
        let ink = mix(
            p.mute,
            p.text,
            fade("reply-mode-ink", auto, HOVER, window, cx),
        );
        let switch = Switch::new("reply-mode-switch", auto, "Auto mode")
            .disabled(self.mode.is_none() || self.pending.is_some())
            .on_click(cx.listener(|this, _, _, cx| this.flip(cx)));
        div()
            .id("reply-mode")
            .h(px(28.))
            .px(px(9.))
            .flex()
            .items_center()
            .gap(px(7.))
            .text_size(px(12.5))
            .text_color(ink)
            .child(switch)
            .child(if auto { "Auto" } else { "Ask" })
            // A word beside the switch, not a block: it fades in and does not fold.
            .when_some(self.said(), |d, said| {
                let color = if self.refused.is_some() {
                    p.fail
                } else {
                    p.faint
                };
                d.child(enter(
                    "reply-mode-said-enter",
                    0,
                    div()
                        .id("reply-mode-said")
                        .text_color(color)
                        .child(SharedString::from(said))
                        .test_support(),
                ))
            })
            .test_support()
    }
}
