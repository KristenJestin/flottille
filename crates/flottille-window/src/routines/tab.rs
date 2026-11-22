//! The Routines tab of a project's page: the list of the project's routines, a routine's page
//! with its history, and a run's result. The three are pages of one pager: the routine's page
//! slides in from the list, the result's from the routine's. What moves: the pages slide, a row
//! of a list unfolds when it arrives and folds away when it goes, the unread dot fades, a page's
//! blocks enter one after another, the items of a result enter one after another with their
//! group, which unfolds, and what left since the last run unfolds at the foot, the status of a run
//! whose work goes on unfolds when it is read, and every button, row and link fades its hover.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_design_system::color::mix;
use gpui_design_system::motion::HOVER;
use gpui_design_system::{
    Button, Confirm, EmptyLine, Group, Icon, LINE_GAP, LabelledLine, ListCard, ListRow, Listing,
    Menu, PageHeader, Section, Skeleton, Toast, Type as _, awaiting, enter, fade, hoverable, icon,
    palette, presence, presence_of, sliding, space, toast,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, TestSupportExt as _,
    WeakEntity, Window, div, px,
};
use serde_json::{Value, json};

use super::{Item, Routine, RoutineForm, Run, Status, mode_line, mode_said};
use crate::actions::Command;
use crate::commands::{perform_later, spaced, stack};
use crate::extensions::Then as Extension;
use crate::session::{MarkdownText, said_at};
use crate::shell::{Selection, Shell};

/// What a link of the path line does.
type Go = Box<dyn Fn(&mut Window, &mut App)>;

/// What the daemon's answers to the tab's requests lead to.
#[derive(Clone, Debug, PartialEq)]
pub enum Then {
    /// `routine.list`, the read `asked`.
    Listed { project: String, asked: u64 },
    /// `routine.show` of the routine `routine`: its history.
    Shown {
        project: String,
        routine: String,
        asked: u64,
    },
    /// A change the user asked for: `run`, `pause`, `resume`, `remove` or `open`.
    Acted {
        project: String,
        what: &'static str,
        name: String,
    },
    /// The form's save, of a routine made or edited.
    Saved { project: String, name: String },
}

impl Then {
    /// The project whose tab the answer is for.
    pub fn project(&self) -> &str {
        match self {
            Then::Listed { project, .. }
            | Then::Shown { project, .. }
            | Then::Acted { project, .. }
            | Then::Saved { project, .. } => project,
        }
    }
}

/// Which page of the tab is shown.
#[derive(Clone, Debug, PartialEq)]
enum Where {
    List,
    /// A routine's page, by its id.
    Routine(String),
    /// A run's result: the routine's id and the run's.
    Result(String, u64),
}

impl Where {
    fn index(&self) -> usize {
        match self {
            Where::List => 0,
            Where::Routine(_) => 1,
            Where::Result(..) => 2,
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

pub struct RoutinesTab {
    shell: WeakEntity<Shell>,
    project: String,
    /// The reads numbered, so that an answer older than one applied is dropped.
    asked: u64,
    applied: u64,
    /// The routines of the project, once read.
    read: Option<Result<Vec<Routine>, String>>,
    /// The runs of each routine, last first, by its id, once its page was read.
    history: HashMap<String, Vec<Run>>,
    history_asked: u64,
    at: Where,
    rows: Listing<()>,
    runs: Listing<()>,
    form: Entity<RoutineForm>,
    menu: Entity<Menu>,
    /// The routine asked to be removed, by its id.
    removing: Option<String>,
    /// What Flottille knows of the session of the run whose result is shown, once read.
    status: Option<(u64, Result<Status, String>)>,
    status_asked: u64,
    /// The branch the project's folder is on: what a run in the folder says of its branch.
    branch: Option<String>,
    /// The report of the result shown, drawn as a reply is.
    report: Option<(u64, String, MarkdownText)>,
    /// A notification's run to open once the list read after it came: the run's session, its
    /// routine's name, and the read that settles it.
    waiting: Option<(String, String, u64)>,
}

impl RoutinesTab {
    pub fn new(shell: WeakEntity<Shell>, project: String, cx: &mut Context<Self>) -> Self {
        let form = {
            let (shell, project) = (shell.clone(), project.clone());
            cx.new(|_| RoutineForm::new(shell, project))
        };
        let menu = {
            let (edit, remove) = (cx.entity().downgrade(), cx.entity().downgrade());
            cx.new(|cx| {
                Menu::new("routine-more", "More", cx)
                    .entry(
                        "routine-edit",
                        "Edit",
                        Some(Icon::Edit),
                        move |window, cx| {
                            edit.update(cx, |tab, cx| tab.edit(window, cx)).ok();
                        },
                    )
                    .danger("routine-remove", "Remove…", move |_, cx| {
                        remove.update(cx, |tab, cx| tab.ask_to_remove(cx)).ok();
                    })
            })
        };
        Self {
            shell,
            project,
            asked: 0,
            applied: 0,
            read: None,
            history: HashMap::new(),
            history_asked: 0,
            at: Where::List,
            rows: Listing::default(),
            runs: Listing::default(),
            form,
            menu,
            removing: None,
            status: None,
            status_asked: 0,
            branch: None,
            report: None,
            waiting: None,
        }
    }

    pub fn form(&self) -> &Entity<RoutineForm> {
        &self.form
    }

    /// How many results with something to say were not opened: the tab's count.
    pub fn unread(&self) -> usize {
        match &self.read {
            Some(Ok(routines)) => routines.iter().map(|r| r.unread as usize).sum(),
            _ => 0,
        }
    }

    /// The routines of the project, once read.
    pub fn routines(&self) -> &[Routine] {
        match &self.read {
            Some(Ok(routines)) => routines,
            _ => &[],
        }
    }

    /// The branch the project's folder is on, which a run's status says when its session works
    /// there.
    pub fn set_branch(&mut self, branch: Option<String>, cx: &mut Context<Self>) {
        if self.branch != branch {
            self.branch = branch;
            cx.notify();
        }
    }

    /// Asks the daemon for the routines, and for the history of the one whose page is shown.
    pub fn read(&mut self, cx: &mut App) {
        self.asked += 1;
        let then = Extension::Routines(Then::Listed {
            project: self.project.clone(),
            asked: self.asked,
        });
        self.ask("routine.list", json!({"project": self.project}), then, cx);
        if let Where::Routine(id) | Where::Result(id, _) = self.at.clone() {
            self.read_history(&id, cx);
        }
    }

    fn read_history(&mut self, id: &str, cx: &mut App) {
        self.history_asked += 1;
        let then = Extension::Routines(Then::Shown {
            project: self.project.clone(),
            routine: id.to_owned(),
            asked: self.history_asked,
        });
        self.ask("routine.show", json!({"routine": id}), then, cx);
    }

    fn ask(&self, method: &str, params: Value, then: Extension, cx: &mut App) {
        perform_later(
            &self.shell,
            Command::Ask {
                method: method.to_owned(),
                params,
                then,
            },
            cx,
        );
    }

    /// An event says a routine of the project changed, or a run began, ended or was opened.
    pub fn changed(&mut self, cx: &mut Context<Self>) {
        self.read(cx);
        if let Where::Result(routine, run) = self.at.clone() {
            self.read_status(&routine, run, cx);
        }
    }

    /// What the daemon answered to a request of the tab.
    pub fn answered(&mut self, then: Then, answer: Result<Value, String>, cx: &mut Context<Self>) {
        match then {
            Then::Listed { asked, .. } => {
                if asked < self.applied {
                    return;
                }
                self.applied = asked;
                let read = answer.map(|answer| {
                    answer["routines"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Routine::of)
                        .collect::<Vec<_>>()
                });
                if self.read.as_ref() != Some(&read) {
                    self.read = Some(read);
                    cx.notify();
                }
                self.open_waiting(cx);
            }
            Then::Shown { routine, asked, .. } => {
                if asked < self.history_asked {
                    return;
                }
                if let Ok(answer) = answer {
                    let runs: Vec<Run> = answer["runs"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Run::of)
                        .collect();
                    if self.history.get(&routine) != Some(&runs) {
                        self.history.insert(routine, runs);
                        cx.notify();
                    }
                }
            }
            Then::Acted { what, name, .. } => {
                if let Err(why) = answer {
                    let title = match what {
                        "run" => format!("{name} did not start"),
                        "remove" => format!("{name} was not removed"),
                        _ => format!("{name} was not changed"),
                    };
                    toast(Toast::failure(title).detail(why), cx);
                }
            }
            Then::Saved { name, .. } => match answer {
                Ok(_) => {
                    self.form.update(cx, |form, cx| form.saved(cx));
                    toast(Toast::success(format!("{name} is kept")), cx);
                }
                Err(why) => {
                    self.form.update(cx, |form, cx| form.refused(cx));
                    toast(
                        Toast::failure(format!("{name} was not kept")).detail(why),
                        cx,
                    );
                }
            },
        }
    }

    /// What was read of a run's session, once the daemon answered.
    pub fn statused(
        &mut self,
        run: u64,
        asked: u64,
        read: Result<Status, String>,
        cx: &mut Context<Self>,
    ) {
        if asked < self.status_asked {
            return;
        }
        if self.status.as_ref().map(|(r, s)| (*r, s)) != Some((run, &read)) {
            self.status = Some((run, read));
            cx.notify();
        }
    }

    fn routine(&self, id: &str) -> Option<&Routine> {
        self.routines().iter().find(|routine| routine.id == id)
    }

    fn open_routine(&mut self, id: &str, cx: &mut Context<Self>) {
        self.at = Where::Routine(id.to_owned());
        self.read_history(id, cx);
        cx.notify();
    }

    /// Opens the result of the run a notification is about, known by its session and its
    /// routine's name: it is found in a list read now. A run that is no longer its routine's last
    /// opens the routine's page, which lists them all.
    pub fn open_run_of(&mut self, session: &str, name: &str, cx: &mut Context<Self>) {
        self.read(cx);
        self.waiting = Some((session.to_owned(), name.to_owned(), self.asked));
    }

    fn open_waiting(&mut self, cx: &mut Context<Self>) {
        let Some((session, name, asked)) = self.waiting.clone() else {
            return;
        };
        if self.applied < asked {
            return;
        }
        self.waiting = None;
        let Some(Ok(routines)) = &self.read else {
            return;
        };
        let last = routines.iter().find_map(|routine| {
            let run = routine.last.as_ref()?;
            (run.session.as_deref() == Some(session.as_str())).then(|| (routine.id.clone(), run.id))
        });
        let named = routines
            .iter()
            .find(|routine| routine.name == name)
            .map(|routine| routine.id.clone());
        match (last, named) {
            (Some((routine, run)), _) => self.open_result(&routine, run, cx),
            (None, Some(id)) => self.open_routine(&id, cx),
            (None, None) => {}
        }
    }

    /// Opens a run's result: it is read by the user from now on, which the daemon is told.
    pub fn open_result(&mut self, routine: &str, run: u64, cx: &mut Context<Self>) {
        self.at = Where::Result(routine.to_owned(), run);
        self.status = None;
        if !self.history.contains_key(routine) {
            self.read_history(routine, cx);
        }
        let name = self
            .routine(routine)
            .map_or_else(|| routine.to_owned(), |r| r.name.clone());
        let unread = self
            .history
            .get(routine)
            .and_then(|runs| runs.iter().find(|r| r.id == run))
            .is_some_and(Run::unread)
            || self
                .routine(routine)
                .and_then(|r| r.last.as_ref())
                .is_some_and(|last| last.id == run && last.unread());
        if unread {
            self.mark_opened(routine, run);
            let then = Extension::Routines(Then::Acted {
                project: self.project.clone(),
                what: "open",
                name,
            });
            self.ask("routine.open", json!({"run": run}), then, cx);
        }
        self.read_status(routine, run, cx);
        cx.notify();
    }

    /// The result opened is read at once here: its dot fades while the daemon is told.
    fn mark_opened(&mut self, routine: &str, run: u64) {
        if let Some(runs) = self.history.get_mut(routine) {
            for found in runs.iter_mut().filter(|r| r.id == run) {
                found.opened = true;
            }
        }
        if let Some(Ok(routines)) = &mut self.read {
            for found in routines.iter_mut().filter(|r| r.id == routine) {
                if let Some(last) = found.last.as_mut().filter(|l| l.id == run && !l.opened) {
                    last.opened = true;
                    found.unread = found.unread.saturating_sub(1);
                }
            }
        }
    }

    /// Reads what Flottille knows of the session of a run whose work goes on.
    fn read_status(&mut self, routine: &str, run: u64, cx: &mut Context<Self>) {
        let Some(found) = self
            .history
            .get(routine)
            .and_then(|runs| runs.iter().find(|r| r.id == run))
        else {
            return;
        };
        let (Some(session), true) = (
            found.session.clone(),
            found.goes_on() || found.outcome == "running",
        ) else {
            return;
        };
        self.status_asked += 1;
        let (project, asked, branch) =
            (self.project.clone(), self.status_asked, self.branch.clone());
        perform_later(
            &self.shell,
            Command::RunStatus {
                project,
                session,
                branch,
                run,
                asked,
            },
            cx,
        );
    }

    fn run_now(&mut self, id: &str, cx: &mut Context<Self>) {
        let name = self
            .routine(id)
            .map_or_else(String::new, |r| r.name.clone());
        let then = Extension::Routines(Then::Acted {
            project: self.project.clone(),
            what: "run",
            name,
        });
        self.ask("routine.run", json!({"routine": id}), then, cx);
    }

    fn pause_or_resume(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(routine) = self.routine(id) else {
            return;
        };
        let (what, method) = match routine.paused {
            true => ("resume", "routine.resume"),
            false => ("pause", "routine.pause"),
        };
        let then = Extension::Routines(Then::Acted {
            project: self.project.clone(),
            what,
            name: routine.name.clone(),
        });
        self.ask(method, json!({"routine": id}), then, cx);
    }

    fn edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Where::Routine(id) | Where::Result(id, _)) = self.at.clone() else {
            return;
        };
        if let Some(routine) = self.routine(&id).cloned() {
            self.form
                .update(cx, |form, cx| form.open_edit(&routine, window, cx));
        }
    }

    fn new_routine(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.form.update(cx, |form, cx| form.open_new(window, cx));
    }

    fn ask_to_remove(&mut self, cx: &mut Context<Self>) {
        if let Where::Routine(id) | Where::Result(id, _) = &self.at {
            self.removing = Some(id.clone());
            cx.notify();
        }
    }

    fn remove(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.removing.take() else {
            return;
        };
        let name = self
            .routine(&id)
            .map_or_else(String::new, |r| r.name.clone());
        let then = Extension::Routines(Then::Acted {
            project: self.project.clone(),
            what: "remove",
            name,
        });
        self.ask("routine.remove", json!({"routine": id}), then, cx);
        self.at = Where::List;
        self.history.remove(&id);
        cx.notify();
    }

    fn open_session(&mut self, session: String, window: &mut Window, cx: &mut Context<Self>) {
        self.shell
            .update(cx, |shell, cx| {
                shell.open(Selection::Session(session), window, cx)
            })
            .ok();
    }

    /// The path line above a routine's page and a result: `Routines / Tickets to review`, whose
    /// words that lead back are links.
    fn path(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = palette(cx);
        let name = match &self.at {
            Where::List => None,
            Where::Routine(id) | Where::Result(id, _) => self.routine(id).map(|r| r.name.clone()),
        };
        let run = match &self.at {
            Where::Result(routine, run) => self
                .history
                .get(routine)
                .and_then(|runs| runs.iter().find(|r| r.id == *run))
                .map(|r| said_at(r.started_at)),
            _ => None,
        };
        let said = [Some("Routines".to_owned()), name.clone(), run.clone()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" / ");
        let link = |id: &'static str, words: String, on_click: Go| {
            hoverable(
                div()
                    .id(id)
                    .aria_label(SharedString::from(words.clone()))
                    .cursor_pointer()
                    .child(SharedString::from(words))
                    .on_click(move |_, window, cx| on_click(window, cx)),
                move |d, hover| d.text_color(mix(p.faint, p.text, hover)),
            )
        };
        let list = cx.entity().downgrade();
        let routine = cx.entity().downgrade();
        let to_routine = match &self.at {
            Where::Result(id, _) => Some(id.clone()),
            _ => None,
        };
        let line = div()
            .id("routines-path")
            .aria_label(SharedString::from(said))
            .flex()
            .items_baseline()
            .gap(space::SM)
            .mono(12.)
            .text_color(p.faint)
            .whitespace_nowrap()
            .overflow_hidden()
            .text_ellipsis()
            .child(link(
                "routines-back",
                "Routines".into(),
                Box::new(move |_, cx| {
                    list.update(cx, |tab, cx| {
                        tab.at = Where::List;
                        tab.status = None;
                        cx.notify();
                    })
                    .ok();
                }),
            ));
        let line = match (name, to_routine) {
            (Some(name), Some(id)) => line.child("/").child(link(
                "routines-back-to-routine",
                name,
                Box::new(move |_, cx| {
                    routine
                        .update(cx, |tab, cx| {
                            tab.at = Where::Routine(id.clone());
                            tab.status = None;
                            cx.notify();
                        })
                        .ok();
                }),
            )),
            (Some(name), None) => line
                .child("/")
                .child(div().text_color(p.text).child(SharedString::from(name))),
            (None, _) => line,
        };
        match run {
            Some(run) => line
                .child("/")
                .child(div().text_color(p.text).child(SharedString::from(run))),
            None => line,
        }
        .test_support()
    }

    // The list.

    fn dot(
        &self,
        id: &str,
        unread: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = palette(cx);
        let shown = fade(
            ElementId::Name(format!("routine-{id}-dot-fade").into()),
            unread,
            HOVER,
            window,
            cx,
        );
        let name = id.to_owned();
        div()
            .relative()
            .size(px(16.))
            .child(icon(Icon::Clock).text_color(p.mute))
            .when(shown > 0., |d| {
                d.child(
                    div()
                        .id(ElementId::Name(format!("routine-{name}-dot").into()))
                        .aria_label("Unread result")
                        .aria_value(format!("{shown}"))
                        .absolute()
                        .top(px(-2.))
                        .right(px(-2.))
                        .size(px(8.))
                        .rounded_full()
                        .bg(p.acc.opacity(shown))
                        .test_support(),
                )
            })
            .into_any_element()
    }

    fn row(&self, routine: &Routine, window: &mut Window, cx: &mut Context<Self>) -> ListRow {
        let p = palette(cx);
        let id = routine.id.clone();
        let last = match &routine.last {
            Some(last) => format!("{} · {}", said_at(last.started_at), last.said()),
            None => "Not run yet".to_owned(),
        };
        let row_id = format!("routine-{}", routine.name);
        let meta = div()
            .id(ElementId::Name(format!("{row_id}-meta").into()))
            .aria_label(SharedString::from(last.clone()))
            .mono(11.)
            .text_color(p.faint)
            .whitespace_nowrap()
            .overflow_hidden()
            .text_ellipsis()
            .child(SharedString::from(last))
            .test_support();
        let unread = routine.unread > 0;
        let leading = self.dot(&routine.name, unread, window, cx);
        ListRow::new(ElementId::Name(row_id.into()), routine.name.clone())
            .leading(leading)
            .tag(mode_said(&routine.mode))
            .line(routine.trigger())
            .trailing(meta)
            .on_click(cx.listener(move |this, _, _, cx| this.open_routine(&id, cx)))
    }

    fn list(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let routines: Vec<Routine> = self.routines().to_vec();
        let (now, reduce) = (cx.background_executor().now(), cx.reduce_motion());
        self.rows
            .sync(routines.iter().map(|r| (r.id.clone(), ())), now, reduce);
        let drawn: Vec<_> = self
            .rows
            .rows()
            .map(|row| (row.key.to_owned(), row.placement))
            .collect();
        let mut card = ListCard::new("routines-list");
        for (key, placement) in drawn {
            // A routine that left folds away from its last row.
            if let Some(routine) = routines.iter().find(|r| r.id == key) {
                let row = self.row(routine, window, cx);
                card = card.item(
                    ElementId::Name(format!("routine-item-{key}").into()),
                    placement,
                    row,
                );
            }
        }
        let loaded = matches!(self.read, Some(Ok(_)));
        let empty = loaded && routines.is_empty();
        let new = Button::secondary("routines-new", "New routine")
            .leading(Icon::Plus)
            .on_click(cx.listener(|this, _, window, cx| this.new_routine(window, cx)));
        let none = EmptyLine::new(
            "routines-empty",
            Icon::Clock,
            "A routine is a prompt that starts a session by itself, on a schedule or when \
             something happens, and tells you what it found. Nothing starts until you add one.",
        )
        .action(
            Button::secondary("routines-empty-new", "New routine")
                .leading(Icon::Plus)
                .on_click(cx.listener(|this, _, window, cx| this.new_routine(window, cx))),
        );
        let failed = match &self.read {
            Some(Err(why)) => Some(SharedString::from(why.clone())),
            _ => None,
        };
        let p = palette(cx);
        let content = stack(space::MD)
            .child(
                presence("routines-none-block", empty && failed.is_none(), none).space(space::MD),
            )
            .child(presence("routines-list-block", loaded && !empty, card).space(space::MD))
            .child(
                presence_of("routines-failed-block", failed, move |why| {
                    div()
                        .id("routines-failed")
                        .aria_label(why.clone())
                        .text_size(px(12.5))
                        .text_color(p.fail)
                        .child(why)
                        .test_support()
                        .into_any_element()
                })
                .space(space::MD),
            );
        let section = Section::new("routines-section", "Routines")
            .sentence("What starts by itself in this project, and where its results are.")
            .when(loaded && !empty, |section| {
                section.count(routines.len().to_string()).action(new)
            })
            .child(awaiting(
                "routines-waiting",
                self.read.is_none(),
                Skeleton::new("routines-skeleton").h(px(64.)),
                content,
            ));
        div()
            .id("routines")
            .child(section)
            .test_support()
            .into_any_element()
    }

    // A routine's page.

    fn run_row(&self, routine: &Routine, run: &Run, cx: &mut Context<Self>) -> ListRow {
        let p = palette(cx);
        let (id, run_id) = (routine.id.clone(), run.id);
        let row_id = format!("routine-run-{}", run.id);
        let dot = run.unread();
        let duration = run
            .ended_at
            .map(|end| crate::session::seconds(end.saturating_sub(run.started_at)));
        let said = run.said();
        let mut row = ListRow::new(
            ElementId::Name(row_id.clone().into()),
            said_at(run.started_at),
        )
        .line(said)
        .on_click(cx.listener(move |this, _, _, cx| this.open_result(&id, run_id, cx)));
        if let Some(duration) = duration {
            row = row.trailing(
                div()
                    .id(ElementId::Name(format!("{row_id}-duration").into()))
                    .mono(11.)
                    .text_color(p.faint)
                    .child(SharedString::from(duration))
                    .test_support(),
            );
        }
        if dot {
            row = row.tag("Unread");
        }
        row
    }

    fn page(&mut self, id: &str, cx: &mut Context<Self>) -> AnyElement {
        let Some(routine) = self.routine(id).cloned() else {
            return div().into_any_element();
        };
        let p = palette(cx);
        let now = now_ms();
        let runs: Vec<Run> = self.history.get(id).cloned().unwrap_or_default();
        let (at, reduce) = (cx.background_executor().now(), cx.reduce_motion());
        self.runs
            .sync(runs.iter().map(|r| (r.id.to_string(), ())), at, reduce);
        let drawn: Vec<_> = self
            .runs
            .rows()
            .map(|row| (row.key.to_owned(), row.placement))
            .collect();
        let mut card = ListCard::new("routine-history-list");
        for (key, placement) in drawn {
            if let Some(run) = runs.iter().find(|r| r.id.to_string() == key) {
                let row = self.run_row(&routine, run, cx);
                card = card.item(
                    ElementId::Name(format!("routine-run-item-{key}").into()),
                    placement,
                    row,
                );
            }
        }
        let header = {
            let (run, pause) = (routine.id.clone(), routine.id.clone());
            let (run_tab, pause_tab) = (cx.entity().downgrade(), cx.entity().downgrade());
            PageHeader::new("routine-header", routine.name.clone())
                .action("routine-run", "Run now", Some(Icon::Play), move |_, cx| {
                    run_tab.update(cx, |tab, cx| tab.run_now(&run, cx)).ok();
                })
                .action(
                    "routine-pause",
                    if routine.paused { "Resume" } else { "Pause" },
                    Some(if routine.paused {
                        Icon::Restart
                    } else {
                        Icon::Stopped
                    }),
                    move |_, cx| {
                        pause_tab
                            .update(cx, |tab, cx| tab.pause_or_resume(&pause, cx))
                            .ok();
                    },
                )
                .more(self.menu.clone())
        };
        let fact = |id: &'static str, glyph: Icon, label: &'static str, said: String| {
            spaced(
                LINE_GAP,
                LabelledLine::new(id, glyph, label, SharedString::from(said.clone()))
                    .child(SharedString::from(said)),
            )
        };
        let next = routine.next(now);
        let facts = stack(LINE_GAP)
            .child(fact(
                "routine-trigger",
                Icon::Clock,
                "Trigger",
                routine.trigger(),
            ))
            .child(fact(
                "routine-mode",
                Icon::Terminal,
                "Mode",
                format!(
                    "{} · {}",
                    mode_said(&routine.mode),
                    mode_line(&routine.mode)
                ),
            ))
            .child(fact(
                "routine-agent",
                Icon::Service,
                "Agent",
                routine.agent(),
            ))
            .child(fact(
                "routine-rings",
                Icon::Bell,
                "Rings",
                match routine.rings.as_str() {
                    "never" => "Never: its history only".to_owned(),
                    "new" => "When an item of its list is new".to_owned(),
                    _ => "When it has something to say".to_owned(),
                },
            ));
        let prompt = div()
            .id("routine-prompt")
            .aria_label(SharedString::from(routine.prompt.clone()))
            .p(space::MD)
            .rounded(px(10.))
            .bg(p.card)
            .border_1()
            .border_color(p.line)
            .text_size(px(12.5))
            .text_color(p.text)
            .child(SharedString::from(routine.prompt.clone()))
            .test_support();
        let next_line = div()
            .id("routine-next")
            .aria_label(SharedString::from(next.clone()))
            .text_size(px(12.5))
            .text_color(p.mute)
            .child(SharedString::from(next))
            .test_support();
        let history = if runs.is_empty() {
            EmptyLine::new(
                "routine-history-empty",
                Icon::Clock,
                "No run yet: its first one will show here, with what it found.",
            )
            .into_any_element()
        } else {
            card.into_any_element()
        };
        let count = runs.len();
        let column = div()
            .flex()
            .flex_col()
            .gap(space::LG)
            .child(enter("routine-enter-path", 0, self.path(cx)))
            .child(enter("routine-enter-header", 1, header))
            .child(enter(
                "routine-enter-facts",
                2,
                div()
                    .flex()
                    .flex_col()
                    .gap(space::MD)
                    .child(facts)
                    .child(prompt)
                    .child(next_line),
            ))
            .child(enter(
                "routine-enter-history",
                3,
                Section::new("routine-history", "History")
                    .count(count.to_string())
                    .sentence("Each run is a session, kept when it ended.")
                    .child(history),
            ));
        div()
            .id("routine-page")
            .child(column)
            .test_support()
            .into_any_element()
    }

    // A result.

    fn result(&mut self, routine_id: &str, run_id: u64, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let Some(routine) = self.routine(routine_id).cloned() else {
            return div().into_any_element();
        };
        let run = self
            .history
            .get(routine_id)
            .and_then(|runs| runs.iter().find(|r| r.id == run_id))
            .cloned()
            .or_else(|| routine.last.clone().filter(|last| last.id == run_id));
        let Some(run) = run else {
            return div()
                .child(enter("result-enter-path", 0, self.path(cx)))
                .into_any_element();
        };
        let summary = run.said();
        // The report is the markdown the result gave beside its items, else its summary, drawn as
        // a reply is: the conversation stays on the session's page.
        let report_text = match (&run.body, &run.summary, &run.why) {
            (Some(body), _, _) => body.clone(),
            (None, Some(summary), _) => summary.clone(),
            (None, None, Some(why)) => why.clone(),
            (None, None, None) => String::new(),
        };
        if self
            .report
            .as_ref()
            .is_none_or(|(id, text, _)| *id != run.id || *text != report_text)
        {
            let drawn = MarkdownText::new(&report_text, cx);
            self.report = Some((run.id, report_text.clone(), drawn));
        }
        let report = self
            .report
            .as_ref()
            .map(|(_, _, drawn)| drawn.view("result-report-text"));
        let head = div()
            .flex()
            .flex_col()
            .gap(space::XS)
            .child(
                div()
                    .id("result-title")
                    .aria_label(SharedString::from(routine.name.clone()))
                    .text_size(px(22.))
                    .text_color(p.text)
                    .child(SharedString::from(routine.name.clone())),
            )
            .child(
                div()
                    .id("result-when")
                    .aria_label(SharedString::from(said_at(run.started_at)))
                    .mono(12.)
                    .text_color(p.faint)
                    .child(SharedString::from(said_at(run.started_at))),
            );
        let summary_line = div()
            .id("result-summary")
            .aria_label(SharedString::from(summary.clone()))
            .text_size(px(15.))
            .text_color(p.text)
            .child(SharedString::from(summary))
            .test_support();
        let report = div()
            .id("result-report")
            .aria_label(SharedString::from(report_text))
            .p(space::LG)
            .rounded(px(12.))
            .bg(p.card)
            .border_1()
            .border_color(p.line)
            .children(report)
            .test_support();
        let session = run.session.clone();
        let session_block = match session {
            Some(session) => Button::secondary("result-open-session", "Open the session")
                .leading(Icon::OpenInEditor)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_session(session.clone(), window, cx)
                }))
                .into_any_element(),
            None => div()
                .id("result-no-session")
                .aria_label("This run was skipped: it has no session.")
                .text_size(px(12.5))
                .text_color(p.mute)
                .child("This run was skipped, so it has no session.")
                .test_support()
                .into_any_element(),
        };
        let status = self.status_block(&run, cx);
        let column = div()
            .flex()
            .flex_col()
            .gap(space::LG)
            .child(enter("result-enter-path", 0, self.path(cx)))
            .child(enter("result-enter-head", 1, head))
            .child(enter("result-enter-summary", 2, summary_line))
            .child(enter("result-enter-report", 3, report))
            .child(self.items_block(&run, cx))
            .child(enter("result-enter-session", 5, session_block))
            .child(status);
        div()
            .id("result-page")
            .child(column)
            .test_support()
            .into_any_element()
    }

    /// A result's list: its items as rows under their group headings, in the agent's order, the
    /// new ones marked, and at the foot what left since the last run. Each row enters after the
    /// one before it, and a group unfolds as it arrives.
    fn items_block(&self, run: &Run, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let left = (!run.left.is_empty()).then(|| {
            SharedString::from(format!("Left since the last run: {}", run.left.join(", ")))
        });
        let mut column = stack(space::MD);
        let mut at = 0;
        for (n, (group, items)) in run.sections().into_iter().enumerate() {
            let mut rows = div().w_full().flex().flex_col().gap(px(2.));
            let count = items.len();
            for item in items {
                rows = rows.child(self.item_row(run.id, item, at, cx));
                at += 1;
            }
            column = column.child(match group {
                Some(group) => Group::new(
                    ElementId::Name(format!("result-group-{group}").into()),
                    group.to_owned(),
                    count,
                )
                .child(rows)
                .into_any_element(),
                None => div()
                    .id(format!("result-ungrouped-{n}"))
                    .child(rows)
                    .into_any_element(),
            });
        }
        let said = left.clone();
        let foot = presence_of(
            ElementId::Name(format!("result-left-block-{}", run.id).into()),
            said,
            move |said| {
                div()
                    .id("result-left")
                    .aria_label(said.clone())
                    .text_size(px(12.5))
                    .text_color(p.mute)
                    .child(said)
                    .test_support()
                    .into_any_element()
            },
        )
        .space(space::MD);
        let shown = !run.items.is_empty();
        let items = Section::new("result-items", "Items")
            .count(run.items.len().to_string())
            .sentence("In the order the agent gave them.")
            .child(column);
        div()
            .id("result-items-block")
            .flex()
            .flex_col()
            .child(presence(
                ElementId::Name(format!("result-items-presence-{}", run.id).into()),
                shown,
                items,
            ))
            .child(foot)
            .into_any_element()
    }

    /// One item: its title, its sentence under it, its tags as chips (`New` first), and its link,
    /// which the system's handler opens as it is; the window never fetches it.
    fn item_row(&self, run: u64, item: &Item, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let id = format!("result-item-{}", item.id);
        let mut row = ListRow::new(ElementId::Name(id.clone().into()), item.title.clone());
        if let Some(text) = &item.text {
            row = row.line(text.clone());
        }
        if item.new {
            row = row.tag("New");
        }
        for tag in &item.tags {
            row = row.tag(tag.clone());
        }
        if let Some(link) = &item.link {
            let (shell, link) = (self.shell.clone(), link.clone());
            row = row
                .on_click(move |_, _, cx| {
                    if let Some(shell) = shell.upgrade() {
                        shell.read(cx).open_link(&link);
                    }
                })
                .trailing(icon(Icon::OpenInEditor).text_color(p.faint));
        }
        enter(
            ElementId::Name(format!("result-enter-item-{run}-{}", item.id).into()),
            index,
            row,
        )
        .into_any_element()
    }

    /// What Flottille knows of the session of a run whose work goes on, read live.
    fn status_block(&self, run: &Run, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let shown = match &self.status {
            Some((id, Ok(status))) if *id == run.id => Some(status.clone()),
            _ => None,
        };
        let line = |id: &'static str, glyph: Icon, label: &'static str, said: String| {
            spaced(
                LINE_GAP,
                LabelledLine::new(id, glyph, label, SharedString::from(said.clone()))
                    .child(SharedString::from(said)),
            )
        };
        let last = shown.as_ref().and_then(|s| s.last.clone());
        let body = shown.map(|status| {
            let pull = match status.pulls.first() {
                Some(pull) => match &pull.mark {
                    Some(mark) => format!("{} · checks {mark}", pull.label()),
                    None => pull.label(),
                },
                None => "No pull request yet".to_owned(),
            };
            let requests = match status.requests {
                0 => "No request waits for you".to_owned(),
                1 => "1 request waits for you".to_owned(),
                n => format!("{n} requests wait for you"),
            };
            let mut lines = stack(LINE_GAP)
                .child(line(
                    "result-status-state",
                    Icon::Running,
                    "Session",
                    status.state.clone(),
                ))
                .child(line(
                    "result-status-branch",
                    Icon::Branch,
                    "Branch",
                    status.branch.clone().unwrap_or_else(|| "None yet".into()),
                ))
                .child(line(
                    "result-status-pull",
                    Icon::Repository,
                    "Pull request",
                    pull,
                ))
                .child(line(
                    "result-status-requests",
                    Icon::Warning,
                    "Requests",
                    requests,
                ));
            if let Some(last) = &last {
                let text = last.clone();
                lines = lines.child(spaced(
                    space::SM,
                    div()
                        .id("result-status-last")
                        .aria_label(SharedString::from(text.clone()))
                        .p(space::MD)
                        .rounded(px(10.))
                        .bg(p.card2)
                        .text_size(px(12.5))
                        .text_color(p.mute)
                        .line_clamp(2)
                        .child(SharedString::from(text))
                        .test_support(),
                ));
            }
            lines.into_any_element()
        });
        let id = ElementId::Name("result-status-block".into());
        presence(
            id,
            body.is_some(),
            body.unwrap_or_else(|| div().into_any_element()),
        )
        .space(space::LG)
        .into_any_element()
    }
}

impl Render for RoutinesTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let at = self.at.clone();
        let content = match &at {
            Where::List => self.list(window, cx),
            Where::Routine(id) => self.page(id, cx),
            Where::Result(routine, run) => self.result(routine, *run, cx),
        };
        let (this, again) = (cx.entity().downgrade(), cx.entity().downgrade());
        let removing = self
            .removing
            .as_ref()
            .and_then(|id| self.routine(id))
            .map_or_else(String::new, |r| r.name.clone());
        let confirm = Confirm::new(
            "routine-remove-confirm",
            format!("Remove the routine {removing}?"),
        )
        .sentence("It stops running and its history goes with it. The sessions of its runs stay.")
        .action("Remove routine")
        .open(self.removing.is_some())
        .on_confirm(move |_, cx| {
            this.update(cx, |this, cx| this.remove(cx)).ok();
        })
        .on_cancel(move |_, cx| {
            again
                .update(cx, |this, cx| {
                    this.removing = None;
                    cx.notify();
                })
                .ok();
        });
        div()
            .id("routines-tab")
            .child(sliding("routines-pager", at.index(), content))
            .child(self.form.clone())
            .child(confirm)
    }
}
