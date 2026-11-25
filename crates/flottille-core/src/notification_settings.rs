//! What rings, and when (`docs/design.md` §10, §11): the user's settings for the notification
//! centre's kinds, by project, the sound, the quiet hours and Do not disturb, as
//! `notifications.toml` of the configuration folder keeps them.
//!
//! A notification is kept in the centre, rings there and on the desktop, or is not kept at all,
//! by its kind and its project; the daemon asks `Settings::level` for each row, and `muted`
//! whether anything may ring now. Nothing here reads a file or the clock but `local_time`: the
//! daemon does, so that a test chooses what it reads.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::notification::NEEDS_YOU;

/// The kinds the user chooses for, in the order a client lists them. A row's own kind maps to one
/// of them (`kind_of`).
pub const KINDS: &[&str] = &[
    "request",
    "question",
    "report",
    "failure",
    "pull_request",
    "ticket",
    "paused",
    "vault",
    "daemon",
    "routine",
];

/// The days, Monday first: the order of `Quiet::days`.
pub const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

/// What a kind does when it comes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// Kept in the centre, and rings: the desktop's notification or the window's toast, and a
    /// sound.
    Ring,
    /// Kept in the centre, nothing more.
    Centre,
    /// Not kept at all.
    Off,
}

impl Level {
    pub fn word(self) -> &'static str {
        match self {
            Level::Ring => "ring",
            Level::Centre => "centre",
            Level::Off => "off",
        }
    }

    pub fn from_word(word: &str) -> Option<Level> {
        match word {
            "ring" => Some(Level::Ring),
            "centre" => Some(Level::Centre),
            "off" => Some(Level::Off),
            _ => None,
        }
    }
}

/// What a client calls a kind: a request, a pause, what the daemon says of itself.
pub fn label(kind: &str) -> &'static str {
    match kind {
        "request" => "Request",
        "question" => "Question",
        "report" => "Report",
        "failure" => "Failure",
        "pull_request" => "Pull request",
        "ticket" => "Ticket",
        "paused" => "Pause",
        "vault" => "Vault ask",
        "daemon" => "Daemon",
        "routine" => "Routine",
        _ => "Other",
    }
}

/// The kind of settings a row of the centre is of: what the daemon says of itself is one kind.
/// A kind no setting names (an extension's) is its own and has the default.
pub fn kind_of(row_kind: &str) -> &str {
    match row_kind {
        "unchecked" | "put_back" | "dropped" => "daemon",
        other => other,
    }
}

/// What a kind does until the user chooses: what needs them or tells them of a result rings, the
/// rest is kept in the centre.
pub fn default_level(kind: &str) -> Level {
    match kind {
        "request" | "question" | "report" | "failure" => Level::Ring,
        _ => Level::Centre,
    }
}

/// Whether a kind may be Off: not what waits for the user, whose ringing alone can be lowered.
pub fn may_be_off(kind: &str) -> bool {
    !NEEDS_YOU.contains(&kind)
}

/// The sound, globally and by kind.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SoundSettings {
    pub on: bool,
    /// From 0 to 100.
    pub volume: u8,
    /// The kinds that ring without a sound (`false`); the others follow `on`.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub kinds: BTreeMap<String, bool>,
}

impl Default for SoundSettings {
    fn default() -> Self {
        SoundSettings {
            on: true,
            volume: 100,
            kinds: BTreeMap::new(),
        }
    }
}

/// A daily range in which nothing rings, on the days it starts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Quiet {
    pub on: bool,
    /// `HH:MM`, on the user's clock.
    pub from: String,
    pub to: String,
    /// The days the range starts on, in `DAYS` order: Friday's 22:00–08:00 ends on Saturday
    /// morning.
    pub days: Vec<String>,
}

impl Default for Quiet {
    fn default() -> Self {
        Quiet {
            on: false,
            from: "22:00".into(),
            to: "08:00".into(),
            days: DAYS.iter().map(|day| (*day).to_owned()).collect(),
        }
    }
}

/// Do not disturb: until the user turns it off, or until a moment.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DoNotDisturb {
    pub on: bool,
    /// When it ends by itself, in milliseconds since the epoch; none for until turned off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<u64>,
}

impl DoNotDisturb {
    /// Whether it holds at `now`: a time that is over is no longer on, nobody has to turn it off.
    pub fn holds(&self, now: u64) -> bool {
        self.on && self.until.is_none_or(|until| now < until)
    }
}

/// A moment on the user's clock: the day of the week, Monday as 0, and the minute of the day.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalTime {
    pub day: usize,
    pub minute: u32,
}

/// `at` (milliseconds since the epoch) on the machine's clock, in its time zone.
pub fn local_time(at: u64) -> LocalTime {
    let seconds = (at / 1000) as libc::time_t;
    // SAFETY: `localtime_r` writes only into `tm`, which outlives the call; a zeroed `tm` is a
    // valid value to be written over.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&seconds, &mut tm) }.is_null() {
        return LocalTime { day: 0, minute: 0 };
    }
    LocalTime {
        // `tm_wday` counts from Sunday.
        day: (tm.tm_wday as usize + 6) % 7,
        minute: (tm.tm_hour * 60 + tm.tm_min) as u32,
    }
}

/// When "until tomorrow" ends, the morning the user's day begins: the next 08:00 on their clock.
pub fn next_morning(now: u64) -> u64 {
    const MORNING: u32 = 8 * 60;
    const DAY: u32 = 24 * 60;
    let minute = local_time(now).minute;
    let wait = match (MORNING + DAY - minute) % DAY {
        0 => DAY,
        wait => wait,
    };
    // From the start of the minute: seconds are not part of the clock a user reads.
    now - now % 60_000 + u64::from(wait) * 60_000
}

/// `HH:MM` as the minute of the day.
pub fn minute_of(time: &str) -> Option<u32> {
    let (hours, minutes) = time.split_once(':')?;
    let two = |part: &str| {
        (part.len() == 2 && part.bytes().all(|b| b.is_ascii_digit()))
            .then(|| part.parse::<u32>().ok())
            .flatten()
    };
    let (hours, minutes) = (two(hours)?, two(minutes)?);
    (hours < 24 && minutes < 60).then_some(hours * 60 + minutes)
}

impl Quiet {
    /// Whether the range holds at `at`.
    pub fn holds(&self, at: LocalTime) -> bool {
        let (Some(from), Some(to)) = (minute_of(&self.from), minute_of(&self.to)) else {
            return false;
        };
        let starts_on = |day: usize| self.days.iter().any(|d| d == DAYS[day % 7]);
        if !self.on {
            false
        } else if from < to {
            starts_on(at.day) && (from..to).contains(&at.minute)
        } else {
            // Across midnight: the evening belongs to the day it started on, the morning to the
            // day before.
            (at.minute >= from && starts_on(at.day)) || (at.minute < to && starts_on(at.day + 6))
        }
    }
}

/// What `notifications.toml` says: only what the user chose, the rest is the defaults.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// The levels the user chose, by kind; a kind not here has its default.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub kinds: BTreeMap<String, Level>,
    #[serde(skip_serializing_if = "is_default")]
    pub sound: SoundSettings,
    #[serde(skip_serializing_if = "is_default")]
    pub quiet: Quiet,
    #[serde(skip_serializing_if = "is_default")]
    pub do_not_disturb: DoNotDisturb,
    /// Each project's own levels: what is there wins over `kinds`.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub projects: BTreeMap<String, ProjectLevels>,
}

/// What a project sets itself, `[projects.<name>.kinds]` (`docs/projects.md`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectLevels {
    pub kinds: BTreeMap<String, Level>,
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

const FILE: &str = "notifications.toml";

impl Settings {
    /// Reads the text of `notifications.toml` and checks it.
    pub fn parse(text: &str) -> Result<Settings, String> {
        let settings: Settings =
            toml::from_str(text).map_err(|error| format!("{FILE}: {}", error.message()))?;
        settings.check()?;
        Ok(settings)
    }

    /// The text of the file: what differs from the defaults.
    pub fn to_toml(&self) -> Result<String, String> {
        toml::to_string_pretty(self).map_err(|error| format!("{FILE}: {error}"))
    }

    /// Whether the settings can be kept: each kind known, a kind that waits for the user never
    /// Off, a volume from 0 to 100, a range that is a range, days that are days.
    pub fn check(&self) -> Result<(), String> {
        let levels = |what: &str, levels: &BTreeMap<String, Level>| {
            for (kind, level) in levels {
                if !KINDS.contains(&kind.as_str()) {
                    return Err(format!("{FILE}: {what}{kind}: not a kind of notification"));
                }
                if *level == Level::Off && !may_be_off(kind) {
                    return Err(format!(
                        "{FILE}: {what}{kind}: a {} needs you, so it cannot be off; only its \
                         ringing can be lowered to centre",
                        kind.replace('_', " ")
                    ));
                }
            }
            Ok(())
        };
        levels("kinds.", &self.kinds)?;
        for (project, own) in &self.projects {
            levels(&format!("projects.{project}.kinds."), &own.kinds)?;
        }
        if self.sound.volume > 100 {
            return Err(format!("{FILE}: sound.volume: from 0 to 100"));
        }
        if let Some(kind) = self
            .sound
            .kinds
            .keys()
            .find(|kind| !KINDS.contains(&kind.as_str()))
        {
            return Err(format!(
                "{FILE}: sound.kinds.{kind}: not a kind of notification"
            ));
        }
        let (from, to) = (minute_of(&self.quiet.from), minute_of(&self.quiet.to));
        match (from, to) {
            (Some(from), Some(to)) if from == to => {
                return Err(format!("{FILE}: quiet: from and to are the same time"));
            }
            (Some(_), Some(_)) => {}
            _ => {
                return Err(format!(
                    "{FILE}: quiet: times are HH:MM, from 00:00 to 23:59"
                ));
            }
        }
        if self.quiet.days.is_empty()
            || self
                .quiet
                .days
                .iter()
                .any(|day| !DAYS.contains(&day.as_str()))
        {
            return Err(format!(
                "{FILE}: quiet.days: one or more of {}",
                DAYS.join(", ")
            ));
        }
        Ok(())
    }

    /// What a notification of `row_kind` in `project` does.
    pub fn level(&self, row_kind: &str, project: Option<&str>) -> Level {
        let kind = kind_of(row_kind);
        let own = project
            .and_then(|project| self.projects.get(project))
            .and_then(|own| own.kinds.get(kind));
        let level = own
            .or_else(|| self.kinds.get(kind))
            .copied()
            .unwrap_or_else(|| default_level(kind));
        // A file edited by hand is checked when it is read; this is what holds if it was not.
        match level {
            Level::Off if !may_be_off(kind) => Level::Centre,
            level => level,
        }
    }

    /// Whether a notification of `row_kind` rings with a sound.
    pub fn sounds(&self, row_kind: &str) -> bool {
        self.sound.on
            && self
                .sound
                .kinds
                .get(kind_of(row_kind))
                .copied()
                .unwrap_or(true)
    }

    /// The volume sounds play at, from 0 to 1.
    pub fn volume(&self) -> f32 {
        f32::from(self.sound.volume) / 100.0
    }

    /// Whether nothing may ring now: Do not disturb, or inside the quiet hours.
    pub fn muted(&self, now: u64, local: LocalTime) -> bool {
        self.do_not_disturb.holds(now) || self.quiet.holds(local)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_user_did_not_choose_is_the_default() {
        let settings = Settings::default();
        for kind in ["request", "question", "report", "failure"] {
            assert_eq!(settings.level(kind, None), Level::Ring, "{kind}");
        }
        for kind in ["pull_request", "ticket", "paused", "vault", "routine", "x"] {
            assert_eq!(settings.level(kind, None), Level::Centre, "{kind}");
        }
        for row in ["unchecked", "put_back", "dropped"] {
            assert_eq!(settings.level(row, None), Level::Centre, "{row}");
        }
        assert!(settings.sounds("report"));
        assert_eq!(settings.volume(), 1.0);
        assert_eq!(settings.to_toml().unwrap(), "");
    }

    #[test]
    fn a_project_s_own_level_wins_over_the_default_and_the_users() {
        let settings = Settings::parse(
            "[kinds]\nreport = \"centre\"\n\n[projects.app.kinds]\nreport = \"ring\"\nticket = \"off\"\n",
        )
        .unwrap();
        assert_eq!(settings.level("report", Some("app")), Level::Ring);
        assert_eq!(settings.level("report", Some("web")), Level::Centre);
        assert_eq!(settings.level("report", None), Level::Centre);
        assert_eq!(settings.level("ticket", Some("app")), Level::Off);
        assert_eq!(settings.level("ticket", Some("web")), Level::Centre);
    }

    #[test]
    fn what_waits_for_the_user_cannot_be_off_but_can_be_centre_only() {
        for kind in ["request", "question", "vault"] {
            let refused = Settings::parse(&format!("[kinds]\n{kind} = \"off\"\n")).unwrap_err();
            assert!(refused.contains("needs you"), "{refused}");
            let refused =
                Settings::parse(&format!("[projects.app.kinds]\n{kind} = \"off\"\n")).unwrap_err();
            assert!(refused.contains("needs you"), "{refused}");
        }
        let settings = Settings::parse("[kinds]\nrequest = \"centre\"\n").unwrap();
        assert_eq!(settings.level("request", None), Level::Centre);
        // Not checked, and still never off.
        let mut loose = Settings::default();
        loose.kinds.insert("request".into(), Level::Off);
        assert_eq!(loose.level("request", None), Level::Centre);
    }

    #[test]
    fn a_text_that_is_not_what_the_file_says_is_refused_with_its_key() {
        for (text, said) in [
            ("[kinds]\nnonsense = \"ring\"\n", "kinds.nonsense"),
            ("[kinds]\nreport = \"loud\"\n", "loud"),
            ("[sound]\nvolume = 101\n", "sound.volume"),
            ("[sound.kinds]\nnonsense = false\n", "sound.kinds.nonsense"),
            ("[quiet]\nfrom = \"25:00\"\n", "quiet"),
            ("[quiet]\nfrom = \"9:00\"\n", "quiet"),
            ("[quiet]\nfrom = \"10:00\"\nto = \"10:00\"\n", "same time"),
            ("[quiet]\ndays = [\"someday\"]\n", "quiet.days"),
            ("[quiet]\ndays = []\n", "quiet.days"),
            ("other = 1\n", "other"),
        ] {
            let refused = Settings::parse(text).unwrap_err();
            assert!(refused.starts_with("notifications.toml: "), "{refused}");
            assert!(refused.contains(said), "{text}: {refused}");
        }
    }

    #[test]
    fn the_file_keeps_what_differs_from_the_defaults_and_reads_back() {
        let mut settings = Settings::default();
        settings.kinds.insert("report".into(), Level::Centre);
        settings.sound.volume = 40;
        settings.sound.kinds.insert("report".into(), false);
        settings.quiet.on = true;
        settings.quiet.days = vec!["mon".into(), "fri".into()];
        settings.do_not_disturb = DoNotDisturb {
            on: true,
            until: Some(5),
        };
        settings.projects.insert(
            "app".into(),
            ProjectLevels {
                kinds: BTreeMap::from([("ticket".into(), Level::Ring)]),
            },
        );

        let text = settings.to_toml().unwrap();

        assert_eq!(Settings::parse(&text).unwrap(), settings, "{text}");
        assert!(!text.contains("ticket = \"centre\""), "{text}");
    }

    #[test]
    fn a_sound_is_off_by_kind_or_for_all() {
        let mut settings = Settings::default();
        settings.sound.kinds.insert("report".into(), false);
        assert!(!settings.sounds("report"));
        assert!(settings.sounds("request"));
        settings.sound.on = false;
        assert!(!settings.sounds("request"));
        settings.sound.on = true;
        settings.sound.volume = 25;
        assert_eq!(settings.volume(), 0.25);
    }

    fn quiet(from: &str, to: &str, days: &[&str]) -> Quiet {
        Quiet {
            on: true,
            from: from.into(),
            to: to.into(),
            days: days.iter().map(|day| (*day).to_owned()).collect(),
        }
    }

    fn at(day: usize, hours: u32, minutes: u32) -> LocalTime {
        LocalTime {
            day,
            minute: hours * 60 + minutes,
        }
    }

    #[test]
    fn a_range_across_midnight_holds_the_evening_and_the_morning_after() {
        let nights = quiet("22:00", "08:00", &["fri"]);
        assert!(nights.holds(at(4, 23, 30)), "Friday 23:30");
        assert!(nights.holds(at(4, 22, 0)), "Friday 22:00");
        assert!(nights.holds(at(5, 7, 59)), "Saturday 07:59");
        assert!(!nights.holds(at(5, 8, 0)), "Saturday 08:00");
        assert!(!nights.holds(at(4, 21, 59)), "Friday 21:59");
        assert!(
            !nights.holds(at(4, 7, 0)),
            "Friday morning belongs to Thursday"
        );
        assert!(!nights.holds(at(5, 23, 0)), "Saturday night is not chosen");
        // Sunday night into Monday morning wraps the week.
        assert!(quiet("22:00", "08:00", &["sun"]).holds(at(0, 7, 0)));
    }

    #[test]
    fn a_range_inside_a_day_holds_on_its_days_only() {
        let lunch = quiet("12:00", "14:00", &["mon", "tue"]);
        assert!(lunch.holds(at(0, 12, 0)));
        assert!(lunch.holds(at(1, 13, 59)));
        assert!(!lunch.holds(at(1, 14, 0)));
        assert!(!lunch.holds(at(2, 13, 0)));
        let mut off = lunch;
        off.on = false;
        assert!(!off.holds(at(0, 13, 0)));
    }

    #[test]
    fn do_not_disturb_holds_until_its_time_is_over() {
        let until = DoNotDisturb {
            on: true,
            until: Some(1000),
        };
        assert!(until.holds(999));
        assert!(!until.holds(1000));
        let forever = DoNotDisturb {
            on: true,
            until: None,
        };
        assert!(forever.holds(u64::MAX - 1));
        assert!(!DoNotDisturb::default().holds(0));
    }

    #[test]
    fn a_time_of_the_day_is_two_digits_a_colon_and_two_digits() {
        assert_eq!(minute_of("00:00"), Some(0));
        assert_eq!(minute_of("23:59"), Some(1439));
        for bad in ["24:00", "12:60", "9:00", "12:5", "ab:cd", "1200", ""] {
            assert_eq!(minute_of(bad), None, "{bad}");
        }
    }

    #[test]
    fn tomorrow_ends_at_the_next_morning_on_the_clock_within_a_day() {
        let now = 1_700_000_123_456;
        let end = next_morning(now);
        assert!(end > now && end - now <= 24 * 3_600_000, "{now} {end}");
        assert_eq!(local_time(end).minute, 8 * 60);
    }

    #[test]
    fn the_local_time_is_a_day_and_a_minute() {
        let now = local_time(1_700_000_000_000);
        assert!(now.day < 7 && now.minute < 1440, "{now:?}");
    }
}
