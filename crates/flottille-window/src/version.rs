//! The Version section of Settings, General (`docs/releases.md`, "Updating"): this build's
//! version, channel and commit, the latest build of the channel as the daemon last checked it,
//! whether the daemon checks on its own, and the actions: Check now, Download, Restart to update.
//! The window decides nothing: each action is a request of the daemon's (`update.*`), and the
//! section shows what the daemon says next (`update.show`, `update.changed`).
//!
//! What moves: the version line and the state line fade when their text changes (`changing`),
//! a download's bar (the kit's `Progress`, its fill easing to each reading over `PANEL`) unfolds
//! under the update line with the amount received and Cancel, fills when the download is done,
//! then folds away (`presence`; the amount moves in place, at most once a second, and does not
//! fade), the message of a failure or a refusal unfolds and folds away (`presence_of`), the
//! What's new link's ink fades over `HOVER` (`hoverable`), the buttons are the kit's (their
//! hover and press fade), the check's choice and the channel's (Stable or Beta, acting at once)
//! are the kit's `Segmented`, which slides its highlight; a channel the command line chose (an
//! alpha) is a line with two buttons that fades into the choice when one is taken (`changing`),
//! the sentence of a build newer than its channel fades in the state line like any state, and
//! the restart is asked in the one confirmation dialog (`Confirm`).

use gpui_design_system::color::mix;
use gpui_design_system::motion::PANEL;
use gpui_design_system::{
    Button, Confirm, Progress, Section, Segmented, Skeleton, Toast, awaiting, changing, hoverable,
    palette, presence, presence_of, space, toast,
};
use gpui_kit::{
    AnyElement, Context, ElementId, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement as _, Styled, TestSupportExt as _, WeakEntity, Window,
    div, px,
};
use serde_json::{Value, json};

use crate::actions::Command;
use crate::shell::Shell;

/// What the section says once a build is staged.
pub const READY: &str = "Ready: applied at the next start";

const DOWNLOADING: &str = "Downloading…";

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
    /// The branch of an alpha channel.
    pub branch: Option<String>,
    /// Whether the daemon checks on its own (`updates.toml`'s `check`).
    pub automatically: bool,
    /// The channel's latest build, when it is newer than this one.
    pub latest: Option<String>,
    /// The channel's latest build, when this build is newer than it (a beta kept above the
    /// latest stable): it stays until a newer one is out.
    pub ahead_of: Option<String>,
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
    /// The sessions a restart stops, as the daemon said them: each `(id, title)`.
    pub stops: Vec<(String, Option<String>)>,
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
            branch: text("branch"),
            automatically: value["check"] != "never",
            latest: text("latest"),
            ahead_of: text("ahead_of"),
            failed: text("failed"),
            checking: value["checking"] == true,
            staged: text("staged"),
            downloading: value["downloading"]
                .as_object()
                .map(|at| (at["done"].as_u64().unwrap_or(0), at["total"].as_u64())),
            download_failed: text("download_failed"),
            when_idle: value["restart"] == "idle",
            refused: text("refused"),
            stops: value["stops"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|session| {
                    let id = session["id"].as_str()?.to_owned();
                    Some((id, session["title"].as_str().map(str::to_owned)))
                })
                .collect(),
        }
    }

    /// This build as the window says it: its version, channel and commit.
    pub fn build_line(&self) -> String {
        match &self.commit {
            Some(commit) => format!("{} · {} · {commit}", self.version, self.build_channel),
            None => format!("{} · {}", self.version, self.build_channel),
        }
    }

    /// The line of the update's state: a download is said once here, its amount and its bar
    /// are beside it.
    pub fn state_line(&self) -> String {
        if self.when_idle {
            return "Restarts to update once no agent runs".to_owned();
        }
        if self.staged.is_some() {
            return READY.to_owned();
        }
        if self.downloading.is_some() {
            return DOWNLOADING.to_owned();
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
        if let Some(ahead) = &self.ahead_of {
            return format!(
                "This build is newer than {} {ahead}: it stays until a newer one is out",
                self.channel
            );
        }
        "Up to date".to_owned()
    }

    /// The channel as the window says it: an alpha with its branch.
    pub fn channel_line(&self) -> String {
        match (self.channel.as_str(), &self.branch) {
            ("alpha", Some(branch)) => format!("alpha · {branch}"),
            _ => self.channel.clone(),
        }
    }

    /// Which of the two choices of the window the channel is: none for an alpha, an rc or a next,
    /// which the command line chose.
    pub fn choice(&self) -> Option<usize> {
        match self.channel.as_str() {
            "stable" => Some(0),
            "beta" => Some(1),
            _ => None,
        }
    }

    /// The amount a download has read, beside its bar: `69.7 MB of 93.1 MB`, or the bytes alone
    /// when the server does not say the size.
    pub fn amount_line(&self) -> Option<String> {
        let (done, total) = self.downloading?;
        Some(match total {
            Some(total) if total > 0 => format!("{} of {}", megabytes(done), megabytes(total)),
            _ => megabytes(done),
        })
    }

    /// The share of the download received, 0 to 1, when the server said the size.
    pub fn share(&self) -> Option<f32> {
        let (done, total) = self.downloading?;
        let total = total.filter(|total| *total > 0)?;
        Some((done as f64 / total as f64).min(1.0) as f32)
    }

    /// What the sidebar's foot says: only something to act on, and never the amount, which
    /// would change its line every second.
    pub fn foot_line(&self) -> Option<String> {
        if self.staged.is_some() && !self.when_idle {
            return Some("Ready to restart".to_owned());
        }
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
    /// Restart to update was clicked: the daemon's answer to `update.show` says which sessions
    /// it stops, read then rather than from the last event (an agent starting sends none).
    restarting: bool,
    /// The sessions the restart stops, while its confirmation is open (kept while it fades out).
    stopping: Vec<String>,
    confirming: bool,
    /// The version last run, once the daemon says something is new since it: the section then
    /// offers its page.
    whats_new: Option<String>,
    /// The bar of the download last shown, kept while it folds away.
    download: Option<Download>,
    /// How many downloads the section has seen: each has a bar of its own, which starts empty.
    downloads: u32,
    /// A download is done: its bar is full and about to fold away.
    finishing: bool,
}

/// What a download's bar says.
struct Download {
    n: u32,
    done: u64,
    total: Option<u64>,
}

impl VersionSection {
    pub fn new(shell: WeakEntity<Shell>) -> Self {
        Self {
            shell,
            shown: None,
            asking: false,
            restarting: false,
            stopping: Vec::new(),
            confirming: false,
            whats_new: None,
            download: None,
            downloads: 0,
            finishing: false,
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
        if self.shown.as_ref() == Some(&shown) {
            return;
        }
        let was = self
            .shown
            .as_ref()
            .is_some_and(|was| was.downloading.is_some());
        match shown.downloading {
            Some((done, total)) => {
                if !was {
                    self.downloads += 1;
                }
                self.download = Some(Download {
                    n: self.downloads,
                    done,
                    total,
                });
                self.finishing = false;
            }
            // Done: the bar fills before it folds away (at once with reduced motion, which
            // has no fill to wait for).
            None if was && shown.staged.is_some() && !cx.reduce_motion() => {
                if let Some(download) = &mut self.download
                    && let Some(total) = download.total
                {
                    download.done = total;
                    self.finishing = true;
                    cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(PANEL).await;
                        this.update(cx, |this, cx| {
                            this.finishing = false;
                            cx.notify();
                        })
                        .ok();
                    })
                    .detach();
                }
            }
            None => {}
        }
        self.shown = Some(shown);
        cx.notify();
    }

    /// What the daemon says is new since `since`, the version last run; none when nothing is.
    pub fn set_whats_new(&mut self, since: Option<String>, cx: &mut Context<Self>) {
        if self.whats_new != since {
            self.whats_new = since;
            cx.notify();
        }
    }

    /// The page of what is new since the version last run.
    fn open_whats_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.shell
            .update(cx, |shell, cx| {
                shell.open(crate::shell::Selection::WhatsNew, window, cx)
            })
            .ok();
    }

    fn ask(&mut self, method: &'static str, params: Value, cx: &mut Context<Self>) {
        if self.asking {
            return;
        }
        self.asking = true;
        // The version is this machine's own, in every machine's Settings: its daemon is asked,
        // whichever is in front, now or once a confirmation is answered (#1271).
        self.shell
            .update(cx, |shell, _| {
                shell.perform_on(None, Command::Update { method, params })
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
        if method == "update.show" && std::mem::take(&mut self.restarting) {
            if let Ok(shown) = &answer {
                self.set_shown(Shown::of(shown), cx);
                // The shell is being updated while it hands the answer: read it after.
                let this = cx.entity().downgrade();
                cx.defer(move |cx| {
                    this.update(cx, |this, cx| this.ask_restart(cx)).ok();
                });
            }
            cx.notify();
            return;
        }
        match answer {
            Ok(shown) => self.set_shown(Shown::of(&shown), cx),
            Err(why) if method != "update.show" => {
                let what = match method {
                    "update.download" => "The update was not downloaded",
                    "update.cancel" => "The download was not cancelled",
                    "update.restart" => "Flottille did not restart",
                    "update.set" => "The update settings were not changed",
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

    pub fn cancel_download(&mut self, cx: &mut Context<Self>) {
        self.ask("update.cancel", json!({}), cx);
    }

    /// Chooses the channel, stable or beta: the daemon remembers it and checks it at once.
    pub fn set_channel(&mut self, channel: &'static str, cx: &mut Context<Self>) {
        self.ask("update.set", json!({"channel": channel}), cx);
    }

    pub fn set_automatically(&mut self, automatically: bool, cx: &mut Context<Self>) {
        let check = if automatically {
            "automatically"
        } else {
            "never"
        };
        self.ask("update.set", json!({"check": check}), cx);
        // Chosen here, it is this machine's own, which its Settings mark.
        self.shell
            .update(cx, |shell, _| {
                shell.perform_on(None, Command::AllSettingsRead)
            })
            .ok();
    }

    /// Restart to update: the daemon says which sessions a restart stops, then it restarts at
    /// once when none, else the confirmation lists them and offers to wait until no agent runs.
    pub fn restart(&mut self, cx: &mut Context<Self>) {
        if self.asking {
            return;
        }
        self.restarting = true;
        self.ask("update.show", json!({}), cx);
    }

    fn ask_restart(&mut self, cx: &mut Context<Self>) {
        let stops = self
            .shown
            .as_ref()
            .map(|shown| shown.stops.clone())
            .unwrap_or_default();
        if stops.is_empty() {
            self.ask("update.restart", json!({"when": "now"}), cx);
            return;
        }
        // The window's title of each, as its sidebar says it, else the daemon's.
        let titles = self
            .shell
            .upgrade()
            .map(|shell| {
                let shell = shell.read(cx);
                let sessions = &shell.snapshot().sessions;
                stops
                    .iter()
                    .map(|(id, title)| {
                        sessions
                            .iter()
                            .find(|session| &session.id == id)
                            .map(|session| session.title.clone())
                            .or_else(|| title.clone())
                            .unwrap_or_else(|| id.clone())
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.stopping = titles;
        self.confirming = true;
        cx.notify();
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
        Self::line_in(palette(cx), id, label, value)
    }

    fn line_in(
        p: gpui_design_system::Palette,
        id: &'static str,
        label: &'static str,
        value: AnyElement,
    ) -> AnyElement {
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

impl VersionSection {
    /// The value of the Channel row: Stable or Beta as a choice that acts at once, as Appearance
    /// and Check for updates do; a channel the command line chose (an alpha with its branch)
    /// is said, with a way back to either. The swap between the two fades.
    fn channel_choice(&self, shown: &Shown, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity().downgrade();
        let busy = self.asking;
        match shown.choice() {
            Some(chosen) => changing("version-channel-value", "choice", move |_| {
                Segmented::new(
                    "version-channel",
                    "Update channel",
                    [("stable", "Stable"), ("beta", "Beta")],
                    chosen,
                )
                .disabled(busy)
                .on_choose(move |index, _, cx| {
                    let channel = if index == 0 { "stable" } else { "beta" };
                    this.update(cx, |this, cx| this.set_channel(channel, cx))
                        .ok();
                })
                .into_any_element()
            })
            .into_any_element(),
            None => {
                let line = SharedString::from(shown.channel_line());
                let (stable, beta) = (this.clone(), this);
                changing("version-channel-value", line.clone(), move |_| {
                    div()
                        .flex()
                        .items_center()
                        .gap(space::MD)
                        .child(
                            div()
                                .id("version-channel-other")
                                .aria_label(line.clone())
                                .child(line)
                                .test_support(),
                        )
                        .child(
                            Button::secondary("version-channel-stable", "Use Stable")
                                .disabled(busy)
                                .on_click(move |_, _, cx| {
                                    stable
                                        .update(cx, |this, cx| this.set_channel("stable", cx))
                                        .ok();
                                }),
                        )
                        .child(
                            Button::secondary("version-channel-beta", "Use Beta")
                                .disabled(busy)
                                .on_click(move |_, _, cx| {
                                    beta.update(cx, |this, cx| this.set_channel("beta", cx))
                                        .ok();
                                }),
                        )
                        .into_any_element()
                })
                .into_any_element()
            }
        }
    }

    /// The row of a download: its bar, the amount received in the quiet ink, and Cancel, in the
    /// column of the other values. Drawn from the download last shown, so that it keeps its
    /// content while it folds away.
    fn download_row(&self, p: gpui_design_system::Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some(Download { n, done, total }) = &self.download else {
            return div().into_any_element();
        };
        let id = ElementId::Name(format!("version-bar-{n}").into());
        let bar = match total.filter(|total| *total > 0) {
            Some(total) => Progress::new(id, (*done as f64 / total as f64) as f32),
            None => Progress::indeterminate(id),
        };
        let amount = SharedString::from(match total {
            Some(total) if *total > 0 => format!("{} of {}", megabytes(*done), megabytes(*total)),
            _ => megabytes(*done),
        });
        div()
            .id("version-download")
            .flex()
            .gap(space::MD)
            .child(div().w(px(96.)).flex_none())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(space::MD)
                    .child(div().flex_1().min_w_0().child(bar))
                    .child(
                        div()
                            .id("version-download-amount")
                            .aria_label(amount.clone())
                            .flex_none()
                            .text_size(px(12.5))
                            .text_color(p.mute)
                            .child(amount)
                            .test_support(),
                    )
                    .child(
                        Button::quiet("version-download-cancel", "Cancel")
                            .disabled(self.asking || self.finishing)
                            .on_click(cx.listener(|this, _, _, cx| this.cancel_download(cx))),
                    ),
            )
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
        let this_build = changing("version-build", shown.build_line(), |text| {
            div()
                .id("version-build-text")
                .aria_label(text.clone())
                .child(text)
                .test_support()
                .into_any_element()
        })
        .into_any_element();
        // The state fades when it changes.
        let line = SharedString::from(shown.state_line());
        let state = changing("version-state", shown.state_line(), move |_| {
            div()
                .id("version-state-text")
                .aria_label(line.clone())
                .child(line)
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
        let channel = self.channel_choice(&shown, cx);
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
                "version-channel-line",
                "Channel",
                channel,
                cx,
            )))
            .child(
                div()
                    .pt(space::SM)
                    .child(Self::line("version-latest", "Update", state, cx)),
            )
            .child(
                presence(
                    "version-download-line",
                    shown.downloading.is_some() || self.finishing,
                    self.download_row(p, cx),
                )
                .space(space::SM),
            )
            .child(
                presence_of(
                    "version-whats-new-line",
                    self.whats_new
                        .clone()
                        .map(|since| SharedString::from(format!("Since {since}"))),
                    {
                        let this = cx.entity().downgrade();
                        move |since| {
                            // A quiet link: it reads as text, its ink fades to the accent.
                            let link = div()
                                .id("version-whats-new-open")
                                .aria_label(since.clone())
                                .cursor_pointer()
                                .child(since)
                                .on_click(move |_, window, cx| {
                                    this.update(cx, |this, cx| this.open_whats_new(window, cx))
                                        .ok();
                                });
                            let link =
                                hoverable(link, move |d, t| d.text_color(mix(p.mute, p.acc, t)));
                            Self::line_in(
                                p,
                                "version-whats-new",
                                "What's new",
                                link.into_any_element(),
                            )
                        }
                    },
                )
                .space(space::SM),
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
