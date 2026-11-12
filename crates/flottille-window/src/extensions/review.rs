//! Review in the window (`docs/extensions.md`, "Review"): `/review` in a session's composer, which
//! Flottille runs and never sends to the agent. A review is one row of the session's rail, folded
//! from its right over its checks and its findings (`session.rs`, `RailRow::Review`). Its page ("Its page") draws what it
//! checks as one card, a line per check with its switch and its model, each check's own
//! instructions folded under its line.

use std::rc::Rc;

use gpui_design_system::{Card, Disclosure, SetHere, Switch, Toast, enter, palette, space, toast};
use gpui_kit::{
    AnyElement, App, Entity, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, TestSupportExt as _, WeakEntity, Window, div, px,
};
use serde_json::{Value, json};

use super::{Listed, OnLine, Registry, Setting};
use crate::actions::Command;
use crate::session::state_of;
use crate::session_link::TreeAgent;
use crate::shell::Shell;
use crate::state::AgentState;

/// Its name in `extension.list`.
pub const NAME: &str = "review";

/// The composer's own command that starts a review of the session.
pub const COMMAND: &str = "review";

/// What `/review` says in the composer's list.
pub const COMMAND_LINE: &str =
    "Review this session's change with fresh eyes: the findings come back to the session";

/// What the card of the checks' models says once under it.
pub const MODELS_LINE: &str = "Models: the strongest of the session's provider unless you choose \
     one. Each check is a turn on your own quota.";

/// What the card says of the rounds, under that.
pub const ROUNDS_LINE: &str = "After fixes, one more review of what changed; two at most.";

/// The title of the shared guidance, and its sentence.
pub const GUIDANCE_TITLE: &str = "What every reviewer is told";
pub const GUIDANCE_SENTENCE: &str = "Added to Flottille's own instructions, which say they change \
     nothing. A check's own instructions come after it.";

/// The fold of a check's own instructions.
pub const INSTRUCTIONS: &str = "Instructions for this check";

/// The two switches of when a review starts besides the user's own doors, in their order; and what
/// the usage line adds for each one that is on.
const DOORS: [(&str, &str); 2] = [
    (
        "auto_on_pull_request",
        "It also starts when a session's pull request opens.",
    ),
    ("agents_start", "A session's agent may ask for one too."),
];

/// The key of the setting of the checks a review runs, and of the last line's model.
const CHECKS: &str = "passes";
const VERIFIER: &str = "verifier_model";

/// What sets the checks a review runs.
type SetChecks = Rc<dyn Fn(Vec<String>, &mut App)>;

/// What sets a switch of when a review starts: its value, or `None` for Settings' in a project.
type SetDoor = Rc<dyn Fn(Option<Value>, &mut App)>;

/// How many characters of a check's instructions its folded line shows.
const PREVIEW: usize = 60;

#[derive(Clone, Debug, PartialEq)]
pub enum Then {
    /// `review.start` of a session.
    Started,
}

/// `/review` in the composer of `session`: the daemon starts the review, or says why not.
pub fn start(shell: &WeakEntity<Shell>, session: &str, cx: &mut App) {
    let command = Command::Ask {
        method: "review.start".into(),
        params: json!({"agent": session}),
        then: super::Then::Review(Then::Started),
    };
    shell.update(cx, |shell, _| shell.perform(command)).ok();
}

/// What the daemon answered: a toast either way.
pub fn answered(_: &mut Registry, then: Then, answer: Result<Value, String>, cx: &mut App) {
    match (then, answer) {
        (Then::Started, Ok(started)) => toast(
            Toast::success(format!(
                "Review started, round {} of 2",
                started["round"].as_u64().unwrap_or(1)
            ))
            .detail("Its findings come back to the session."),
            cx,
        ),
        (Then::Started, Err(why)) => toast(Toast::failure("No review started").detail(why), cx),
    }
}

/// The words of a list of checks.
fn words(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// `checks` with `check` in it as `on` says, in the declaration's order.
fn with(options: &[String], checks: &[String], check: &str, on: bool) -> Vec<String> {
    options
        .iter()
        .filter(|option| {
            if option.as_str() == check {
                on
            } else {
                checks.contains(option)
            }
        })
        .cloned()
        .collect()
}

/// The name a check goes by on the page: its model's label (`Project rules` for `standards`).
fn named(listed: &Listed, check: &str) -> String {
    listed
        .settings
        .iter()
        .find(|setting| setting.key == format!("{check}_model"))
        .map_or_else(
            || super::capitalized(check),
            |setting| setting.label.clone(),
        )
}

/// Whether the switch `key` is on in `project`, or in Settings: the project's own value, else
/// Settings'.
fn door_on(listed: &Listed, project: Option<&str>, key: &str) -> bool {
    let Some(setting) = listed.settings.iter().find(|setting| setting.key == key) else {
        return false;
    };
    project
        .and_then(|project| listed.in_project(project))
        .and_then(|side| side.settings.get(key))
        .and_then(Value::as_bool)
        .unwrap_or_else(|| setting.value.as_bool().unwrap_or(false))
}

/// The usage line of Review's page, in Settings or in `project`: what the daemon says, and a
/// sentence for each switch that is on there.
pub fn usage(listed: &Listed, project: Option<&str>) -> String {
    let mut said = listed.usage.clone();
    for (key, adds) in DOORS {
        if door_on(listed, project, key) {
            said.push(' ');
            said.push_str(adds);
        }
    }
    said
}

/// What each project that sets its own checks does otherwise (`runs Security too`,
/// `skips Bugs`), by project.
pub fn otherwise(listed: &Listed) -> Vec<(String, String)> {
    let Some(setting) = listed.settings.iter().find(|s| s.key == CHECKS) else {
        return Vec::new();
    };
    let global = words(&setting.value);
    listed
        .projects
        .iter()
        .filter_map(|side| {
            let own = words(side.settings.get(CHECKS).filter(|own| !own.is_null())?);
            let mut said = Vec::new();
            let added: Vec<String> = setting
                .options
                .iter()
                .filter(|check| own.contains(check) && !global.contains(check))
                .map(|check| named(listed, check))
                .collect();
            let left: Vec<String> = setting
                .options
                .iter()
                .filter(|check| !own.contains(check) && global.contains(check))
                .map(|check| named(listed, check))
                .collect();
            if !added.is_empty() {
                said.push(format!("runs {} too", added.join(" and ")));
            }
            if !left.is_empty() {
                said.push(format!("skips {}", left.join(" and ")));
            }
            (!said.is_empty()).then(|| (side.project.clone(), said.join(" and ")))
        })
        .collect()
}

/// What Review checks, as one card (`docs/extensions.md`, "Its page"): a line per check, its
/// switch, its name, its line and its model on the right; the last line, checking the findings,
/// with no switch; under the card, the default and the cost said once, then the rounds. In
/// Settings, each check's own instructions fold under its line; in `project`, each switch and
/// model is the project's, Settings' value when it sets none, and one it sets carries the dot.
pub fn checks(
    registry: &Entity<Registry>,
    listed: &Listed,
    project: Option<&str>,
    prefix: &str,
    cx: &App,
) -> AnyElement {
    let p = palette(cx);
    let read = registry.read(cx);
    let providers = read
        .shell
        .upgrade()
        .map(|shell| shell.read(cx).providers().clone())
        .unwrap_or_default();
    let side = project.and_then(|project| listed.in_project(project));
    let checks = listed.settings.iter().find(|s| s.key == CHECKS);
    let options = checks.map(|s| s.options.clone()).unwrap_or_default();
    let global = checks.map(|s| words(&s.value)).unwrap_or_default();
    let own = side
        .and_then(|side| side.settings.get(CHECKS))
        .filter(|own| !own.is_null())
        .map(words);
    let chosen = own.clone().unwrap_or_else(|| global.clone());
    // Sets the checks a review runs: in a project, Settings' value is no value of its own.
    let set_checks: SetChecks = {
        let (registry, name, project, setting) = (
            registry.downgrade(),
            listed.name.clone(),
            project.map(str::to_owned),
            checks.cloned(),
        );
        let global = global.clone();
        Rc::new(move |list: Vec<String>, cx: &mut App| {
            let Some(setting) = &setting else { return };
            let value = match project {
                Some(_) if list == global => None,
                _ => Some(json!(list)),
            };
            registry
                .update(cx, |registry, cx| {
                    registry.set(&name, project.as_deref(), setting, value, cx)
                })
                .ok();
        })
    };
    // The model of a line: its own value, and Settings' as the one it inherits.
    let model = |setting: &Setting, on_line: Option<&OnLine>| {
        let global = setting.value.clone();
        let own = match side {
            None => setting.set.then(|| global.clone()),
            Some(side) => side.settings.get(&setting.key).cloned(),
        };
        super::worded_control(
            registry,
            &listed.name,
            project,
            setting,
            own,
            global,
            &format!("{prefix}-setting-{}", setting.key),
            &providers,
            on_line,
        )
    };
    let doors = doors(registry, listed, project, prefix);
    let mut lines: Vec<AnyElement> = Vec::new();
    for check in &options {
        let Some(setting) = listed
            .settings
            .iter()
            .find(|setting| setting.key == format!("{check}_model"))
        else {
            continue;
        };
        let on = chosen.contains(check);
        let id = format!("{prefix}-check-{check}");
        let switch = Switch::new(
            SharedString::from(format!("{id}-switch")),
            on,
            setting.label.clone(),
        )
        .hint(setting.hint.clone())
        .on_click({
            let (set_checks, options, chosen, check) = (
                set_checks.clone(),
                options.clone(),
                chosen.clone(),
                check.clone(),
            );
            move |_, _, cx| set_checks(with(&options, &chosen, &check, !on), cx)
        });
        let switch = match project {
            None => switch.into_any_element(),
            // A check the project runs otherwise than Settings carries the dot, and goes back.
            Some(_) => {
                let differs = own.is_some() && on != global.contains(check);
                let back = {
                    let (set_checks, options, chosen, check) = (
                        set_checks.clone(),
                        options.clone(),
                        chosen.clone(),
                        check.clone(),
                    );
                    let inherited = global.contains(&check);
                    move |_: &mut Window, cx: &mut App| {
                        set_checks(with(&options, &chosen, &check, inherited), cx)
                    }
                };
                SetHere::new(
                    SharedString::from(id.clone()),
                    switch,
                    "set here",
                    "Use Settings' value",
                )
                .set(differs)
                .on_reset(back)
                .into_any_element()
            }
        };
        // The switch names the model; it fades with the switch, and is out of reach while off.
        let on_line = OnLine {
            name: format!("{} model", setting.label),
            disabled: !on,
        };
        let select = model(setting, Some(&on_line));
        let line = div()
            .flex()
            .items_start()
            .gap(space::MD)
            .child(div().flex_1().min_w_0().child(switch))
            .children(select);
        // Its own instructions, Settings' alone: a quiet fold under the line.
        let instructions = match project {
            Some(_) => None,
            None => read
                .check_guidance(&listed.name, check)
                .cloned()
                .map(|editor| {
                    let open = read.is_unfolded(&listed.name, check);
                    let preview = editor.read(cx).shown().map(|shown| shown.text.clone());
                    let (registry, name, part) =
                        (registry.downgrade(), listed.name.clone(), check.clone());
                    Disclosure::new(
                        SharedString::from(format!("{id}-instructions")),
                        INSTRUCTIONS,
                        open,
                    )
                    .summary(preview_of(preview.as_deref()))
                    .body(editor)
                    .on_toggle(move |_, _, cx| {
                        registry
                            .update(cx, |registry, cx| registry.toggle_part(&name, &part, cx))
                            .ok();
                    })
                }),
        };
        lines.push(
            div()
                .id(SharedString::from(id))
                .flex()
                .flex_col()
                .gap(space::XS)
                .child(line)
                .children(instructions)
                .test_support()
                .into_any_element(),
        );
    }
    // The last line checks the findings: it always runs, so it has no switch.
    if let Some(setting) = listed.settings.iter().find(|s| s.key == VERIFIER)
        && let Some(select) = model(setting, None)
    {
        lines.push(
            div()
                .id(SharedString::from(format!("{prefix}-check-verifier")))
                .child(select)
                .test_support()
                .into_any_element(),
        );
    }
    let id = format!("{prefix}-checks");
    let under = |key: &str, said: &'static str| {
        div()
            .id(SharedString::from(format!("{id}-{key}")))
            .aria_label(said)
            .text_size(px(12.))
            .text_color(p.faint)
            .child(said)
            .test_support()
    };
    let card =
        Card::new().child(
            div()
                .id(SharedString::from(id.clone()))
                .p(space::LG)
                .flex()
                .flex_col()
                .gap(space::MD)
                .children(lines.into_iter().enumerate().map(|(at, line)| {
                    enter(SharedString::from(format!("{id}-line-{at}")), at, line)
                }))
                .test_support(),
        );
    div()
        .flex()
        .flex_col()
        .gap(space::LG)
        .child(enter(
            SharedString::from(format!("{prefix}-doors-enter")),
            0,
            doors,
        ))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(space::SM)
                .child(card)
                .child(under("models", MODELS_LINE))
                .child(under("rounds", ROUNDS_LINE)),
        )
        .into_any_element()
}

/// The two switches of when a review starts besides the user's own doors, one card, a line each
/// with its label and its help line. In `project`, each is the project's, Settings' value when it
/// sets none, and one it sets carries the dot and goes back.
fn doors(
    registry: &Entity<Registry>,
    listed: &Listed,
    project: Option<&str>,
    prefix: &str,
) -> AnyElement {
    let lines: Vec<AnyElement> = DOORS
        .iter()
        .filter_map(|(key, _)| {
            let setting = listed.settings.iter().find(|setting| setting.key == *key)?;
            let global = setting.value.as_bool().unwrap_or(false);
            let own = project
                .and_then(|project| listed.in_project(project))
                .and_then(|side| side.settings.get(*key))
                .and_then(Value::as_bool);
            let on = own.unwrap_or(global);
            let id = format!("{prefix}-door-{key}");
            let set: SetDoor = {
                let (registry, name, project, setting) = (
                    registry.downgrade(),
                    listed.name.clone(),
                    project.map(str::to_owned),
                    setting.clone(),
                );
                Rc::new(move |value, cx| {
                    registry
                        .update(cx, |registry, cx| {
                            registry.set(&name, project.as_deref(), &setting, value, cx)
                        })
                        .ok();
                })
            };
            let switch = Switch::new(
                SharedString::from(format!("{id}-switch")),
                on,
                setting.label.clone(),
            )
            .hint(setting.hint.clone())
            .on_click({
                let (set, in_project) = (set.clone(), project.is_some());
                move |_, _, cx| {
                    // In a project, Settings' value is no value of its own.
                    let value = (!in_project || !on != global).then(|| json!(!on));
                    set(value, cx)
                }
            });
            Some(match project {
                None => switch.into_any_element(),
                Some(_) => SetHere::new(
                    SharedString::from(id),
                    switch,
                    "set here",
                    "Use Settings' value",
                )
                .set(own.is_some_and(|own| own != global))
                .on_reset(move |_: &mut Window, cx: &mut App| set(None, cx))
                .into_any_element(),
            })
        })
        .collect();
    let id = format!("{prefix}-doors");
    Card::new()
        .child(
            div()
                .id(SharedString::from(id.clone()))
                .p(space::LG)
                .flex()
                .flex_col()
                .gap(space::MD)
                .children(lines.into_iter().enumerate().map(|(at, line)| {
                    enter(SharedString::from(format!("{id}-line-{at}")), at, line)
                }))
                .test_support(),
        )
        .into_any_element()
}

/// What a folded check's instructions show: `none`, or the start of their first line.
fn preview_of(text: Option<&str>) -> SharedString {
    let line = text
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty());
    match line {
        None => "none".into(),
        Some(line) if line.chars().count() > PREVIEW => {
            let start: String = line.chars().take(PREVIEW).collect();
            format!("{}…", start.trim_end()).into()
        }
        Some(line) => line.to_owned().into(),
    }
}

/// What a review waits for, said on its conversation: `Waiting for 3 checks: bugs, standards,
/// security`. `None` when `agent` is not a review's, or when every check it started ended.
pub fn waiting_for(tree: &[TreeAgent], agent: &str) -> Option<String> {
    let review = tree.iter().find(|a| a.id == agent)?;
    if review.extensions["review"]["kind"] != "pilot" {
        return None;
    }
    // The last line's own agent is no check of the user's.
    let running: Vec<&str> = tree
        .iter()
        .filter(|a| a.parent.as_deref() == Some(agent))
        .filter(|a| a.extensions["review"]["kind"] == "pass")
        .filter(|a| {
            !matches!(
                state_of(a),
                AgentState::Completed | AgentState::Stopped | AgentState::Failed
            )
        })
        .filter_map(|a| a.extensions["review"]["pass"].as_str())
        .filter(|pass| *pass != "verifier")
        .collect();
    (!running.is_empty()).then(|| {
        format!(
            "Waiting for {} check{}: {}",
            running.len(),
            if running.len() == 1 { "" } else { "s" },
            running.join(", ")
        )
    })
}
