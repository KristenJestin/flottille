//! The Version section of Settings, General (`docs/releases.md`, "Updating"): this build's
//! version, channel and commit, the latest build of the channel as the daemon last checked it,
//! whether the daemon checks on its own, and the actions: Check now, Download, Restart to update.
//! The window decides nothing: each action is a request of the daemon's (`update.*`), and the
//! section shows what the daemon says next (`update.show`, `update.changed`).
//!
//! What moves: the version line and the state line fade when their text changes (`changing`),
//! the message of a download, a failure or a refusal unfolds and folds away (`presence_of`), the
//! buttons are the kit's (their hover and press fade), the check's choice is the kit's
//! `Segmented`, and the restart is asked in the one confirmation dialog (`Confirm`).

use gpui_design_system::{
    Button, Confirm, Section, Segmented, Skeleton, Toast, awaiting, changing, palette, presence_of,
    space, toast,
};
use gpui_kit::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement as _, Styled, TestSupportExt as _, WeakEntity, Window, div, px,
};
use serde_json::{Value, json};

use crate::actions::Command;
use crate::shell::Shell;
use crate::state::AgentState;

/// What the section says once a build is staged.
pub const READY: &str = "Ready: applied at the next start";

/// What `update.show` says.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shown {
    /// The daemon's version alone, `0.4.0-beta.124`.
    pub version: String,
    /// The daemon's `--version`, its channel and commit with it: what tells two builds apart.
    pub full: String,
    /// The channel the build was made on (`local` for a build CI did not make).
    pub build_channel: String,
    pub commit: Option<String>,
    /// The channel the daemon checks, the one `flottille update` would take.
    pub channel: String,
    /// Whether the daemon checks on its own (`updates.toml`'s `check`).
    pub automatically: bool,
    /// The channel's latest build, when it is newer than this one.
    pub latest: Option<String>,
    /// Why the last check failed.
    pub failed: Option<String>,
    pub checking: bool,
    /// The build downloaded, waiting for the next start.
    pub staged: Option<String>,
    /// The bytes downloaded, and of how many when known.
    pub downloading: Option<(u64, Option<u64>)>,
    pub download_failed: Option<String>,
    /// A restart is asked for once no agent runs.
    pub when_idle: bool,
    /// What the daemon's start refused of a staged build.
    pub refused: Option<String>,
}

impl Shown {
    pub fn of(value: &Value) -> Shown {
        let text = |key: &str| value[key].as_str().map(str::to_owned);
        Shown {
            version: text("version").unwrap_or_default(),
            full: text("full").unwrap_or_default(),
            build_channel: text("build_channel").unwrap_or_default(),
            commit: text("commit"),
            channel: text("channel").unwrap_or_default(),
            automatically: value["check"] != "never",
            latest: text("latest"),
            failed: text("failed"),
            checking: value["checking"] == true,
            staged: text("staged"),
            downloading: value["downloading"]
                .as_object()
                .map(|at| (at["done"].as_u64().unwrap_or(0), at["total"].as_u64())),
            download_failed: text("download_failed"),
            when_idle: value["restart"] == "idle",
            refused: text("refused"),
        }
    }

    /// The line of the update's state: what the sidebar's foot says too, when there is
    /// something to do.
    pub fn state_line(&self) -> String {
        if self.when_idle {
            return "Restarts to update once no agent runs".to_owned();
        }
        if self.staged.is_some() {
            return READY.to_owned();
        }
        if let Some((done, total)) = self.downloading {
            return match total {
                Some(total) if total > 0 => {
                    format!("Downloading… {} of {}", megabytes(done), megabytes(total))
                }
                _ => format!("Downloading… {}", megabytes(done)),
            };
        }
        if let Some(latest) = &self.latest {
            return format!("{latest} is available");
        }
        if self.checking {
            return "Checking…".to_owned();
        }
        if self.failed.is_some() {
            return "Could not check for an update".to_owned();
        }
        "Up to date".to_owned()
    }

    /// What the sidebar's foot says: only something to act on.
    pub fn foot_line(&self) -> Option<String> {
        (self.latest.is_some() || self.staged.is_some()).then(|| self.state_line())
    }
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

pub struct VersionSection {
    shell: WeakEntity<Shell>,
    shown: Option<Shown>,
    /// A request sent, until the daemon answers: its buttons wait.
    asking: bool,
    /// The sessions the restart stops, while its confirmation is open (kept while it fades out).
    stopping: Vec<String>,
    confirming: bool,
}

impl VersionSection {
    pub fn new(shell: WeakEntity<Shell>) -> Self {
        Self {
            shell,
            shown: None,
            asking: false,
            stopping: Vec::new(),
            confirming: false,
        }
    }

    pub fn shown(&self) -> Option<&Shown> {
        self.shown.as_ref()
    }

    /// Whether the restart's confirmation is open.
    pub fn is_confirming(&self) -> bool {
        self.confirming
    }

    /// The sessions the confirmation says stop.
    pub fn stopping(&self) -> &[String] {
        &self.stopping
    }

    pub fn set_shown(&mut self, shown: Shown, cx: &mut Context<Self>) {
        if self.shown.as_ref() != Some(&shown) {
            self.shown = Some(shown);
            cx.notify();
        }
    }

    fn ask(&mut self, method: &'static str, params: Value, cx: &mut Context<Self>) {
        if self.asking {
            return;
        }
        self.asking = true;
        self.shell
            .update(cx, |shell, _| {
                shell.perform(Command::Update { method, params })
            })
            .ok();
        cx.notify();
    }

    /// The daemon's answer to a request: what it shows, or a toast of why not.
    pub fn answered(
        &mut self,
        method: &'static str,
        answer: Result<Value, String>,
        cx: &mut Context<Self>,
    ) {
        self.asking = false;
        match answer {
            Ok(shown) => self.set_shown(Shown::of(&shown), cx),
            Err(why) if method != "update.show" => {
                let what = match method {
                    "update.download" => "The update was not downloaded",
                    "update.restart" => "Flottille did not restart",
                    "update.set" => "The check was not changed",
                    _ => "Flottille did not check for an update",
                };
                toast(Toast::failure(what).detail(why), cx);
            }
            Err(_) => {}
        }
        cx.notify();
    }

    pub fn check_now(&mut self, cx: &mut Context<Self>) {
        self.ask("update.check", json!({}), cx);
    }

    pub fn download(&mut self, cx: &mut Context<Self>) {
        self.ask("update.download", json!({}), cx);
    }

    pub fn set_automatically(&mut self, automatically: bool, cx: &mut Context<Self>) {
        let check = if automatically {
            "automatically"
        } else {
            "never"
        };
        self.ask("update.set", json!({"check": check}), cx);
    }

    /// Restart to update: at once when no agent runs, else the confirmation says which sessions
    /// stop and offers to wait until none runs.
    pub fn restart(&mut self, cx: &mut Context<Self>) {
        let running: Vec<String> = self
            .shell
            .upgrade()
            .map(|shell| {
                shell
                    .read(cx)
                    .snapshot()
                    .sessions
                    .iter()
                    .filter(|session| session.mark == Some(AgentState::Running))
                    .map(|session| session.title.clone())
                    .collect()
            })
            .unwrap_or_default();
        if running.is_empty() {
            self.ask("update.restart", json!({"when": "now"}), cx);
        } else {
            self.stopping = running;
            self.confirming = true;
            cx.notify();
        }
    }

    /// The confirmation's answer: now, stopping the sessions, or once no agent runs.
    pub fn confirm_restart(&mut self, idle: bool, cx: &mut Context<Self>) {
        self.confirming = false;
        let when = if idle { "idle" } else { "now" };
        self.ask("update.restart", json!({"when": when}), cx);
    }

    fn cancel_restart(&mut self, cx: &mut Context<Self>) {
        self.confirming = false;
        cx.notify();
    }

    fn confirm(&self, cx: &mut Context<Self>) -> Confirm {
        let p = palette(cx);
        let (now, idle, cancel) = (
            cx.entity().downgrade(),
            cx.entity().downgrade(),
            cx.entity().downgrade(),
        );
        let count = self.stopping.len();
        let sessions = div()
            .id("version-restart-sessions")
            .flex()
            .flex_col()
            .gap(space::XS)
            .children(self.stopping.iter().enumerate().map(|(n, title)| {
                div()
                    .id(("version-restart-session", n))
                    .aria_label(SharedString::from(title.clone()))
                    .text_size(px(13.))
                    .text_color(p.text)
                    .child(SharedString::from(title.clone()))
                    .test_support()
            }))
            .test_support();
        let wait = div().pt(space::SM).child(
            Button::secondary("version-restart-idle", "Restart when idle").on_click(
                move |_, _, cx| {
                    idle.update(cx, |this, cx| this.confirm_restart(true, cx))
                        .ok();
                },
            ),
        );
        let sentence = if count == 1 {
            "This session stops; it is resumed once Flottille is back, as after any restart."
        } else {
            "These sessions stop; they are resumed once Flottille is back, as after any restart."
        };
        Confirm::new("version-restart", "Restart Flottille to update?")
            .sentence(sentence)
            .action("Restart now")
            .open(self.confirming)
            .on_confirm(move |_, cx| {
                now.update(cx, |this, cx| this.confirm_restart(false, cx))
                    .ok();
            })
            .on_cancel(move |_, cx| {
                cancel.update(cx, |this, cx| this.cancel_restart(cx)).ok();
            })
            .child(sessions)
            .child(wait)
    }

    fn line(
        id: &'static str,
        label: &'static str,
        value: AnyElement,
        cx: &Context<Self>,
    ) -> AnyElement {
        let p = palette(cx);
        div()
            .id(id)
            .flex()
            .gap(space::MD)
            .text_size(px(13.))
            .child(div().w(px(96.)).flex_none().text_color(p.mute).child(label))
            .child(div().min_w_0().text_color(p.text).child(value))
            .test_support()
            .into_any_element()
    }
}

impl Render for VersionSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let skeleton = Skeleton::new("version-skeleton").h(px(160.));
        let Some(shown) = self.shown.clone() else {
            return Section::new("version", "Version")
                .child(awaiting("version-awaiting", true, skeleton, div()))
                .into_any_element();
        };
        let build = match &shown.commit {
            Some(commit) => format!("{} · {} · {commit}", shown.version, shown.build_channel),
            None => format!("{} · {}", shown.version, shown.build_channel),
        };
        let this_build = changing("version-build", build, |text| {
            div()
                .id("version-build-text")
                .aria_label(text.clone())
                .child(text)
                .test_support()
                .into_any_element()
        })
        .into_any_element();
        let state = changing("version-state", shown.state_line(), |text| {
            div()
                .id("version-state-text")
                .aria_label(text.clone())
                .child(text)
                .test_support()
                .into_any_element()
        })
        .into_any_element();
        // One message at a time, the most recent cause first.
        let message = shown
            .refused
            .clone()
            .or(shown.download_failed.clone())
            .or(shown.failed.clone())
            .map(SharedString::from);
        let busy = self.asking || shown.downloading.is_some();
        let mut actions = div().flex().gap(space::SM).pt(space::MD);
        if shown.staged.is_some() {
            actions = actions.child(
                Button::primary("version-restart-button", "Restart to update")
                    .disabled(busy || shown.when_idle)
                    .on_click(cx.listener(|this, _, _, cx| this.restart(cx))),
            );
        } else if shown.latest.is_some() {
            actions = actions.child(
                Button::primary("version-download", "Download")
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| this.download(cx))),
            );
        }
        actions = actions.child(
            Button::secondary("version-check-now", "Check now")
                .disabled(busy || shown.checking)
                .on_click(cx.listener(|this, _, _, cx| this.check_now(cx))),
        );
        let check = Segmented::new(
            "version-check",
            "Check for updates",
            [("automatically", "Automatically"), ("never", "Never")],
            if shown.automatically { 0 } else { 1 },
        )
        .disabled(self.asking)
        .on_choose({
            let this = cx.entity().downgrade();
            move |index, _, cx| {
                this.update(cx, |this, cx| this.set_automatically(index == 0, cx))
                    .ok();
            }
        });
        let card = div()
            .id("version-card")
            .p(space::LG)
            .flex()
            .flex_col()
            .rounded(px(12.))
            .bg(p.card)
            .border_1()
            .border_color(p.line)
            .child(Self::line("version-this", "This build", this_build, cx))
            .child(div().pt(space::SM).child(Self::line(
                "version-channel",
                "Channel",
                SharedString::from(shown.channel.clone()).into_any_element(),
                cx,
            )))
            .child(
                div()
                    .pt(space::SM)
                    .child(Self::line("version-latest", "Update", state, cx)),
            )
            .child(
                presence_of("version-message-line", message, move |why| {
                    div()
                        .id("version-message")
                        .aria_label(why.clone())
                        .text_size(px(12.5))
                        .text_color(p.fail)
                        .child(why)
                        .test_support()
                        .into_any_element()
                })
                .space(space::SM),
            )
            .child(div().pt(space::MD).child(check))
            .child(actions)
            .test_support();
        Section::new("version", "Version")
            .child(awaiting("version-awaiting", false, skeleton, card))
            .child(self.confirm(cx))
            .into_any_element()
    }
}
