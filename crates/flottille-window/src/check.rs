//! The Check Flottille section of Settings, General: what `flottille doctor` checks (#1179,
//! `docs/cli.md`, "flottille doctor"). Each provider is a row with whether its agents can run,
//! why not, and where the window can make it the fix (Turn on); then the sandbox that confines
//! the agents' commands, then the command lines the projects' forges and trackers need. The
//! window decides nothing: each part is the daemon's own answer (`provider.list`, `daemon.status`,
//! `project.programs`), read when the section is first shown and when the user asks again.
//!
//! What moves: the section's body unfolds in place of its skeleton (`awaiting`), a state fades
//! when its words change (`changing`), a fix unfolds and folds away with the state it mends
//! (`presence_of`), and the buttons are the kit's (their hover and press fade).

use gpui_design_system::{
    Button, Icon, ListCard, ListRow, Section, Skeleton, Toast, awaiting, changing, icon, palette,
    presence_of, space, toast,
};
use gpui_kit::{
    Context, ElementId, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement as _, Styled, TestSupportExt as _, Window, div, px,
};
use serde_json::{Value, json};

use crate::actions::Command;
use crate::glyphs;
use crate::programs::ProgramState;
use crate::providers::Provider;

/// What to do with the daemon's answer to a request of the section.
#[derive(Clone, Debug, PartialEq)]
pub enum Then {
    /// `provider.list`.
    Providers,
    /// `daemon.status`: the sandbox's part.
    Sandbox,
    /// `project.programs` of `project`.
    Programs { project: String },
    /// `provider.enable` of `name`, after which the providers are read again.
    Enable { name: String },
}

/// What the daemon says confines the commands of the agents (`daemon.status`'s `sandbox`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sandbox {
    pub warnings: Vec<String>,
}

impl Sandbox {
    fn of(value: &Value) -> Sandbox {
        Sandbox {
            warnings: value["warnings"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|warning| warning.as_str().map(str::to_owned))
                .collect(),
        }
    }
}

/// A command line a project's forge or tracker needs, with the projects that need it.
#[derive(Clone, Debug, PartialEq)]
struct Needed {
    state: ProgramState,
    used_by: Vec<String>,
}

pub struct CheckSection {
    shell: crate::commands::ShellOf,
    /// Asked once the section is shown: never in a loop.
    started: bool,
    providers: Option<Vec<Provider>>,
    sandbox: Option<Result<Sandbox, String>>,
    programs: Vec<Needed>,
    /// Requests sent and not answered: Check again waits for them.
    pending: usize,
}

impl CheckSection {
    pub fn new(shell: crate::commands::ShellOf) -> Self {
        Self {
            shell,
            started: false,
            providers: None,
            sandbox: None,
            programs: Vec::new(),
            pending: 0,
        }
    }

    /// Reads the checks the first time the section is shown.
    pub fn ensure(&mut self, cx: &mut Context<Self>) {
        if !self.started {
            self.started = true;
            // The page is being drawn while it asks: after.
            let this = cx.entity().downgrade();
            cx.defer(move |cx| {
                this.update(cx, |this, cx| this.check_again(cx)).ok();
            });
        }
    }

    /// Reads every check anew.
    pub fn check_again(&mut self, cx: &mut Context<Self>) {
        if self.pending > 0 {
            return;
        }
        self.started = true;
        // Its machine's projects, not those of the machine in front (#1271).
        let machine = self.shell.machine();
        let projects: Vec<String> = self
            .shell
            .upgrade()
            .map(|shell| shell.read(cx).projects_on(machine))
            .unwrap_or_default();
        self.programs.clear();
        self.ask("provider.list", json!({}), Then::Providers, cx);
        self.ask("daemon.status", json!({}), Then::Sandbox, cx);
        for project in projects {
            let params = json!({"name": project});
            self.ask("project.programs", params, Then::Programs { project }, cx);
        }
        cx.notify();
    }

    fn ask(&mut self, method: &str, params: Value, then: Then, cx: &mut Context<Self>) {
        self.pending += 1;
        self.shell.perform(
            Command::Ask {
                method: method.to_owned(),
                params,
                then: crate::extensions::Then::Check(then),
            },
            cx,
        );
    }

    /// The fix of a provider that is off: what its switch in Settings does.
    pub fn turn_on(&mut self, name: &str, cx: &mut Context<Self>) {
        let then = Then::Enable {
            name: name.to_owned(),
        };
        self.ask("provider.enable", json!({"name": name}), then, cx);
    }

    /// The providers as the section reads them, with the program each one needs.
    pub fn providers(&self) -> &[Provider] {
        self.providers.as_deref().unwrap_or_default()
    }

    /// The daemon's answer to a request of the section.
    pub fn answered(&mut self, then: Then, answer: Result<Value, String>, cx: &mut Context<Self>) {
        self.pending = self.pending.saturating_sub(1);
        match then {
            Then::Providers => {
                if let Ok(read) = &answer {
                    self.providers = Some(
                        read["providers"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(Provider::of)
                            .collect(),
                    );
                }
            }
            Then::Sandbox => {
                self.sandbox = Some(answer.map(|status| Sandbox::of(&status["sandbox"])));
            }
            Then::Programs { project } => {
                if let Ok(read) = &answer {
                    let states = read["repositories"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .chain(std::iter::once(&read["tracker"]).filter(|t| !t.is_null()));
                    for state in states {
                        let state = ProgramState::of(state);
                        match self.programs.iter_mut().find(|needed| {
                            needed.state.program == state.program && needed.state.host == state.host
                        }) {
                            Some(needed) => {
                                if !needed.used_by.contains(&project) {
                                    needed.used_by.push(project.clone());
                                }
                            }
                            None => self.programs.push(Needed {
                                state,
                                used_by: vec![project.clone()],
                            }),
                        }
                    }
                }
            }
            Then::Enable { name } => match answer {
                // The state of the row is read again, as `doctor` would say it.
                // The shell is being updated while it hands the answer: ask after.
                Ok(_) => {
                    let this = cx.entity().downgrade();
                    self.pending += 1;
                    cx.defer(move |cx| {
                        this.update(cx, |this, cx| {
                            this.pending -= 1;
                            this.ask("provider.list", json!({}), Then::Providers, cx);
                        })
                        .ok();
                    });
                }
                Err(why) => toast(
                    Toast::failure(format!("{name} was not turned on")).detail(why),
                    cx,
                ),
            },
        }
        cx.notify();
    }
}

/// The mark before a row's title, so that no row starts with an empty slot.
fn mark_of(id: &str, glyph: Icon, ink: gpui_kit::Hsla) -> impl IntoElement {
    div()
        .id(ElementId::Name(format!("{id}-mark").into()))
        .child(icon(glyph).size(px(14.)).text_color(ink))
        .test_support()
}

/// What a provider's row says of whether its agents can run.
fn provider_state(provider: &Provider) -> &'static str {
    match (provider.enabled, provider.ready) {
        (false, _) => "Off",
        (true, true) => "Can run",
        (true, false) => "Cannot run",
    }
}

impl Render for CheckSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let skeleton = Skeleton::new("check-flottille-skeleton").h(px(160.));
        let again = Button::secondary("check-flottille-again", "Check again")
            .disabled(self.pending > 0)
            .on_click(cx.listener(|this, _, _, cx| this.check_again(cx)));
        let section = |body| {
            Section::new("check-flottille", "Check Flottille")
                .sentence("What the agents need to run, as `flottille doctor` says it.")
                .action(again)
                .child(body)
        };
        let Some(providers) = self.providers.clone() else {
            return section(awaiting("check-flottille-awaiting", true, skeleton, div()))
                .into_any_element();
        };
        let mut list = ListCard::new("check-flottille-list");
        for provider in &providers {
            let id = format!("check-flottille-provider-{}", provider.name);
            let state = provider_state(provider);
            let said = SharedString::from(state);
            let mark = changing(
                ElementId::Name(format!("{id}-state-change").into()),
                state.to_owned(),
                {
                    let id = id.clone();
                    move |_| {
                        div()
                            .id(ElementId::Name(format!("{id}-state").into()))
                            .aria_label(said.clone())
                            .flex_none()
                            .whitespace_nowrap()
                            .text_size(px(13.))
                            .text_color(p.text)
                            .child(said)
                            .test_support()
                            .into_any_element()
                    }
                },
            );
            let fix = {
                let this = cx.entity().downgrade();
                let name = provider.name.clone();
                let button_id = ElementId::Name(format!("{id}-fix").into());
                presence_of(
                    ElementId::Name(format!("{id}-fix-presence").into()),
                    (!provider.enabled).then(|| SharedString::from("Turn on")),
                    move |label| {
                        Button::quiet(button_id, label)
                            .on_click(move |_, _, cx| {
                                this.update(cx, |this, cx| this.turn_on(&name, cx)).ok();
                            })
                            .into_any_element()
                    },
                )
                .space(space::SM)
            };
            let trailing = div()
                .flex()
                .flex_none()
                .items_center()
                .gap(space::MD)
                .child(mark)
                .child(fix);
            let mut why: Vec<String> = Vec::new();
            if provider.enabled && !provider.ready {
                why.extend(provider.why.clone());
            }
            why.extend(provider.warnings.iter().cloned());
            let mut row = ListRow::new(ElementId::Name(id.clone().into()), provider.title.clone())
                .leading(mark_of(&id, glyphs::AGENTS, p.faint))
                .trailing(trailing);
            if !why.is_empty() {
                row = row.line(why.join(" · "));
            }
            list = list.row(row);
        }
        let sandbox = match &self.sandbox {
            None => ("Reading…".to_owned(), None),
            Some(Err(why)) => ("Not said".to_owned(), Some(why.clone())),
            Some(Ok(sandbox)) if sandbox.warnings.is_empty() => ("Confined".to_owned(), None),
            Some(Ok(sandbox)) => ("Gaps".to_owned(), Some(sandbox.warnings.join(" · "))),
        };
        let mut row = ListRow::new("check-flottille-sandbox", "Sandbox")
            .leading(mark_of("check-flottille-sandbox", glyphs::SHIELD, p.faint))
            .trailing(
                div()
                    .id("check-flottille-sandbox-state")
                    .aria_label(SharedString::from(sandbox.0.clone()))
                    .flex_none()
                    .whitespace_nowrap()
                    .text_size(px(13.))
                    .text_color(p.text)
                    .child(SharedString::from(sandbox.0))
                    .test_support(),
            );
        if let Some(line) = sandbox.1 {
            row = row.line(line);
        }
        list = list.row(row);
        for needed in &self.programs {
            let state = &needed.state;
            let id = format!("check-flottille-program-{}-{}", state.program, state.host);
            let line = state
                .lacks()
                .map(|lacks| format!("{lacks} · {}", needed.used_by.join(", ")))
                .unwrap_or_else(|| format!("{} · {}", state.said(), needed.used_by.join(", ")));
            let mark = if state.is_ready() { "Ready" } else { "To do" };
            list = list.row(
                ListRow::new(ElementId::Name(id.clone().into()), state.program.clone())
                    .leading(mark_of(&id, Icon::Terminal, p.faint))
                    .line(line)
                    .trailing(
                        div()
                            .id(ElementId::Name(format!("{id}-state").into()))
                            .aria_label(SharedString::from(mark))
                            .flex_none()
                            .whitespace_nowrap()
                            .text_size(px(13.))
                            .text_color(p.text)
                            .child(SharedString::from(mark))
                            .test_support(),
                    ),
            );
        }
        section(awaiting(
            "check-flottille-awaiting",
            false,
            skeleton,
            div().id("check-flottille-card").child(list).test_support(),
        ))
        .into_any_element()
    }
}
