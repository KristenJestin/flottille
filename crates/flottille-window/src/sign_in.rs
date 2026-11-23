//! Signing the agents and the forge programs in, in Settings, Agents (#1185, `docs/agents.md`,
//! "Signing in"): each program's row says whether it is signed in, as the daemon read its own
//! status (`signin.list`), and offers its own sign-in, run by the daemon of the machine shown. The
//! dialog shows what the user needs (the address, opened with this machine's handler, and the
//! one-time code with a copy button, or a hidden field for a code the browser shows) while the
//! user finishes in their browser; it closes when the daemon says how it ended. Sign out is asked
//! once. A program that signs in only in a terminal says the line to type there.
//!
//! The window keeps nothing of a sign-in: the address and the code live in the dialog while it is
//! open, the pasted code leaves the field as it is sent, and none is ever written anywhere.

use std::collections::HashSet;

use gpui_design_system::{
    Button, Confirm, Field, ListCard, ListRow, Listing, Modal, Section, Skeleton, Toast, awaiting,
    changing, hoverable, palette, presence, presence_of, space, toast,
};
use gpui_kit::base::input::{Input, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, FocusHandle, Focusable as _,
    InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, TestSupportExt as _, WeakEntity, Window, div, px,
};
use serde_json::Value;

use crate::actions::{Command, PastedCode};
use crate::commands::{boxed, perform_later, problem_line, spaced};
use crate::modals::Modals;
use crate::shell::Shell;

/// The id of the sign-in's dialog, the one the window's host holds.
pub const MODAL: &str = "sign-in-modal";

/// What the section says under its title.
pub const SENTENCE: &str = "The command lines your projects' forges and trackers use. Each signs \
                            in with its own login, on this machine.";

/// What the dialog says under its title.
pub const DIALOG_SENTENCE: &str = "Finish in your browser, on any device. Flottille never sees your \
                                   password and keeps nothing the program prints.";

/// What the dialog says when the program shows a code and none could be read from what it printed.
pub const NO_CODE_SENTENCE: &str = "Finish in your browser with the code the program shows.";

/// What the field of a pasted code says under it.
pub const PASTED_HINT: &str = "If your browser shows a code, paste it here: it goes to the program \
                               once and is kept nowhere.";

/// The row of `program` in what `signin.list` answered: an older daemon answers the whole list
/// whatever the program asked.
fn row_of(answer: &Value, program: &str) -> Option<SignInRow> {
    answer["programs"]
        .as_array()?
        .iter()
        .map(SignInRow::of)
        .find(|row| row.program == program)
}

/// A program as `signin.list` gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct SignInRow {
    pub program: String,
    pub title: String,
    pub agent: bool,
    pub host: Option<String>,
    pub found: bool,
    pub version: Option<String>,
    /// What its own status says; `None` when it could not say.
    pub signed_in: Option<bool>,
    pub account: Option<String>,
    pub can_sign_in: bool,
    /// The line to type in a terminal when it cannot sign in from here.
    pub line: Option<String>,
    pub running: bool,
    /// Its check runs: the row says so, and offers nothing, until the daemon says the result.
    pub checking: bool,
    /// Why its check could not be made (#1315): the command line's first error line, masked, or
    /// `no answer in N s`. A program that could not be checked is not a program that is not signed
    /// in: it is never offered a sign-in.
    pub problem: Option<String>,
    /// The window's own request for the check failed, so no command line ever said anything:
    /// `problem` says why the window could not ask, not what a program answered.
    pub unreached: bool,
}

impl SignInRow {
    pub fn of(value: &Value) -> SignInRow {
        let text = |key: &str| value[key].as_str().map(str::to_owned);
        SignInRow {
            program: text("program").unwrap_or_default(),
            title: text("title").unwrap_or_default(),
            agent: value["kind"] == "agent",
            host: text("host"),
            found: value["found"] == true,
            version: text("version"),
            signed_in: value["signed_in"].as_bool(),
            account: text("account"),
            can_sign_in: value["can_sign_in"] == true,
            line: text("line"),
            running: value["running"] == true,
            checking: value["checking"] == true,
            problem: text("problem"),
            unreached: false,
        }
    }

    /// Its state, once: signing in, checking, not installed, signed in (as whom), not signed in,
    /// or could not be checked.
    pub fn said(&self) -> String {
        if self.running && self.found {
            return "Signing in…".into();
        }
        // Never a guessed state while its check runs: not the last result as if it were fresh.
        if self.checking {
            return "Checking…".into();
        }
        if !self.found {
            return "Not installed".into();
        }
        match (self.signed_in, &self.account, &self.problem) {
            (Some(true), Some(account), _) => format!("Signed in as {account}"),
            (Some(true), None, _) => "Signed in".into(),
            (Some(false), ..) => "Not signed in".into(),
            (None, _, Some(problem)) if problem.starts_with("no answer in") => {
                format!("Could not check: {problem}")
            }
            (None, _, Some(_)) if self.unreached => {
                "Could not check: Flottille could not ask the daemon".into()
            }
            (None, _, Some(_)) => format!("Could not check: {} failed", self.program),
            (None, _, None) => format!("{} did not say whether it is signed in", self.program),
        }
    }

    /// The window's own request for this row's check failed: the daemon never said anything.
    fn failed(&mut self, why: String) {
        self.checking = false;
        self.signed_in = None;
        self.problem = Some(why);
        self.unreached = true;
    }

    /// What the command line said of the failure, under the row: none for a check that got no
    /// answer, which the state says whole.
    fn reason(&self) -> Option<&str> {
        self.problem
            .as_deref()
            .filter(|problem| !problem.starts_with("no answer in"))
            .filter(|_| !self.checking && self.signed_in.is_none())
    }

    fn offers_sign_in(&self) -> bool {
        self.found
            && self.can_sign_in
            && self.signed_in != Some(true)
            && self.problem.is_none()
            && !self.running
            && !self.checking
    }

    fn offers_sign_out(&self) -> bool {
        self.found && self.signed_in == Some(true) && !self.running && !self.checking
    }
}

/// A sign-in the dialog shows: what the daemon answered of it.
#[derive(Clone, Debug, Default, PartialEq)]
struct Flow {
    program: String,
    title: String,
    address: Option<String>,
    code: Option<String>,
    asks_code: bool,
    /// The program shows a one-time code (a device flow).
    shows_code: bool,
    /// The daemon answered the start.
    started: bool,
    /// The pasted code was sent.
    sent: bool,
}

pub struct SignInSection {
    shell: WeakEntity<Shell>,
    modals: Entity<Modals>,
    /// The programs as last listed; `None` until the daemon said.
    listed: Option<Vec<SignInRow>>,
    /// The forges' rows as drawn: a row arrives, leaves and moves animated.
    rows: Listing<SignInRow>,
    problem: Option<SharedString>,
    /// The sign-in the dialog shows, while it is open.
    flow: Option<Flow>,
    /// The last one shown, kept while the dialog fades away.
    shown: Flow,
    /// The field of a pasted code, made when the dialog first opens, masked.
    pasted: Option<Entity<InputState>>,
    /// The program whose sign-out is asked.
    confirming: Option<String>,
    /// The one it named, kept while the confirmation fades away.
    asked: String,
    /// The programs a request is out for: their buttons wait for the daemon.
    pending: HashSet<String>,
    /// The programs whose check is asked and not answered: a list read again meanwhile does not
    /// ask it twice.
    checks: HashSet<String>,
}

impl SignInSection {
    pub fn new(shell: WeakEntity<Shell>, modals: Entity<Modals>) -> Self {
        Self {
            shell,
            modals,
            listed: None,
            rows: Listing::default(),
            problem: None,
            flow: None,
            shown: Flow::default(),
            pasted: None,
            confirming: None,
            asked: String::new(),
            pending: HashSet::new(),
            checks: HashSet::new(),
        }
    }

    /// The programs as last listed.
    pub fn programs(&self) -> Option<&[SignInRow]> {
        self.listed.as_deref()
    }

    pub fn row(&self, program: &str) -> Option<&SignInRow> {
        self.listed
            .iter()
            .flatten()
            .find(|row| row.program == program)
    }

    pub fn is_open(&self) -> bool {
        self.flow.is_some()
    }

    pub fn focus_handle(&self, cx: &App) -> Option<FocusHandle> {
        self.flow
            .as_ref()
            .filter(|flow| flow.asks_code)
            .and(self.pasted.as_ref())
            .map(|field| field.read(cx).focus_handle(cx))
    }

    /// Asks the daemon for the rows: each says it is being checked, and its check is asked as the
    /// rows arrive (`listed`).
    pub fn read(&self, cx: &mut Context<Self>) {
        perform_later(&self.shell, Command::SignIns, cx);
    }

    /// Checks one program again: its row says so at once and its buttons wait.
    pub fn check(&mut self, program: &str, cx: &mut Context<Self>) {
        if let Some(row) = self
            .listed
            .iter_mut()
            .flatten()
            .find(|row| row.program == program)
        {
            row.checking = true;
        }
        self.checks.insert(program.to_owned());
        perform_later(&self.shell, Command::SignInCheck(program.to_owned()), cx);
        cx.notify();
    }

    /// The daemon's rows, in the daemon's fixed order; the check of each that has not been made is
    /// asked, one by one, each answered by its own result (`checked`).
    pub fn listed(&mut self, listed: Result<Value, String>, cx: &mut Context<Self>) {
        match listed {
            Ok(listed) => {
                self.problem = None;
                let rows: Vec<SignInRow> = listed["programs"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(SignInRow::of)
                    .collect();
                for row in rows.iter().filter(|row| row.checking) {
                    if self.checks.insert(row.program.clone()) {
                        perform_later(&self.shell, Command::SignInCheck(row.program.clone()), cx);
                    }
                }
                if self.listed.as_ref() != Some(&rows) {
                    self.listed = Some(rows);
                }
            }
            Err(why) => self.problem = Some(why.into()),
        }
        cx.notify();
    }

    /// One program's result replaces its row where it stands: the others keep their places. A check
    /// the daemon could not answer is that row's reason, not a sign-in to offer.
    pub fn checked(
        &mut self,
        program: &str,
        answer: Result<Value, String>,
        cx: &mut Context<Self>,
    ) {
        self.checks.remove(program);
        let Some(row) = self
            .listed
            .iter_mut()
            .flatten()
            .find(|row| row.program == program)
        else {
            return;
        };
        match answer {
            Ok(answer) => {
                if let Some(checked) = row_of(&answer, program) {
                    *row = checked;
                }
            }
            Err(why) => row.failed(why),
        }
        cx.notify();
    }

    fn field(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if self.pasted.is_none() {
            self.pasted = Some(cx.new(|cx| {
                let mut field = InputState::new(window, cx).masked(true);
                gpui_design_system::follow_theme(&mut field, cx);
                field
            }));
        }
        self.pasted.clone().expect("made above")
    }

    /// The pasted code leaves the field as soon as it is sent, or the dialog closed.
    fn clear_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = &self.pasted {
            field.update(cx, |f, cx| f.set_value("", window, cx));
        }
    }

    /// "Sign in": the daemon starts the program's own sign-in; the dialog opens at once and
    /// shows what the program says as soon as the daemon answers.
    pub fn start(&mut self, program: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.row(program).cloned() else {
            return;
        };
        self.field(window, cx);
        self.clear_field(window, cx);
        let flow = Flow {
            program: row.program.clone(),
            title: row.title.clone(),
            ..Flow::default()
        };
        self.shown = flow.clone();
        self.flow = Some(flow);
        self.pending.insert(row.program.clone());
        perform_later(&self.shell, Command::SignInStart(row.program), cx);
        self.modals.update(cx, |modals, cx| modals.open(MODAL, cx));
        cx.notify();
    }

    /// The daemon's answer to a start: what the user needs, or why it did not start.
    pub fn started(
        &mut self,
        program: &str,
        answer: Result<Value, String>,
        cx: &mut Context<Self>,
    ) {
        self.pending.remove(program);
        let Some(flow) = self.flow.as_mut().filter(|flow| flow.program == program) else {
            cx.notify();
            return;
        };
        match answer {
            Ok(answer) => {
                flow.started = true;
                flow.address = answer["address"].as_str().map(str::to_owned);
                flow.code = answer["code"].as_str().map(str::to_owned);
                flow.asks_code = answer["asks_code"] == true;
                flow.shows_code = answer["shows_code"] == true;
                self.shown = flow.clone();
            }
            Err(why) => {
                self.flow = None;
                toast(
                    Toast::failure(format!("{program} could not start its sign-in")).detail(why),
                    cx,
                );
            }
        }
        self.check(program, cx);
        cx.notify();
    }

    /// The daemon said that a program's sign-in moved (`signin.changed`): the list is read again,
    /// and the dialog of that program closes once it ended, saying how.
    pub fn changed(
        &mut self,
        program: &str,
        state: &str,
        why: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.check(program, cx);
        if !matches!(state, "signed_in" | "failed" | "cancelled") {
            return;
        }
        let shown = self
            .flow
            .as_ref()
            .is_some_and(|flow| flow.program == program);
        if shown {
            self.flow = None;
            match state {
                "signed_in" => toast(Toast::success(format!("{program} is signed in")), cx),
                "failed" => toast(
                    Toast::failure(format!("{program} is not signed in"))
                        .detail(why.unwrap_or_default()),
                    cx,
                ),
                _ => {}
            }
        }
        cx.notify();
    }

    /// "Send code": the pasted code goes to the daemon once, and leaves the field.
    fn send_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(flow) = self
            .flow
            .as_mut()
            .filter(|flow| flow.asks_code && !flow.sent)
        else {
            return;
        };
        let Some(field) = self.pasted.clone() else {
            return;
        };
        let code = field.read(cx).value().trim().to_owned();
        if code.is_empty() {
            return;
        }
        flow.sent = true;
        self.shown = flow.clone();
        let program = flow.program.clone();
        self.clear_field(window, cx);
        perform_later(
            &self.shell,
            Command::SignInCode(program, PastedCode::from(code)),
            cx,
        );
        cx.notify();
    }

    /// The dialog closed by Cancel, Escape or its scrim: a sign-in that runs is stopped.
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(flow) = self.flow.take() {
            perform_later(&self.shell, Command::SignInCancel(flow.program), cx);
        }
        self.clear_field(window, cx);
        cx.notify();
    }

    /// What the daemon answered to a code, a cancel or a sign-out: a refusal is a toast.
    pub fn answered(
        &mut self,
        program: &str,
        what: &str,
        refused: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.pending.remove(program);
        match (what, refused) {
            ("sign_out", None) => toast(Toast::success(format!("{program} is signed out")), cx),
            ("sign_out", Some(why)) => toast(
                Toast::failure(format!("{program} was not signed out")).detail(why),
                cx,
            ),
            ("code", Some(why)) => {
                if let Some(flow) = self.flow.as_mut().filter(|flow| flow.program == program) {
                    flow.sent = false;
                }
                toast(
                    Toast::failure(format!("The code did not reach {program}")).detail(why),
                    cx,
                )
            }
            _ => {}
        }
        self.check(program, cx);
        cx.notify();
    }

    fn sign_out(&mut self, program: &str, cx: &mut Context<Self>) {
        self.asked = program.to_owned();
        self.confirming = Some(program.to_owned());
        cx.notify();
    }

    fn confirmed(&mut self, cx: &mut Context<Self>) {
        if let Some(program) = self.confirming.take() {
            self.pending.insert(program.clone());
            perform_later(&self.shell, Command::SignOut(program), cx);
        }
        cx.notify();
    }

    /// What a row offers: its state, said once (its ink and text cross-fade when it changes),
    /// the line to type when it signs in only in a terminal, and its buttons. `line` is false
    /// where the row already says that line (a provider's own sign-in block).
    pub fn controls(&self, program: &str, line: bool, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let Some(row) = self.row(program).cloned() else {
            return div().into_any_element();
        };
        let said = row.said();
        let ink = match row.signed_in {
            _ if row.checking => p.mute,
            Some(true) if row.found => p.ok,
            _ if !row.found => p.faint,
            _ => p.mute,
        };
        let state_id = format!("sign-in-state-{program}");
        let state = changing(format!("{state_id}-text"), said, move |said| {
            div()
                .id(ElementId::Name(state_id.into()))
                .aria_label(said.clone())
                .text_size(px(12.5))
                .text_color(ink)
                .child(said)
                .test_support()
                .into_any_element()
        });
        let pending = self.pending.contains(program);
        let buttons = div()
            .flex()
            .items_center()
            .gap(space::SM)
            .child(presence(
                format!("sign-in-{program}-block"),
                row.offers_sign_in(),
                Button::secondary(
                    ElementId::Name(format!("sign-in-{program}").into()),
                    "Sign in",
                )
                .disabled(pending)
                .on_click({
                    let program = program.to_owned();
                    cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.start(&program, window, cx)
                    })
                }),
            ))
            .child(presence(
                format!("sign-out-{program}-block"),
                row.offers_sign_out(),
                Button::quiet(
                    ElementId::Name(format!("sign-out-{program}").into()),
                    "Sign out",
                )
                .disabled(pending)
                .on_click({
                    let program = program.to_owned();
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.sign_out(&program, cx)
                    })
                }),
            ))
            .child(presence(
                format!("sign-in-check-{program}-block"),
                row.found,
                Button::quiet(
                    ElementId::Name(format!("sign-in-check-{program}").into()),
                    "Check again",
                )
                .disabled(row.checking)
                .on_click({
                    let program = program.to_owned();
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        if this.row(&program).is_some_and(|row| !row.checking) {
                            this.check(&program, cx)
                        }
                    })
                }),
            ));
        // A skeleton under the state while its check runs (rule 11: a part that waits for a slow
        // read): it pulses, and fades out when the result comes; the state keeps saying Checking….
        let checking = awaiting(
            format!("sign-in-checking-{program}"),
            row.checking,
            div().pt(space::SM).child(
                Skeleton::new(format!("sign-in-checking-{program}-skeleton"))
                    .w(px(64.))
                    .h(px(6.)),
            ),
            div(),
        );
        // What the command line said when its check could not be made, under the state.
        let reason = presence_of(
            format!("sign-in-reason-{program}-block"),
            row.reason().map(|why| SharedString::from(why.to_owned())),
            {
                let id = format!("sign-in-reason-{program}");
                move |why| crate::programs::command_row(id, why, p)
            },
        )
        .space(space::SM);
        let typed = presence_of(
            format!("sign-in-line-{program}-block"),
            row.line
                .clone()
                .filter(|_| {
                    line && row.found
                        && row.signed_in != Some(true)
                        && !row.checking
                        && row.problem.is_none()
                })
                .map(SharedString::from),
            {
                let id = format!("sign-in-line-{program}");
                move |line| crate::programs::command_row(id, line, p)
            },
        )
        .space(space::SM);
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::MD)
                    .child(div().flex_1().min_w_0().child(state))
                    .child(buttons),
            )
            .child(checking)
            .child(reason)
            .child(typed)
            .into_any_element()
    }

    /// The confirmation of a sign-out.
    fn confirm(&self, cx: &mut Context<Self>) -> Confirm {
        let program = self.asked.clone();
        let (this, again) = (cx.entity().downgrade(), cx.entity().downgrade());
        Confirm::new("sign-out", format!("Sign {program} out?"))
            .sentence(format!(
                "{program} runs its own sign-out on this machine: what uses it stops working until \
                 it is signed in again."
            ))
            .action("Sign out")
            .open(self.confirming.is_some())
            .on_confirm(move |_, cx| {
                this.update(cx, |this, cx| this.confirmed(cx)).ok();
            })
            .on_cancel(move |_, cx| {
                again
                    .update(cx, |this, cx| {
                        this.confirming = None;
                        cx.notify();
                    })
                    .ok();
            })
    }

    /// The dialog of a sign-in, drawn by the window's host: open while a sign-in is shown.
    pub fn dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Modal {
        let p = palette(cx);
        let this = cx.entity().downgrade();
        let flow = self.flow.clone().unwrap_or_else(|| self.shown.clone());
        let field = self.field(window, cx);
        let modal = Modal::new(MODAL, format!("Sign {} in", flow.title))
            .open(self.flow.is_some())
            .on_close(move |window, cx| {
                this.update(cx, |this, cx| this.close(window, cx)).ok();
            })
            .sentence(DIALOG_SENTENCE);
        let shell = self.shell.clone();
        let address = flow.address.clone().map(SharedString::from);
        let link = presence_of("sign-in-address-block", address, move |address| {
            let opened = address.to_string();
            hoverable(
                div()
                    .id("sign-in-address")
                    .aria_label(address.clone())
                    .text_size(px(13.))
                    .text_color(p.run)
                    .cursor_pointer()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(address)
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        if opens(&opened) {
                            shell.update(cx, |shell, _| shell.open_link(&opened)).ok();
                        }
                    }),
                move |link, t| link.bg(p.hover.opacity(t)),
            )
            .into_any_element()
        });
        let code = presence_of(
            "sign-in-code-block",
            flow.code.clone().map(SharedString::from),
            move |code| crate::programs::command_row("sign-in-code".into(), code, p),
        )
        .space(space::MD);
        let no_code = presence(
            "sign-in-no-code-block",
            flow.shows_code && flow.code.is_none() && flow.address.is_some(),
            div()
                .id("sign-in-no-code")
                .aria_label(NO_CODE_SENTENCE)
                .text_size(px(12.5))
                .text_color(p.mute)
                .child(NO_CODE_SENTENCE)
                .test_support(),
        )
        .space(space::MD);
        let pasted = presence(
            "sign-in-pasted-block",
            flow.asks_code,
            Field::new(
                "sign-in-pasted-field",
                "Code from your browser",
                boxed("sign-in-pasted", Input::new(&field), cx),
            )
            .hint(PASTED_HINT),
        )
        .space(space::LG);
        let waiting_line = presence(
            "sign-in-waiting-line-block",
            flow.started,
            div()
                .id("sign-in-waiting-line")
                .text_size(px(12.5))
                .text_color(p.mute)
                .child(if flow.sent {
                    "The code was sent: waiting for the program to say it is signed in."
                } else {
                    "Waiting for you to finish in your browser."
                })
                .test_support(),
        )
        .space(space::MD);
        let body = div()
            .id("sign-in-body")
            .flex()
            .flex_col()
            .child(awaiting(
                "sign-in-starting",
                !flow.started,
                Skeleton::new("sign-in-skeleton").h(px(48.)),
                div()
                    .flex()
                    .flex_col()
                    .child(link)
                    .child(code)
                    .child(no_code),
            ))
            .child(pasted)
            .child(waiting_line);
        let modal = modal.child(body);
        match flow.asks_code {
            true => modal.action(
                Button::primary("sign-in-send-code", "Send code")
                    .disabled(flow.sent)
                    .on_click(cx.listener(|this, _, window, cx| this.send_code(window, cx))),
            ),
            false => modal,
        }
    }
}

impl Render for SignInSection {
    /// The section of the forges and trackers: a card of rows, a skeleton until the daemon said
    /// them, and the confirmation of a sign-out.
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let forges: Vec<SignInRow> = self
            .listed
            .iter()
            .flatten()
            .filter(|row| !row.agent)
            .cloned()
            .collect();
        let now = cx.background_executor().now();
        let reduce = cx.reduce_motion();
        self.rows.sync(
            forges.into_iter().map(|row| (row.program.clone(), row)),
            now,
            reduce,
        );
        let drawn: Vec<_> = self
            .rows
            .rows()
            .map(|row| (row.key.to_owned(), row.placement, row.value.clone()))
            .collect();
        let mut list = ListCard::new("forges-list");
        for (key, placement, row) in drawn {
            let mut item = ListRow::new(
                ElementId::Name(format!("sign-in-row-{key}").into()),
                SharedString::from(row.title.clone()),
            )
            .tag(SharedString::from(row.program.clone()));
            if let Some(version) = &row.version {
                item = item.tag(SharedString::from(version.clone()));
            }
            if let Some(host) = &row.host {
                item = item.tag(SharedString::from(host.clone()));
            }
            let controls = self.controls(&key, true, cx);
            list = list.item(
                ElementId::Name(format!("sign-in-item-{key}").into()),
                placement,
                item.below(controls),
            );
        }
        let known = self.listed.is_some();
        let count = self
            .listed
            .iter()
            .flatten()
            .filter(|row| !row.agent && row.signed_in == Some(true))
            .count();
        let check = Button::secondary("forges-check", "Check again")
            .on_click(cx.listener(|this, _, _, cx| this.read(cx)));
        let section = Section::new("forges", "Forges and trackers")
            .when(known, |section| section.count(count.to_string()))
            .sentence(SENTENCE)
            .action(check)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(problem_line(
                        "forges-problem",
                        self.problem.clone(),
                        space::MD,
                        cx,
                    ))
                    .child(spaced(
                        space::MD,
                        awaiting(
                            "forges-waiting",
                            !known,
                            Skeleton::new("forges-skeleton").h(px(96.)),
                            presence("forges-list-block", !self.rows.is_empty(), list),
                        ),
                    )),
            );
        div()
            .text_color(p.text)
            .child(section)
            .child(self.confirm(cx))
    }
}

/// Whether the dialog opens `address`: one of the web only, since the handler it is given to
/// would open a file or run a scheme's program as readily.
fn opens(address: &str) -> bool {
    address.starts_with("https://") || address.starts_with("http://")
}

#[cfg(test)]
mod tests {
    use super::{SignInRow, opens, row_of};
    use serde_json::json;

    #[test]
    fn a_check_is_found_by_its_program_whatever_the_order_of_the_answer() {
        // An older daemon answers every program, not the one asked.
        let answer = json!({"programs": [
            {"program": "gh", "found": true, "signed_in": true},
            {"program": "glab", "found": true, "signed_in": false},
        ]});

        let row = row_of(&answer, "glab").expect("glab's row");

        assert_eq!(row.program, "glab");
        assert_eq!(row.signed_in, Some(false));
        assert!(row_of(&answer, "bkt").is_none());
    }

    #[test]
    fn a_request_the_window_could_not_make_does_not_blame_the_program() {
        let mut row = SignInRow::of(&json!({"program": "gh", "found": true, "checking": true}));

        row.failed("the daemon did not answer".to_owned());

        let said = row.said();
        assert!(!said.contains("gh failed"), "{said}");
        assert!(said.starts_with("Could not check"), "{said}");
        assert_eq!(row.reason(), Some("the daemon did not answer"));
    }

    /// #1185 round 2: the dialog opens an address of the web only, whatever a daemon answered.
    #[test]
    fn the_dialog_opens_only_an_address_of_the_web() {
        assert!(opens("https://github.com/login/device"));
        assert!(opens("http://127.0.0.1:8080/callback"));
        for refused in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "ssh://git.example.com",
            "/home/someone/.bashrc",
            "",
        ] {
            assert!(!opens(refused), "{refused}");
        }
    }
}
