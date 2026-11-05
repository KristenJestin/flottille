//! How agents are told to work, edited in the window (`docs/projects.md`, "Rules"): the "Rules"
//! page of Settings for every project, and the "Rules" section of a project's own Settings. The
//! window writes nothing itself: Save and Reset to default are the daemon's (`rules.set`,
//! `rules.reset`), and the editor shows what the daemon says next.
//!
//! The text is frozen in an agent when it is created, so a change reaches the agents created
//! after it only: the editor says so.
//!
//! The same editor holds the Auto judge's guidance in Settings (`judge.show`, `judge.set`,
//! `judge.reset`), read at each judgement: a change reaches the next one.

use flottille_ui::{
    Button, Icon, Section, Switch, Toast, follow_theme, hoverable, icon, palette, presence,
    presence_of, space, toast, turning,
};
use gpui_kit::base::input::{InputEvent, TextareaState};
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, StatefulInteractiveElement as _, Styled, Subscription,
    TestSupportExt as _, Transformation, WeakEntity, Window, div, px, radians,
};
use serde_json::Value;

use crate::actions::Command;
use crate::commands::perform_later;
use crate::settings::Reveal;
use crate::shell::Shell;

/// What the editor says of when a change takes effect.
pub const REACH: &str = "A change reaches the agents created from now on; an agent already \
                         created keeps the rules it was created with.";

/// What a project's editor says when its text starts with the global one.
pub const REPEATS: &str = "These rules repeat the global ones; you can remove that part.";

/// What the judge's editor says of when a change takes effect.
pub const JUDGE_REACH: &str = "A change reaches the next judgement. Calls that always come to you \
                               never reach the judge, whatever this says.";

/// Where the text the daemon last gave comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// Flottille's own text.
    Default,
    /// The user's, for every project.
    Global,
    /// The project's own.
    Project,
}

/// The rules as the daemon says them (`rules.show`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
    /// What the editor holds: for a project, its own rules, empty when it has none.
    pub text: String,
    pub source: Source,
    /// What every project shares: the user's rules, else Flottille's default (`Source::Global`
    /// or `Source::Default`).
    pub global: String,
    pub global_source: Source,
    /// The project gets its own rules alone.
    pub alone: bool,
    /// The project's text starts with the global one, which its agents get once.
    pub repeats: bool,
    /// Why the stored text was cut to the limit: edited by hand past it.
    pub cut: Option<String>,
    /// Why the judge cannot follow the stored guidance (`judge.show`): every judgement fails,
    /// and asks, until it is fixed.
    pub error: Option<String>,
}

impl Shown {
    pub fn of(value: &Value) -> Shown {
        Shown {
            text: value["text"].as_str().unwrap_or_default().to_owned(),
            source: match value["source"].as_str() {
                Some("project") => Source::Project,
                Some("global") => Source::Global,
                _ => Source::Default,
            },
            global: value["global"].as_str().unwrap_or_default().to_owned(),
            global_source: match value["global_source"].as_str() {
                Some("global") => Source::Global,
                _ => Source::Default,
            },
            alone: value["alone"].as_bool().unwrap_or(false),
            repeats: value["repeats"].as_bool().unwrap_or(false),
            cut: value["cut"].as_str().map(str::to_owned),
            error: value["error"].as_str().map(str::to_owned),
        }
    }
}

pub struct RulesEditor {
    shell: WeakEntity<Shell>,
    /// The project it edits the rules of; the user's own for every project when none.
    project: Option<String>,
    /// It edits the Auto judge's guidance, not the rules.
    judge: bool,
    /// Made when first drawn, since a text field needs a window.
    field: Option<Entity<TextareaState>>,
    shown: Option<Shown>,
    /// A save or a reset waits for the daemon.
    waiting: bool,
    /// What waits is a reset.
    resetting: bool,
    /// The global rules above a project's editor are unfolded.
    global_open: bool,
    problem: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl RulesEditor {
    pub fn new(shell: WeakEntity<Shell>, project: Option<String>) -> Self {
        Self {
            shell,
            project,
            judge: false,
            field: None,
            shown: None,
            waiting: false,
            resetting: false,
            global_open: false,
            problem: None,
            _subscriptions: Vec::new(),
        }
    }

    /// The editor of the Auto judge's guidance.
    pub fn judge(shell: WeakEntity<Shell>) -> Self {
        Self {
            judge: true,
            ..Self::new(shell, None)
        }
    }

    pub fn is_judge(&self) -> bool {
        self.judge
    }

    pub fn project(&self) -> Option<&str> {
        self.project.as_deref()
    }

    /// Asks the daemon for the rules.
    pub fn read(&self, cx: &mut Context<Self>) {
        let project = self.project.clone();
        let command = if self.judge {
            Command::JudgeRead
        } else {
            Command::RulesRead { project }
        };
        perform_later(&self.shell, command, cx);
    }

    /// Saves `text`, or with none resets to the default.
    fn write(&self, text: Option<String>, cx: &mut Context<Self>) {
        let project = self.project.clone();
        let command = if self.judge {
            Command::JudgeWrite { text }
        } else {
            Command::RulesWrite { project, text }
        };
        perform_later(&self.shell, command, cx);
    }

    pub fn shown(&self) -> Option<&Shown> {
        self.shown.as_ref()
    }

    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    pub fn field(&self) -> Option<&Entity<TextareaState>> {
        self.field.as_ref()
    }

    /// The text in the editor.
    pub fn typed(&self, cx: &gpui_kit::App) -> Option<String> {
        self.field
            .as_ref()
            .map(|field| field.read(cx).value().to_string())
    }

    /// The text differs from what the daemon last said.
    pub fn is_edited(&self, cx: &gpui_kit::App) -> bool {
        match (&self.shown, self.typed(cx)) {
            (Some(shown), Some(typed)) => shown.text != typed,
            _ => false,
        }
    }

    /// Whether Save has something to write: the text differs from what this level stores. The
    /// user's rules can be saved as they are over the default they show; a text cut to the limit
    /// differs from what is stored too. A project stores nothing until it is given a text, which
    /// the editor shows empty.
    pub fn savable(&self, cx: &gpui_kit::App) -> bool {
        let Some(shown) = &self.shown else {
            return false;
        };
        self.typed(cx).is_some()
            && (self.is_edited(cx)
                || (self.project.is_none() && !self.is_own())
                || shown.cut.is_some()
                || shown.error.is_some())
    }

    /// Whether Reset to default has something to remove: rules of its own at this level, or a
    /// project's `rules_alone`.
    pub fn is_own(&self) -> bool {
        let own = match self.project {
            Some(_) => Source::Project,
            None => Source::Global,
        };
        self.shown
            .as_ref()
            .is_some_and(|shown| shown.source == own || shown.alone)
    }

    /// Whether the project's agents get its rules alone.
    pub fn is_alone(&self) -> bool {
        self.shown.as_ref().is_some_and(|shown| shown.alone)
    }

    /// Whether the global rules above a project's editor are unfolded.
    pub fn is_global_open(&self) -> bool {
        self.global_open
    }

    pub fn toggle_global(&mut self, cx: &mut Context<Self>) {
        self.global_open = !self.global_open;
        cx.notify();
    }

    /// Makes the project's agents get its rules alone, or the global ones too.
    pub fn set_alone(&mut self, alone: bool, cx: &mut Context<Self>) {
        let Some(project) = self.project.clone() else {
            return;
        };
        if self.waiting || self.is_alone() == alone {
            return;
        }
        self.waiting = true;
        self.problem = None;
        perform_later(&self.shell, Command::RulesAlone { project, alone }, cx);
        cx.notify();
    }

    fn make_field(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<TextareaState> {
        let field = cx.new(|cx| {
            let mut field =
                TextareaState::new(window, cx)
                    .auto_grow(8, 18)
                    .placeholder(if self.judge {
                        "how the judge should weigh a call"
                    } else if self.project.is_some() {
                        "what is particular to this project"
                    } else {
                        "how your agents should work"
                    });
            follow_theme(&mut field, cx);
            field
        });
        self._subscriptions
            .push(cx.subscribe(&field, |this, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    this.problem = None;
                    cx.notify();
                }
            }));
        if let Some(shown) = &self.shown {
            field.update(cx, |field, cx| field.set_value(&shown.text, window, cx));
        }
        self.field = Some(field.clone());
        field
    }

    /// The daemon's answer to a read, a save or a reset: what it says the rules are, or why not.
    /// Typing in progress is kept when a read comes back, and replaced by what was saved.
    pub fn answered(
        &mut self,
        said: Result<Shown, String>,
        written: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.waiting = false;
        let what = if self.judge {
            "Judge guidance"
        } else {
            "Rules"
        };
        let resetting = std::mem::take(&mut self.resetting);
        if written {
            let said = match &said {
                Ok(_) if resetting => Toast::success(format!("{what} reset to the default")),
                Ok(_) => Toast::success(format!("{what} saved")),
                Err(why) => Toast::failure(format!("{what} not saved")).detail(why.clone()),
            };
            let reach = if self.judge {
                "They reach the next judgement."
            } else {
                "The agents created from now on follow them."
            };
            toast(
                if said.is_failure() {
                    said
                } else {
                    said.detail(reach)
                },
                cx,
            );
        }
        match said {
            Ok(shown) => {
                self.problem = None;
                let keep = !written && self.is_edited(cx);
                if !keep && let Some(field) = self.field.clone() {
                    field.update(cx, |field, cx| field.set_value(&shown.text, window, cx));
                }
                self.shown = Some(shown);
            }
            Err(why) if !written => self.problem = Some(why.into()),
            Err(_) => {}
        }
        cx.notify();
    }

    /// The daemon's answer to the switch: what it says the rules are now. What is typed stays
    /// as it is, and is not saved with the switch.
    pub fn switched(&mut self, said: Result<Shown, String>, cx: &mut Context<Self>) {
        self.waiting = false;
        match said {
            Ok(shown) => {
                let said = if shown.alone {
                    Toast::success("Only this project's rules are used")
                } else {
                    Toast::success("The global rules are used too")
                };
                toast(
                    said.detail("The agents created from now on follow them."),
                    cx,
                );
                self.problem = None;
                self.shown = Some(shown);
            }
            Err(why) => toast(Toast::failure("Rules not switched").detail(why), cx),
        }
        cx.notify();
    }

    pub fn save(&mut self, cx: &mut Context<Self>) {
        let Some(text) = self.typed(cx) else {
            return;
        };
        if self.waiting || !self.savable(cx) {
            return;
        }
        self.waiting = true;
        self.problem = None;
        self.write(Some(text), cx);
        cx.notify();
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        if self.waiting || !self.is_own() {
            return;
        }
        self.waiting = true;
        self.resetting = true;
        self.problem = None;
        self.write(None, cx);
        cx.notify();
    }

    /// Where the text comes from, in a few words: the section's tag.
    pub fn origin(&self) -> &'static str {
        match (
            self.judge,
            &self.project,
            self.shown.as_ref().map(|shown| shown.source),
        ) {
            (_, _, None) => "asking the daemon",
            (true, _, Some(Source::Global)) => "your own",
            (true, _, Some(_)) => "Flottille's default",
            (false, None, Some(Source::Global)) => "your own",
            (false, None, Some(_)) => "Flottille's default",
            (false, Some(_), Some(Source::Project)) => "this project's own",
            (false, Some(_), Some(_)) => "none of its own",
        }
    }

    /// What the line beside the buttons says: how to make the text one's own, or when a change
    /// takes effect.
    fn footer(&self) -> &'static str {
        match (self.judge, &self.project, self.is_own()) {
            (true, _, _) => JUDGE_REACH,
            (false, Some(_), false) => {
                "Write the project's rules: they come after the global ones."
            }
            (false, _, _) => REACH,
        }
    }
}

impl Render for RulesEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let field = match self.field.clone() {
            Some(field) => field,
            None => self.make_field(window, cx),
        };
        let scope = self.project.clone().unwrap_or_default();
        let judge = self.judge;
        let id = |part: &str| {
            ElementId::Name(
                if judge {
                    format!("judge-guidance-{part}")
                } else {
                    format!("rules-{part}-{scope}")
                }
                .into(),
            )
        };
        let savable = self.savable(cx);
        let footer = div()
            .mt(space::XS)
            .flex()
            .items_center()
            .gap(space::SM)
            .child(
                div()
                    .id(id("reach"))
                    .flex_1()
                    .min_w_0()
                    .text_size(px(12.))
                    .text_color(p.faint)
                    .child(self.footer())
                    .test_support(),
            )
            .child(
                Button::quiet(id("reset"), "Reset to default")
                    .disabled(!self.is_own() || self.waiting)
                    .on_click(cx.listener(|this, _, _, cx| this.reset(cx))),
            )
            .child(
                Button::primary(id("save"), "Save")
                    .disabled(!savable || self.waiting)
                    .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
            );
        let text = div()
            .id(id("text"))
            .min_h(px(150.))
            .px(space::MD)
            .py(space::MD)
            .rounded(px(10.))
            .bg(p.field)
            .border_1()
            .border_color(p.line2)
            .text_size(px(13.))
            .line_height(px(20.))
            .child(field)
            .test_support();
        let line = move |id: ElementId| {
            move |said: SharedString| {
                div()
                    .id(id.clone())
                    .aria_label(said.clone())
                    .text_size(px(12.5))
                    .text_color(p.fail)
                    .child(said)
                    .test_support()
                    .into_any_element()
            }
        };
        let (title, sentence, section) = match (judge, &self.project) {
            (true, _) => (
                "Judge guidance",
                "What the judge is told to allow. It can add to the floor (ask for anything \
                 destructive), never lift it.",
                "guidance",
            ),
            (false, Some(_)) => (
                "Rules",
                "How the project's agents work, after your rules for every project. A change \
                 reaches the agents created from now on.",
                "rules",
            ),
            (false, None) => (
                "Rules",
                "How every project's agents work, unless a project has rules of its own.",
                "rules",
            ),
        };
        let project_part = self.project.is_some().then(|| {
            let shown = self.shown.clone();
            let alone = shown.as_ref().is_some_and(|shown| shown.alone);
            let global = shown.as_ref().map(|shown| {
                let name = match shown.global_source {
                    Source::Global => "Your rules for every project",
                    _ => "Flottille's default rules",
                };
                (name, shown.global.clone())
            });
            let open = self.global_open;
            let turned = turning(id("global-chevron"), open, window, cx);
            let toggle = hoverable(
                div()
                    .id(id("global-toggle"))
                    .aria_label(match &global {
                        Some((name, _)) => format!("{name}, read only"),
                        None => "Global rules".to_owned(),
                    })
                    .flex()
                    .items_center()
                    .gap(space::XS)
                    .cursor_pointer()
                    .text_size(px(12.5))
                    .child(icon(Icon::Chevron).size(px(12.)).with_transformation(
                        Transformation::rotate(radians(-std::f32::consts::FRAC_PI_2 * turned)),
                    ))
                    .child(match &global {
                        Some((name, _)) => SharedString::from(*name),
                        None => SharedString::from("Global rules"),
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_global(cx))),
                move |d, t| d.text_color(flottille_ui::color::mix(p.faint, p.text, t)),
            );
            let shell = self.shell.clone();
            let body = div()
                .flex()
                .flex_col()
                .gap(space::SM)
                .pl(space::LG)
                .child(
                    div()
                        .id(id("global-text"))
                        .px(space::MD)
                        .py(space::SM)
                        .rounded(px(10.))
                        .bg(p.field)
                        .border_1()
                        .border_color(p.line)
                        .text_size(px(12.5))
                        .line_height(px(19.))
                        .text_color(p.mute)
                        .child(global.map(|(_, text)| text).unwrap_or_default())
                        .test_support(),
                )
                .child(div().flex().child(
                    Button::quiet(id("global-edit"), "Edit them in Settings").on_click(
                        move |_, window, cx| {
                            shell
                                .update(cx, |shell, cx| {
                                    shell.open_settings_at(Reveal::Rules, window, cx)
                                })
                                .ok();
                        },
                    ),
                ));
            let waiting = self.waiting || shown.is_none();
            let switch = Switch::new(id("alone"), alone, "Use only this project's rules")
                .hint("Your rules for every project are left out for this project's agents.")
                .disabled(waiting)
                .on_click(cx.listener(move |this, _, _, cx| this.set_alone(!alone, cx)));
            (
                toggle,
                presence(id("global-block"), open, body).gap(space::MD),
                switch,
            )
        });
        let (global_toggle, global_body, alone_switch) = match project_part {
            Some((toggle, body, switch)) => (Some(toggle), Some(body), Some(switch)),
            None => (None, None, None),
        };
        let note = move |id: ElementId| {
            move |said: SharedString| {
                div()
                    .id(id.clone())
                    .aria_label(said.clone())
                    .text_size(px(12.5))
                    .text_color(p.mute)
                    .child(said)
                    .test_support()
                    .into_any_element()
            }
        };
        Section::new(section, title)
            .tag(self.origin())
            .sentence(sentence)
            .children(global_toggle)
            .children(global_body)
            .children(alone_switch)
            .child(text)
            .child(
                presence_of(
                    id("repeats-block"),
                    self.shown
                        .as_ref()
                        .filter(|shown| shown.repeats)
                        .map(|_| SharedString::from(REPEATS)),
                    note(id("repeats")),
                )
                .gap(space::MD),
            )
            .child(
                presence_of(
                    id("cut-block"),
                    self.shown
                        .as_ref()
                        .and_then(|shown| shown.cut.clone())
                        .map(|cut| SharedString::from(format!("Cut: {cut}."))),
                    line(id("cut")),
                )
                .gap(space::MD),
            )
            .child(
                presence_of(
                    id("error-block"),
                    self.shown
                        .as_ref()
                        .and_then(|shown| shown.error.clone())
                        .map(|error| {
                            SharedString::from(format!(
                                "The judge asks you about every call until this is fixed: {error}."
                            ))
                        }),
                    line(id("error")),
                )
                .gap(space::MD),
            )
            .child(
                presence_of(
                    id("problem-block"),
                    self.problem.clone(),
                    line(id("problem")),
                )
                .gap(space::MD),
            )
            .child(footer)
    }
}
