//! What the project's Settings say of the command lines its forges and tracker need (`gh`, `bkt`,
//! `acli`): found or not with its version, signed in or not, and when it is not, how to install it
//! (the program's own page, and a command where one fits most systems) or how to sign in, each
//! command with a button that copies it. Flottille installs nothing. The daemon checks
//! (`project.programs`, `docs/protocol.md`); the window reads when the page opens, after a change of
//! forge or tracker and when the user asks (Check again), never in a loop.

use flottille_ui::motion::PANEL;
use flottille_ui::{
    Button, Icon, Palette, Toast, Type, hoverable, icon, palette, presence_of, space, tint, toast,
};
use gpui_kit::{
    AnyElement, App, ClickEvent, Context, ElementId, InteractiveElement, IntoElement,
    ParentElement, Render, RenderOnce, SharedString, StatefulInteractiveElement, Styled,
    TestSupportExt as _, WeakEntity, Window, div, px,
};
use serde_json::Value;

use crate::actions::Command;
use crate::commands::perform_later;
use crate::shell::Shell;

/// One program's state for one host, as `project.programs` says it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProgramState {
    pub program: String,
    pub host: String,
    pub found: bool,
    pub version: Option<String>,
    /// None while the program is not found, or could not say (`problem`).
    pub signed_in: Option<bool>,
    /// The other site `acli` is signed in to, when it is not the tracker's.
    pub signed_in_to: Option<String>,
    pub install_page: Option<String>,
    pub install_command: Option<String>,
    /// The commands that sign in, in order.
    pub sign_in: Vec<String>,
    pub problem: Option<String>,
}

impl ProgramState {
    pub fn of(value: &Value) -> Self {
        let text = |value: &Value| value.as_str().map(str::to_owned);
        Self {
            program: text(&value["program"]).unwrap_or_default(),
            host: text(&value["host"]).unwrap_or_default(),
            found: value["found"] == true,
            version: text(&value["version"]),
            signed_in: value["signed_in"].as_bool(),
            signed_in_to: text(&value["signed_in_to"]),
            install_page: text(&value["install"]["page"]),
            install_command: text(&value["install"]["command"]),
            sign_in: value["sign_in"]["commands"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|command| command.as_str().map(str::to_owned))
                .collect(),
            problem: text(&value["problem"]),
        }
    }

    /// Whether nothing is left to do about it.
    pub fn is_ready(&self) -> bool {
        self.found && self.signed_in == Some(true)
    }

    /// Which of the state's sentences it is: a change of it is what moves in the window.
    fn phase(&self) -> Phase {
        match (self.found, self.signed_in) {
            (false, _) => Phase::Missing,
            (true, Some(true)) => Phase::Ready,
            (true, Some(false)) if self.signed_in_to.is_some() => Phase::Elsewhere,
            (true, Some(false)) => Phase::SignedOut,
            (true, None) => Phase::Unknown,
        }
    }

    /// The state in one line: `gh 2.60.0 · signed in to github.com`.
    pub fn said(&self) -> String {
        if !self.found {
            return format!("{} not found", self.program);
        }
        let version = self.version.as_deref().unwrap_or("version unknown");
        let signed = match (self.signed_in, &self.problem) {
            (Some(true), _) => format!("signed in to {}", self.host),
            (Some(false), _) => match &self.signed_in_to {
                Some(other) => format!("signed in to {other}, not to {}", self.host),
                None => format!("not signed in to {}", self.host),
            },
            (None, Some(why)) => format!("could not tell if it is signed in: {why}"),
            (None, None) => "could not tell if it is signed in".to_owned(),
        };
        format!("{} {version} · {signed}", self.program)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Missing,
    SignedOut,
    Elsewhere,
    Unknown,
    Ready,
}

impl Phase {
    const ALL: [Phase; 5] = [
        Phase::Missing,
        Phase::SignedOut,
        Phase::Elsewhere,
        Phase::Unknown,
        Phase::Ready,
    ];

    fn name(self) -> &'static str {
        match self {
            Phase::Missing => "missing",
            Phase::SignedOut => "signed-out",
            Phase::Elsewhere => "elsewhere",
            Phase::Unknown => "unknown",
            Phase::Ready => "ready",
        }
    }
}

/// The programs of a project's forges and tracker, checked.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProjectPrograms {
    /// By the repository's path in the project.
    pub repositories: Vec<(String, ProgramState)>,
    pub tracker: Option<ProgramState>,
}

impl ProjectPrograms {
    pub fn of(value: &Value) -> Self {
        Self {
            repositories: value["repositories"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|state| {
                    (
                        state["repository"].as_str().unwrap_or_default().to_owned(),
                        ProgramState::of(state),
                    )
                })
                .collect(),
            tracker: value["tracker"]
                .is_object()
                .then(|| ProgramState::of(&value["tracker"])),
        }
    }
}

pub struct ProgramChecks {
    shell: WeakEntity<Shell>,
    project: String,
    read: Option<ProjectPrograms>,
    /// How many reads were asked: an answer to an older one never replaces a newer one's.
    requests: u64,
    /// The number of the read on its way, if one is.
    waiting: Option<u64>,
}

/// What the page shows of a failed read (a toast, as for the other reads that fail).
pub fn refused_toast(why: &str) -> Toast {
    Toast::failure("The programs were not checked").detail(why.to_owned())
}

impl ProgramChecks {
    pub fn new(shell: WeakEntity<Shell>, project: String) -> Self {
        Self {
            shell,
            project,
            read: None,
            requests: 0,
            waiting: None,
        }
    }

    /// What the last read found; none before the first answer.
    pub fn checked(&self) -> Option<&ProjectPrograms> {
        self.read.as_ref()
    }

    pub fn repository(&self, path: &str) -> Option<&ProgramState> {
        self.read
            .as_ref()?
            .repositories
            .iter()
            .find(|(known, _)| known == path)
            .map(|(_, state)| state)
    }

    pub fn tracker(&self) -> Option<&ProgramState> {
        self.read.as_ref()?.tracker.as_ref()
    }

    pub fn is_checking(&self) -> bool {
        self.waiting.is_some()
    }

    pub fn last_request(&self) -> u64 {
        self.requests
    }

    pub fn is_latest(&self, serial: u64) -> bool {
        serial == self.requests
    }

    /// Reads the programs' state anew: when the page opens, after a change of forge or tracker,
    /// and for Check again.
    pub fn check(&mut self, cx: &mut Context<Self>) {
        self.requests += 1;
        self.waiting = Some(self.requests);
        let command = Command::Programs {
            project: self.project.clone(),
            serial: self.requests,
        };
        perform_later(&self.shell, command, cx);
        cx.notify();
    }

    /// The daemon's answer to read `serial`. A refusal leaves what was known and is said; the
    /// next read is free to be asked again.
    pub fn answered(&mut self, serial: u64, read: Result<Value, String>, cx: &mut Context<Self>) {
        if !self.is_latest(serial) {
            return;
        }
        self.waiting = None;
        match read {
            Ok(value) => self.read = Some(ProjectPrograms::of(&value)),
            Err(why) => toast(refused_toast(&why), cx),
        }
        cx.notify();
    }
}

impl Render for ProgramChecks {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// A line of the block, that comes and goes with its text (`presence_of`).
fn line_of<B: FnOnce(SharedString) -> AnyElement + 'static>(
    id: &str,
    text: Option<String>,
    build: B,
) -> impl IntoElement + use<B> {
    presence_of(format!("{id}-block"), text.map(SharedString::from), build).gap(space::XS)
}

/// A command with the button that copies it.
fn command_row(id: String, command: SharedString, p: Palette) -> AnyElement {
    let copied = command.clone();
    div()
        .flex()
        .items_center()
        .gap(space::SM)
        .child(
            div()
                .id(ElementId::Name(id.clone().into()))
                .aria_label(command.clone())
                .flex_1()
                .min_w_0()
                .mono(12.)
                .text_color(p.text)
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .child(command)
                .test_support(),
        )
        .child(
            Button::quiet(ElementId::Name(format!("{id}-copy").into()), "Copy")
                .leading(Icon::Copy)
                .on_click(move |_: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(copied.to_string()));
                    toast(Toast::success("Copied").detail(copied.clone()), cx);
                }),
        )
        .into_any_element()
}

/// The state line of a program: `✓ acli 1.3.2 · signed in to example.atlassian.net`. When the
/// state changes from one sentence to another (not found, signed out, signed in elsewhere,
/// ready), the old line folds away and fades out while the new one unfolds and fades in, and the
/// ink moves from red to green with them (`tint`, as a step of a setup does).
#[derive(IntoElement)]
struct StateLine {
    id: String,
    state: Option<ProgramState>,
}

impl RenderOnce for StateLine {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let phase = self.state.as_ref().map(ProgramState::phase);
        let said = self.state.as_ref().map(ProgramState::said);
        let ready = phase == Some(Phase::Ready);
        let target = if ready { p.ok } else { p.fail };
        let ink = tint(self.id.clone(), target, PANEL, window, cx);
        div().flex().flex_col().children(Phase::ALL.map(|at| {
            let line_id = self.id.clone();
            // Each line keeps its own mark while it folds away.
            let mark = if at == Phase::Ready {
                Icon::Check
            } else {
                Icon::Close
            };
            let shown = said.clone().filter(|_| phase == Some(at));
            presence_of(
                format!("{}-{}", self.id, at.name()),
                shown.map(SharedString::from),
                move |said| {
                    div()
                        .id(ElementId::Name(line_id.into()))
                        .aria_label(said.clone())
                        .flex()
                        .items_center()
                        .gap(space::XS)
                        .text_size(px(12.))
                        .text_color(ink)
                        .child(icon(mark).size(px(12.)).text_color(ink))
                        .child(said)
                        .test_support()
                        .into_any_element()
                },
            )
            .gap(space::XS)
        }))
    }
}

/// The state line of a program, and under it the advice it needs: how to install it, or how to
/// sign in. `id` names the block (`<id>-state`, `<id>-install`, `<id>-install-command`,
/// `<id>-sign-in-0`, `-1`, each command's button `<id>-…-copy`). Every line unfolds and fades in
/// when it comes and folds away when it goes.
pub fn program_block(
    id: &str,
    state: Option<&ProgramState>,
    shell: &WeakEntity<Shell>,
    cx: &App,
) -> impl IntoElement + use<> {
    let p = palette(cx);
    let state_line = StateLine {
        id: format!("{id}-state"),
        state: state.cloned(),
    };
    let install_page = {
        let shell = shell.clone();
        let link_id = format!("{id}-install");
        let page = state.and_then(|s| s.install_page.clone());
        line_of(&link_id.clone(), page, move |page| {
            let opened = page.clone();
            hoverable(
                div()
                    .id(ElementId::Name(link_id.into()))
                    .aria_label(page.clone())
                    .text_size(px(12.))
                    .text_color(p.run)
                    .cursor_pointer()
                    .child(SharedString::from(format!("Install it: {page}")))
                    .on_click(move |_: &ClickEvent, _, cx| {
                        // The row under it opens its dialog on a click: not this one.
                        cx.stop_propagation();
                        shell.update(cx, |shell, _| shell.open_link(&opened)).ok();
                    }),
                move |link, t| link.bg(p.hover.opacity(t)),
            )
            .into_any_element()
        })
    };
    let install_command = {
        let command_id = format!("{id}-install-command");
        let command = state.and_then(|s| s.install_command.clone());
        line_of(&command_id.clone(), command, move |command| {
            command_row(command_id, command, p)
        })
    };
    let sign_in: Vec<_> = (0..2)
        .map(|at| {
            let command_id = format!("{id}-sign-in-{at}");
            let command = state.and_then(|s| s.sign_in.get(at).cloned());
            line_of(&command_id.clone(), command, move |command| {
                command_row(command_id, command, p)
            })
        })
        .collect();
    div()
        .flex()
        .flex_col()
        .child(state_line)
        .child(install_page)
        .child(install_command)
        .children(sign_in)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn state(found: bool, signed_in: Value, signed_in_to: Value) -> ProgramState {
        ProgramState::of(&json!({
            "program": "acli", "host": "a.example.net", "found": found,
            "signed_in": signed_in, "signed_in_to": signed_in_to
        }))
    }

    #[test]
    fn each_sentence_a_state_can_change_to_is_its_own_phase_so_that_the_change_moves() {
        let phases = [
            state(false, Value::Null, Value::Null).phase(),
            state(true, json!(false), Value::Null).phase(),
            state(true, json!(false), json!("b.example.net")).phase(),
            state(true, Value::Null, Value::Null).phase(),
            state(true, json!(true), Value::Null).phase(),
        ];
        assert_eq!(phases, Phase::ALL);
    }

    #[test]
    fn the_repository_is_read_by_its_own_key_and_not_by_the_program_s_path() {
        let read = ProjectPrograms::of(&json!({
            "repositories": [{"repository": "api", "path": "/usr/bin/gh", "program": "gh"}],
            "tracker": null
        }));
        assert_eq!(read.repositories[0].0, "api");
    }
}
