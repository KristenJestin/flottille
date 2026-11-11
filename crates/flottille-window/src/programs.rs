//! What the project's Settings say of the command lines its forges and tracker need (`gh`, `bkt`,
//! `acli`): found or not with its version, signed in or not, and when it is not, how to install it
//! (the program's own page, and a command where one fits most systems) or how to sign in, each
//! command with a button that copies it. Flottille installs nothing. The daemon checks
//! (`project.programs`, `docs/protocol.md`); the window reads when the page opens, after a change of
//! forge or tracker and when the user asks (Check again), never in a loop.

use gpui_design_system::motion::{HOVER, PANEL};
use gpui_design_system::{
    Button, Icon, Palette, Toast, Type, fade, hoverable, icon, palette, presence, presence_of,
    space, tint, tipped, toast,
};
use gpui_kit::{
    AnyElement, App, ClickEvent, Context, ElementId, Entity, InteractiveElement, IntoElement,
    ParentElement, Render, RenderOnce, SharedString, StatefulInteractiveElement, Styled,
    TestSupportExt as _, WeakEntity, Window, div, px, relative,
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

    /// What stands for a program whose state was never read: not known, so not ready.
    pub fn unread(program: &str) -> Self {
        Self {
            program: program.to_owned(),
            found: true,
            ..Self::default()
        }
    }

    /// What is left to do about it, in one line: `gh is not signed in to github.com`. Nothing
    /// when it is ready.
    pub fn lacks(&self) -> Option<String> {
        let program = &self.program;
        Some(match self.phase() {
            Phase::Ready => return None,
            Phase::Missing => format!("{program} not found"),
            Phase::SignedOut => format!("{program} is not signed in to {}", self.host),
            Phase::Elsewhere => format!(
                "{program} is signed in to {}, not to {}",
                self.signed_in_to.as_deref().unwrap_or_default(),
                self.host
            ),
            Phase::Unknown => match &self.problem {
                Some(why) => format!("could not tell if {program} is signed in: {why}"),
                None => format!("could not tell if {program} is signed in"),
            },
        })
    }

    /// The state in one line, what the value's tooltip says: `gh 2.60.0 · signed in to
    /// github.com`.
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
    /// Whether the user asked for the read on its way (Check again): only then is a failure said.
    asked: bool,
    /// The last read failed.
    failed: bool,
}

/// What the page shows of a failed Check again (a toast, as for the other reads that fail).
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
            asked: false,
            failed: false,
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

    /// Whether the programs were never read: the last read failed and nothing was known before.
    /// The page then says it on the line of each program it needs, with Check again.
    pub fn is_unread(&self) -> bool {
        self.failed && self.read.is_none()
    }

    /// Whether a read the user asked for is on its way.
    pub fn is_asked(&self) -> bool {
        self.waiting.is_some() && self.asked
    }

    pub fn last_request(&self) -> u64 {
        self.requests
    }

    pub fn is_latest(&self, serial: u64) -> bool {
        serial == self.requests
    }

    /// Reads the programs' state anew, in silence: when the tab opens, after a change of forge or
    /// tracker and when the window takes the focus back. A read that fails changes nothing.
    /// Nothing while a read is on its way: a window that comes and goes asks for one read, not
    /// one for each time.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.waiting.is_none() {
            self.read(false, cx);
        }
    }

    /// The same, made even when one is on its way: what a read began before a change of forge or
    /// tracker says is not about the new one.
    pub fn reread(&mut self, cx: &mut Context<Self>) {
        self.read(false, cx);
    }

    /// Check again, asked by the user on the line of a program: a read that fails is said.
    pub fn check(&mut self, cx: &mut Context<Self>) {
        self.read(true, cx);
    }

    fn read(&mut self, asked: bool, cx: &mut Context<Self>) {
        self.requests += 1;
        // A silent read that overtakes the user's does not take their failure away.
        self.asked = asked || self.is_asked();
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
            Ok(value) => {
                self.read = Some(ProjectPrograms::of(&value));
                self.failed = false;
            }
            Err(why) => {
                self.failed = true;
                if self.asked {
                    toast(refused_toast(&why), cx);
                }
            }
        }
        self.asked = false;
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
pub(crate) fn command_row(id: String, command: SharedString, p: Palette) -> AnyElement {
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

/// What a program that is not ready lacks, in one line: `acli not found`, `gh is not signed in
/// to github.com`. When the state changes from one sentence to another (not found, signed out,
/// signed in elsewhere), the old line folds away and fades out while the new one unfolds and fades
/// in, and the ink moves from red to green when nothing is left to say: the line then folds away
/// with its ink turned (`tint`, as a step of a setup does). A program that is ready has no line:
/// its value says it (`ProgramValue`).
#[derive(IntoElement)]
struct StateLine {
    id: String,
    state: Option<ProgramState>,
}

impl RenderOnce for StateLine {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let phase = self.state.as_ref().map(ProgramState::phase);
        let said = self.state.as_ref().and_then(ProgramState::lacks);
        let target = if phase == Some(Phase::Ready) {
            p.ok
        } else {
            p.fail
        };
        let ink = tint(self.id.clone(), target, PANEL, window, cx);
        div().flex().flex_col().children(
            Phase::ALL
                .into_iter()
                .filter(|at| *at != Phase::Ready)
                .map(|at| {
                    let line_id = self.id.clone();
                    let shown = said.clone().filter(|_| phase == Some(at));
                    presence_of(
                        format!("{}-{}", self.id, at.name()),
                        shown.map(SharedString::from),
                        move |said| {
                            div()
                                .id(ElementId::Name(line_id.into()))
                                .aria_label(said.clone())
                                .text_size(px(12.))
                                .text_color(ink)
                                .child(said)
                                .test_support()
                                .into_any_element()
                        },
                    )
                    .gap(space::XS)
                }),
        )
    }
}

/// A labelled value that says a program's state in the one value: `GitHub · gh` and a check mark. The mark
/// fades in when a read makes the program ready and out when it stops being; the version and the
/// host the program is signed in to are the tooltip's. A program that is not ready has no mark:
/// the line under it says what is missing.
#[derive(IntoElement)]
pub struct ProgramValue {
    id: ElementId,
    label_id: ElementId,
    label: &'static str,
    /// What comes before the program: the forge.
    head: Option<String>,
    program: String,
    state: Option<ProgramState>,
    width: f32,
}

impl ProgramValue {
    /// `id` names the value and `label_id` its label. `program` is what the forge or the tracker
    /// needs, known before the first read.
    pub fn new(
        id: impl Into<ElementId>,
        label_id: impl Into<ElementId>,
        label: &'static str,
        head: Option<String>,
        program: &str,
        state: Option<&ProgramState>,
    ) -> Self {
        Self {
            id: id.into(),
            label_id: label_id.into(),
            label,
            head,
            program: program.to_owned(),
            state: state.cloned(),
            width: 1.,
        }
    }

    /// Its share of the row's width.
    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }
}

impl RenderOnce for ProgramValue {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let ready = self.state.as_ref().is_some_and(ProgramState::is_ready);
        let mark = fade(
            ElementId::NamedChild(std::sync::Arc::new(self.id.clone()), "ready".into()),
            ready,
            HOVER,
            window,
            cx,
        );
        let said = match &self.head {
            Some(head) => format!("{head} · {}", self.program),
            None => self.program.clone(),
        };
        // What the mark says to a screen reader and to a test; the icon is what is drawn.
        let aria = if ready {
            format!("{said}, ready")
        } else {
            said.clone()
        };
        let value = div()
            .id(self.id)
            .aria_label(SharedString::from(aria))
            .flex()
            .items_center()
            .gap(space::XS)
            .mono(12.)
            .text_color(p.mute)
            .child(
                div()
                    .min_w_0()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(SharedString::from(said)),
            )
            .child(
                div().flex_none().child(
                    icon(Icon::Check)
                        .size(px(12.))
                        .text_color(p.ok.opacity(mark)),
                ),
            );
        let value = match &self.state {
            Some(state) => {
                tipped(value, SharedString::from(state.said()), 18., window, cx).into_any_element()
            }
            None => value.test_support().into_any_element(),
        };
        div()
            .w(relative(self.width))
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .id(self.label_id)
                    .aria_label(self.label)
                    .text_size(px(11.))
                    .text_color(p.faint)
                    .child(self.label)
                    .test_support(),
            )
            .child(value)
    }
}

/// What a program that is not ready lacks, and under it the advice it needs: how to install it,
/// or how to sign in, and a quiet Check again on that line only. `id` names the block
/// (`<id>-state`, `<id>-install`, `<id>-install-command`, `<id>-sign-in-0`, `-1`, each command's
/// button `<id>-…-copy`, and `<id>-check-again`). Every line unfolds and fades in when it comes
/// and folds away when it goes; a program that is ready has none.
pub fn program_block(
    id: &str,
    state: Option<&ProgramState>,
    shell: &WeakEntity<Shell>,
    checks: &Entity<ProgramChecks>,
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
    let waiting = checks.read(cx).is_asked();
    let again = {
        let checks = checks.downgrade();
        presence(
            format!("{id}-check-block"),
            state.is_some_and(|state| !state.is_ready()),
            Button::quiet(format!("{id}-check-again"), "Check again")
                .disabled(waiting)
                .on_click(move |_: &ClickEvent, _, cx| {
                    // The row under it opens its dialog on a click: not this one.
                    cx.stop_propagation();
                    checks.update(cx, |checks, cx| checks.check(cx)).ok();
                }),
        )
    };
    div()
        .flex()
        .items_start()
        .gap(space::SM)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(state_line)
                .child(install_page)
                .child(install_command)
                .children(sign_in),
        )
        .child(div().flex_none().child(again))
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
