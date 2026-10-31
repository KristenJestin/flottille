//! The session's mode in the composer's bar: a small Ask | Auto | Full control, which only the
//! user changes (`agent.mode`). In Auto without a judge chosen, calls to judge come to the user as
//! in Ask: a warning icon beside the control says so in a tooltip and leads to Settings. Full, where
//! nothing is asked, is drawn in the alarm colour so that it is never on by mistake.

use flottille_ui::motion::PANEL;
use flottille_ui::{Button, Icon, Segmented, enter, fade, palette, space};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Context, InteractiveElement, IntoElement, ParentElement, Render, SharedString, Styled,
    TestSupportExt as _, WeakEntity, Window, div, px,
};

use crate::actions::Command;
use crate::shell::{Selection, Shell};

/// What the warning icon says: Auto has no judge to answer, so each call to judge comes to the
/// user as in Ask.
pub const NO_JUDGE: &str =
    "Auto has no judge, so every call to judge comes to you, as in Ask. Choose one in Settings.";

/// The modes, in the control's order.
const MODES: [(&str, &str); 3] = [("ask", "Ask"), ("auto", "Auto"), ("full", "Full")];
/// Full's place in [`MODES`].
const FULL: usize = 2;

pub struct ModeSwitch {
    shell: WeakEntity<Shell>,
    /// The root agent, whose mode its whole tree takes at its start.
    session: String,
    /// `ask`, `auto` or `full`, as the tree last said it; `None` until it is read.
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

    /// What the daemon said when it refused the last change, shown beside the control.
    pub fn said(&self) -> Option<String> {
        self.refused.clone()
    }

    /// Full is shown: the control is drawn in the alarm colour.
    pub fn alarm(&self) -> bool {
        self.mode() == Some("full")
    }

    /// Auto is shown and no judge is chosen: the warning icon is there.
    pub fn no_judge(&self) -> bool {
        self.mode() == Some("auto") && !self.judge_chosen
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

    /// The user chose `wanted` (`ask`, `auto` or `full`) in the control.
    pub fn choose(&mut self, wanted: &str, cx: &mut Context<Self>) {
        if self.pending.is_some() || self.mode.as_deref().is_none_or(|mode| mode == wanted) {
            return;
        }
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
        let chosen = MODES
            .iter()
            .position(|(mode, _)| self.mode() == Some(*mode))
            .unwrap_or(0);
        let control = Segmented::new("reply-mode", "Mode", MODES, chosen)
            .alarm(FULL)
            .disabled(self.mode.is_none() || self.pending.is_some())
            .on_choose({
                let this = cx.entity().downgrade();
                move |index, _, cx| {
                    let wanted = MODES[index].0;
                    this.update(cx, |this, cx| this.choose(wanted, cx)).ok();
                }
            });
        // The warning comes and goes beside the control: its room opens and its icon fades in.
        let warn = fade(
            "reply-mode-warning-shown",
            self.no_judge(),
            PANEL,
            window,
            cx,
        );
        let shell = self.shell.clone();
        div()
            .id("reply-mode-bar")
            .h(px(28.))
            .px(space::SM)
            .flex()
            .items_center()
            .text_size(px(12.5))
            .child(control)
            .when(warn > 0.0, |d| {
                d.child(
                    div()
                        .flex_none()
                        .w(px(28. * warn))
                        .opacity(warn)
                        .overflow_hidden()
                        .child(
                            Button::icon("reply-mode-warning", Icon::Warning, NO_JUDGE).on_click(
                                move |_, window, cx| {
                                    shell
                                        .update(cx, |shell, cx| {
                                            shell.open(Selection::Settings, window, cx)
                                        })
                                        .ok();
                                },
                            ),
                        ),
                )
            })
            // A refusal is a word beside the control, not a block: it fades in and does not fold.
            .when_some(self.said(), |d, said| {
                d.child(enter(
                    "reply-mode-said-enter",
                    0,
                    div()
                        .id("reply-mode-said")
                        .ml(px(8.))
                        .text_color(p.fail)
                        .child(SharedString::from(said))
                        .test_support(),
                ))
            })
            .test_support()
    }
}
