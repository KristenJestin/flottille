//! The places the window gives the parts of Flottille the user can turn off (`docs/extensions.md`,
//! "In the window"): each one's row of Settings' Features and the page it opens, and its section
//! of a project's settings, drawn here from what it declares (`extension.list`), never by the part
//! itself; and its screens (a project tab, a page, an action on a row), shown only where it is on.
//! The window never says what these parts are called in the code: each is a feature of the app,
//! under its own title.

pub mod review;
pub mod specs;

use std::collections::HashMap;
use std::time::Duration;

use flottille_ui::{
    Chosen, Field, Icon, ModelChoice, Section, Select, SelectOption, Switch, Toast, enter, palette,
    presence, presence_of, space, toast,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FontWeight, HighlightStyle,
    InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement,
    Styled, StyledText, Task, TestSupportExt as _, WeakEntity, div, px,
};
use serde_json::{Map, Value, json};

use crate::actions::Command;
use crate::commands::perform_later;
use crate::rules::RulesEditor;
use crate::shell::Shell;

/// One of its settings, as the daemon lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Setting {
    pub key: String,
    pub label: String,
    pub hint: String,
    /// `language`, `subset` (some of `options`) or `model` (a slot): the kinds come with their
    /// first need.
    pub kind: String,
    /// The values a `subset` takes some of, in their order.
    pub options: Vec<String>,
    /// A project may set its own value.
    pub per_project: bool,
    /// Its global value: its default when not `set`.
    pub value: Value,
    pub set: bool,
}

/// What a project says of it.
#[derive(Clone, Debug, PartialEq)]
pub struct InProject {
    pub project: String,
    /// It is on there: on globally, and not turned off in the project.
    pub active: bool,
    /// The project's own switch, when its file says one.
    pub switch: Option<bool>,
    /// The project's own values.
    pub settings: Map<String, Value>,
    pub error: Option<String>,
}

/// A part of Flottille that can be turned off, as `extension.list` says it.
#[derive(Clone, Debug, PartialEq)]
pub struct Listed {
    pub name: String,
    pub title: String,
    /// What it does and what it costs: the line its page shows with its switch while it is off.
    pub sentence: String,
    /// It touches a project: a project may turn it off, and its settings have a section there.
    pub per_project: bool,
    /// Its row of Features: the icon (a Lucide file name), the line that says what it is for, and
    /// its place among the rows.
    pub icon: String,
    pub line: String,
    pub order: u64,
    /// The label of its switch, and the usage line under it (`**` around the words found
    /// elsewhere in the window).
    pub switch: String,
    pub usage: String,
    /// The global switch.
    pub enabled: bool,
    /// Why its file cannot be used: it is off until it can.
    pub error: Option<String>,
    pub settings: Vec<Setting>,
    pub projects: Vec<InProject>,
    /// It has a guidance its agents follow, edited in its tab.
    pub guidance: bool,
}

impl Listed {
    fn of(value: &Value) -> Self {
        let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
        Self {
            name: text(&value["name"]),
            title: text(&value["title"]),
            sentence: text(&value["sentence"]),
            per_project: value["scope"] == "project",
            icon: text(&value["place"]["icon"]),
            line: text(&value["place"]["line"]),
            order: value["place"]["order"].as_u64().unwrap_or(u64::MAX),
            switch: text(&value["switch"]),
            usage: text(&value["usage"]),
            enabled: value["enabled"] == true,
            error: value["error"].as_str().map(str::to_owned),
            settings: value["settings"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|setting| Setting {
                    key: text(&setting["key"]),
                    label: text(&setting["label"]),
                    hint: text(&setting["hint"]),
                    kind: match &setting["kind"] {
                        Value::Object(kind) if kind.contains_key("subset") => "subset".into(),
                        kind => text(kind),
                    },
                    options: setting["kind"]["subset"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(text)
                        .collect(),
                    per_project: setting["per_project"] == true,
                    value: setting["value"].clone(),
                    set: setting["set"] == true,
                })
                .collect(),
            projects: value["projects"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|project| InProject {
                    project: text(&project["project"]),
                    active: project["enabled"] == true,
                    switch: project["switch"].as_bool(),
                    settings: project["settings"].as_object().cloned().unwrap_or_default(),
                    error: project["error"].as_str().map(str::to_owned),
                })
                .collect(),
            guidance: value["guidance"] == true,
        }
    }

    /// Whether it is on in `project`: its screens show there.
    pub fn active_in(&self, project: &str) -> bool {
        self.enabled
            && self
                .projects
                .iter()
                .any(|side| side.project == project && side.active)
    }

    fn in_project(&self, project: &str) -> Option<&InProject> {
        self.projects.iter().find(|side| side.project == project)
    }
}

/// What to do with the daemon's answer to a [`Command::Ask`].
#[derive(Clone, Debug, PartialEq)]
pub enum Then {
    /// `extension.list`, the read `asked`.
    Listed {
        asked: u64,
    },
    /// `extension.set` of `name`, globally or in `project`: a switch, or a setting labelled.
    Set {
        name: String,
        project: Option<String>,
        setting: Option<String>,
    },
    Specs(specs::Then),
    Review(review::Then),
}

/// How long the events that change what is listed must stop before it is read again: a switch
/// says several at once, and one read answers them all.
const QUIET: Duration = Duration::from_millis(100);

/// What the window knows of the parts it draws in its places: read at its start, and again once
/// the events that change them go quiet.
pub struct Registry {
    shell: WeakEntity<Shell>,
    list: Vec<Listed>,
    /// The daemon answered a read.
    known: bool,
    /// Why the last read failed.
    error: Option<String>,
    asked: u64,
    listed_at: u64,
    /// A switch flipped, until the daemon answers: what it was asked to become, by the part's
    /// name and its project (`None` for the global switch).
    pending: HashMap<(String, Option<String>), bool>,
    /// What Specs lists of its specs.
    specs: specs::Specs,
    /// The editor of each one's guidance, by its name, made once it is listed with one.
    guidance: HashMap<String, Entity<RulesEditor>>,
    quiet: Option<Task<()>>,
}

impl Registry {
    pub fn new(shell: WeakEntity<Shell>) -> Self {
        Self {
            shell,
            list: Vec::new(),
            known: false,
            error: None,
            asked: 0,
            listed_at: 0,
            pending: HashMap::new(),
            specs: specs::Specs::default(),
            guidance: HashMap::new(),
            quiet: None,
        }
    }

    /// Asks the daemon for the list.
    pub fn read(&mut self, cx: &mut App) {
        self.asked += 1;
        let command = Command::Ask {
            method: "extension.list".into(),
            params: json!({}),
            then: Then::Listed { asked: self.asked },
        };
        perform_later(&self.shell, command, cx);
    }

    /// An event says what is listed changed: it is read again once they go quiet.
    pub fn changed(&mut self, cx: &mut Context<Self>) {
        self.quiet = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(QUIET).await;
            this.update(cx, |this, cx| {
                this.quiet = None;
                this.read(cx);
            })
            .ok();
        }));
    }

    /// Whether the daemon answered a read.
    pub fn is_known(&self) -> bool {
        self.known
    }

    pub fn list(&self) -> &[Listed] {
        &self.list
    }

    pub fn listed(&self, name: &str) -> Option<&Listed> {
        self.list.iter().find(|listed| listed.name == name)
    }

    /// Whether `name` is on in `project`: its screens show there.
    pub fn active(&self, name: &str, project: &str) -> bool {
        self.listed(name)
            .is_some_and(|listed| listed.active_in(project))
    }

    /// The rows of Settings' Features they have, in their order.
    pub fn rows(&self) -> Vec<&Listed> {
        let mut rows: Vec<&Listed> = self.list.iter().collect();
        rows.sort_by_key(|listed| listed.order);
        rows
    }

    /// The sections a project's settings have: those that touch a project and are on globally.
    pub fn project_tabs(&self) -> Vec<(String, String)> {
        let mut tabs: Vec<&Listed> = self
            .list
            .iter()
            .filter(|listed| listed.per_project && listed.enabled)
            .collect();
        tabs.sort_by_key(|listed| listed.order);
        tabs.into_iter()
            .map(|listed| (listed.name.clone(), listed.title.clone()))
            .collect()
    }

    pub fn specs(&self) -> &specs::Specs {
        &self.specs
    }

    /// The editor of `name`'s guidance.
    pub fn guidance(&self, name: &str) -> Option<&Entity<RulesEditor>> {
        self.guidance.get(name)
    }

    /// Each part listed with a guidance has its editor, which reads it again: what is typed in
    /// it stays.
    fn read_guidance(&mut self, cx: &mut Context<Self>) {
        for listed in self.list.iter().filter(|listed| listed.guidance) {
            let shell = self.shell.clone();
            let editor = self
                .guidance
                .entry(listed.name.clone())
                .or_insert_with(|| {
                    cx.new(|_| RulesEditor::part(shell, &listed.name, &listed.title))
                })
                .clone();
            editor.update(cx, |editor, cx| editor.read(cx));
        }
    }

    /// Whether the switch shows `name` on, globally or in `project`: what it was asked to become
    /// while the daemon has not answered.
    pub fn switch_on(&self, name: &str, project: Option<&str>) -> bool {
        if let Some(asked) = self
            .pending
            .get(&(name.to_owned(), project.map(str::to_owned)))
        {
            return *asked;
        }
        let Some(listed) = self.listed(name) else {
            return false;
        };
        match project {
            None => listed.enabled,
            Some(project) => listed
                .in_project(project)
                .is_some_and(|side| side.switch != Some(false)),
        }
    }

    /// Flips a switch: the daemon is asked, and the switch shows what it will be meanwhile.
    pub fn flip(&mut self, name: &str, project: Option<&str>, cx: &mut Context<Self>) {
        let key = (name.to_owned(), project.map(str::to_owned));
        if self.pending.contains_key(&key) || self.listed(name).is_none() {
            return;
        }
        let enabled = !self.switch_on(name, project);
        self.pending.insert(key, enabled);
        let command = Command::Ask {
            method: "extension.set".into(),
            params: json!({"name": name, "project": project, "enabled": enabled}),
            then: Then::Set {
                name: name.to_owned(),
                project: project.map(str::to_owned),
                setting: None,
            },
        };
        perform_later(&self.shell, command, cx);
        cx.notify();
    }

    /// Sets a value, globally or in `project`; `None` goes back to the default.
    pub fn set(
        &mut self,
        name: &str,
        project: Option<&str>,
        setting: &Setting,
        value: Option<Value>,
        cx: &mut Context<Self>,
    ) {
        let command = Command::Ask {
            method: "extension.set".into(),
            params: json!({"name": name, "project": project,
                "settings": {setting.key.clone(): value}}),
            then: Then::Set {
                name: name.to_owned(),
                project: project.map(str::to_owned),
                setting: Some(setting.label.clone()),
            },
        };
        perform_later(&self.shell, command, cx);
    }

    /// What the daemon answered to something the registry asked.
    pub fn answered(&mut self, then: Then, answer: Result<Value, String>, cx: &mut Context<Self>) {
        match then {
            Then::Listed { asked } => {
                if asked < self.listed_at {
                    return;
                }
                self.listed_at = asked;
                self.known = true;
                match answer {
                    Ok(listed) => {
                        let list: Vec<Listed> = listed["extensions"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(Listed::of)
                            .collect();
                        self.error = None;
                        if self.list != list {
                            self.list = list;
                            cx.notify();
                        }
                        self.read_guidance(cx);
                    }
                    Err(why) => self.error = Some(why),
                }
                self.read_specs(cx);
            }
            Then::Set {
                name,
                project,
                setting,
            } => {
                let title = self
                    .listed(&name)
                    .map_or_else(|| name.clone(), |listed| listed.title.clone());
                match answer {
                    Ok(listed) => {
                        let listed = Listed::of(&listed);
                        if let Some(at) = self.list.iter().position(|l| l.name == listed.name) {
                            self.list[at] = listed;
                        }
                        if let Some(label) = setting {
                            toast(
                                Toast::success(format!("{title}: {label} saved"))
                                    .detail("It applies from now on."),
                                cx,
                            );
                        }
                    }
                    Err(why) => {
                        let what = match setting {
                            Some(label) => format!("{title}: {label} was not saved"),
                            None => format!("{title} was not switched"),
                        };
                        toast(Toast::failure(what).detail(why), cx);
                    }
                }
                self.pending.remove(&(name, project));
                self.read_specs(cx);
                cx.notify();
            }
            Then::Specs(then) => specs::answered(self, then, answer, cx),
            Then::Review(then) => review::answered(self, then, answer, cx),
        }
    }

    fn read_specs(&mut self, cx: &mut Context<Self>) {
        if self
            .listed(specs::NAME)
            .is_some_and(|listed| listed.enabled)
        {
            specs::read(self, cx);
        } else if self.specs.forget() {
            cx.notify();
        }
    }
}

/// The languages a select offers, by their tag; a value set by hand that is none of them is
/// offered too.
const LANGUAGES: [(&str, &str); 10] = [
    ("en", "English"),
    ("fr", "French"),
    ("de", "German"),
    ("es", "Spanish"),
    ("it", "Italian"),
    ("pt", "Portuguese"),
    ("nl", "Dutch"),
    ("pl", "Polish"),
    ("ja", "Japanese"),
    ("zh", "Chinese"),
];

fn language_said(tag: &str) -> String {
    LANGUAGES
        .iter()
        .find(|(key, _)| *key == tag)
        .map_or_else(|| tag.to_owned(), |(_, name)| (*name).to_owned())
}

/// The control of one setting, chosen by its kind (rule 4 of `docs/reference/gpui.md`): a language
/// is a select, the first option the value it has when none is set here (`default`); a subset is a
/// switch for each of its values; a model slot is a select of the models the providers offer, the
/// first option the default.
#[allow(clippy::too_many_arguments)]
fn control(
    registry: &Entity<Registry>,
    name: &str,
    project: Option<&str>,
    setting: &Setting,
    own: Option<Value>,
    default: Value,
    id: &str,
    providers: &crate::providers::Providers,
) -> Option<AnyElement> {
    let (rule, gives) = model_default(name, &setting.key);
    let (registry, name, project_name, setting_of) = (
        registry.downgrade(),
        name.to_owned(),
        project.map(str::to_owned),
        setting.clone(),
    );
    let set = move |value: Option<Value>, cx: &mut App| {
        registry
            .update(cx, |registry, cx| {
                registry.set(&name, project_name.as_deref(), &setting_of, value, cx)
            })
            .ok();
    };
    match setting.kind.as_str() {
        "language" => {
            let own = own.as_ref().and_then(Value::as_str).map(str::to_owned);
            let default = default.as_str().unwrap_or_default().to_owned();
            let mut tags: Vec<String> = LANGUAGES.iter().map(|(t, _)| (*t).to_owned()).collect();
            if let Some(own) = &own
                && !tags.contains(own)
            {
                tags.push(own.clone());
            }
            let first = match project {
                None => format!("Yours ({})", language_said(&default)),
                Some(_) => format!("As in Settings ({})", language_said(&default)),
            };
            let options = std::iter::once(SelectOption::new("default", first)).chain(
                tags.iter()
                    .map(|tag| SelectOption::new(tag.clone(), language_said(tag))),
            );
            let chosen = own
                .as_ref()
                .and_then(|own| tags.iter().position(|tag| tag == own))
                .map_or(0, |at| at + 1);
            let select = Select::new(SharedString::from(format!("{id}-select")), options, chosen)
                .on_choose(move |at, _, cx| {
                    set(at.checked_sub(1).map(|at| json!(tags[at])), cx);
                });
            Some(select.into_any_element())
        }
        "subset" => {
            let words = |value: &Value| -> Vec<String> {
                value
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            };
            let chosen = words(own.as_ref().unwrap_or(&default));
            let set = std::rc::Rc::new(set);
            let switches = setting.options.iter().map(|option| {
                let on = chosen.contains(option);
                let (set, chosen, option, options) = (
                    set.clone(),
                    chosen.clone(),
                    option.clone(),
                    setting.options.clone(),
                );
                let label = capitalized(&option);
                Switch::new(SharedString::from(format!("{id}-{option}")), on, label).on_click(
                    move |_, _, cx| {
                        let list: Vec<&String> = options
                            .iter()
                            .filter(|word| (*word == &option) != chosen.contains(word))
                            .collect();
                        set(Some(json!(list)), cx);
                    },
                )
            });
            Some(
                div()
                    .flex()
                    .flex_col()
                    .gap(space::SM)
                    .children(switches)
                    .into_any_element(),
            )
        }
        "model" => {
            let text = |value: &Value, key: &str| value[key].as_str().map(str::to_owned);
            let chosen = match &own {
                Some(own) => match text(own, "provider") {
                    Some(provider) => providers.chosen(&provider, text(own, "model").as_deref()),
                    None => Chosen::First,
                },
                None => Chosen::First,
            };
            let choice = ModelChoice::new(
                SharedString::from(id.to_owned()),
                setting.label.clone(),
                chosen,
            )
            .compact()
            .hint(setting.hint.clone())
            .models(providers.model_entries())
            .effort(
                own.as_ref()
                    .and_then(|own| text(own, "effort"))
                    .map(Into::into),
            );
            let choice = match project {
                None => choice.default(rule, gives),
                // In a project, the first option is Settings' value: the model set there, else
                // the default's rule.
                Some(_) => choice.inherited(match text(&default, "provider") {
                    Some(provider) => {
                        let title = providers
                            .get(&provider)
                            .map_or(provider.clone(), |p| p.title.clone());
                        format!(
                            "{title} · {}",
                            text(&default, "model").unwrap_or_else(|| "its default".into())
                        )
                    }
                    None => rule.to_owned(),
                }),
            };
            let choice = choice.on_choose(move |chosen, effort, _, cx| {
                let slot = match chosen {
                    Chosen::Model(provider, model) => {
                        let mut slot = Map::new();
                        slot.insert("provider".into(), json!(provider));
                        if let Some(model) = model {
                            slot.insert("model".into(), json!(model));
                        }
                        if let Some(effort) = effort {
                            slot.insert("effort".into(), json!(effort));
                        }
                        Some(Value::Object(slot))
                    }
                    _ => None,
                };
                set(slot, cx);
            });
            Some(choice.into_any_element())
        }
        _ => None,
    }
}

/// The default of a model slot, as its first option says it: its rule and what it gives today.
fn model_default(name: &str, key: &str) -> (&'static str, &'static str) {
    match name {
        review::NAME => review::model_default(key),
        _ => ("Its feature's choice", "Chosen when it runs"),
    }
}

/// The line said once under the card of a feature's models.
fn models_line(name: &str) -> Option<&'static str> {
    (name == review::NAME).then_some(review::MODELS_LINE)
}

/// A value as a switch's label says it: `bugs` is "Bugs".
fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// The fields of the settings, each a labelled control, entering one after the other.
fn fields<'a>(
    registry: &Entity<Registry>,
    listed: &Listed,
    project: Option<&str>,
    settings: impl Iterator<Item = &'a Setting>,
    prefix: &str,
    cx: &App,
) -> AnyElement {
    let side = project.and_then(|project| listed.in_project(project));
    let providers = registry
        .read(cx)
        .shell
        .upgrade()
        .map(|shell| shell.read(cx).providers().clone())
        .unwrap_or_default();
    let (models, settings): (Vec<&Setting>, Vec<&Setting>) =
        settings.partition(|setting| setting.kind == "model");
    let models_card = (!models.is_empty()).then(|| {
        let p = palette(cx);
        let lines = models.iter().filter_map(|setting| {
            let global = setting.value.clone();
            let own = match side {
                None => setting.set.then(|| global.clone()),
                Some(side) => side.settings.get(&setting.key).cloned(),
            };
            control(
                registry,
                &listed.name,
                project,
                setting,
                own,
                global,
                &format!("{prefix}-setting-{}", setting.key),
                &providers,
            )
        });
        let id = format!("{prefix}-models");
        div()
            .flex()
            .flex_col()
            .gap(space::SM)
            .child(
                div()
                    .id(SharedString::from(id.clone()))
                    .p(space::LG)
                    .flex()
                    .flex_col()
                    .gap(space::MD)
                    .rounded(px(12.))
                    .border_1()
                    .border_color(p.line)
                    .children(lines)
                    .test_support(),
            )
            .children(models_line(&listed.name).map(|line| {
                div()
                    .id(SharedString::from(format!("{id}-cost")))
                    .aria_label(line)
                    .text_size(px(12.))
                    .text_color(p.faint)
                    .child(line)
                    .test_support()
            }))
    });
    div()
        .flex()
        .flex_col()
        .gap(space::LG)
        .children(
            settings
                .into_iter()
                .enumerate()
                .filter_map(|(at, setting)| {
                    let global = setting.value.clone();
                    let (own, default) = match side {
                        None => (setting.set.then(|| global.clone()), global),
                        Some(side) => (side.settings.get(&setting.key).cloned(), global),
                    };
                    let id = format!("{prefix}-setting-{}", setting.key);
                    let control = control(
                        registry,
                        &listed.name,
                        project,
                        setting,
                        own,
                        default,
                        &id,
                        &providers,
                    )?;
                    let field = Field::new(
                        SharedString::from(id.clone()),
                        setting.label.clone(),
                        control,
                    )
                    .hint(setting.hint.clone());
                    Some(enter(SharedString::from(format!("{id}-enter")), at, field))
                }),
        )
        .children(models_card.map(|card| {
            enter(
                SharedString::from(format!("{prefix}-models-enter")),
                1,
                card,
            )
        }))
        .into_any_element()
}

/// A usage line (`docs/reference/gpui.md`, rule 14): where and how a feature acts, the words found
/// elsewhere in the window in bold (`**` around them in what the declaration says). Its label is
/// the plain text.
pub fn usage_line(id: impl Into<SharedString>, usage: &str, cx: &App) -> impl IntoElement {
    let p = palette(cx);
    let mut plain = String::new();
    let mut bold = Vec::new();
    for (at, piece) in usage.split("**").enumerate() {
        if at % 2 == 1 {
            bold.push(plain.len()..plain.len() + piece.len());
        }
        plain.push_str(piece);
    }
    let strong = HighlightStyle {
        color: Some(p.text),
        font_weight: Some(FontWeight::MEDIUM),
        ..Default::default()
    };
    div()
        .id(id.into())
        .aria_label(SharedString::from(plain.clone()))
        .text_size(px(12.5))
        .text_color(p.mute)
        .child(StyledText::new(plain).with_highlights(bold.into_iter().map(|at| (at, strong))))
        .test_support()
}

/// The card of a switch, its usage line under it, its settings unfolding under that while it is
/// on, and why the daemon cannot use its file, when it cannot.
fn card(
    id: &str,
    switch: Switch,
    usage: Option<AnyElement>,
    on: bool,
    settings: AnyElement,
    error: Option<String>,
    cx: &App,
) -> impl IntoElement {
    let p = palette(cx);
    div()
        .id(SharedString::from(id.to_owned()))
        .p(space::LG)
        .flex()
        .flex_col()
        .rounded(px(12.))
        .bg(p.card)
        .border_1()
        .border_color(p.line)
        .child(switch)
        .children(usage.map(|usage| div().pt(space::SM).child(usage)))
        .child(
            presence_of(
                SharedString::from(format!("{id}-error-block")),
                error.map(SharedString::from),
                {
                    let id = format!("{id}-error");
                    move |error| {
                        div()
                            .id(SharedString::from(id))
                            .aria_label(error.clone())
                            .text_size(px(12.5))
                            .text_color(p.fail)
                            .child(error)
                            .test_support()
                            .into_any_element()
                    }
                },
            )
            .gap(space::MD),
        )
        .child(
            presence(
                SharedString::from(format!("{id}-settings-block")),
                on,
                settings,
            )
            .gap(space::LG),
        )
        .test_support()
}

/// The page of the feature `name` (`docs/extensions.md`, "In the window"; rule 14): its switch,
/// labelled with what it does, its sentence as hint and its usage line, then, while it is on, its
/// settings and its guidance. Off, that block alone.
pub fn feature_page(registry: &Entity<Registry>, name: &str, cx: &App) -> Option<AnyElement> {
    let read = registry.read(cx);
    let listed = read.listed(name)?.clone();
    let on = read.switch_on(name, None);
    let pending = read
        .pending
        .contains_key(&(name.to_owned(), None::<String>));
    let flip = {
        let (registry, name) = (registry.downgrade(), name.to_owned());
        move |_: &gpui_kit::ClickEvent, _: &mut gpui_kit::Window, cx: &mut App| {
            registry
                .update(cx, |registry, cx| registry.flip(&name, None, cx))
                .ok();
        }
    };
    let switch = Switch::new(
        SharedString::from(format!("{name}-switch")),
        on,
        listed.switch.clone(),
    )
    .hint(listed.sentence.clone())
    .disabled(pending)
    .on_click(flip);
    let settings = fields(registry, &listed, None, listed.settings.iter(), name, cx);
    let usage = usage_line(format!("{name}-usage"), &listed.usage, cx).into_any_element();
    let card = card(
        name,
        switch,
        Some(usage),
        on && listed.enabled,
        settings,
        listed.error.clone(),
        cx,
    );
    // Its guidance, a section of its own under the settings while it is on, in the editor the
    // judge's has.
    let guidance = read.guidance(name).cloned().map(|editor| {
        presence(
            SharedString::from(format!("{name}-guidance-block")),
            on && listed.enabled,
            editor.into_any_element(),
        )
        .gap(space::XXL)
    });
    Some(
        div()
            .flex()
            .flex_col()
            .child(enter(
                SharedString::from(format!("{name}-page-enter")),
                0,
                card,
            ))
            .children(guidance)
            .into_any_element(),
    )
}

/// The icon a declaration names (a Lucide file name): those the window embeds, else a generic one.
pub fn icon_named(name: &str) -> Icon {
    match name {
        "file-text" => Icon::Lucide("file-text"),
        "scan-search" => Icon::Lucide("scan-search"),
        "shrink" => Icon::Lucide("shrink"),
        "plug" => Icon::Lucide("plug"),
        _ => Icon::Lucide("blocks"),
    }
}

/// The section of a project's settings of `name`: the project's switch and, while it is on, the
/// values a project may set. Only while `name` is on globally.
pub fn project_section(
    registry: &Entity<Registry>,
    name: &str,
    project: &str,
    cx: &App,
) -> Option<AnyElement> {
    let read = registry.read(cx);
    let listed = read.listed(name).filter(|listed| listed.enabled)?.clone();
    let on = read.switch_on(name, Some(project));
    let pending = read
        .pending
        .contains_key(&(name.to_owned(), Some(project.to_owned())));
    let flip = {
        let (registry, name, project) = (registry.downgrade(), name.to_owned(), project.to_owned());
        move |_: &gpui_kit::ClickEvent, _: &mut gpui_kit::Window, cx: &mut App| {
            registry
                .update(cx, |registry, cx| registry.flip(&name, Some(&project), cx))
                .ok();
        }
    };
    let prefix = format!("project-{name}");
    let switch = Switch::new(
        SharedString::from(format!("{prefix}-switch")),
        on,
        "On in this project",
    )
    .hint(listed.sentence.clone())
    .disabled(pending)
    .on_click(flip);
    let settings = fields(
        registry,
        &listed,
        Some(project),
        listed.settings.iter().filter(|setting| setting.per_project),
        &prefix,
        cx,
    );
    let error = listed
        .in_project(project)
        .and_then(|side| side.error.clone());
    let card = card(&prefix, switch, None, on, settings, error, cx);
    Some(
        Section::new(SharedString::from(name.to_owned()), listed.title.clone())
            .child(card)
            .into_any_element(),
    )
}
