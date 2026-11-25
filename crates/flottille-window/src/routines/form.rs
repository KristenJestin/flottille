//! The form that creates and edits a routine, in the kit's dialog: its name, project and prompt;
//! the agent, model and effort side by side as the session's composer offers them; the trigger
//! (on a schedule: days and an hour, never a cron line; or on an event: its kind, then the fields
//! of that kind); the mode, each with its one line; where the result rings; and one sentence that
//! says the next run. The fields a choice needs unfold when it is chosen and fold away when
//! another is.

use gpui_design_system::{
    Button, Chosen, Field, Modal, ModelChoice, RadioChoices, Select, SelectOption, Switch,
    changing, follow_theme, palette, presence, space,
};
use gpui_kit::base::input::{InputEvent, InputState, TextareaState};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, StatefulInteractiveElement, Styled, Subscription, TestSupportExt as _,
    Window, div, px,
};
use serde_json::Value;

use super::tab::Then;
use super::{DAYS, Event, Routine, hour_of, mode_line, mode_said, next_run, next_said};
use crate::actions::Command;
use crate::commands::{ShellOf, perform_later, spaced, stack};
use crate::extensions::Then as Extension;
use crate::project::input_frame;
use crate::project_settings::styled_input;

const KINDS: [(&str, &str); 4] = [
    ("workflow_run", "A workflow run ends"),
    ("review_requested", "A review is asked of you"),
    ("mentioned", "You are mentioned in the tracker"),
    ("assigned", "A ticket is assigned to you"),
];

const STARTED_BY: [(&str, &str); 4] = [
    ("any", "Any way"),
    ("schedule", "A schedule"),
    ("push", "A push"),
    ("manual", "By hand"),
];

const OUTCOMES: [(&str, &str); 2] = [("failed", "It failed"), ("any", "Whatever it ends in")];

const AT_MOST_ONCE: [(&str, &str); 3] = [
    ("", "As often as it happens"),
    ("day", "A day"),
    ("hour", "An hour"),
];

const MODES: [&str; 3] = ["read_only", "ask", "auto"];
/// What the form's choices of where the result rings are, in their order.
const RINGS: [&str; 3] = ["something", "new", "never"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Trigger {
    Schedule,
    Event,
}

/// The form's fields.
struct Fields {
    name: Entity<InputState>,
    prompt: Entity<TextareaState>,
    hour: Entity<InputState>,
    texts: Texts,
    _subscriptions: Vec<Subscription>,
}

/// The texts of the event's fields.
struct Texts {
    repository: Entity<InputState>,
    workflow: Entity<InputState>,
    branch: Entity<InputState>,
    tracker_project: Entity<InputState>,
    ticket_type: Entity<InputState>,
    not_before: Entity<InputState>,
}

pub struct RoutineForm {
    shell: ShellOf,
    project: String,
    open: bool,
    /// The routine edited, by its id; none for a new one.
    editing: Option<String>,
    /// Its fields, made the first time the form opens (they need the window).
    fields: Option<Fields>,
    /// The project the routine belongs to.
    chosen_project: String,
    model: Chosen,
    effort: Option<String>,
    trigger: Trigger,
    days: usize,
    kind: usize,
    started_by: usize,
    outcome: usize,
    at_most_once: usize,
    new_session: bool,
    mode: usize,
    rings: usize,
    saving: bool,
}

impl RoutineForm {
    pub fn new(shell: ShellOf, project: String) -> Self {
        Self {
            shell,
            chosen_project: project.clone(),
            project,
            open: false,
            editing: None,
            fields: None,
            model: Chosen::First,
            effort: None,
            trigger: Trigger::Schedule,
            days: 1,
            kind: 0,
            started_by: 0,
            outcome: 0,
            at_most_once: 0,
            new_session: false,
            mode: 0,
            rings: 0,
            saving: false,
        }
    }

    fn make_fields(window: &mut Window, cx: &mut Context<Self>) -> Fields {
        let line = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| styled_input(placeholder, window, cx))
        };
        let name = line("for example Tickets to review", window, cx);
        let hour = line("08:30", window, cx);
        let texts = Texts {
            repository: line("any of the project's", window, cx),
            workflow: line("nightly.yml, any when empty", window, cx),
            branch: line("dev, any when empty", window, cx),
            tracker_project: line("ABC", window, cx),
            ticket_type: line("any when empty", window, cx),
            not_before: line("08:00", window, cx),
        };
        let prompt = cx.new(|cx| {
            let mut field = TextareaState::new(window, cx)
                .auto_grow(3, 10)
                .placeholder("List the tickets waiting for my review, newest first");
            follow_theme(&mut field, cx);
            field
        });
        let mut subscriptions = Vec::new();
        for input in [
            &name,
            &hour,
            &texts.repository,
            &texts.workflow,
            &texts.branch,
            &texts.tracker_project,
            &texts.ticket_type,
            &texts.not_before,
        ] {
            subscriptions.push(cx.subscribe(input, |_, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }));
        }
        subscriptions.push(cx.subscribe(&prompt, |_, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        }));
        Fields {
            name,
            prompt,
            hour,
            texts,
            _subscriptions: subscriptions,
        }
    }

    fn f(&self) -> &Fields {
        self.fields.as_ref().expect("the form was opened")
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn name_field(&self) -> &Entity<InputState> {
        &self.f().name
    }

    pub fn prompt_field(&self) -> &Entity<TextareaState> {
        &self.f().prompt
    }

    pub fn hour_field(&self) -> &Entity<InputState> {
        &self.f().hour
    }

    fn set(entity: &Entity<InputState>, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        entity.update(cx, |field, cx| field.set_value(text, window, cx));
    }

    /// Opens the form on a routine that is new: read only, on weekdays at 08:00.
    pub fn open_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.fields.is_none() {
            self.fields = Some(Self::make_fields(window, cx));
        }
        self.editing = None;
        self.chosen_project = self.project.clone();
        Self::set(&self.f().name.clone(), "", window, cx);
        self.f()
            .prompt
            .clone()
            .update(cx, |field, cx| field.set_value("", window, cx));
        Self::set(&self.f().hour.clone(), "08:00", window, cx);
        for field in self.text_fields() {
            Self::set(&field, "", window, cx);
        }
        self.model = Chosen::First;
        self.effort = None;
        self.trigger = Trigger::Schedule;
        self.days = 1;
        self.kind = 0;
        self.started_by = 0;
        self.outcome = 0;
        self.at_most_once = 0;
        self.new_session = false;
        self.mode = 0;
        self.rings = 0;
        self.saving = false;
        self.open = true;
        cx.notify();
    }

    /// Opens the form on `routine`, with what it has now.
    pub fn open_edit(&mut self, routine: &Routine, window: &mut Window, cx: &mut Context<Self>) {
        self.open_new(window, cx);
        self.editing = Some(routine.id.clone());
        self.chosen_project = routine.project.clone();
        Self::set(&self.f().name.clone(), &routine.name, window, cx);
        self.f()
            .prompt
            .clone()
            .update(cx, |field, cx| field.set_value(&routine.prompt, window, cx));
        if let Some(provider) = &routine.provider {
            self.model = Chosen::Model(
                provider.clone().into(),
                routine.model.clone().map(Into::into),
            );
        }
        self.effort = routine.effort.clone();
        self.mode = MODES.iter().position(|m| *m == routine.mode).unwrap_or(0);
        self.rings = RINGS.iter().position(|r| *r == routine.rings).unwrap_or(0);
        if let Some(schedule) = &routine.schedule
            && let Some((days, hour)) = schedule.rsplit_once(' ')
        {
            self.days = DAYS.iter().position(|(d, _)| *d == days).unwrap_or(1);
            Self::set(&self.f().hour.clone(), hour, window, cx);
        }
        if let Some(event) = &routine.event {
            self.trigger = Trigger::Event;
            self.kind = KINDS
                .iter()
                .position(|(k, _)| *k == event.kind)
                .unwrap_or(0);
            self.started_by = STARTED_BY
                .iter()
                .position(|(k, _)| *k == event.started_by)
                .unwrap_or(0);
            self.outcome = OUTCOMES
                .iter()
                .position(|(k, _)| *k == event.outcome)
                .unwrap_or(0);
            self.at_most_once = AT_MOST_ONCE
                .iter()
                .position(|(k, _)| Some(*k) == event.at_most_once.as_deref().or(Some("")))
                .unwrap_or(0);
            self.new_session = event.new_session;
            let texts = [
                (&self.f().texts.repository, &event.repository),
                (&self.f().texts.workflow, &event.workflow),
                (&self.f().texts.branch, &event.branch),
                (&self.f().texts.tracker_project, &event.tracker_project),
                (&self.f().texts.ticket_type, &event.ticket_type),
                (&self.f().texts.not_before, &event.not_before),
            ];
            for (field, value) in texts {
                Self::set(
                    &field.clone(),
                    value.as_deref().unwrap_or_default(),
                    window,
                    cx,
                );
            }
        }
        cx.notify();
    }

    fn text_fields(&self) -> [Entity<InputState>; 6] {
        [
            self.f().texts.repository.clone(),
            self.f().texts.workflow.clone(),
            self.f().texts.branch.clone(),
            self.f().texts.tracker_project.clone(),
            self.f().texts.ticket_type.clone(),
            self.f().texts.not_before.clone(),
        ]
    }

    /// The daemon kept the routine: the form closes.
    pub fn saved(&mut self, cx: &mut Context<Self>) {
        self.saving = false;
        self.open = false;
        cx.notify();
    }

    /// The daemon refused it: the form stays, to be corrected.
    pub fn refused(&mut self, cx: &mut Context<Self>) {
        self.saving = false;
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        cx.notify();
    }

    fn typed(field: &Entity<InputState>, cx: &Context<Self>) -> String {
        field.read(cx).value().trim().to_owned()
    }

    fn text(field: &Entity<InputState>, cx: &Context<Self>) -> Option<String> {
        Some(Self::typed(field, cx)).filter(|text| !text.is_empty())
    }

    /// The event as chosen, with its fields.
    fn event(&self, cx: &Context<Self>) -> Event {
        Event {
            kind: KINDS[self.kind].0.to_owned(),
            repository: Self::text(&self.f().texts.repository, cx),
            workflow: Self::text(&self.f().texts.workflow, cx),
            branch: Self::text(&self.f().texts.branch, cx),
            started_by: STARTED_BY[self.started_by].0.to_owned(),
            outcome: OUTCOMES[self.outcome].0.to_owned(),
            tracker_project: Self::text(&self.f().texts.tracker_project, cx),
            ticket_type: Self::text(&self.f().texts.ticket_type, cx),
            not_before: Self::text(&self.f().texts.not_before, cx),
            at_most_once: Some(AT_MOST_ONCE[self.at_most_once].0)
                .filter(|per| !per.is_empty())
                .map(str::to_owned),
            new_session: self.new_session,
        }
    }

    /// Why the hour typed is none; `None` when it is one.
    fn hour_problem(&self, cx: &Context<Self>) -> Option<SharedString> {
        hour_of(&Self::typed(&self.f().hour, cx))
            .is_none()
            .then(|| "An hour as 08:30.".into())
    }

    fn not_before_problem(&self, cx: &Context<Self>) -> Option<SharedString> {
        let typed = Self::typed(&self.f().texts.not_before, cx);
        (!typed.is_empty() && hour_of(&typed).is_none()).then(|| "An hour as 08:00.".into())
    }

    fn ready(&self, cx: &Context<Self>) -> bool {
        let named = !Self::typed(&self.f().name, cx).is_empty();
        let prompt = !self.f().prompt.read(cx).value().trim().is_empty();
        let when = match self.trigger {
            Trigger::Schedule => self.hour_problem(cx).is_none(),
            Trigger::Event => self.not_before_problem(cx).is_none(),
        };
        named && prompt && when && !self.saving
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if !self.ready(cx) {
            return;
        }
        let name = Self::typed(&self.f().name, cx);
        let mut params = serde_json::Map::new();
        let mut put = |key: &str, value: Value| {
            params.insert(key.to_owned(), value);
        };
        put("name", name.clone().into());
        put("project", self.chosen_project.clone().into());
        put(
            "prompt",
            self.f().prompt.read(cx).value().trim().to_owned().into(),
        );
        put("mode", MODES[self.mode].into());
        put("rings", RINGS[self.rings].into());
        match self.trigger {
            Trigger::Schedule => {
                let hour = hour_of(&Self::typed(&self.f().hour, cx)).unwrap_or((8, 0));
                put(
                    "schedule",
                    format!("{} {:02}:{:02}", DAYS[self.days].0, hour.0, hour.1).into(),
                );
            }
            Trigger::Event => put("event", self.event(cx).to_json()),
        }
        // An empty text sets what may be left to the defaults back to them, on an edit.
        let (provider, model) = match &self.model {
            Chosen::Model(provider, model) => (
                provider.to_string(),
                model.as_ref().map(ToString::to_string).unwrap_or_default(),
            ),
            _ => (String::new(), String::new()),
        };
        let effort = self.effort.clone().unwrap_or_default();
        let method = match &self.editing {
            Some(id) => {
                put("routine", id.clone().into());
                for (key, value) in [("provider", provider), ("model", model), ("effort", effort)] {
                    put(key, value.into());
                }
                "routine.edit"
            }
            None => {
                for (key, value) in [("provider", provider), ("model", model), ("effort", effort)] {
                    if !value.is_empty() {
                        put(key, value.into());
                    }
                }
                "routine.add"
            }
        };
        self.saving = true;
        cx.notify();
        perform_later(
            &self.shell,
            Command::Ask {
                method: method.to_owned(),
                params: Value::Object(params),
                then: Extension::Routines(Then::Saved {
                    project: self.project.clone(),
                    name,
                }),
            },
            cx,
        );
    }

    fn choice<const N: usize>(
        id: &'static str,
        options: &[(&'static str, &'static str); N],
        chosen: usize,
        cx: &mut Context<Self>,
        apply: impl Fn(&mut Self, usize) + 'static,
    ) -> Select {
        let this = cx.entity().downgrade();
        Select::new(
            id,
            options
                .iter()
                .map(|(key, title)| SelectOption::new(key.replace(' ', "-"), title.to_string())),
            chosen,
        )
        .on_choose(move |index, _, cx| {
            this.update(cx, |this, cx| {
                apply(this, index);
                cx.notify();
            })
            .ok();
        })
    }

    fn text_field(
        id: &'static str,
        label: &'static str,
        state: &Entity<InputState>,
        error: Option<SharedString>,
        cx: &Context<Self>,
    ) -> Field {
        Field::new(id, label, input_frame(format!("{id}-box"), state, cx))
            .optional()
            .error(error)
    }

    fn event_fields(&self, cx: &mut Context<Self>) -> AnyElement {
        let kind = KINDS[self.kind].0;
        let workflow = kind == "workflow_run";
        let repository = matches!(kind, "workflow_run" | "review_requested");
        let tracker = matches!(kind, "mentioned" | "assigned");
        let kinds = Field::new(
            "routine-event-kind-field",
            "Starts when",
            Self::choice("routine-event-kind", &KINDS, self.kind, cx, |this, at| {
                this.kind = at
            }),
        );
        let started_by = Field::new(
            "routine-event-started-by-field",
            "Started by",
            Self::choice(
                "routine-event-started-by",
                &STARTED_BY,
                self.started_by,
                cx,
                |this, at| this.started_by = at,
            ),
        );
        let outcome = Field::new(
            "routine-event-outcome-field",
            "Ended",
            Self::choice(
                "routine-event-outcome",
                &OUTCOMES,
                self.outcome,
                cx,
                |this, at| this.outcome = at,
            ),
        );
        let once = Field::new(
            "routine-event-at-most-once-field",
            "At most once a",
            Self::choice(
                "routine-event-at-most-once",
                &AT_MOST_ONCE,
                self.at_most_once,
                cx,
                |this, at| this.at_most_once = at,
            ),
        )
        .optional();
        let this = cx.entity().downgrade();
        let new_session = Switch::new(
            "routine-event-new-session",
            self.new_session,
            "Always start a new session",
        )
        .hint("Otherwise a trigger while its last session is open goes to that session.")
        .on_click(move |_, _, cx| {
            this.update(cx, |this, cx| {
                this.new_session = !this.new_session;
                cx.notify();
            })
            .ok();
        });
        let block = |id: &'static str, shown: bool, field: Field| {
            presence(id, shown, field).space(space::MD)
        };
        stack(space::MD)
            .child(spaced(space::MD, kinds))
            .child(block(
                "routine-event-repository-block",
                repository,
                Self::text_field(
                    "routine-event-repository",
                    "Repository",
                    &self.f().texts.repository,
                    None,
                    cx,
                ),
            ))
            .child(block(
                "routine-event-workflow-block",
                workflow,
                Self::text_field(
                    "routine-event-workflow",
                    "Workflow",
                    &self.f().texts.workflow,
                    None,
                    cx,
                ),
            ))
            .child(block(
                "routine-event-branch-block",
                workflow,
                Self::text_field(
                    "routine-event-branch",
                    "Branch",
                    &self.f().texts.branch,
                    None,
                    cx,
                ),
            ))
            .child(block(
                "routine-event-started-by-block",
                workflow,
                started_by,
            ))
            .child(block("routine-event-outcome-block", workflow, outcome))
            .child(block(
                "routine-event-tracker-project-block",
                tracker,
                Self::text_field(
                    "routine-event-tracker-project",
                    "Tracker project",
                    &self.f().texts.tracker_project,
                    None,
                    cx,
                ),
            ))
            .child(block(
                "routine-event-tracker-type-block",
                kind == "assigned",
                Self::text_field(
                    "routine-event-tracker-type",
                    "Ticket type",
                    &self.f().texts.ticket_type,
                    None,
                    cx,
                ),
            ))
            .child(spaced(
                space::MD,
                Self::text_field(
                    "routine-event-not-before",
                    "Not before",
                    &self.f().texts.not_before,
                    self.not_before_problem(cx),
                    cx,
                ),
            ))
            .child(spaced(space::MD, once))
            .child(spaced(space::MD, new_session))
            .into_any_element()
    }

    fn schedule_fields(&self, cx: &mut Context<Self>) -> AnyElement {
        let days = Field::new(
            "routine-days-field",
            "Days",
            Self::choice("routine-days", &DAYS, self.days, cx, |this, at| {
                this.days = at
            }),
        );
        let hour = Field::new(
            "routine-hour",
            "At",
            input_frame("routine-hour-box", &self.f().hour, cx),
        )
        .hint("In your time zone, as 08:30.")
        .error(self.hour_problem(cx));
        stack(space::MD)
            .child(spaced(space::MD, days))
            .child(spaced(space::MD, hour))
            .into_any_element()
    }

    /// The one sentence that says when the routine runs next.
    fn next_sentence(&self, cx: &Context<Self>) -> Option<String> {
        match self.trigger {
            Trigger::Event => Some("Runs when it happens.".into()),
            Trigger::Schedule => {
                let hour = hour_of(&Self::typed(&self.f().hour, cx))?;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis() as u64);
                let at = next_run(DAYS[self.days].0, hour, now)?;
                Some(format!("Next: {}", next_said(at, now)))
            }
        }
    }
}

impl Render for RoutineForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Closed for good until the first time it opens: nothing to draw yet.
        if self.fields.is_none() {
            return div().into_any_element();
        }
        let p = palette(cx);
        let this = cx.entity().downgrade();
        let projects: Vec<String> = self
            .shell
            .upgrade()
            .map(|shell| {
                shell
                    .read(cx)
                    .snapshot()
                    .projects
                    .iter()
                    .map(|project| project.name.clone())
                    .collect()
            })
            .unwrap_or_default();
        let project_at = projects
            .iter()
            .position(|name| *name == self.chosen_project)
            .unwrap_or(0);
        let project = {
            let (this, names) = (cx.entity().downgrade(), projects.clone());
            Select::new(
                "routine-project",
                projects
                    .iter()
                    .map(|name| SelectOption::new(name.replace(' ', "-"), name.clone())),
                project_at,
            )
            .on_choose(move |at, _, cx| {
                this.update(cx, |this, cx| {
                    this.chosen_project = names[at].clone();
                    cx.notify();
                })
                .ok();
            })
        };
        let model = {
            let (entries, chosen) = match self.shell.upgrade() {
                Some(shell) => {
                    let providers = shell.read(cx).providers();
                    (providers.model_entries(), self.model.clone())
                }
                None => (Vec::new(), Chosen::First),
            };
            let this = cx.entity().downgrade();
            ModelChoice::new("routine-model", "Agent", chosen)
                .follows()
                .models(entries)
                .effort(self.effort.clone().map(Into::into))
                .on_choose(move |chosen, effort, _, cx| {
                    this.update(cx, |this, cx| {
                        this.model = chosen;
                        this.effort = effort.map(|e| e.to_string());
                        cx.notify();
                    })
                    .ok();
                })
        };
        let trigger = {
            let this = cx.entity().downgrade();
            RadioChoices::new("routine-trigger-choice", "Starts")
                .choice(
                    "schedule",
                    "On a schedule",
                    "Days and an hour, in your time zone.",
                )
                .choice(
                    "event",
                    "On an event",
                    "When something happens that Flottille watches.",
                )
                .chosen(usize::from(self.trigger == Trigger::Event))
                .on_choose(move |at, _, cx| {
                    this.update(cx, |this, cx| {
                        this.trigger = if at == 0 {
                            Trigger::Schedule
                        } else {
                            Trigger::Event
                        };
                        cx.notify();
                    })
                    .ok();
                })
        };
        let mode = {
            let this = cx.entity().downgrade();
            let mut radios = RadioChoices::new("routine-mode-choice", "Mode");
            for mode in MODES {
                radios = radios.choice(mode, mode_said(mode), mode_line(mode));
            }
            radios.chosen(self.mode).on_choose(move |at, _, cx| {
                this.update(cx, |this, cx| {
                    this.mode = at;
                    cx.notify();
                })
                .ok();
            })
        };
        let rings = {
            let this = cx.entity().downgrade();
            RadioChoices::new("routine-rings-choice", "Where the result rings")
                .choice(
                    "something",
                    "When it has something to say",
                    "A notification, with its summary.",
                )
                .choice(
                    "new",
                    "Only when something is new",
                    "A notification when its list has an item the last run's had not.",
                )
                .choice("never", "Never", "Its history only.")
                .chosen(self.rings)
                .on_choose(move |at, _, cx| {
                    this.update(cx, |this, cx| {
                        this.rings = at;
                        cx.notify();
                    })
                    .ok();
                })
        };
        let said = self.next_sentence(cx).unwrap_or_default();
        let next = changing("routine-next-text", SharedString::from(said), move |said| {
            div()
                .id("routine-next")
                .aria_label(said.clone())
                .text_size(px(12.5))
                .text_color(p.mute)
                .child(said)
                .test_support()
                .into_any_element()
        });
        let prompt = Field::new(
            "routine-prompt-field",
            "Prompt",
            div()
                .id("routine-prompt-box")
                .min_h(px(84.))
                .px(space::MD)
                .py(space::SM)
                .rounded(px(10.))
                .bg(p.field)
                .border_1()
                .border_color(p.line2)
                .text_size(px(13.))
                .line_height(px(20.))
                .child(self.f().prompt.clone())
                .test_support(),
        )
        .hint("Your own words: the first message of each run.");
        let schedule = self.trigger == Trigger::Schedule;
        let body = stack(space::MD)
            .child(spaced(
                space::MD,
                Field::new(
                    "routine-name",
                    "Name",
                    input_frame("routine-name-box", &self.f().name, cx),
                ),
            ))
            .child(spaced(
                space::MD,
                Field::new("routine-project-field", "Project", project),
            ))
            .child(spaced(space::MD, prompt))
            .child(spaced(space::MD, model))
            .child(spaced(space::MD, trigger))
            .child(
                presence("routine-schedule-block", schedule, self.schedule_fields(cx))
                    .space(space::MD),
            )
            .child(
                presence("routine-event-block", !schedule, self.event_fields(cx)).space(space::MD),
            )
            .child(spaced(space::MD, mode))
            .child(spaced(space::MD, rings))
            .child(spaced(space::MD, next));
        let ready = self.ready(cx);
        let saving = self.saving;
        let editing = self.editing.is_some();
        Modal::new(
            "routine-form",
            if editing {
                "Edit routine"
            } else {
                "New routine"
            },
        )
        .sentence("A prompt that starts a session by itself, and where its result goes.")
        .wide()
        .open(self.open)
        .on_close(move |_, cx| {
            this.update(cx, |this, cx| this.close(cx)).ok();
        })
        .child(body)
        .action(
            Button::primary(
                "routine-save",
                match (saving, editing) {
                    (true, _) => "Saving…",
                    (false, true) => "Save",
                    (false, false) => "Create",
                },
            )
            .disabled(!ready)
            .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
        )
        .into_any_element()
    }
}
