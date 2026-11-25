//! How agents are told to work, edited in the window (`docs/projects.md`, "Rules"): the "Rules"
//! page of Settings for every project, and the "Rules" section of a project's own Settings. The
//! window writes nothing itself: Save and Reset to default are the daemon's (`rules.set`,
//! `rules.reset`), and the editor shows what the daemon says next.
//!
//! The text is frozen in an agent when it is created, so a change reaches the agents created
//! after it only: the editor says so.
//!
//! The same editor holds the Auto judge's guidance in Settings (`judge.show`, `judge.set`,
//! `judge.reset`), read at each judgement: a change reaches the next one; a part's guidance
//! (Review's, `review.show`…) with its title; and a check's own instructions (`review.show
//! {part}`), drawn bare, the field and its buttons, under the check's line.

use gpui_design_system::{
    Button, Icon, Section, Switch, Toast, follow_theme, hoverable, icon, palette, presence,
    presence_of, space, toast, turning,
};
use gpui_kit::base::input::{InputEvent, TextareaState};
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, StatefulInteractiveElement as _, Styled, Subscription,
    TestSupportExt as _, Transformation, Window, div, px, radians,
};
use serde_json::Value;

use crate::actions::Command;
use crate::commands::{perform_later, spaced, stack};
use crate::settings::Reveal;
use crate::shared_level::{Place, edge, follows_line, set_here};

/// What a project's editor says when its text starts with the global one.
pub const REPEATS: &str = "These rules repeat the global ones; you can remove that part.";

/// What the compactor's editor says of when a change takes effect.
pub const COMPACTION_REACH: &str = "A change reaches the next compaction.";

/// What the editor of a part's guidance says of when a change takes effect.
pub const PART_REACH: &str = "A change reaches the agents started from now on.";

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
    /// The text of all machines, this machine having none of its own (`docs/remote.md`,
    /// "Settings for all machines").
    All,
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
    /// The text of all machines, `""` while they say none: none without a hub.
    pub all: Option<String>,
    /// This machine holds a text of its own (its file).
    pub own: bool,
}

impl Shown {
    pub fn of(value: &Value) -> Shown {
        Shown {
            text: value["text"].as_str().unwrap_or_default().to_owned(),
            source: match value["source"].as_str() {
                Some("project") => Source::Project,
                Some("global") => Source::Global,
                Some("all") => Source::All,
                _ => Source::Default,
            },
            global: value["global"].as_str().unwrap_or_default().to_owned(),
            global_source: match value["global_source"].as_str() {
                Some("global") => Source::Global,
                Some("all") => Source::All,
                _ => Source::Default,
            },
            alone: value["alone"].as_bool().unwrap_or(false),
            repeats: value["repeats"].as_bool().unwrap_or(false),
            cut: value["cut"].as_str().map(str::to_owned),
            error: value["error"].as_str().map(str::to_owned),
            all: value["all"].as_str().map(str::to_owned),
            own: value["own"].as_bool().unwrap_or(false),
        }
    }
}

pub struct RulesEditor {
    shell: crate::commands::ShellOf,
    /// The project it edits the rules of; the user's own for every project when none.
    project: Option<String>,
    /// It edits the Auto judge's guidance, not the rules.
    judge: bool,
    /// It edits the compactor's guidance, not the rules.
    compaction: bool,
    /// It edits the guidance of the part of Flottille of that name (Review's), not the rules:
    /// its title, as the window says it.
    part: Option<(String, String)>,
    /// The title and the sentence of a part's guidance section, when the part words them.
    heading: Option<(String, String)>,
    /// It edits the guidance of one part of that guidance (Review's check `bugs`): the part of
    /// Flottille, the part of its guidance and the check's name.
    check: Option<(String, String, String)>,
    /// Made when first drawn, since a text field needs a window.
    field: Option<Entity<TextareaState>>,
    shown: Option<Shown>,
    /// A save or a reset waits for the daemon.
    waiting: bool,
    /// What waits is a reset.
    resetting: bool,
    /// What waits is the way back to all machines' text, not a reset to the default.
    using_all: bool,
    /// Where the editor is drawn: on All machines, or on a machine with or without a hub.
    place: Place,
    /// The global rules above a project's editor are unfolded.
    global_open: bool,
    problem: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl RulesEditor {
    pub fn new(shell: crate::commands::ShellOf, project: Option<String>) -> Self {
        Self {
            shell,
            project,
            judge: false,
            compaction: false,
            part: None,
            heading: None,
            check: None,
            field: None,
            shown: None,
            waiting: false,
            resetting: false,
            using_all: false,
            place: Place::default(),
            global_open: false,
            problem: None,
            _subscriptions: Vec::new(),
        }
    }

    /// The editor of the Auto judge's guidance.
    pub fn judge(shell: crate::commands::ShellOf) -> Self {
        Self {
            judge: true,
            ..Self::new(shell, None)
        }
    }

    /// The editor of the compactor's guidance.
    pub fn compaction(shell: crate::commands::ShellOf) -> Self {
        Self {
            compaction: true,
            ..Self::new(shell, None)
        }
    }

    /// The editor of the guidance of the part `name` of Flottille, titled `title`.
    pub fn part(shell: crate::commands::ShellOf, name: &str, title: &str) -> Self {
        Self {
            part: Some((name.to_owned(), title.to_owned())),
            ..Self::new(shell, None)
        }
    }

    /// The section of the guidance titled `title`, its sentence `sentence`.
    pub fn headed(mut self, title: &str, sentence: &str) -> Self {
        self.heading = Some((title.to_owned(), sentence.to_owned()));
        self
    }

    /// The editor of the instructions of the check `part` of the part `name` of Flottille, the
    /// check named `title`.
    pub fn check(shell: crate::commands::ShellOf, name: &str, part: &str, title: &str) -> Self {
        Self {
            check: Some((name.to_owned(), part.to_owned(), title.to_owned())),
            ..Self::new(shell, None)
        }
    }

    /// A guidance, the judge's, the compactor's, a part's or a check's, rather than rules.
    fn is_guidance(&self) -> bool {
        self.judge || self.compaction || self.part.is_some() || self.check.is_some()
    }

    pub fn is_judge(&self) -> bool {
        self.judge
    }

    pub fn is_compaction(&self) -> bool {
        self.compaction
    }

    pub fn project(&self) -> Option<&str> {
        self.project.as_deref()
    }

    /// What asks the daemon for the text this editor shows.
    fn read_command(&self) -> Command {
        let project = self.project.clone();
        if let Some((name, part, _)) = &self.check {
            Command::GuidanceRead {
                name: name.clone(),
                part: Some(part.clone()),
            }
        } else if let Some((name, _)) = &self.part {
            Command::GuidanceRead {
                name: name.clone(),
                part: None,
            }
        } else if self.judge {
            Command::JudgeRead
        } else if self.compaction {
            Command::CompactionRead
        } else {
            Command::RulesRead { project }
        }
    }

    /// Asks the daemon for the rules.
    pub fn read(&self, cx: &mut Context<Self>) {
        perform_later(&self.shell, self.read_command(), cx);
    }

    /// The name of the text among the values of all machines: none for a project's rules, which
    /// no machine shares.
    fn shared_name(&self) -> Option<String> {
        if let Some((name, part, _)) = &self.check {
            Some(format!("{name}.guidance.{part}"))
        } else if let Some((name, _)) = &self.part {
            Some(format!("{name}.guidance"))
        } else if self.judge {
            Some("judge.guidance".to_owned())
        } else if self.compaction {
            Some("compaction.guidance".to_owned())
        } else {
            self.project.is_none().then(|| "rules".to_owned())
        }
    }

    /// Where the editor is drawn: on All machines, or on `machine`, which has a hub or not. Coming
    /// to All machines or leaving it puts in the field what that place holds.
    pub fn place(&mut self, place: Place, window: &mut Window, cx: &mut Context<Self>) {
        let place = Place {
            on_all: place.on_all && self.shared_name().is_some(),
            ..place
        };
        if self.place == place {
            return;
        }
        let moved = self.place.on_all != place.on_all;
        self.place = place;
        if moved && let (Some(field), Some(shown)) = (self.field.clone(), self.shown.clone()) {
            let text = self.text_of(&shown);
            field.update(cx, |field, cx| field.set_value(&text, window, cx));
        }
        cx.notify();
    }

    pub fn on_all(&self) -> bool {
        self.place.on_all
    }

    /// What the field holds of `shown` where the editor is drawn: all machines' text on All
    /// machines, what the machine's agents are told on a machine.
    fn text_of(&self, shown: &Shown) -> String {
        match self.place.on_all {
            true => shown.all.clone().unwrap_or_default(),
            false => shown.text.clone(),
        }
    }

    /// Whether this machine holds a text of its own, while it has a hub: the field wears the
    /// mark of a value set apart.
    pub fn held_here(&self) -> bool {
        self.project.is_none() && self.shown.as_ref().is_some_and(|shown| shown.own)
    }

    /// Drops the machine's own text: the field follows all machines again.
    pub fn use_all(&mut self, cx: &mut Context<Self>) {
        if self.waiting || !self.place.is_own(self.held_here()) {
            return;
        }
        self.waiting = true;
        self.using_all = true;
        self.problem = None;
        self.write(None, cx);
        cx.notify();
    }

    /// Saves `text`, or with none resets to the default.
    fn write(&self, text: Option<String>, cx: &mut Context<Self>) {
        // On All machines it is the value every machine uses, signed with the device key.
        if self.place.on_all
            && let Some(name) = self.shared_name()
        {
            let command = Command::Shared {
                name,
                value: text.unwrap_or_default(),
                then: Box::new(self.read_command()),
            };
            perform_later(&self.shell, command, cx);
            return;
        }
        let project = self.project.clone();
        let command = if let Some((name, part, _)) = &self.check {
            Command::GuidanceWrite {
                name: name.clone(),
                part: Some(part.clone()),
                text,
            }
        } else if let Some((name, _)) = &self.part {
            Command::GuidanceWrite {
                name: name.clone(),
                part: None,
                text,
            }
        } else if self.judge {
            Command::JudgeWrite { text }
        } else if self.compaction {
            Command::CompactionGuidance { text }
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
            (Some(shown), Some(typed)) => self.text_of(shown) != typed,
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
                || (self.project.is_none() && !self.place.on_all && !self.is_own())
                || (!self.place.on_all && (shown.cut.is_some() || shown.error.is_some())))
    }

    /// Whether Save is drawn: the text differs from what is saved, or what is stored must be
    /// written anew (cut to the limit, or a file edited by hand that cannot be used). The user's
    /// rules left as the default show no Save: nothing changed (rule 4).
    pub fn save_shown(&self, cx: &gpui_kit::App) -> bool {
        self.savable(cx)
            && self.shown.as_ref().is_some_and(|shown| {
                self.is_edited(cx)
                    || (!self.place.on_all && (shown.cut.is_some() || shown.error.is_some()))
            })
    }

    /// Whether Reset to default has something to remove: rules of its own at this level, or a
    /// project's `rules_alone`.
    pub fn is_own(&self) -> bool {
        // On All machines, what Reset forgets is what all machines say.
        if self.place.on_all {
            return self
                .shown
                .as_ref()
                .and_then(|shown| shown.all.as_ref())
                .is_some_and(|all| !all.trim().is_empty());
        }
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
        let example = if self.check.is_some() {
            "what this check should look for"
        } else if self.part.is_some() {
            "what its agents should look for"
        } else if self.judge {
            "how the judge should weigh a call"
        } else if self.compaction {
            "what a summary must keep"
        } else if self.project.is_some() {
            "what is particular to this project"
        } else {
            "how your agents should work"
        };
        let field = cx.new(|cx| {
            let mut field = TextareaState::new(window, cx)
                .auto_grow(8, 18)
                .placeholder(example);
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
            let text = self.text_of(shown);
            field.update(cx, |field, cx| field.set_value(&text, window, cx));
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
        let what = if let Some((_, _, title)) = &self.check {
            format!("Instructions for {title}")
        } else if let Some((_, title)) = &self.part {
            format!("{title} guidance")
        } else if self.judge {
            "Judge guidance".to_owned()
        } else if self.compaction {
            "Compaction guidance".to_owned()
        } else {
            "Rules".to_owned()
        };
        let resetting = std::mem::take(&mut self.resetting);
        let using_all = std::mem::take(&mut self.using_all);
        let on_all = self.place.on_all;
        if written {
            let said = match &said {
                Ok(_) if using_all => Toast::success(format!("{what}: all machines' text used")),
                Ok(_) if resetting && on_all => {
                    Toast::success(format!("{what} reset for all machines"))
                }
                Ok(_) if resetting => Toast::success(format!("{what} reset to the default")),
                Ok(_) if on_all => Toast::success(format!("{what} saved for all machines")),
                Ok(_) => Toast::success(format!("{what} saved")),
                Err(why) => Toast::failure(format!("{what} not saved")).detail(why.clone()),
            };
            let reach = if self.check.is_some() {
                "They reach the checks started from now on."
            } else if self.part.is_some() {
                "It reaches the agents started from now on."
            } else if self.judge {
                "They reach the next judgement."
            } else if self.compaction {
                "They reach the next compaction."
            } else {
                "Agents started from now on follow them."
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
                    let text = self.text_of(&shown);
                    field.update(cx, |field, cx| field.set_value(&text, window, cx));
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
                toast(said.detail("Agents started from now on follow them."), cx);
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
            self.is_guidance(),
            &self.project,
            self.shown.as_ref().map(|shown| shown.source),
        ) {
            (_, _, None) => "asking Flottille",
            (_, None, Some(Source::All)) if !self.place.on_all => "all your machines'",
            (_, None, Some(_)) if self.place.on_all => "all your machines'",
            (true, _, Some(Source::Global)) => "your own",
            (true, _, Some(_)) => "Flottille's own",
            (false, None, Some(Source::Global)) => "your own",
            (false, None, Some(_)) => "Flottille's own",
            (false, Some(_), Some(Source::Project)) => "this project's own",
            (false, Some(_), Some(_)) => "none of its own",
        }
    }

    /// What the line beside the buttons says: how to make the text one's own, or when a change
    /// takes effect.
    ///
    /// The rules say it in the toast that follows a save, never in a line under the editor.
    fn footer(&self) -> Option<&'static str> {
        if self.compaction {
            return Some(COMPACTION_REACH);
        }
        if self.part.is_some() || self.check.is_some() {
            return Some(PART_REACH);
        }
        match (self.judge, &self.project, self.is_own()) {
            (true, _, _) => Some(JUDGE_REACH),
            (false, Some(_), false) => {
                Some("Write the project's rules: they come after the global ones.")
            }
            (false, _, _) => None,
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
        let compaction = self.compaction;
        let named = match &self.check {
            Some((name, part, _)) => Some(format!("{name}-{part}")),
            None => self.part.as_ref().map(|(name, _)| name.clone()),
        };
        let id = |part: &str| {
            ElementId::Name(
                if let Some(name) = &named {
                    format!("{name}-guidance-{part}")
                } else if judge {
                    format!("judge-guidance-{part}")
                } else if compaction {
                    format!("compaction-guidance-{part}")
                } else {
                    format!("rules-{part}-{scope}")
                }
                .into(),
            )
        };
        // Under the field with the gap of whatever column holds it, and `XS` more.
        let footer = div()
            .flex()
            .items_center()
            .gap(space::SM)
            // The line comes and goes with the project's own rules: it folds and fades, never pops.
            .child(div().flex_1().min_w_0().child(presence_of(
                id("reach-block"),
                self.footer().map(SharedString::from),
                {
                    let reach = id("reach");
                    move |line| {
                        div()
                            .id(reach)
                            .text_size(px(12.))
                            .text_color(p.faint)
                            .child(line)
                            .test_support()
                            .into_any_element()
                    }
                },
            )))
            .child(
                Button::quiet(id("reset"), "Reset to default")
                    .disabled(!self.is_own() || self.waiting)
                    .on_click(cx.listener(|this, _, _, cx| this.reset(cx))),
            )
            // Save is not drawn at rest: it unfolds beside the field once it differs from what is
            // saved, and folds away after it is saved or set back (rule 4). The global editor's is
            // the view's one primary; a project's sits among the Instructions tab's own Saves, so
            // it is secondary.
            .child(
                // A presence is as wide as its parent: the button keeps its own width beside the
                // line.
                div().flex_none().child(presence(
                    id("save-block"),
                    self.save_shown(cx),
                    match (&self.project, &self.check) {
                        (None, None) => Button::primary(id("save"), "Save"),
                        _ => Button::secondary(id("save"), "Save"),
                    }
                    .disabled(self.waiting)
                    .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                )),
            );
        // On a machine with a hub, a text it holds itself has the edge of what is set apart and
        // the way back to all machines' text under it; on All machines, a plain edge.
        let prefix = if let Some(name) = &named {
            format!("{name}-guidance")
        } else if judge {
            "judge-guidance".to_owned()
        } else if compaction {
            "compaction-guidance".to_owned()
        } else {
            format!("rules-{scope}")
        };
        let held = self.held_here();
        let edge_colour = edge(&prefix, &self.place, held, window, cx);
        let editor = cx.entity().downgrade();
        let text = set_here(
            &prefix,
            &self.place,
            div()
                .id(id("text"))
                .min_h(px(150.))
                .px(space::MD)
                .py(space::MD)
                .rounded(px(10.))
                .bg(p.field)
                .border_1()
                .border_color(edge_colour)
                .text_size(px(13.))
                .line_height(px(20.))
                .child(field)
                .test_support(),
            held,
            move |_, cx| {
                editor.update(cx, |editor, cx| editor.use_all(cx)).ok();
            },
        );
        let follows = self.place.follows()
            && self.project.is_none()
            && self.shown.as_ref().is_some_and(|shown| shown.all.is_some());
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
        // A check's instructions are the field and its buttons alone, under the check's line.
        if let Some((_, _, title)) = &self.check {
            return div()
                .flex()
                .flex_col()
                .child(text)
                .child(
                    presence_of(
                        id("error-block"),
                        self.shown
                            .as_ref()
                            .and_then(|shown| shown.error.clone())
                            .map(|error| {
                                SharedString::from(format!(
                                    "The {title} check does not run until this is fixed: {error}."
                                ))
                            }),
                        line(id("error")),
                    )
                    .space(space::SM),
                )
                .child(
                    presence_of(
                        id("problem-block"),
                        self.problem.clone(),
                        line(id("problem")),
                    )
                    .space(space::SM),
                )
                .child(spaced(space::SM + space::XS, footer))
                .into_any_element();
        }
        let (title, sentence, section): (String, String, String) = match (judge, &self.project) {
            _ if self.part.is_some() => {
                let (name, title) = self.part.clone().expect("a part");
                let (title, sentence) = self.heading.clone().unwrap_or_else(|| {
                    (
                        format!("{title} guidance"),
                        "What its agents are told to look for. It sits inside Flottille's fixed \
                         text for each of them, which says they modify nothing."
                            .into(),
                    )
                });
                (title, sentence, format!("{name}-guidance"))
            }
            _ if compaction => (
                "What a summary keeps".into(),
                "What a summary must keep. It sits inside Flottille's fixed text, which says the \
                 summary is data and smaller than what it replaces."
                    .into(),
                "compaction-guidance".into(),
            ),
            (true, _) => (
                "What the judge is told".into(),
                "Added to Flottille's own instructions, which always ask before anything \
                 destructive."
                    .into(),
                "guidance".into(),
            ),
            (false, Some(_)) => (
                "Rules".into(),
                "How the project's agents work, after your rules for every project.".into(),
                "rules".into(),
            ),
            (false, None) => (
                "Rules for every project".into(),
                "What every agent is told, in every project. A project adds its own, or uses only \
                 its own."
                    .into(),
                "rules".into(),
            ),
        };
        let project_part = self.project.is_some().then(|| {
            let shown = self.shown.clone();
            let alone = shown.as_ref().is_some_and(|shown| shown.alone);
            // The same thing, the same name, whoever wrote what it says and whether or not the
            // daemon has sent it yet: the fold never says another name while it waits.
            const GLOBAL: &str = "Rules for every project";
            let global = shown.as_ref().map(|shown| shown.global.clone());
            let open = self.global_open;
            let turned = turning(id("global-chevron"), open, window, cx);
            let toggle = hoverable(
                div()
                    .id(id("global-toggle"))
                    .aria_label(format!("{GLOBAL}, read only"))
                    .flex()
                    .items_center()
                    .gap(space::XS)
                    .cursor_pointer()
                    .text_size(px(12.5))
                    .child(icon(Icon::Chevron).size(px(12.)).with_transformation(
                        Transformation::rotate(radians(-std::f32::consts::FRAC_PI_2 * turned)),
                    ))
                    .child(SharedString::from(GLOBAL))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_global(cx))),
                move |d, t| d.text_color(gpui_design_system::color::mix(p.faint, p.text, t)),
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
                        .child(global.unwrap_or_default())
                        .test_support(),
                )
                .child(div().flex().child(
                    Button::quiet(id("global-edit"), "Edit in Settings").on_click(
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
                presence(id("global-block"), open, body).space(space::MD),
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
            .child(
                stack(space::MD)
                    .child(follows_line(&prefix, follows, cx))
                    .children(global_toggle.map(|toggle| spaced(space::MD, toggle)))
                    .children(global_body)
                    .children(alone_switch.map(|switch| spaced(space::MD, switch)))
                    .child(spaced(space::MD, text))
                    .child(
                        presence_of(
                            id("repeats-block"),
                            self.shown
                                .as_ref()
                                .filter(|shown| shown.repeats)
                                .map(|_| SharedString::from(REPEATS)),
                            note(id("repeats")),
                        )
                        .space(space::MD),
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
                        .space(space::MD),
                    )
                    .child(
                        presence_of(
                            id("error-block"),
                            self.shown
                                .as_ref()
                                .and_then(|shown| shown.error.clone())
                                .map(|error| {
                                    SharedString::from(if let Some((_, title)) = &self.part {
                                        format!(
                                            "{title} does not run until this is fixed: {error}."
                                        )
                                    } else if compaction {
                                        format!(
                                            "Every compaction fails until this is fixed: {error}."
                                        )
                                    } else {
                                        format!(
                                            "The judge asks you about every call until this is \
                                             fixed: {error}."
                                        )
                                    })
                                }),
                            line(id("error")),
                        )
                        .space(space::MD),
                    )
                    .child(
                        presence_of(
                            id("problem-block"),
                            self.problem.clone(),
                            line(id("problem")),
                        )
                        .space(space::MD),
                    )
                    .child(spaced(space::MD + space::XS, footer)),
            )
            .into_any_element()
    }
}
