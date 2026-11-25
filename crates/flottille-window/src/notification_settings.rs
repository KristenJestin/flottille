//! What rings, and when, chosen in the window (`docs/design.md` §10 and §11, #1003): the
//! Notifications tab of Settings (what each kind does, its sound, the volume, the quiet hours),
//! the Notifications tab of a project (its own level for each kind) and the Do not disturb switch
//! of the bell's panel. The window writes nothing itself: a choice is the daemon's
//! (`notifications.set_settings`), and what is drawn is what the daemon says next
//! (`notifications.settings`), so that a change made anywhere, the command line included, is seen
//! at once (`notifications.settings_changed`).
//!
//! One view holds what the daemon said and sends the choices; the sections are drawn by functions
//! of it, so that each enters on its own in its page (`docs/reference/gpui.md`, "Motion").

use std::collections::BTreeMap;

use flottille_core::notification_settings::{DAYS, KINDS, label, may_be_off, next_morning};
use gpui_design_system::{
    CheckState, Checkbox, Field, Section, Segmented, Select, SelectOption, Skeleton, Switch, Toast,
    awaiting, changing, palette, presence, space, toast,
};
use gpui_kit::{
    App, Context, ElementId, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement as _, Styled, TestSupportExt as _, WeakEntity, div, px,
};
use serde_json::{Value, json};

use crate::actions::Command;
use crate::commands::{ShellOf, perform_later};

/// What the Kinds section says of the three things a kind can do.
pub const KINDS_SENTENCE: &str = "Ring: a notification of the desktop, or a toast when the \
    window is focused, with a sound. Centre only: kept in the bell, nothing more. Off: not even \
    there. A request, a question or a vault ask cannot be off.";

/// What the Sound section says it does.
pub const SOUND_HINT: &str =
    "A short sound when something rings. Quiet hours and Do not disturb mute it.";

/// What the Quiet hours say they do.
pub const QUIET_HINT: &str = "Inside the range nothing rings: everything goes to the bell, and \
    the first ring after it says how many waited.";

/// The volumes the select offers; the daemon's own, when it is another, is added.
pub const VOLUMES: [u64; 4] = [25, 50, 75, 100];

/// What a project's section says of its levels.
pub const PROJECT_SENTENCE: &str = "What each kind does in this project. A kind follows the \
    levels of Settings unless it is set here.";

/// How long Do not disturb lasts when it is turned on from the bell: the choices of its select.
pub const HOW_LONG: [(&str, &str); 3] = [
    ("until-off", "Until I turn it off"),
    ("one-hour", "For 1 hour"),
    ("tomorrow", "Until tomorrow"),
];

const HOUR_MS: u64 = 3_600_000;

/// What `notifications.settings` says, as the window draws it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shown {
    /// What each kind does by default: `ring`, `centre` or `off`.
    pub kinds: BTreeMap<String, String>,
    pub sound_on: bool,
    pub volume: u64,
    /// Whether each kind rings with its sound.
    pub sound_kinds: BTreeMap<String, bool>,
    pub quiet_on: bool,
    pub quiet_from: String,
    pub quiet_to: String,
    pub quiet_days: Vec<String>,
    /// Whether Do not disturb holds now, and when it ends by itself.
    pub dnd_on: bool,
    pub dnd_until: Option<u64>,
    /// What each project sets itself, by project and kind.
    pub projects: BTreeMap<String, BTreeMap<String, String>>,
    /// Whether nothing rings now: Do not disturb, or the quiet hours.
    pub muted: bool,
}

impl Shown {
    pub fn of(value: &Value) -> Shown {
        let words = |value: &Value| -> BTreeMap<String, String> {
            value
                .as_object()
                .into_iter()
                .flatten()
                .filter_map(|(kind, word)| Some((kind.clone(), word.as_str()?.to_owned())))
                .collect()
        };
        let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
        Shown {
            kinds: words(&value["kinds"]),
            sound_on: value["sound"]["on"] == true,
            volume: value["sound"]["volume"].as_u64().unwrap_or(100),
            sound_kinds: value["sound"]["kinds"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(kind, on)| (kind.clone(), *on != false))
                .collect(),
            quiet_on: value["quiet"]["on"] == true,
            quiet_from: text(&value["quiet"]["from"]),
            quiet_to: text(&value["quiet"]["to"]),
            quiet_days: value["quiet"]["days"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|day| day.as_str().map(str::to_owned))
                .collect(),
            dnd_on: value["do_not_disturb"]["on"] == true,
            dnd_until: value["do_not_disturb"]["until"].as_u64(),
            projects: value["projects"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(project, own)| (project.clone(), words(own)))
                .collect(),
            muted: value["muted"] == true,
        }
    }

    /// What a kind does by default, `centre` for a kind the daemon did not say.
    pub fn level(&self, kind: &str) -> &str {
        self.kinds.get(kind).map_or("centre", String::as_str)
    }

    /// What a project sets for a kind itself, if it does.
    pub fn own(&self, project: &str, kind: &str) -> Option<&str> {
        self.projects.get(project)?.get(kind).map(String::as_str)
    }

    /// Whether a kind rings with its sound.
    pub fn sounds(&self, kind: &str) -> bool {
        self.sound_kinds.get(kind).copied().unwrap_or(true)
    }

    /// What the bell's panel says under Do not disturb.
    pub fn dnd_line(&self, at: impl Fn(u64) -> (u32, u32)) -> String {
        match (self.dnd_on, self.dnd_until) {
            (true, Some(until)) => {
                let (hours, minutes) = at(until);
                format!("Until {hours:02}:{minutes:02}")
            }
            (true, None) => "Until you turn it off".to_owned(),
            (false, _) => "Nothing rings while it is on.".to_owned(),
        }
    }
}

/// The words of a level.
pub fn words_of(level: &str) -> &'static str {
    match level {
        "ring" => "Ring",
        "centre" => "Centre only",
        _ => "Off",
    }
}

/// The options of a kind's level: Off is not one of what waits for the user.
fn levels_of(kind: &str) -> Vec<(&'static str, &'static str)> {
    let mut levels = vec![("ring", "Ring"), ("centre", "Centre only")];
    if may_be_off(kind) {
        levels.push(("off", "Off"));
    }
    levels
}

/// `HH:MM` at every half hour, and `current` when it is another.
pub fn times(current: &str) -> Vec<String> {
    let mut times: Vec<String> = (0..48)
        .map(|half| format!("{:02}:{:02}", half / 2, half % 2 * 30))
        .collect();
    if !current.is_empty() && !times.iter().any(|time| time == current) {
        times.push(current.to_owned());
        times.sort();
    }
    times
}

/// The volumes to choose from, and the daemon's when it is another.
pub fn volumes(current: u64) -> Vec<u64> {
    let mut volumes = VOLUMES.to_vec();
    if !volumes.contains(&current) {
        volumes.push(current);
        volumes.sort_unstable();
    }
    volumes
}

/// What Do not disturb is asked to be when the bell turns it on, for the choice `how` of
/// [`HOW_LONG`], at `now`.
pub fn dnd_patch(on: bool, how: usize, now: u64) -> Value {
    match (on, how) {
        (false, _) => json!({"do_not_disturb": {"on": false}}),
        (true, 1) => json!({"do_not_disturb": {"on": true, "until": now + HOUR_MS}}),
        (true, 2) => json!({"do_not_disturb": {"on": true, "until": next_morning(now)}}),
        (true, _) => json!({"do_not_disturb": {"on": true}}),
    }
}

/// The view that holds what the daemon said and sends the choices: the Settings tab's, or one
/// project's.
pub struct NotificationSettings {
    shell: ShellOf,
    /// The project whose own levels it sets; every project's defaults when none.
    project: Option<String>,
    shown: Option<Shown>,
    /// Why the daemon could not say them.
    problem: Option<SharedString>,
}

impl NotificationSettings {
    pub fn new(shell: ShellOf, project: Option<String>) -> Self {
        Self {
            shell,
            project,
            shown: None,
            problem: None,
        }
    }

    pub fn project(&self) -> Option<&str> {
        self.project.as_deref()
    }

    pub fn shown(&self) -> Option<&Shown> {
        self.shown.as_ref()
    }

    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    /// Asks the daemon.
    pub fn read(&self, cx: &mut Context<Self>) {
        perform_later(&self.shell, Command::NotificationSettingsRead, cx);
    }

    /// The daemon's answer to a read or a choice: what it keeps now. A choice it refused is a
    /// toast, and the controls stay as the daemon last said them.
    pub fn answered(&mut self, said: Result<Shown, String>, written: bool, cx: &mut Context<Self>) {
        match said {
            Ok(shown) => {
                self.shown = Some(shown);
                self.problem = None;
            }
            Err(why) if written => toast(
                Toast::failure("The notification settings were not changed").detail(why),
                cx,
            ),
            Err(why) if self.shown.is_none() => self.problem = Some(why.into()),
            Err(_) => {}
        }
        cx.notify();
    }

    fn write(&self, params: Value, cx: &mut Context<Self>) {
        perform_later(&self.shell, Command::NotificationSettingsSet { params }, cx);
    }

    /// What a kind does, here: the default's, or this project's own (`default` follows the
    /// default again).
    pub fn set_kind(&mut self, kind: &str, word: &str, cx: &mut Context<Self>) {
        let params = match &self.project {
            Some(project) => json!({"project": project, "kinds": {kind: word}}),
            None => json!({"kinds": {kind: word}}),
        };
        self.write(params, cx);
    }

    pub fn set_sound_on(&mut self, on: bool, cx: &mut Context<Self>) {
        self.write(json!({"sound": {"on": on}}), cx);
    }

    pub fn set_volume(&mut self, volume: u64, cx: &mut Context<Self>) {
        self.write(json!({"sound": {"volume": volume}}), cx);
    }

    pub fn set_sound_kind(&mut self, kind: &str, on: bool, cx: &mut Context<Self>) {
        self.write(json!({"sound": {"kinds": {kind: on}}}), cx);
    }

    pub fn set_quiet_on(&mut self, on: bool, cx: &mut Context<Self>) {
        self.write(json!({"quiet": {"on": on}}), cx);
    }

    /// The start (`from`) or the end of the range.
    pub fn set_quiet_time(&mut self, from: bool, time: &str, cx: &mut Context<Self>) {
        let key = if from { "from" } else { "to" };
        self.write(json!({"quiet": {key: time}}), cx);
    }

    /// A day the range starts on, added or taken off.
    pub fn set_quiet_day(&mut self, day: &str, on: bool, cx: &mut Context<Self>) {
        let Some(shown) = &self.shown else {
            return;
        };
        let days: Vec<&str> = DAYS
            .iter()
            .copied()
            .filter(|each| match *each == day {
                true => on,
                false => shown.quiet_days.iter().any(|chosen| chosen == each),
            })
            .collect();
        self.write(json!({"quiet": {"days": days}}), cx);
    }
}

fn id(part: impl AsRef<str>) -> ElementId {
    ElementId::Name(format!("notify-{}", part.as_ref()).into())
}

/// A card of the page's look.
fn card(name: &str, cx: &App) -> impl ParentElement + IntoElement {
    let p = palette(cx);
    div()
        .id(id(name))
        .p(space::LG)
        .flex()
        .flex_col()
        .gap(space::MD)
        .rounded(px(12.))
        .bg(p.card)
        .border_1()
        .border_color(p.line)
}

/// The sentence a section says in place of its controls while the daemon has not answered, or
/// cannot.
fn waiting(
    section: &'static str,
    view: &NotificationSettings,
    content: impl IntoElement,
    cx: &App,
) -> gpui_kit::AnyElement {
    let p = palette(cx);
    match (&view.shown, &view.problem) {
        (None, Some(why)) => div()
            .id(id(format!("{section}-problem")))
            .aria_label(why.clone())
            .text_size(px(12.5))
            .text_color(p.fail)
            .child(why.clone())
            .test_support()
            .into_any_element(),
        (None, None) => awaiting(
            id(format!("{section}-waiting")),
            true,
            Skeleton::new(id(format!("{section}-skeleton"))).h(px(96.)),
            div(),
        )
        .into_any_element(),
        (Some(_), _) => content.into_any_element(),
    }
}

/// What each kind does, and whether it rings with its sound: one row a kind.
pub fn kinds_section(
    view: &NotificationSettings,
    this: WeakEntity<NotificationSettings>,
    cx: &App,
) -> Section {
    let mut rows = div().flex().flex_col().gap(space::LG);
    if let Some(shown) = &view.shown {
        for kind in KINDS {
            let levels = levels_of(kind);
            let chosen = levels
                .iter()
                .position(|(key, _)| *key == shown.level(kind))
                .unwrap_or(1);
            let (on_level, on_sound) = (this.clone(), this.clone());
            let keys: Vec<&'static str> = levels.iter().map(|(key, _)| *key).collect();
            let level = Segmented::new(id(format!("kind-{kind}")), label(kind), levels, chosen)
                .on_choose(move |index, _, cx| {
                    let (kind, word) = (*kind, keys[index]);
                    on_level
                        .update(cx, |this, cx| this.set_kind(kind, word, cx))
                        .ok();
                });
            let sound = Switch::new(
                id(format!("kind-{kind}-sound")),
                shown.sounds(kind),
                "Sound",
            )
            .disabled(!shown.sound_on)
            .on_click(move |_, _, cx| {
                on_sound
                    .update(cx, |this, cx| {
                        let on = this.shown().is_none_or(|shown| !shown.sounds(kind));
                        this.set_sound_kind(kind, on, cx)
                    })
                    .ok();
            });
            rows = rows.child(Field::new(
                id(format!("kind-{kind}-field")),
                label(kind),
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(space::LG)
                    .child(level)
                    .child(sound),
            ));
        }
    }
    Section::new("notifications-kinds", "What rings")
        .sentence(KINDS_SENTENCE)
        .child(waiting("kinds", view, card("kinds", cx).child(rows), cx))
}

/// The sound for every kind: on or off, and how loud.
pub fn sound_section(
    view: &NotificationSettings,
    this: WeakEntity<NotificationSettings>,
    cx: &App,
) -> Section {
    let mut body = card("sound", cx);
    if let Some(shown) = &view.shown {
        let (on_switch, on_volume) = (this.clone(), this);
        let volumes = volumes(shown.volume);
        let options = volumes.iter().map(|volume| {
            SelectOption::new(
                SharedString::from(volume.to_string()),
                SharedString::from(format!("{volume} %")),
            )
        });
        let chosen = volumes
            .iter()
            .position(|volume| *volume == shown.volume)
            .unwrap_or(0);
        let picked = volumes.clone();
        let select = Select::new(id("volume"), options, chosen)
            .disabled(!shown.sound_on)
            .on_choose(move |index, _, cx| {
                let volume = picked[index];
                on_volume
                    .update(cx, |this, cx| this.set_volume(volume, cx))
                    .ok();
            });
        body = body
            .child(
                Switch::new(id("sound-switch"), shown.sound_on, "Sound")
                    .hint(SOUND_HINT)
                    .on_click(move |_, _, cx| {
                        on_switch
                            .update(cx, |this, cx| {
                                let on = this.shown().is_none_or(|shown| !shown.sound_on);
                                this.set_sound_on(on, cx)
                            })
                            .ok();
                    }),
            )
            .child(Field::new(id("volume-field"), "Volume", select));
    }
    Section::new("notifications-sound", "Sound").child(waiting("sound", view, body, cx))
}

/// The quiet hours: on or off, the range and the days it starts on. The range and the days
/// unfold under the switch while it is on.
pub fn quiet_section(
    view: &NotificationSettings,
    this: WeakEntity<NotificationSettings>,
    cx: &App,
) -> Section {
    let mut body = card("quiet", cx);
    if let Some(shown) = &view.shown {
        let on_switch = this.clone();
        let time = |from: bool| {
            let current = if from {
                &shown.quiet_from
            } else {
                &shown.quiet_to
            };
            let all = times(current);
            let options = all.iter().map(|time| {
                SelectOption::new(
                    SharedString::from(time.clone()),
                    SharedString::from(time.clone()),
                )
            });
            let chosen = all.iter().position(|time| time == current).unwrap_or(0);
            let this = this.clone();
            Select::new(
                id(if from { "quiet-from" } else { "quiet-to" }),
                options,
                chosen,
            )
            .on_choose(move |index, _, cx| {
                let time = all[index].clone();
                this.update(cx, |this, cx| this.set_quiet_time(from, &time, cx))
                    .ok();
            })
        };
        let days = DAYS
            .iter()
            .fold(div().flex().flex_wrap().gap(space::LG), |row, day| {
                let on = shown.quiet_days.iter().any(|chosen| chosen == day);
                let this = this.clone();
                let day = *day;
                row.child(
                    Checkbox::new(
                        id(format!("quiet-day-{day}")),
                        if on {
                            CheckState::Checked
                        } else {
                            CheckState::Empty
                        },
                        day_label(day),
                    )
                    .labelled()
                    .on_change(move |state, _, cx| {
                        this.update(cx, |this, cx| {
                            this.set_quiet_day(day, state == CheckState::Checked, cx)
                        })
                        .ok();
                    }),
                )
            });
        let range = div()
            .flex()
            .flex_col()
            .gap(space::MD)
            .child(
                div()
                    .flex()
                    .gap(space::LG)
                    .child(div().flex_1().child(Field::new(
                        id("quiet-from-field"),
                        "From",
                        time(true),
                    )))
                    .child(div().flex_1().child(Field::new(
                        id("quiet-to-field"),
                        "To",
                        time(false),
                    ))),
            )
            .child(Field::new(id("quiet-days-field"), "Days", days));
        body = body
            .child(
                Switch::new(id("quiet-switch"), shown.quiet_on, "Quiet hours")
                    .hint(QUIET_HINT)
                    .on_click(move |_, _, cx| {
                        on_switch
                            .update(cx, |this, cx| {
                                let on = this.shown().is_none_or(|shown| !shown.quiet_on);
                                this.set_quiet_on(on, cx)
                            })
                            .ok();
                    }),
            )
            .child(presence(id("quiet-range-block"), shown.quiet_on, range).space(space::MD));
    }
    Section::new("notifications-quiet", "Quiet hours").child(waiting("quiet", view, body, cx))
}

/// A project's own level for each kind: a select a kind, "As Settings" first.
pub fn project_section(
    view: &NotificationSettings,
    this: WeakEntity<NotificationSettings>,
    cx: &App,
) -> Section {
    let mut rows = div().flex().flex_col().gap(space::LG);
    if let (Some(shown), Some(project)) = (&view.shown, view.project()) {
        for kind in KINDS {
            let levels = levels_of(kind);
            let follows = format!(
                "As Settings: {}",
                words_of(shown.level(kind)).to_lowercase()
            );
            let mut options = vec![SelectOption::new("default", follows)];
            options.extend(
                levels
                    .iter()
                    .map(|(key, words)| SelectOption::new(*key, *words)),
            );
            let chosen = shown
                .own(project, kind)
                .and_then(|own| levels.iter().position(|(key, _)| *key == own))
                .map_or(0, |at| at + 1);
            let keys: Vec<&'static str> = std::iter::once("default")
                .chain(levels.iter().map(|(key, _)| *key))
                .collect();
            let this = this.clone();
            let select = Select::new(id(format!("project-{kind}")), options, chosen).on_choose(
                move |index, _, cx| {
                    let (kind, word) = (*kind, keys[index]);
                    this.update(cx, |this, cx| this.set_kind(kind, word, cx))
                        .ok();
                },
            );
            rows = rows.child(Field::new(
                id(format!("project-{kind}-field")),
                label(kind),
                select,
            ));
        }
    }
    Section::new("notifications-project", "Notifications")
        .sentence(PROJECT_SENTENCE)
        .child(waiting(
            "project",
            view,
            card("project", cx).child(rows),
            cx,
        ))
}

/// A day as the page says it.
pub fn day_label(day: &str) -> &'static str {
    match day {
        "mon" => "Mon",
        "tue" => "Tue",
        "wed" => "Wed",
        "thu" => "Thu",
        "fri" => "Fri",
        "sat" => "Sat",
        _ => "Sun",
    }
}

/// The Do not disturb of the bell's panel: its switch, what it says under it, and, while it is
/// off, how long it lasts when it is turned on.
pub struct DoNotDisturb;

impl DoNotDisturb {
    /// `how` is the choice of [`HOW_LONG`]; `at` says the hour and minute of a moment on the
    /// user's clock. `on_switch` is called with the state a click asks for, `on_how` with the
    /// choice picked.
    pub fn block(
        cx: &App,
        shown: Option<&Shown>,
        how: usize,
        at: impl Fn(u64) -> (u32, u32),
        on_switch: impl Fn(bool, &mut App) + 'static,
        on_how: impl Fn(usize, &mut App) + 'static,
    ) -> impl IntoElement {
        let on = shown.is_some_and(|shown| shown.dnd_on);
        let line = shown.map_or_else(String::new, |shown| shown.dnd_line(at));
        let p = palette(cx);
        let switch = Switch::new("bell-dnd-switch", on, "Do not disturb")
            .disabled(shown.is_none())
            .on_click(move |_, _, cx| on_switch(!on, cx));
        let options = HOW_LONG
            .iter()
            .map(|(key, words)| SelectOption::new(*key, *words));
        let how = Select::new("bell-dnd-how", options, how)
            .compact()
            .on_choose(move |index, _, cx| on_how(index, cx));
        div()
            .flex_none()
            .px(space::SM)
            .pt(space::SM)
            .flex()
            .flex_col()
            .child(switch)
            // What it says under the switch fades when it changes (a time, then none).
            .child(changing("bell-dnd-line-change", line, move |line| {
                div()
                    .id("bell-dnd-line")
                    .aria_label(line.clone())
                    .pl(space::XXXL - space::XS)
                    .text_size(px(12.))
                    .text_color(p.mute)
                    .child(line)
                    .test_support()
                    .into_any_element()
            }))
            .child(
                presence(
                    "bell-dnd-how-block",
                    !on && shown.is_some(),
                    Field::new("bell-dnd-how-field", "For", how),
                )
                .space(space::SM),
            )
    }
}
