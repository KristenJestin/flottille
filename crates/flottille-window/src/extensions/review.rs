//! Review in the window (`docs/extensions.md`, "Review"): `/review` in a session's composer, which
//! Flottille runs and never sends to the agent. A review's agents and findings are one folded line
//! of the session's rail (`session.rs`, `RailRow::Review`). Its page ("Its page") draws what it
//! checks as one card, a line per check with its switch and its model, each check's own
//! instructions folded under its line.

use std::rc::Rc;

use gpui_design_system::color::mix;
use gpui_design_system::motion::HOVER;
use gpui_design_system::{
    Icon, SetHere, Switch, Toast, enter, fade, hoverable, icon, palette, presence, space, toast,
    turning,
};
use gpui_kit::{
    AnyElement, App, ElementId, Entity, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    SharedString, StatefulInteractiveElement, Styled, TestSupportExt as _, Transformation,
    WeakEntity, Window, div, px, radians,
};
use serde_json::{Value, json};

use super::{Listed, Registry, Setting};
use crate::actions::Command;
use crate::shell::Shell;

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

/// How a check's select says its default: the rule is said once, under the card.
const DEFAULT: &str = "Default";

/// The key of the setting of the checks a review runs, and of the last line's model.
const CHECKS: &str = "passes";
const VERIFIER: &str = "verifier_model";

/// What sets the checks a review runs.
type SetChecks = Rc<dyn Fn(Vec<String>, &mut App)>;

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
    let model = |setting: &Setting, words_drawn: bool, rule: Option<&str>| {
        let global = setting.value.clone();
        let own = match side {
            None => setting.set.then(|| global.clone()),
            Some(side) => side.settings.get(&setting.key).cloned(),
        };
        let mut setting = setting.clone();
        if let (Some(rule), Some(default)) = (rule, setting.default.as_mut()) {
            default.rule = rule.to_owned();
        }
        super::worded_control(
            registry,
            &listed.name,
            project,
            &setting,
            own,
            global,
            &format!("{prefix}-setting-{}", setting.key),
            &providers,
            words_drawn,
        )
    };
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
        let rule = project.is_none().then_some(DEFAULT);
        let select = model(setting, false, rule).map(|select| Faded {
            id: SharedString::from(format!("{id}-model")),
            on,
            child: select,
        });
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
                    let toggle = Toggle {
                        id: SharedString::from(format!("{id}-instructions")),
                        open,
                        preview: preview_of(preview.as_deref()),
                        registry: registry.downgrade(),
                        name: listed.name.clone(),
                        part: check.clone(),
                    };
                    div().flex().flex_col().child(toggle).child(
                        presence(
                            SharedString::from(format!("{id}-fold")),
                            open,
                            div().pt(space::SM).child(editor),
                        )
                        .gap(space::XS),
                    )
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
        && let Some(select) = model(setting, true, None)
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
    let card = div()
        .id(SharedString::from(id.clone()))
        .p(space::LG)
        .flex()
        .flex_col()
        .gap(space::MD)
        .rounded(px(12.))
        .border_1()
        .border_color(p.line)
        .children(
            lines
                .into_iter()
                .enumerate()
                .map(|(at, line)| enter(SharedString::from(format!("{id}-line-{at}")), at, line)),
        )
        .test_support();
    div()
        .flex()
        .flex_col()
        .gap(space::SM)
        .child(card)
        .child(under("models", MODELS_LINE))
        .child(under("rounds", ROUNDS_LINE))
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

/// A check's model, faded to disabled while the check is off; a click does nothing then.
#[derive(IntoElement)]
struct Faded {
    id: SharedString,
    on: bool,
    child: AnyElement,
}

impl RenderOnce for Faded {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = fade(ElementId::Name(self.id.clone()), self.on, HOVER, window, cx);
        div()
            .id(ElementId::Name(self.id))
            .relative()
            .flex_none()
            .opacity(0.35 + 0.65 * t)
            .child(self.child)
            // Off, the select under it is out of reach.
            .children((!self.on).then(|| div().absolute().size_full().top_0().left_0().occlude()))
    }
}

/// The quiet line that unfolds a check's own instructions: its chevron turns, and what they
/// start with fades away while they are open.
#[derive(IntoElement)]
struct Toggle {
    id: SharedString,
    open: bool,
    preview: SharedString,
    registry: WeakEntity<Registry>,
    name: String,
    part: String,
}

impl RenderOnce for Toggle {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let turned = turning(
            ElementId::Name(format!("{}-chevron", self.id).into()),
            self.open,
            window,
            cx,
        );
        let shown = fade(
            ElementId::Name(format!("{}-preview", self.id).into()),
            !self.open,
            HOVER,
            window,
            cx,
        );
        let (registry, name, part) = (self.registry, self.name, self.part);
        hoverable(
            div()
                .id(ElementId::Name(self.id.clone()))
                .aria_label(INSTRUCTIONS)
                .aria_expanded(self.open)
                .flex()
                .items_center()
                .gap(space::XS)
                .cursor_pointer()
                .text_size(px(12.))
                .child(icon(Icon::Chevron).size(px(12.)).with_transformation(
                    Transformation::rotate(radians(-std::f32::consts::FRAC_PI_2 * turned)),
                ))
                .child(INSTRUCTIONS)
                .child(
                    div()
                        .id(ElementId::Name(format!("{}-preview", self.id).into()))
                        .aria_label(self.preview.clone())
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_color(p.faint)
                        .opacity(shown)
                        .child(self.preview)
                        .test_support(),
                )
                .on_click(move |_, _, cx| {
                    registry
                        .update(cx, |registry, cx| registry.toggle_part(&name, &part, cx))
                        .ok();
                }),
            move |d, t| d.text_color(mix(p.mute, p.text, t)),
        )
    }
}
