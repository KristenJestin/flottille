//! The left tree: "New session", then each project and its sessions in one flat list, the one that
//! moved last on top, each on two lines (its title, then the place it works in and its pull
//! request). Every row is its own view, so one session changing redraws its row and nothing else;
//! a folded project, and the "Ended" group, show the strongest mark of what they hold.

use crate::glyphs;
use crate::state::{AgentState, StateIcon, Status};
use std::collections::{HashMap, HashSet};

use gpui_design_system::color::mix;
use gpui_design_system::motion::{ENTER, PANEL};
use gpui_design_system::rows::TWO_LINES;
use gpui_design_system::{
    Button, Fold, Icon, Item, Listing, MachineMark, MarkSize, Menu, Modal, ReorderGroup,
    Reorderable, Tie, TreeRow, Type as _, changed, changing, fade, hoverable, meta, palette,
    presence, presence_of, section_label, space, tipped, tipped_when_cut, tree_children, tree_tie,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, ElementId, Entity, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Subscription, Task,
    TestSupportExt as _, WeakEntity, Window, div, px,
};

use crate::end_led::{self, LedEnd};
use crate::machines::shade;
use crate::model::{Home, Place, ago};
use crate::project_removal::ProjectRemoval;
use crate::session::end_sentence;
use crate::session_link::PullRequest;
use crate::shell::{Selection, Shell};

/// The levels of Flottille's left tree: what a row stands for, drawn with the parts of the tree
/// the kit has: a project is a first-level row, a session a plain row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeLevel {
    Project,
    Session,
}

impl TreeLevel {
    /// The row of this level.
    pub fn row(self, id: impl Into<ElementId>, title: impl Into<SharedString>) -> TreeRow {
        let row = TreeRow::new(id, title);
        match self {
            Self::Project => row.top(),
            Self::Session => row,
        }
    }
}

/// The element id of a project's row.
pub fn project_id(name: &str) -> ElementId {
    ElementId::Name(format!("project-{name}").into())
}

/// The element id of a project's row in a machine's section (Group by machine).
pub fn project_on_id(name: &str, machine: &str) -> ElementId {
    ElementId::Name(format!("project-{name}-on-{machine}").into())
}

/// The element id of the mark of `machine` on a project's row: its icon in its colour, which says
/// the machine's name and state when it is hovered.
pub fn project_mark_id(name: &str, machine: &str) -> ElementId {
    ElementId::Name(format!("project-mark-{name}-{machine}").into())
}

/// The element id of the machine's name a project row says after its own, in small, when it
/// shares its name with a project of another repository.
pub fn project_apart_id(name: &str) -> ElementId {
    ElementId::Name(format!("project-apart-{name}").into())
}

/// The element id of the line under a project's row that says since when its machine is off.
pub fn project_offline_id(name: &str) -> ElementId {
    ElementId::Name(format!("project-offline-{name}").into())
}

/// What a row key of a project whose settings file is gone starts with: no project name holds a
/// colon, so such a row is never the node, the key or the element ids of another machine's
/// project of the same name.
pub const GONE: &str = "gone:";

/// The element id of the row of a project whose settings file is gone.
pub fn project_gone_id(name: &str) -> ElementId {
    ElementId::Name(format!("project-{GONE}{name}").into())
}

/// The element id of the row of a project whose settings file is gone, in a machine's section.
pub fn project_gone_on_id(name: &str, machine: &str) -> ElementId {
    ElementId::Name(format!("project-{GONE}{name}-on-{machine}").into())
}

/// The words on the row of a project whose settings file is gone.
pub fn project_unconfigured_id(name: &str) -> ElementId {
    ElementId::Name(format!("project-unconfigured-{name}").into())
}

/// The menu of the row of a project whose settings file is gone.
pub fn project_menu_id(name: &str) -> ElementId {
    ElementId::Name(format!("project-menu-{name}").into())
}

/// Remove, in that menu.
pub fn project_remove_id(name: &str) -> ElementId {
    ElementId::Name(format!("project-remove-{name}").into())
}

/// The element id of a machine's section header (Group by machine).
pub fn machine_id(machine: &str) -> ElementId {
    ElementId::Name(format!("machine-{machine}").into())
}

/// The element id of the icon of a machine's section header.
pub fn machine_mark_id(machine: &str) -> ElementId {
    ElementId::Name(format!("machine-mark-{machine}").into())
}

/// The element id of the state a machine's section header says.
pub fn machine_state_id(machine: &str) -> ElementId {
    ElementId::Name(format!("machine-state-{machine}").into())
}

/// The element id of the projects' list: its rows and groups are parts of it.
pub fn projects_list_id() -> ElementId {
    ElementId::Name("projects-list".into())
}

/// The element id of a project's place in the list (the row, and its fold, as the list moves
/// it).
pub fn project_slot_id(name: &str) -> ElementId {
    ElementId::Name(format!("projects-list-row-{name}").into())
}

/// The element id of a group of the user's.
pub fn group_id(group: &str) -> ElementId {
    ElementId::Name(format!("projects-list-group-{group}").into())
}

/// The element id of a group's heading, which folds it and is dragged.
pub fn group_header_id(group: &str) -> ElementId {
    ElementId::Name(format!("projects-list-group-{group}-header").into())
}

/// The element id of the number of projects a group holds.
pub fn group_count_id(group: &str) -> ElementId {
    ElementId::Name(format!("projects-list-group-{group}-count").into())
}

/// The element id of what a folded group's heading says of the rows it hides.
pub fn group_mark_id(group: &str) -> ElementId {
    ElementId::Name(format!("projects-list-group-{group}-mark").into())
}

/// The element id of the "⋯" menu a group's heading offers.
pub fn group_menu_id(group: &str) -> ElementId {
    ElementId::Name(format!("projects-group-menu-{group}").into())
}

/// The element id of Rename… in a group's menu.
pub fn group_rename_id(group: &str) -> ElementId {
    ElementId::Name(format!("projects-group-rename-{group}").into())
}

/// The element id of Remove in a group's menu.
pub fn group_remove_id(group: &str) -> ElementId {
    ElementId::Name(format!("projects-group-remove-{group}").into())
}

/// Why New group… cannot be picked while the projects are shown by machine.
const SHOWN_BY_MACHINE: &str = "Shown by machine";
/// Why Group by machine cannot be switched on: there is nothing to group by.
const ONE_MACHINE: &str = "One machine";

/// The element id of New group… in the Projects header's menu.
pub fn new_group_id() -> ElementId {
    ElementId::Name("projects-new-group".into())
}

/// The element id of the Projects header's "⋯" menu.
pub fn projects_menu_id() -> ElementId {
    ElementId::Name("projects-menu".into())
}

/// The element id of Group by machine in that menu: a switch.
pub fn by_machine_id() -> ElementId {
    ElementId::Name("projects-by-machine".into())
}

/// The element id of the "Ended" group, at the bottom of the tree.
pub fn ended_id() -> ElementId {
    ElementId::Name("ended".into())
}

/// The element id of the "Blocked by others" group, above "Ended".
pub fn blocked_id() -> ElementId {
    ElementId::Name("blocked".into())
}

/// The element id of what the "Blocked by others" heading says: the count and who.
pub fn blocked_who_id() -> ElementId {
    ElementId::Name("blocked-who".into())
}

/// The element id of the block inside the "Ended" group that holds its rows and scrolls.
pub fn ended_scroll_id() -> ElementId {
    ElementId::Name("ended-sessions".into())
}

/// The element id of the machine a session on another machine runs on, and its state, on its
/// row's second line: `server · running`.
pub fn session_machine_id(id: &str) -> ElementId {
    ElementId::Name(format!("session-machine-{id}").into())
}

/// The element id of the "⋯" menu a session's row offers.
pub fn session_menu_id(id: &str) -> ElementId {
    ElementId::Name(format!("session-menu-{id}").into())
}

/// The element id of End session in a session's row menu.
pub fn session_end_id(id: &str) -> ElementId {
    ElementId::Name(format!("session-row-end-{id}").into())
}

/// The element id of Resume in an ended session's row menu.
pub fn session_resume_id(id: &str) -> ElementId {
    ElementId::Name(format!("session-row-resume-{id}").into())
}

/// The element id of "Rename…" in a session's row menu.
pub fn session_rename_id(id: &str) -> ElementId {
    ElementId::Name(format!("session-row-rename-{id}").into())
}

/// The element id of "Detach from …" in the row menu of a session an agent started.
pub fn session_detach_id(id: &str) -> ElementId {
    ElementId::Name(format!("session-row-detach-{id}").into())
}

/// The element id of a session's row.
pub fn session_id(id: &str) -> ElementId {
    ElementId::Name(format!("session-{id}").into())
}

pub fn place_name(place: &Place) -> &str {
    place.as_deref().unwrap_or("main")
}

/// The element id of a session row's second line, which holds its place and its pull request.
pub fn session_line_id(id: &str) -> ElementId {
    ElementId::Name(format!("session-line-{id}").into())
}

/// The element id of the place a session works in, on its second line: labelled `main` or the
/// workspace's name.
pub fn session_place_id(id: &str) -> ElementId {
    ElementId::Name(format!("session-place-{id}").into())
}

/// The element id of what a waiting session waits for, on its second line after the place:
/// labelled `waits for checks of #12`, `out of quota until 13:00`.
pub fn session_waits_id(id: &str) -> ElementId {
    ElementId::Name(format!("session-waits-{id}").into())
}

/// The element id of what a led session runs on, on its second line after the place: labelled
/// `Sonnet 5.5 · medium`, the model and the effort by name.
pub fn session_runs_id(id: &str) -> ElementId {
    ElementId::Name(format!("session-runs-{id}").into())
}

/// The element id of the icon before the place's name: a folder (`Project folder`) or a branch
/// (`Workspace`).
pub fn session_place_icon_id(id: &str) -> ElementId {
    ElementId::Name(format!("session-place-icon-{id}").into())
}

/// The element id of a session's pull request on its second line, labelled with its number and
/// its checks.
pub fn session_pull_id(id: &str) -> ElementId {
    ElementId::Name(format!("session-pull-{id}").into())
}

/// The element id of the chip of the key of what a session is about, before its title on its
/// row's first line.
pub fn session_key_id(id: &str) -> ElementId {
    ElementId::Name(format!("session-key-{id}").into())
}

/// The element id of the same chip on the second line, where it goes when the user named the
/// session: their title keeps the first.
pub fn session_key_beside_id(id: &str) -> ElementId {
    ElementId::Name(format!("session-key-beside-{id}").into())
}

/// The element id of the icon that says a command runs in a project.
pub fn project_commands_id(name: &str) -> ElementId {
    ElementId::Name(format!("commands-project-{name}").into())
}

/// The element id of the dot a session's row shows while something new in it waits to be seen.
pub fn session_unread_id(id: &str) -> ElementId {
    ElementId::Name(format!("unread-session-{id}").into())
}

/// The element id of the one mark at the right of a session's row, a state or the unread dot:
/// its label and its tooltip say what it means.
pub fn session_mark_id(id: &str) -> ElementId {
    ElementId::Name(format!("mark-session-{id}").into())
}

/// The element id of the pencil a session's row shows while its composer holds a draft the user
/// has not sent and has left.
pub fn session_draft_id(id: &str) -> ElementId {
    ElementId::Name(format!("draft-session-{id}").into())
}

/// The element id of the icon that says a command runs in a session.
pub fn session_commands_id(id: &str) -> ElementId {
    ElementId::Name(format!("commands-session-{id}").into())
}

/// What ends a row: the small terminal icon while a command of the project or the session runs,
/// so that no server is forgotten, then the row's own mark.
fn trailing(id: ElementId, commands: usize, last: impl IntoElement, cx: &App) -> impl IntoElement {
    let p = palette(cx);
    let said = match commands {
        1 => "A command is running".to_owned(),
        n => format!("{n} commands are running"),
    };
    div()
        .flex()
        .items_center()
        .gap(space::SM)
        .when(commands > 0, |d| {
            d.child(
                div()
                    .id(id)
                    .flex_none()
                    .aria_label(SharedString::from(said))
                    .child(
                        gpui_design_system::icon(Icon::Terminal)
                            .size(px(12.))
                            .text_color(p.run),
                    )
                    .test_support(),
            )
        })
        .child(last)
}

/// The height of the one mark at the right of a session's row, which its tooltip hangs from.
const MARK: f32 = 16.;

/// A mark shown on a row: a state icon over its own model, shown or not.
struct Mark {
    shown: Option<AgentState>,
    status: Entity<Status>,
    icon: Entity<StateIcon>,
    /// Puts the model at rest once the icon has folded away: it keeps its state while it does.
    settling: Option<Task<()>>,
}

impl Mark {
    fn new<V: 'static>(cx: &mut Context<V>) -> Self {
        let status = cx.new(|_| Status::new(AgentState::Idle));
        let icon = cx.new(|cx| StateIcon::new(status.clone(), cx));
        Self {
            shown: None,
            status,
            icon,
            settling: None,
        }
    }

    /// Whether the row must be drawn again: only when the mark appears or goes away. A change
    /// from one mark to another redraws the icon alone.
    fn set<V: 'static>(&mut self, mark: Option<AgentState>, cx: &mut Context<V>) -> bool {
        let changed = self.shown.is_some() != mark.is_some();
        self.shown = mark;
        match mark {
            Some(state) => {
                self.settling = None;
                // GPUI Fast takes any update of an entity for a change: touch it only when it is
                // one.
                if self.status.read(cx).state() != state {
                    self.status.update(cx, |status, cx| status.set(state, cx));
                }
            }
            // A row at rest holds a still state, so that no ticker runs for it: once the icon,
            // which keeps the state it had, has folded away.
            None if changed => {
                let status = self.status.clone();
                self.settling = Some(cx.spawn(async move |_, cx| {
                    cx.background_executor().timer(PANEL).await;
                    status.update(cx, |status, cx| status.set(AgentState::Idle, cx));
                }));
            }
            None => {}
        }
        changed
    }

    /// The icon, folding in when the mark appears and away when it goes.
    fn icon(&self, id: &str) -> impl IntoElement {
        presence(
            ElementId::Name(format!("mark-{id}").into()),
            self.shown.is_some(),
            div().flex().items_center().child(self.icon.clone()),
        )
    }
}

/// What a folded group's heading says of the projects it hides: their strongest mark, which comes
/// in and goes with a fade. A heading that leaves draws it once more, as it was: this holds
/// nothing the group's removal clears.
struct GroupMark {
    mark: Mark,
    /// The mark that went is still drawn while it folds away: it is held until then, so that the
    /// heading keeps its slot.
    leaving: Option<Task<()>>,
}

impl GroupMark {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            mark: Mark::new(cx),
            leaving: None,
        }
    }

    /// Whether it says this already.
    fn says(&self, state: Option<AgentState>) -> bool {
        self.mark.shown == state
    }

    fn set(&mut self, state: Option<AgentState>, cx: &mut Context<Self>) {
        let changed = self.mark.set(state, cx);
        match state {
            Some(_) => self.leaving = None,
            None if changed => {
                self.leaving = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(PANEL).await;
                    this.update(cx, |this, cx| {
                        this.leaving = None;
                        cx.notify();
                    })
                    .ok();
                }));
            }
            None => {}
        }
        if changed {
            cx.notify();
        }
    }

    /// Whether the heading has a mark to draw: one that is shown, or one that folds away.
    fn drawn(&self) -> bool {
        self.mark.shown.is_some() || self.leaving.is_some()
    }
}

/// A group's heading: its "⋯" menu (Rename…, Remove) and its mark.
struct GroupParts {
    menu: Entity<Menu>,
    mark: Entity<GroupMark>,
    /// The sidebar is drawn again as the mark comes and goes.
    _watch: Subscription,
}

impl GroupParts {
    fn new(id: &str, shell: WeakEntity<Shell>, cx: &mut Context<Sidebar>) -> Self {
        let menu = cx.new(|cx| {
            let (renames, removes) = (shell.clone(), shell);
            let (rename, remove) = (id.to_owned(), id.to_owned());
            Menu::new(group_menu_id(id), "Group actions", cx)
                .entry(
                    group_rename_id(id),
                    crate::rename::RENAME_LABEL,
                    Some(Icon::Edit),
                    move |window, cx| {
                        let id = rename.clone();
                        renames
                            .update(cx, |shell, cx| shell.ask_rename_group(&id, window, cx))
                            .ok();
                    },
                )
                .entry(
                    group_remove_id(id),
                    "Remove",
                    Some(Icon::Trash),
                    move |window, cx| {
                        let id = remove.clone();
                        removes
                            .update(cx, |shell, cx| shell.remove_group(&id, window, cx))
                            .ok();
                    },
                )
        });
        let mark = cx.new(GroupMark::new);
        let _watch = cx.observe(&mark, |_, _, cx| cx.notify());
        Self { menu, mark, _watch }
    }
}

/// The side panel: the primary button, the `PROJECTS` label, the projects and, pinned at the
/// panel's bottom under a line whatever the tree's length, the "Ended" group, which unfolds upward
/// over the tree.
pub struct Sidebar {
    shell: WeakEntity<Shell>,
    projects: Vec<Entity<ProjectNode>>,
    /// The rows as drawn: a project that went folds away before it is gone. Fed in the order the
    /// daemon lists them, never the user's: a row the user moved is the reorderable list's to
    /// slide.
    list: Listing<Entity<ProjectNode>>,
    /// The projects as the user arranged them: the groups, then the rows outside any group.
    arranged: Arranged,
    /// The places of the rows as last drawn (a project's name and its group), so that a row that
    /// is going stays where it was while it folds away.
    drawn: Vec<(String, Option<String>)>,
    /// The rows the list opened with: they rise into place, the rows that come after unfold.
    opening: Option<HashSet<String>>,
    /// The machines' sections, which replace the projects' list while they are shown by machine.
    sections: Vec<Entity<MachineNode>>,
    section_list: Listing<Entity<MachineNode>>,
    /// The projects are shown by machine, one section each.
    by_machine: bool,
    /// The Projects header's "⋯": Group by machine, offered once there are machines to group by.
    arrange: Option<Entity<Menu>>,
    /// What each group's heading holds, by the group's id: its "⋯" menu and the mark it shows
    /// while folded. They outlive the group while its heading folds away.
    group_parts: HashMap<String, GroupParts>,
    offered: bool,
    /// The "Ended" group, and whether it holds a session: it folds away when it holds none.
    ended: Option<(Entity<EndedNode>, bool)>,
    /// The "Blocked by others" group, and whether it holds a session: absent while empty.
    blocked: Option<(Entity<BlockedNode>, bool)>,
    /// The session whose ending the user is asked to confirm, and how many agents end.
    ending: Option<(String, usize)>,
    /// The count the confirmation said last, which it still shows while it fades out.
    said: usize,
    /// The led sessions that end with the session asked, as the daemon read them; they stay while
    /// the confirmation fades out.
    led: Vec<LedEnd>,
    /// The sessions asked of the daemon from here: an answer that refuses is said here only for
    /// them (the session's page says its own).
    asked: HashSet<String>,
    /// What the daemon refused of what was asked from a row.
    refused: Option<SharedString>,
    /// Whether the daemon has not told yet what the tree shows: a skeleton stands in until then,
    /// but only while there is no row to show.
    loading: bool,
    /// What the foot says of an update of Flottille, when there is one to act on.
    version_line: Option<SharedString>,
    /// The quiet mark of what is new since the version last run, until its page is opened.
    whats_new: Option<SharedString>,
}

impl Sidebar {
    pub fn new(shell: WeakEntity<Shell>) -> Self {
        Self {
            shell,
            projects: Vec::new(),
            list: Listing::default(),
            arranged: Arranged::default(),
            drawn: Vec::new(),
            opening: None,
            sections: Vec::new(),
            section_list: Listing::default(),
            by_machine: false,
            arrange: None,
            group_parts: HashMap::new(),
            offered: false,
            ended: None,
            blocked: None,
            ending: None,
            said: 0,
            led: Vec::new(),
            asked: HashSet::new(),
            refused: None,
            loading: true,
            version_line: None,
            whats_new: None,
        }
    }

    /// The projects' rows as the user arranged them.
    pub fn arranged(&self) -> &Arranged {
        &self.arranged
    }

    pub fn projects(&self) -> &[Entity<ProjectNode>] {
        &self.projects
    }

    /// The projects as the user arranged them: while the projects are shown by machine, the list
    /// keeps what it was, folding away as a whole.
    pub fn set_arranged(&mut self, arranged: Arranged, cx: &mut Context<Self>) {
        if !self.by_machine && self.arranged != arranged {
            self.group_parts_for(&arranged, cx);
            self.arranged = arranged;
            cx.notify();
        }
    }

    /// Each group has its menu and its mark from the first time it is drawn, and a mark that
    /// changed says it.
    fn group_parts_for(&mut self, arranged: &Arranged, cx: &mut Context<Self>) {
        self.group_parts
            .retain(|id, _| arranged.groups.iter().any(|group| &group.id == id));
        for group in &arranged.groups {
            if !self.group_parts.contains_key(&group.id) {
                let parts = GroupParts::new(&group.id, self.shell.clone(), cx);
                self.group_parts.insert(group.id.clone(), parts);
            }
            let parts = &self.group_parts[&group.id];
            if !parts.mark.read(cx).says(group.mark) {
                parts.mark.update(cx, |mark, cx| mark.set(group.mark, cx));
            }
        }
    }

    pub fn set_projects(&mut self, projects: Vec<Entity<ProjectNode>>, cx: &mut Context<Self>) {
        if !self.by_machine && self.projects != projects {
            self.projects = projects;
            cx.notify();
        }
    }

    pub fn sections(&self) -> &[Entity<MachineNode>] {
        &self.sections
    }

    pub fn set_sections(&mut self, sections: Vec<Entity<MachineNode>>, cx: &mut Context<Self>) {
        if self.sections != sections {
            self.sections = sections;
            cx.notify();
        }
    }

    /// Whether the projects are shown by machine.
    pub fn is_by_machine(&self) -> bool {
        self.by_machine
    }

    /// Whether the Projects header's menu is made.
    pub fn has_menu(&self) -> bool {
        self.arrange.is_some()
    }

    /// Whether the header offers Group by machine now.
    pub fn offers_grouping(&self) -> bool {
        self.offered
    }

    /// How the projects are shown, and whether the user may group them by machine (there is
    /// another machine): the Projects header's "⋯" menu always offers New group…, and Group by
    /// machine, a switch that slides, once there is a machine to group by.
    pub fn set_arrangement(&mut self, by_machine: bool, offered: bool, cx: &mut Context<Self>) {
        if self.arrange.is_none() {
            let shell = self.shell.clone();
            let toggles = self.shell.clone();
            self.arrange = Some(cx.new(|cx| {
                let menu = Menu::new(projects_menu_id(), "Arrange projects", cx)
                    .width(px(210.))
                    .entry(
                        new_group_id(),
                        crate::group_name::NEW_GROUP_LABEL,
                        Some(Icon::Plus),
                        move |window, cx| {
                            shell
                                .update(cx, |shell, cx| shell.new_group(window, cx))
                                .ok();
                        },
                    );
                // The groups are the user's own list: shown by machine, there is none to add to.
                let menu = match by_machine {
                    true => menu.disabled(SHOWN_BY_MACHINE),
                    false => menu,
                };
                // Always there: a machine that comes or goes enables or disables it in place, so
                // that a menu that is open stays open.
                let menu = menu.switch(
                    by_machine_id(),
                    "Group by machine",
                    Some(glyphs::device("desktop")),
                    by_machine,
                    move |window, cx| {
                        toggles
                            .update(cx, |shell, cx| shell.toggle_by_machine(window, cx))
                            .ok();
                    },
                );
                // Switched on, it stays one to switch off.
                match offered || by_machine {
                    true => menu,
                    false => menu.disabled(ONE_MACHINE),
                }
            }));
        } else if (self.by_machine != by_machine || self.offered != offered)
            && let Some(menu) = &self.arrange
        {
            menu.update(cx, |menu, cx| {
                menu.set_disabled(
                    &new_group_id(),
                    by_machine.then(|| SHOWN_BY_MACHINE.into()),
                    cx,
                );
                menu.set_disabled(
                    &by_machine_id(),
                    (!offered && !by_machine).then(|| ONE_MACHINE.into()),
                    cx,
                );
                menu.set_switch(&by_machine_id(), by_machine, cx);
            });
        }
        if self.by_machine != by_machine || self.offered != offered {
            self.by_machine = by_machine;
            self.offered = offered;
            cx.notify();
        }
    }

    /// The "Ended" group, and whether it holds a session now.
    pub fn set_ended(&mut self, group: Entity<EndedNode>, present: bool, cx: &mut Context<Self>) {
        let same = self
            .ended
            .as_ref()
            .is_some_and(|(held, shown)| *held == group && *shown == present);
        if !same {
            self.ended = Some((group, present));
            cx.notify();
        }
    }

    /// The "Blocked by others" group, and whether it holds a session now.
    pub fn set_blocked(
        &mut self,
        group: Entity<BlockedNode>,
        present: bool,
        cx: &mut Context<Self>,
    ) {
        let same = self
            .blocked
            .as_ref()
            .is_some_and(|(held, shown)| *held == group && *shown == present);
        if !same {
            self.blocked = Some((group, present));
            cx.notify();
        }
    }

    /// Asks the user to confirm ending `session`, which ends `count` agents.
    pub fn ask_end(&mut self, session: String, count: usize, cx: &mut Context<Self>) {
        self.ending = Some((session, count));
        self.said = count;
        self.led.clear();
        self.refused = None;
        cx.notify();
    }

    /// The daemon read the led sessions ending `session` would end: the confirmation lists them
    /// if it is still asked for that session.
    pub fn led_read(&mut self, session: &str, led: Vec<LedEnd>, cx: &mut Context<Self>) {
        if self.ending() == Some(session) && self.led != led {
            self.led = led;
            cx.notify();
        }
    }

    /// The session whose ending is asked.
    pub fn ending(&self) -> Option<&str> {
        self.ending.as_ref().map(|(session, _)| session.as_str())
    }

    fn cancel_end(&mut self, cx: &mut Context<Self>) {
        if self.ending.take().is_some() {
            cx.notify();
        }
    }

    fn confirm_end(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((session, _)) = self.ending.take() else {
            return;
        };
        self.asked.insert(session.clone());
        let shell = self.shell.clone();
        // Once this update is done: the shell draws the tree again, this sidebar's included.
        window.defer(cx, move |window, cx| {
            shell
                .update(cx, |shell, cx| shell.end_confirmed(session, window, cx))
                .ok();
        });
        cx.notify();
    }

    /// A resume asked from a row.
    pub fn resuming(&mut self, session: &str, cx: &mut Context<Self>) {
        self.asked.insert(session.to_owned());
        self.refused = None;
        cx.notify();
    }

    /// The daemon answered what was asked of `session` from a row: a refusal is said.
    pub fn answered(&mut self, session: &str, refused: Option<String>, cx: &mut Context<Self>) {
        if self.asked.remove(session) && refused.is_some() {
            self.refused = refused.map(SharedString::from);
            cx.notify();
        }
    }

    /// What the daemon refused of what was asked from a row.
    pub fn refused(&self) -> Option<&str> {
        self.refused.as_deref()
    }

    /// Whether the sidebar has a row to show, from any machine: a skeleton stands for what is not
    /// there yet, never beside what is.
    fn has_rows(&self) -> bool {
        !self.projects.is_empty() || !self.sections.is_empty()
    }

    /// The daemon answered (or cannot be reached): the tree is not waiting for it any more.
    pub fn set_loading(&mut self, loading: bool, cx: &mut Context<Self>) {
        if self.loading != loading {
            self.loading = loading;
            cx.notify();
        }
    }
}

/// The rows of a level inside their frames: one that comes unfolds, one that goes folds away.
///
/// A row rises into place as it comes, at its full size, unless `unfolds` says it unfolds from
/// nothing, as a fold opens (the sessions under a session's row): it is then shown as it folds.
fn framed<V: Render>(
    list: &mut Listing<Entity<V>>,
    rows: &[Entity<V>],
    scope: &str,
    unfolds: impl Fn(&Entity<V>, &App) -> bool,
    cx: &mut App,
) -> Vec<gpui_kit::AnyElement> {
    let now = cx.background_executor().now();
    let reduce = cx.reduce_motion();
    list.sync(
        rows.iter()
            .map(|row| (row.entity_id().as_u64().to_string(), row.clone())),
        now,
        reduce,
    );
    list.rows()
        .map(|row| {
            let item = Item::new(
                ElementId::Name(format!("{scope}-{}", row.key).into()),
                row.placement,
                row.value.clone(),
            );
            match unfolds(row.value, cx) {
                true => item,
                false => item.rising(),
            }
            .into_any_element()
        })
        .collect()
}

impl Sidebar {
    /// What the foot says of an update: it unfolds when there is one, fades when it changes.
    pub fn set_version_line(&mut self, line: Option<String>, cx: &mut Context<Self>) {
        let line = line.map(SharedString::from);
        if self.version_line != line {
            self.version_line = line;
            cx.notify();
        }
    }

    pub fn version_line(&self) -> Option<&SharedString> {
        self.version_line.as_ref()
    }

    /// The mark of what is new: it unfolds and fades in once there is something new that was not
    /// opened, and folds away and fades out once it was.
    pub fn set_whats_new(&mut self, mark: Option<String>, cx: &mut Context<Self>) {
        let mark = mark.map(SharedString::from);
        if self.whats_new != mark {
            self.whats_new = mark;
            cx.notify();
        }
    }
}

/// A group of the user's as the list draws it: its projects' rows, in order.
#[derive(Clone, PartialEq)]
pub struct ArrangedGroup {
    pub id: String,
    pub name: String,
    pub open: bool,
    pub rows: Vec<Entity<ProjectNode>>,
    /// The strongest mark of what its projects hold, which its heading shows while it is folded.
    pub mark: Option<AgentState>,
}

/// The projects' rows as the user arranged them: the groups, then the rows outside any group.
#[derive(Clone, Default, PartialEq)]
pub struct Arranged {
    pub groups: Vec<ArrangedGroup>,
    pub loose: Vec<Entity<ProjectNode>>,
}

/// The place the list says a row was dropped at, among the rows of `group` once the row has left
/// its own, as a place among the rows the arrangement holds: the rows still folding away
/// (`going`) are drawn, and counted by the list, but are not the arrangement's.
fn counted(
    drawn: &[(String, Option<String>)],
    going: &HashSet<String>,
    moved: &str,
    group: Option<&str>,
    index: usize,
) -> usize {
    drawn
        .iter()
        .filter(|(key, place)| key != moved && place.as_deref() == group)
        .take(index)
        .filter(|(key, _)| !going.contains(key))
        .count()
}

impl Sidebar {
    /// The user's list: the groups and the rows outside any, dragged to their place. The rows
    /// are the listing's items (one that comes unfolds, one that goes folds away), the list as a
    /// whole folds away while the projects are shown by machine.
    fn project_list(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let now = cx.background_executor().now();
        let reduce = cx.reduce_motion();
        self.list.sync(
            self.projects
                .iter()
                .map(|row| (row.entity_id().as_u64().to_string(), row.clone())),
            now,
            reduce,
        );
        // Each row by its project's name, which the list keys it by; two rows of one name (one
        // going, one coming) are told apart by the node. A new row unfolds from nothing, at the
        // end of the list.
        let mut items: HashMap<String, gpui_kit::AnyElement> = HashMap::new();
        let mut canonical: Vec<String> = Vec::new();
        for row in self.list.rows() {
            let mut key = row.value.read(cx).list_key();
            if items.contains_key(&key) {
                key = format!("{key}~{}", row.key);
            }
            let item = Item::new(
                ElementId::Name(format!("tree-project-{}", row.key).into()),
                row.placement,
                row.value.clone(),
            );
            // The rows the window opens with rise into place; one that comes later unfolds.
            let item = match self.opening.as_ref().is_none_or(|rows| rows.contains(&key)) {
                true => item.rising(),
                false => item,
            };
            canonical.push(key.clone());
            items.insert(key, item.into_any_element());
        }
        let name = |node: &Entity<ProjectNode>| node.read(cx).name().to_owned();
        let mut order: Vec<(String, Option<String>)> = Vec::new();
        for group in &self.arranged.groups {
            for node in &group.rows {
                order.push((name(node), Some(group.id.clone())));
            }
        }
        for node in &self.arranged.loose {
            order.push((name(node), None));
        }
        // A row that is going stays where it was while it folds away.
        let last = std::mem::take(&mut self.drawn);
        for (at, (key, place)) in last.iter().enumerate() {
            if order.iter().any(|(held, _)| held == key) || !items.contains_key(key) {
                continue;
            }
            let after = last[..at]
                .iter()
                .rev()
                .find_map(|(before, _)| order.iter().position(|(held, _)| held == before));
            let place = place
                .clone()
                .filter(|id| self.arranged.groups.iter().any(|group| &group.id == id));
            order.insert(after.map_or(0, |at| at + 1), (key.clone(), place));
        }
        // What the arrangement does not name is at the end.
        for key in canonical {
            if !order.iter().any(|(held, _)| *held == key) {
                order.push((key, None));
            }
        }
        self.drawn = order.clone();
        // The list counts the rows that are still folding away; the arrangement does not.
        let arranged: HashSet<String> = self
            .arranged
            .groups
            .iter()
            .flat_map(|group| group.rows.iter())
            .chain(&self.arranged.loose)
            .map(name)
            .collect();
        let going: HashSet<String> = order
            .iter()
            .filter(|(key, _)| !arranged.contains(key))
            .map(|(key, _)| key.clone())
            .collect();
        let drawn = order.clone();
        if self.opening.is_none() && !order.is_empty() {
            self.opening = Some(order.iter().map(|(key, _)| key.clone()).collect());
        }
        // A project that left is no longer one the window opened with: the same name coming back
        // is a new row, which unfolds.
        if let Some(rows) = &mut self.opening {
            rows.retain(|key| order.iter().any(|(held, _)| held == key));
        }

        let mut list = Reorderable::new(projects_list_id());
        for group in &self.arranged.groups {
            let mut held = ReorderGroup::new(group.id.clone(), group.name.clone()).open(group.open);
            // Built again at each frame, up to a moment after the group is gone: both hold
            // entities of their own, never what the group's removal clears.
            if let Some(parts) = self.group_parts.get(&group.id) {
                let (menu, mark) = (parts.menu.clone(), parts.mark.clone());
                if mark.read(cx).drawn() {
                    let id = group.id.clone();
                    held = held.mark(move |_, cx| mark.read(cx).mark.icon(&id).into_any_element());
                }
                held = held.trailing(move |_, _| menu.clone());
            }
            for (key, place) in &order {
                if place.as_deref() == Some(group.id.as_str())
                    && let Some(item) = items.remove(key)
                {
                    held = held.row(key.clone(), item);
                }
            }
            list = list.group(held);
        }
        for (key, place) in &order {
            if place.is_none()
                && let Some(item) = items.remove(key)
            {
                list = list.row(key.clone(), item);
            }
        }
        let (moved, groups, toggled) = (self.shell.clone(), self.shell.clone(), self.shell.clone());
        let list = list
            .on_move_row(move |row, window, cx| {
                let name = row.row.to_string();
                let group = row.group.clone();
                let index = counted(&drawn, &going, &name, group.as_deref(), row.index);
                moved
                    .update(cx, |shell, cx| {
                        shell.project_moved(&name, group.as_deref(), index, window, cx)
                    })
                    .ok();
            })
            .on_move_group(move |id, index, window, cx| {
                groups
                    .update(cx, |shell, cx| shell.group_moved(id, index, window, cx))
                    .ok();
            })
            .on_toggle(move |id, open, window, cx| {
                toggled
                    .update(cx, |shell, cx| shell.group_opened(id, open, window, cx))
                    .ok();
            });
        presence("sidebar-projects-list", !self.by_machine, list)
    }
}

impl Render for Sidebar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        // Shown by machine, the projects' list folds away and the machines' sections come in, and
        // back.
        let projects = self.project_list(cx);
        let sectioned: &[Entity<MachineNode>] = match self.by_machine {
            true => &self.sections,
            false => &[],
        };
        let sections = framed(
            &mut self.section_list,
            sectioned,
            "tree-machine",
            |_, _| false,
            cx,
        );
        let arrange = self.arrange.clone();
        let shell = self.shell.clone();
        let adds = self.shell.clone();
        let version = self.shell.clone();
        let whats_new = self.shell.clone();
        let ended = self.ended.clone().map(|(group, present)| {
            // At the panel's bottom, under a line; its rows scroll inside their own block, and it
            // folds away when its last session leaves.
            presence(
                "sidebar-ended",
                present,
                div()
                    .id("ended-footer")
                    .flex_none()
                    .border_t_1()
                    .border_color(p.line)
                    .bg(p.side)
                    .px(space::SM)
                    .pt(space::XS)
                    .pb(space::SM)
                    .child(group)
                    .test_support(),
            )
        });
        // Above "Ended", the same footer's pattern: it unfolds and fades in with its first row,
        // folds away with its last.
        let blocked = self.blocked.clone().map(|(group, present)| {
            presence(
                "sidebar-blocked",
                present,
                div()
                    .id("blocked-footer")
                    .flex_none()
                    .border_t_1()
                    .border_color(p.line)
                    .bg(p.side)
                    .px(space::SM)
                    .pt(space::XS)
                    .pb(space::SM)
                    .child(group)
                    .test_support(),
            )
        });
        let count = self.ending.as_ref().map_or(self.said, |(_, count)| *count);
        let this = cx.entity().downgrade();
        let modal = Modal::new("sidebar-end-modal", "End the session?")
            .open(self.ending.is_some())
            .on_close(move |_, cx| {
                this.update(cx, |this, cx| this.cancel_end(cx)).ok();
            })
            .child(
                div()
                    .id("sidebar-end-question")
                    .text_size(px(12.5))
                    .text_color(p.mute)
                    .child(SharedString::from(end_sentence(count)))
                    .test_support(),
            )
            .child(end_led::block("sidebar-end", &self.led, cx))
            .action(
                Button::danger("sidebar-end-confirm", end_led::end_label(count, &self.led))
                    .on_click(cx.listener(|this, _, window, cx| this.confirm_end(window, cx))),
            );
        div()
            .id("sidebar")
            .size_full()
            .bg(p.side)
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .pt(space::MD)
                    .px(space::MD)
                    .pb(space::XS)
                    .child(
                        Button::primary("new-session", "New session")
                            .leading(Icon::Plus)
                            .full_width()
                            .on_click(move |_, window, cx| {
                                shell
                                    .update(cx, |shell, cx| shell.new_session(window, cx))
                                    .ok();
                            }),
                    ),
            )
            .child(
                div()
                    .id("tree")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(space::SM)
                    .pt(space::SM)
                    .pb(space::LG)
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .pr(space::XS)
                            .child(section_label("Projects", cx))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(space::XS)
                                    .when_some(arrange, |d, menu| d.child(menu))
                                    .child(
                                        Button::icon("add-project", Icon::Plus, "Add a project")
                                            .on_click(move |_, window, cx| {
                                                adds.update(cx, |shell, cx| {
                                                    shell.add_project(window, cx)
                                                })
                                                .ok();
                                            }),
                                    ),
                            ),
                    )
                    .child(presence(
                        "sidebar-loading",
                        self.loading && !self.has_rows(),
                        crate::loading::sidebar(),
                    ))
                    .child(projects)
                    .children(sections),
            )
            .children(blocked)
            .children(ended)
            .child(presence_of(
                "sidebar-refused-line",
                self.refused.clone(),
                move |line| {
                    div()
                        .id("sidebar-refused")
                        .aria_label(line.clone())
                        .px(space::LG)
                        .pb(space::MD)
                        .text_size(px(12.5))
                        .text_color(p.fail)
                        .child(line)
                        .test_support()
                        .into_any_element()
                },
            ))
            .child(presence_of(
                "sidebar-whats-new-line",
                self.whats_new.clone(),
                move |mark| {
                    let opens = whats_new.clone();
                    div()
                        .flex_none()
                        .border_t_1()
                        .border_color(p.line)
                        .px(space::SM)
                        .py(space::XS)
                        .flex()
                        .items_center()
                        .gap(space::SM)
                        .child(
                            div().flex_1().min_w_0().child(
                                Button::quiet("sidebar-whats-new", mark)
                                    .full_width()
                                    .on_click(move |_, window, cx| {
                                        opens
                                            .update(cx, |shell, cx| {
                                                shell.open(Selection::WhatsNew, window, cx)
                                            })
                                            .ok();
                                    }),
                            ),
                        )
                        // The dot says it was not opened yet; it goes with the mark.
                        .child(
                            div()
                                .id("sidebar-whats-new-dot")
                                .flex_none()
                                .w(px(8.))
                                .h(px(8.))
                                .rounded_full()
                                .bg(p.acc)
                                .test_support(),
                        )
                        .into_any_element()
                },
            ))
            .child(presence_of(
                "sidebar-version-line",
                self.version_line.clone(),
                move |line| {
                    let opens = version.clone();
                    div()
                        .flex_none()
                        .border_t_1()
                        .border_color(p.line)
                        .px(space::SM)
                        .py(space::XS)
                        // Its one change (downloading, then ready) fades.
                        .child(changing("sidebar-version-label", line, move |line| {
                            Button::quiet("sidebar-version", line)
                                .full_width()
                                .on_click(move |_, window, cx| {
                                    opens
                                        .update(cx, |shell, cx| {
                                            shell.open_settings_at(
                                                crate::settings::Reveal::Version,
                                                window,
                                                cx,
                                            )
                                        })
                                        .ok();
                                })
                                .into_any_element()
                        }))
                        .into_any_element()
                },
            ))
            .child(modal)
            .test_support()
    }
}

/// A project's row and, folding under it, its sessions in one list.
pub struct ProjectNode {
    name: String,
    /// The machine whose section it is in, when the projects are shown by machine: its row then
    /// holds that machine's sessions only.
    machine: Option<String>,
    /// What the row says: the name, without what tells it from another of the same name.
    title: SharedString,
    /// The machine that has a project of this name which this is not: said in small after it.
    apart: Option<SharedString>,
    /// The machines that have it, each marked after the name; none while this machine is alone.
    homes: Vec<Home>,
    open: bool,
    mark: Mark,
    /// The strongest mark of what it holds, which it shows only while it is folded: open, the
    /// work is on its own row.
    held: Option<AgentState>,
    /// How many commands its agents run now.
    commands: usize,
    /// Its settings file is gone, and the daemon still holds sessions or workspaces of it: the
    /// row says so, and its menu offers Remove, with the confirmation (`docs/projects.md`,
    /// "Removing a project").
    unconfigured: bool,
    removal: Option<Entity<ProjectRemoval>>,
    menu: Option<(Entity<Menu>, Subscription)>,
    sessions: Entity<Level>,
    fold: Entity<Fold>,
    shell: WeakEntity<Shell>,
}

impl ProjectNode {
    /// The node of a project, unfolded or folded as the user left it: made so, it never animates
    /// from the other. `machine` is the section it is in, when the projects are shown by machine.
    pub fn new(
        name: String,
        machine: Option<String>,
        shell: WeakEntity<Shell>,
        open: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let sessions = cx.new(|_| Level::new());
        let key = match &machine {
            Some(machine) => format!("{name}-on-{machine}"),
            None => name.clone(),
        };
        let fold = cx.new(|_| Fold::new(format!("fold-project-{key}"), sessions.clone(), open));
        Self {
            title: name.clone().into(),
            name,
            machine,
            apart: None,
            homes: Vec::new(),
            open,
            mark: Mark::new(cx),
            held: None,
            commands: 0,
            unconfigured: false,
            removal: None,
            menu: None,
            sessions,
            fold,
            shell,
        }
    }

    /// The row of a project whose settings file is gone, which the daemon still holds sessions or
    /// workspaces of: this machine's, with its menu and Remove. It is a node of its own from its
    /// first frame to its last, never another machine's project of the same name.
    pub fn gone(
        name: String,
        machine: Option<String>,
        shell: WeakEntity<Shell>,
        open: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut node = Self::new(name, machine, shell, open, cx);
        node.unconfigured = true;
        let removal = cx.new(|_| ProjectRemoval::new(node.shell.clone().into(), node.name.clone()));
        node.removal = Some(removal.clone());
        let name = node.name.clone();
        let menu = cx.new(|cx| {
            Menu::new(project_menu_id(&name), "Project actions", cx).danger(
                project_remove_id(&name),
                "Remove…",
                move |_, cx| removal.update(cx, |removal, cx| removal.ask(cx)),
            )
        });
        // The row keeps the menu in sight while it is open: it draws again as it changes.
        let watch = cx.observe(&menu, |_, _, cx| cx.notify());
        node.menu = Some((menu, watch));
        // Its ids are made from the key, which says it: so the fold is made again.
        let key = node.key();
        let sessions = node.sessions.clone();
        node.fold = cx.new(|_| Fold::new(format!("fold-project-{key}"), sessions, open));
        node
    }

    /// The project's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// What its ids are made from: its name, and its machine in a machine's section; a row of a
    /// project with no file is [`GONE`] before.
    fn key(&self) -> String {
        let key = match &self.machine {
            Some(machine) => format!("{}-on-{machine}", self.name),
            None => self.name.clone(),
        };
        match self.unconfigured {
            true => format!("{GONE}{key}"),
            false => key,
        }
    }

    /// What the list of projects tells its rows by: the name, [`GONE`] before it on a row of a
    /// project with no file.
    fn list_key(&self) -> String {
        match self.unconfigured {
            true => format!("{GONE}{}", self.name),
            false => self.name.clone(),
        }
    }

    /// What the ids that are not the key's are made from: the name, [`GONE`] before it on a row
    /// of a project with no file.
    fn id_name(&self) -> String {
        self.list_key()
    }

    /// The machine whose section it is in.
    pub fn machine(&self) -> Option<&str> {
        self.machine.as_deref()
    }

    pub fn mark(&self) -> Option<AgentState> {
        self.mark.shown
    }

    /// The strongest mark of what it holds, whether it is folded or not.
    pub fn held(&self) -> Option<AgentState> {
        self.held
    }

    /// The machines that have it, as the row marks them.
    pub fn homes(&self) -> &[Home] {
        &self.homes
    }

    /// The machine said in small after its name, when it has a namesake that is not it.
    pub fn apart(&self) -> Option<&str> {
        self.apart.as_deref()
    }

    /// Whether every machine that has it is off: the row is greyed.
    pub fn offline(&self) -> bool {
        !self.homes.is_empty() && self.homes.iter().all(|home| home.offline)
    }

    /// Whether it says this already.
    pub fn says(&self, title: &str, apart: Option<&str>, homes: &[Home]) -> bool {
        self.title.as_ref() == title && self.apart.as_deref() == apart && self.homes == homes
    }

    /// What the row says of where the project lives: its title, the machine that tells it from a
    /// namesake, and the machines that have it.
    pub fn set_look(
        &mut self,
        title: &str,
        apart: Option<&str>,
        homes: Vec<Home>,
        cx: &mut Context<Self>,
    ) {
        if !self.says(title, apart, &homes) {
            self.title = title.to_owned().into();
            self.apart = apart.map(|apart| apart.to_owned().into());
            self.homes = homes;
            cx.notify();
        }
    }

    /// How many commands its agents run now.
    pub fn commands(&self) -> usize {
        self.commands
    }

    pub fn set_commands(&mut self, commands: usize, cx: &mut Context<Self>) {
        if self.commands != commands {
            self.commands = commands;
            cx.notify();
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Whether its settings file is gone.
    pub fn unconfigured(&self) -> bool {
        self.unconfigured
    }

    /// The confirmation of its removal, while its settings file is gone.
    pub fn removal(&self) -> Option<&Entity<ProjectRemoval>> {
        self.removal.as_ref()
    }

    /// Its sessions' rows in the order drawn: the one that moved last first.
    pub fn sessions(&self, cx: &App) -> Vec<Entity<SessionRow>> {
        self.sessions.read(cx).rows.clone()
    }

    /// Whether it holds these sessions and this mark already.
    pub fn holds(
        &self,
        sessions: &[Entity<SessionRow>],
        mark: Option<AgentState>,
        cx: &App,
    ) -> bool {
        self.held == mark && self.sessions.read(cx).rows == sessions
    }

    pub fn set(
        &mut self,
        sessions: Vec<Entity<SessionRow>>,
        mark: Option<AgentState>,
        cx: &mut Context<Self>,
    ) {
        self.set_rows(sessions, cx);
        self.held = mark;
        self.show_mark(cx);
    }

    /// Its sessions' rows alone, as a fold under a session's row changed them.
    pub fn set_rows(&mut self, sessions: Vec<Entity<SessionRow>>, cx: &mut Context<Self>) {
        let rows = &self.sessions.read(cx).rows;
        if *rows != sessions {
            // Its chevron comes with its first row and goes with its last.
            let chevron = rows.is_empty() != sessions.is_empty();
            self.sessions
                .update(cx, |level, cx| level.set(sessions, cx));
            if chevron {
                cx.notify();
            }
        }
    }

    fn show_mark(&mut self, cx: &mut Context<Self>) {
        let shown = self.held.filter(|_| !self.open);
        if self.mark.set(shown, cx) {
            cx.notify();
        }
    }

    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        self.open = !self.open;
        let open = self.open;
        self.fold.update(cx, |fold, cx| fold.set_open(open, cx));
        self.show_mark(cx);
        cx.notify();
        kept_changed(&self.shell, cx);
    }
}

/// What a row says of a project whose settings file is gone.
const MISSING: &str = "settings file missing";

/// A machine's mark (the kit's `MachineMark`): its icon in its colour, a badge whose shape says
/// its state, and its name and state in words in its tooltip and label, for the eyes that do not
/// see colours. On a row that fills (a heading, which `leading_with` hands its fill), the badge's
/// cut-out takes that fill: on the tree's own colour it would show as a ring on a hovered row.
fn machine_mark(
    id: ElementId,
    home: &Home,
    size: MarkSize,
    on: Option<gpui_kit::Hsla>,
) -> gpui_kit::AnyElement {
    let mark = MachineMark::new(
        id,
        glyphs::device(&home.icon),
        shade(&home.colour),
        home.presence(),
        home.said(),
    )
    .size(size);
    let mark = match on {
        Some(fill) => mark.on(fill),
        None => mark,
    };
    // The badge hangs out of the icon by a quarter of its side at most (4 px at 16): the room
    // left for it beside the next element.
    div()
        .flex_none()
        .flex()
        .items_center()
        .pr(space::XS)
        .child(mark)
        .into_any_element()
}

impl Render for ProjectNode {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shell = self.shell.clone();
        let name = self.name.clone();
        let key = self.key();
        let p = palette(cx);
        let id = match (&self.machine, self.unconfigured) {
            (Some(machine), false) => project_on_id(&self.name, machine),
            (None, false) => project_id(&self.name),
            (Some(machine), true) => project_gone_on_id(&self.name, machine),
            (None, true) => project_gone_id(&self.name),
        };
        // The machines' marks fade over each other as the machines that have the project change.
        let marks_said = SharedString::from(
            self.homes
                .iter()
                .map(|home| format!("{}:{}:{}", home.machine, home.icon, home.colour))
                .collect::<Vec<_>>()
                .join(","),
        );
        let shown = changed(
            &ElementId::Name(format!("project-marks-{key}").into()),
            &marks_said,
            window,
            cx,
        );
        // Inside a machine's group, the heading has the machine's mark: its projects do not repeat it.
        let marks: Vec<_> = self
            .homes
            .iter()
            .filter(|_| self.machine.is_none())
            .map(|home| {
                machine_mark(
                    project_mark_id(&self.name, &home.machine),
                    home,
                    MarkSize::Medium,
                    None,
                )
            })
            .collect();
        let apart = self.apart.clone().map(|machine| {
            div()
                .id(project_apart_id(&self.id_name()))
                .flex_none()
                .aria_label(machine.clone())
                .child(meta(machine, cx))
                .test_support()
        });
        // After the name, before the project's own mark, as the approved prototype draws them.
        let marks = (!marks.is_empty()).then(|| {
            div()
                .flex()
                .items_center()
                .gap(space::XS)
                .opacity(shown)
                .children(marks)
        });
        let trailing = div()
            .flex()
            .items_center()
            .gap(space::SM)
            .children(apart)
            .children(marks)
            .child(trailing(
                project_commands_id(&key),
                self.commands,
                self.mark.icon(&format!("project-{key}")),
                cx,
            ));
        // A project with no file has no page: its row folds and unfolds what it holds.
        let unconfigured = self.unconfigured;
        let toggled = cx.entity().downgrade();
        // A project with nothing under it has nothing to fold: the kit fades its chevron out where
        // it stands, so its title starts where the others' do, and back in when something comes.
        let holds = !self.sessions.read(cx).rows.is_empty();
        let row = TreeLevel::Project
            .row(id, self.title.clone())
            .expanded(self.open)
            .foldable(holds)
            .trailing(trailing)
            .on_toggle(cx.listener(|this, _, _, cx| this.toggle(cx)))
            .when_some(self.menu.as_ref(), |row, (menu, _)| {
                row.actions(menu.clone(), menu.read(cx).is_open())
            })
            // Not a `cx.listener`: that runs inside this node's update, and opening a page may
            // update this node again (#993).
            .on_click(move |_, window, cx| {
                let project = name.clone();
                match unconfigured {
                    true => {
                        toggled.update(cx, |node, cx| node.toggle(cx)).ok();
                    }
                    false => {
                        shell
                            .update(cx, |shell, cx| {
                                shell.open(Selection::Project(project), window, cx)
                            })
                            .ok();
                    }
                }
            });
        // A project only machines that are off have: its row fades its ink, and says since when.
        let greyed = fade(
            ElementId::Name(format!("project-greyed-fade-{key}").into()),
            self.offline(),
            PANEL,
            window,
            cx,
        );
        let since = match self.homes.as_slice() {
            [home] => home.state.clone(),
            _ => "offline".to_owned(),
        };
        let offline = presence(
            ElementId::Name(format!("project-offline-fade-{key}").into()),
            self.offline(),
            div()
                .id(project_offline_id(&self.id_name()))
                .pl(space::XXL)
                .pb(space::XS)
                .text_size(px(11.5))
                .text_color(p.faint)
                .aria_label(SharedString::from(since.clone()))
                .child(SharedString::from(since))
                .test_support(),
        );
        // What is wrong with it, in words, under its name; it unfolds when the file goes and folds
        // away when it comes back.
        let missing = presence(
            ElementId::Name(format!("project-unconfigured-fade-{key}").into()),
            self.unconfigured,
            div()
                .id(project_unconfigured_id(&self.name))
                .pl(space::XXL)
                .pb(space::XS)
                .text_size(px(11.5))
                .text_color(p.faint)
                .aria_label(SharedString::from(MISSING))
                .child(SharedString::from(MISSING))
                .test_support(),
        );
        div()
            .flex()
            .flex_col()
            .flex_none()
            .child(
                div()
                    .opacity(1.0 - GREYED * greyed)
                    .flex()
                    .flex_col()
                    .child(row)
                    .child(missing)
                    .child(offline),
            )
            .children(self.removal.clone().filter(|_| self.unconfigured))
            // A press on what the project holds (a session's row, a button) is that row's own: it
            // never starts the drag of the whole project, which begins on the project's row.
            .child(
                div()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(self.fold.clone()),
            )
    }
}

/// The projects of a machine's section, on a guide line.
pub struct Projects {
    rows: Vec<Entity<ProjectNode>>,
    list: Listing<Entity<ProjectNode>>,
}

impl Projects {
    fn new() -> Self {
        Self {
            rows: Vec::new(),
            list: Listing::default(),
        }
    }

    fn set(&mut self, rows: Vec<Entity<ProjectNode>>, cx: &mut Context<Self>) {
        if self.rows != rows {
            self.rows = rows;
            cx.notify();
        }
    }
}

impl Render for Projects {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = framed(
            &mut self.list,
            &self.rows,
            "tree-on-machine",
            |_, _| false,
            cx,
        );
        match rows.is_empty() {
            true => div().into_any_element(),
            false => tree_children(rows, cx).into_any_element(),
        }
    }
}

/// A machine's section, when the projects are shown by machine: its icon in its colour, its name
/// and its state, folding over the projects it has.
pub struct MachineNode {
    machine: String,
    home: Home,
    open: bool,
    projects: Entity<Projects>,
    fold: Entity<Fold>,
    shell: WeakEntity<Shell>,
}

impl MachineNode {
    /// The section of `home`, unfolded or folded as the user left it.
    pub fn new(home: Home, shell: WeakEntity<Shell>, open: bool, cx: &mut Context<Self>) -> Self {
        let projects = cx.new(|_| Projects::new());
        let fold = cx.new(|_| {
            Fold::new(
                format!("fold-machine-{}", home.machine),
                projects.clone(),
                open,
            )
        });
        Self {
            machine: home.machine.clone(),
            home,
            open,
            projects,
            fold,
            shell,
        }
    }

    pub fn machine(&self) -> &str {
        &self.machine
    }

    pub fn home(&self) -> &Home {
        &self.home
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Its projects' rows, in order.
    pub fn projects(&self, cx: &App) -> Vec<Entity<ProjectNode>> {
        self.projects.read(cx).rows.clone()
    }

    pub fn set(&mut self, home: Home, projects: Vec<Entity<ProjectNode>>, cx: &mut Context<Self>) {
        if self.projects.read(cx).rows != projects {
            self.projects
                .update(cx, |level, cx| level.set(projects, cx));
        }
        if self.home != home {
            self.home = home;
            cx.notify();
        }
    }

    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        self.open = !self.open;
        let open = self.open;
        self.fold.update(cx, |fold, cx| fold.set_open(open, cx));
        cx.notify();
        kept_changed(&self.shell, cx);
    }
}

impl Render for MachineNode {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Its state is the mark's badge, said in its tooltip: the heading ends with no words.
        let (id, home) = (machine_mark_id(&self.machine), self.home.clone());
        let row = TreeLevel::Project
            .row(machine_id(&self.machine), self.machine.clone())
            .expanded(self.open)
            .leading_with(move |fill| machine_mark(id, &home, MarkSize::Large, Some(fill)))
            .on_toggle(cx.listener(|this, _, _, cx| this.toggle(cx)))
            .on_click(cx.listener(|this, _, _, cx| this.toggle(cx)));
        // A machine that is off fades its ink, and takes it back when it is online.
        let greyed = fade(
            ElementId::Name(format!("machine-greyed-fade-{}", self.machine).into()),
            self.home.offline,
            PANEL,
            window,
            cx,
        );
        div()
            .flex()
            .flex_col()
            .flex_none()
            .child(div().opacity(1.0 - GREYED * greyed).child(row))
            .child(self.fold.clone())
    }
}

/// The rows of one level, on a guide line: the sessions of a project, or of the "Ended" group.
pub struct Level {
    rows: Vec<Entity<SessionRow>>,
    list: Listing<Entity<SessionRow>>,
    /// Takes at most [`ENDED_ROWS`] rows, or [`ENDED_SHARE`] of the window when that is less, and
    /// scrolls inside itself beyond it, in the block it names; `None` for a project's level.
    scroll: Option<ElementId>,
}

/// How many rows the "Ended" group shows before it scrolls.
const ENDED_ROWS: f32 = 8.;
/// The height of a session's row of one line, which the "Ended" group's rows are.
const ROW: f32 = 28.;
/// The share of the window's height the "Ended" group takes at most, when that is less than
/// [`ENDED_ROWS`] rows.
const ENDED_SHARE: f32 = 0.35;

impl Level {
    fn new() -> Self {
        Self {
            rows: Vec::new(),
            list: Listing::default(),
            scroll: None,
        }
    }

    /// The level of the "Ended" group, which scrolls inside itself.
    fn scrolling() -> Self {
        Self::scrolling_as(ended_scroll_id())
    }

    /// A level of a group pinned at the bottom, which scrolls inside itself in the block `id`.
    fn scrolling_as(id: ElementId) -> Self {
        Self {
            scroll: Some(id),
            ..Self::new()
        }
    }

    fn set(&mut self, rows: Vec<Entity<SessionRow>>, cx: &mut Context<Self>) {
        if self.rows != rows {
            self.rows = rows;
            cx.notify();
        }
    }
}

impl Render for Level {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = framed(
            &mut self.list,
            &self.rows,
            "tree-row",
            |row, cx| row.read(cx).led_by().is_some(),
            cx,
        );
        if rows.is_empty() {
            return div().into_any_element();
        }
        let rows = tree_children(rows, cx);
        let Some(id) = self.scroll.clone() else {
            return rows.into_any_element();
        };
        let most = (px(ENDED_ROWS * ROW) + px(4.)).min(window.viewport_size().height * ENDED_SHARE);
        div()
            .id(id)
            .max_h(most)
            .overflow_y_scroll()
            .child(rows)
            .test_support()
            .into_any_element()
    }
}

/// A node was folded or unfolded: the shell keeps what the sidebar has folded, once the node's
/// own update is over (the shell reads the nodes).
fn kept_changed<T: 'static>(shell: &WeakEntity<Shell>, cx: &mut Context<T>) {
    let shell = shell.clone();
    cx.defer(move |cx| {
        shell.update(cx, |shell, cx| shell.sidebar_changed(cx)).ok();
    });
}

/// The "Ended" group, at the bottom of the tree: it gathers the ended sessions of every project,
/// folded until the user opens it.
pub struct EndedNode {
    open: bool,
    /// Why the ended sessions of a machine are not listed (one that is off), said once (#1280).
    unlisted: Option<SharedString>,
    sessions: Entity<Level>,
    fold: Entity<Fold>,
    shell: WeakEntity<Shell>,
}

impl EndedNode {
    /// The group, folded unless the user left it unfolded.
    pub fn new(shell: WeakEntity<Shell>, open: bool, cx: &mut Context<Self>) -> Self {
        let sessions = cx.new(|_| Level::scrolling());
        let fold = cx.new(|_| Fold::new("fold-ended", sessions.clone(), open));
        Self {
            open,
            unlisted: None,
            sessions,
            fold,
            shell,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// What its line says of the machines whose ended sessions are not listed.
    pub fn unlisted(&self) -> Option<&str> {
        self.unlisted.as_deref()
    }

    pub fn set_unlisted(&mut self, said: Option<String>, cx: &mut Context<Self>) {
        self.unlisted = said.map(SharedString::from);
        cx.notify();
    }

    /// Its sessions' rows, newest first.
    pub fn sessions(&self, cx: &gpui_kit::App) -> Vec<Entity<SessionRow>> {
        self.sessions.read(cx).rows.clone()
    }

    /// Whether it holds these sessions already.
    pub fn holds(&self, sessions: &[Entity<SessionRow>], cx: &App) -> bool {
        self.sessions.read(cx).rows == sessions
    }

    pub fn set(&mut self, sessions: Vec<Entity<SessionRow>>, cx: &mut Context<Self>) {
        if self.sessions.read(cx).rows != sessions {
            self.sessions
                .update(cx, |level, cx| level.set(sessions, cx));
            // The row counts them.
            cx.notify();
        }
    }

    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        self.open = !self.open;
        let open = self.open;
        self.fold.update(cx, |fold, cx| fold.set_open(open, cx));
        cx.notify();
        kept_changed(&self.shell, cx);
        // Opened, it asks every other machine for its ended sessions (#1280).
        if open {
            let shell = self.shell.clone();
            cx.defer(move |cx| {
                shell.update(cx, |shell, cx| shell.ended_opened(cx)).ok();
            });
        }
    }
}

impl Render for EndedNode {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.sessions.read(cx).rows.len();
        let row = TreeLevel::Session
            .row(ended_id(), "Ended")
            .unfolds_upward()
            .trailing(meta(count.to_string(), cx))
            .expanded(self.open)
            .on_click(cx.listener(|this, _, _, cx| this.toggle(cx)));
        let p = palette(cx);
        div()
            .flex()
            .flex_col()
            .flex_none()
            .child(row)
            .child(self.fold.clone())
            .child(presence_of(
                "ended-unlisted-line",
                self.unlisted.clone().filter(|_| self.open),
                move |line| {
                    div()
                        .id("ended-unlisted")
                        .aria_label(line.clone())
                        .px(space::LG)
                        .pb(space::SM)
                        .text_size(px(12.))
                        .text_color(p.faint)
                        .child(line)
                        .test_support()
                        .into_any_element()
                },
            ))
    }
}

/// The "Blocked by others" group, above "Ended": the roots at rest that wait on a person, of every
/// project, folded until the user opens it (`docs/design.md` §10). Its heading says how many and
/// who; it is absent while it holds none.
pub struct BlockedNode {
    open: bool,
    /// Whom its sessions wait on, each once.
    who: Vec<String>,
    sessions: Entity<Level>,
    fold: Entity<Fold>,
    shell: WeakEntity<Shell>,
}

impl BlockedNode {
    /// The group, folded unless the user left it unfolded.
    pub fn new(shell: WeakEntity<Shell>, open: bool, cx: &mut Context<Self>) -> Self {
        let sessions = cx.new(|_| Level::scrolling_as(blocked_scroll_id()));
        let fold = cx.new(|_| Fold::new("fold-blocked", sessions.clone(), open));
        Self {
            open,
            who: Vec::new(),
            sessions,
            fold,
            shell,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Its sessions' rows, the most recently blocked first.
    pub fn sessions(&self, cx: &App) -> Vec<Entity<SessionRow>> {
        self.sessions.read(cx).rows.clone()
    }

    /// What its heading says beside its name: `2 · Maya, Noor`.
    pub fn said(&self, cx: &App) -> String {
        heading_words(self.sessions.read(cx).rows.len(), &self.who)
    }

    /// Whether it holds these sessions, blocked by these people, already.
    pub fn holds(&self, sessions: &[Entity<SessionRow>], who: &[String], cx: &App) -> bool {
        self.sessions.read(cx).rows == sessions && self.who == who
    }

    pub fn set(
        &mut self,
        sessions: Vec<Entity<SessionRow>>,
        who: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        if self.sessions.read(cx).rows != sessions {
            self.sessions
                .update(cx, |level, cx| level.set(sessions, cx));
        }
        self.who = who;
        // The heading counts them and names who.
        cx.notify();
    }

    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        self.open = !self.open;
        let open = self.open;
        self.fold.update(cx, |fold, cx| fold.set_open(open, cx));
        cx.notify();
        kept_changed(&self.shell, cx);
    }
}

/// The words of the "Blocked by others" heading: the count, then who.
fn heading_words(count: usize, who: &[String]) -> String {
    match who.is_empty() {
        true => count.to_string(),
        false => format!("{count} · {}", who.join(", ")),
    }
}

/// The element id of the block inside the "Blocked by others" group that holds its rows.
pub fn blocked_scroll_id() -> ElementId {
    ElementId::Name("blocked-sessions".into())
}

impl Render for BlockedNode {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let said = SharedString::from(self.said(cx));
        let faint = palette(cx).faint;
        // The count and the names fade to their new words.
        let words = div()
            .id(blocked_who_id())
            .aria_label(said.clone())
            .min_w_0()
            .overflow_hidden()
            .test_support()
            .child(changing("blocked-who-words", said, move |said| {
                div()
                    .text_size(px(11.))
                    .text_color(faint)
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(said)
                    .into_any_element()
            }));
        let row = TreeLevel::Session
            .row(blocked_id(), "Blocked by others")
            .unfolds_upward()
            .trailing(words)
            .expanded(self.open)
            .on_click(cx.listener(|this, _, _, cx| this.toggle(cx)));
        div()
            .flex()
            .flex_col()
            .flex_none()
            .child(row)
            .child(self.fold.clone())
    }
}

/// What ends a session's row: one mark, chosen in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trailing {
    /// The session runs, needs the user or waits.
    State(AgentState),
    /// Something new was written in it since it was last opened.
    Unread,
    /// How long ago it was last active.
    Ago,
}

/// A session's row: its title, and on the right one mark: its state when it runs, needs the user
/// or waits, else a dot when something new waits to be seen, else how long ago it was last
/// active.
pub struct SessionRow {
    id: String,
    /// How many times it was drawn: the tests count them.
    renders: usize,
    title: SharedString,
    mark: Mark,
    /// How many commands its agents run now.
    commands: usize,
    /// What is new in the session since it was last opened, the last of it in words: a small dot
    /// on the row says it until the session is opened, unless a state takes its place.
    unread: Option<SharedString>,
    /// Its composer holds something unsent: a pencil says it once the session is not the open
    /// one.
    draft: bool,
    /// What its state mark means, in words.
    said: SharedString,
    /// The words of the mark shown, kept while it folds away.
    words: SharedString,
    /// What it waits for, for the second line, and the last words it said, which stay drawn
    /// while the line folds away.
    waits: SharedString,
    waits_shown: SharedString,
    /// What a led session runs on, for the second line, and the last words it said, which stay
    /// drawn while the line folds away; empty for a session no agent leads.
    runs: SharedString,
    runs_shown: SharedString,
    active_at: u64,
    ago: SharedString,
    selected: bool,
    /// The project it belongs to, said in the "Ended" group where projects are mixed; a row with
    /// none is a live one, which says its place on a second line.
    scope: Option<SharedString>,
    /// Where it works: `main`, or a workspace.
    place: Place,
    /// Its place's pull request, when the window has read it, and the last one it showed, which
    /// stays drawn while it fades out.
    pull: Option<PullRequest>,
    pull_shown: Option<PullRequest>,
    /// What its "⋯" menu offers: End session while it lives, Resume when it ended and can.
    actions: Actions,
    /// The agent that started it, while it is not detached: its menu offers "Detach from …".
    detach: Option<SharedString>,
    menu: Option<(Entity<Menu>, Subscription)>,
    /// The agent that leads it, while its row sits under its pilot's: the row is indented and tied
    /// to it.
    led_by: Option<SharedString>,
    /// The state of the session itself, which the mark shows unless it hides a led session that
    /// asks more of the user.
    own: Option<AgentState>,
    /// The most urgent state of the led sessions its folded row hides.
    hidden: Option<AgentState>,
    /// How many led sessions sit under its row, and whether they are unfolded.
    leads: usize,
    open: bool,
    /// Where its row sits among the sessions its pilot leads, which the connector to the row
    /// above draws, and the last one it drew, which stays while it fades out.
    tie: Option<Tie>,
    tie_shown: Tie,
    /// It runs on another machine: its second line says which and its state as last known, and
    /// opening it shows that machine.
    elsewhere: Option<crate::model::Elsewhere>,
    /// The machine it runs on, when it is another one, as the sidebar marks it.
    home: Option<Home>,
    /// The machine it runs on, when it is not this one: its second line says it.
    tag: Option<SharedString>,
    /// What it is about, as the daemon derives it, and whether the user named it (their title
    /// keeps the first line, the key goes to the second).
    subject: Option<crate::model::Subject>,
    renamed: bool,
    /// The last key its chip said, which stays drawn while the chip fades out.
    key_shown: SharedString,
    shell: WeakEntity<Shell>,
}

/// How far the ink of a session whose machine is off fades.
const GREYED: f32 = 0.45;

/// What a session's row offers under the pointer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Actions {
    #[default]
    None,
    End,
    Resume,
}

impl SessionRow {
    pub fn new(id: String, shell: WeakEntity<Shell>, cx: &mut Context<Self>) -> Self {
        Self {
            id,
            renders: 0,
            title: SharedString::default(),
            mark: Mark::new(cx),
            commands: 0,
            unread: None,
            draft: false,
            said: SharedString::default(),
            words: SharedString::default(),
            waits: SharedString::default(),
            waits_shown: SharedString::default(),
            runs: SharedString::default(),
            runs_shown: SharedString::default(),
            active_at: 0,
            ago: SharedString::default(),
            selected: false,
            scope: None,
            place: None,
            pull: None,
            pull_shown: None,
            actions: Actions::None,
            detach: None,
            menu: None,
            led_by: None,
            own: None,
            hidden: None,
            leads: 0,
            open: true,
            tie: None,
            tie_shown: Tie::default(),
            elsewhere: None,
            home: None,
            tag: None,
            subject: None,
            renamed: false,
            key_shown: SharedString::default(),
            shell,
        }
    }

    /// Whether the row says this subject already, named or not by the user.
    pub fn holds_subject(&self, subject: Option<&crate::model::Subject>, renamed: bool) -> bool {
        self.subject.as_ref() == subject && self.renamed == renamed
    }

    pub fn set_subject(
        &mut self,
        subject: Option<&crate::model::Subject>,
        renamed: bool,
        cx: &mut Context<Self>,
    ) {
        if self.holds_subject(subject, renamed) {
            return;
        }
        self.subject = subject.cloned();
        self.renamed = renamed;
        if let Some(key) = self.key() {
            self.key_shown = key;
        }
        cx.notify();
    }

    /// The key of what it is about: `SHOP-60`, `#12`; none for a free session.
    fn key(&self) -> Option<SharedString> {
        let subject = self.subject.as_ref()?;
        subject
            .key
            .clone()
            .filter(|_| subject.kind != "free")
            .map(SharedString::from)
    }

    /// The words of its first line: what it is about, unless the user named it.
    pub fn first_line(&self) -> SharedString {
        match self.subject.as_ref().filter(|_| !self.renamed) {
            Some(subject) => subject.line().into(),
            None => self.title.clone(),
        }
    }

    /// The key's chip, fading in as it comes and out as it goes over `PANEL`, `beside` on the
    /// second line.
    fn key_chip(
        &self,
        beside: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui_kit::AnyElement> {
        let on = self.key().is_some() && self.renamed == beside;
        let fade_id = match beside {
            false => format!("session-key-fade-{}", self.id),
            true => format!("session-key-beside-fade-{}", self.id),
        };
        let t = fade(ElementId::Name(fade_id.into()), on, PANEL, window, cx);
        if t <= 0.0 || self.key_shown.is_empty() {
            return None;
        }
        let (key, gap) = match beside {
            false => (
                gpui_design_system::Tag::new(session_key_id(&self.id), self.key_shown.clone())
                    .into_any_element(),
                px(ROW_GAP),
            ),
            // The second line is 16 px high, a tag is taller: there the key is its mono word,
            // which fades to a new one as a tag's does and says itself whole when cut.
            true => {
                let id = session_key_beside_id(&self.id);
                let opacity = changed(&id, &self.key_shown, window, cx);
                (
                    tipped_when_cut(
                        div()
                            .id(id)
                            .aria_label(self.key_shown.clone())
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .mono(11.)
                            .opacity(opacity)
                            .child(self.key_shown.clone()),
                        self.key_shown.clone(),
                        window,
                        cx,
                    ),
                    space::SM,
                )
            }
        };
        Some(
            div()
                .flex_none()
                .min_w_0()
                .overflow_hidden()
                .max_w(px(KEY_WIDTH * t))
                // The gap after it comes and goes with it.
                .mr(-gap * (1.0 - t))
                .opacity(t)
                .child(key)
                .into_any_element(),
        )
    }

    /// The machine it runs on, when it is not this one.
    pub fn tag(&self) -> Option<&str> {
        self.tag.as_deref()
    }

    pub fn set_tag(&mut self, tag: Option<&str>, cx: &mut Context<Self>) {
        if self.tag.as_deref() != tag {
            self.tag = tag.map(|tag| SharedString::from(tag.to_owned()));
            cx.notify();
        }
    }

    /// Its machine is off: the row is greyed, and does not open.
    pub fn offline(&self) -> bool {
        self.elsewhere.as_ref().is_some_and(|there| there.offline)
    }

    /// The machine it runs on and its state, for a session on another machine.
    pub fn elsewhere(&self) -> Option<&crate::model::Elsewhere> {
        self.elsewhere.as_ref()
    }

    pub fn set_elsewhere(
        &mut self,
        elsewhere: Option<&crate::model::Elsewhere>,
        cx: &mut Context<Self>,
    ) {
        if self.elsewhere.as_ref() != elsewhere {
            self.elsewhere = elsewhere.cloned();
            cx.notify();
        }
    }

    /// The machine it runs on, when it is another one, as the sidebar marks it.
    pub fn set_home(&mut self, home: Option<&Home>, cx: &mut Context<Self>) {
        if self.home.as_ref() != home {
            self.home = home.cloned();
            cx.notify();
        }
    }

    /// Where its row sits among the sessions its pilot leads, `None` for a row that no session
    /// leads.
    pub fn tie(&self) -> Option<&Tie> {
        self.tie.as_ref()
    }

    pub fn set_tie(&mut self, tie: Option<Tie>, cx: &mut Context<Self>) {
        if self.tie != tie {
            if let Some(tie) = &tie {
                self.tie_shown = tie.clone();
            }
            self.tie = tie;
            cx.notify();
        }
    }

    /// The agent that leads it, while its row sits under its pilot's.
    pub fn led_by(&self) -> Option<&str> {
        self.led_by.as_deref()
    }

    pub fn set_led_by(&mut self, led_by: Option<&str>, cx: &mut Context<Self>) {
        if self.led_by() != led_by {
            self.led_by = led_by.map(|by| SharedString::from(by.to_owned()));
            cx.notify();
        }
    }

    /// How many led sessions sit under its row.
    pub fn leads(&self) -> usize {
        self.leads
    }

    pub fn set_leads(&mut self, leads: usize, cx: &mut Context<Self>) {
        if self.leads != leads {
            self.leads = leads;
            cx.notify();
        }
    }

    /// What its folded row hides.
    pub fn hidden(&self) -> Option<AgentState> {
        self.hidden
    }

    /// The most urgent state among the led sessions its folded row hides: the mark shows it when
    /// it asks more than the session's own. `None` while the row is unfolded.
    pub fn set_hidden(&mut self, hidden: Option<AgentState>, cx: &mut Context<Self>) {
        if self.hidden != hidden {
            self.hidden = hidden;
            if self.mark.set(self.shown_mark(), cx) {
                cx.notify();
            }
        }
    }

    /// What the mark shows: waiting for the user beats running, which beats the session's own.
    fn shown_mark(&self) -> Option<AgentState> {
        AgentState::strongest(self.own.into_iter().chain(self.hidden)).or(self.own)
    }

    /// Whether the led sessions under its row are unfolded.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Makes its led rows folded or unfolded as the user left them, before it is first listed:
    /// nothing animates.
    pub fn restore_open(&mut self, open: bool) {
        self.open = open;
    }

    /// Folds or unfolds the led sessions under its row: the tree draws its level again.
    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        self.open = !self.open;
        cx.notify();
        // The level reads this row: it is drawn again once this update is over.
        let (shell, id) = (self.shell.clone(), self.id.clone());
        cx.defer(move |cx| {
            shell
                .update(cx, |shell, cx| {
                    shell.relist(&id, cx);
                    shell.sidebar_changed(cx);
                })
                .ok();
        });
    }

    /// What its meta says of its project (the "Ended" group).
    pub fn scope(&self) -> Option<&str> {
        self.scope.as_deref()
    }

    /// Says its project in its meta (the "Ended" group), or stops.
    pub fn set_scope(&mut self, scope: Option<&str>, cx: &mut Context<Self>) {
        if self.scope.as_deref() != scope {
            self.scope = scope.map(|scope| SharedString::from(scope.to_owned()));
            cx.notify();
        }
    }

    /// The place it works in, `None` for the project folder.
    pub fn place(&self) -> &Place {
        &self.place
    }

    pub fn set_place(&mut self, place: &Place, cx: &mut Context<Self>) {
        if &self.place != place {
            self.place = place.clone();
            cx.notify();
        }
    }

    /// The pull request of its place, which the second line shows when there is room.
    pub fn pull(&self) -> Option<&PullRequest> {
        self.pull.as_ref()
    }

    pub fn set_pull(&mut self, pull: Option<&PullRequest>, cx: &mut Context<Self>) {
        if self.pull.as_ref() != pull {
            self.pull = pull.cloned();
            if self.pull.is_some() {
                self.pull_shown = self.pull.clone();
            }
            cx.notify();
        }
    }

    /// What the row's menu offers: End session for a session that lives, and "Detach from …"
    /// while an agent that started it (`detach`) still has it; Resume for an ended one whose
    /// agent can resume; nothing else.
    pub fn set_actions(
        &mut self,
        ended: bool,
        resumable: bool,
        detach: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let actions = match (ended, resumable) {
            (false, _) => Actions::End,
            (true, true) => Actions::Resume,
            (true, false) => Actions::None,
        };
        let detach = detach.filter(|_| actions == Actions::End);
        if self.actions == actions && self.detach.as_deref() == detach {
            return;
        }
        self.actions = actions;
        self.detach = detach.map(|by| SharedString::from(by.to_owned()));
        let detach = self.detach.clone();
        // A session another machine lists is renamed there, never from its light row.
        let renamed = self.elsewhere.is_none();
        self.menu = match actions {
            Actions::None => None,
            offered => {
                let (shell, id) = (self.shell.clone(), self.id.clone());
                let menu = cx.new(|cx| {
                    let mut menu = Menu::new(session_menu_id(&id), "Session actions", cx);
                    if renamed {
                        let (shell, id) = (shell.clone(), id.clone());
                        menu = menu.entry(
                            session_rename_id(&id),
                            crate::rename::RENAME_LABEL,
                            Some(Icon::Edit),
                            move |window, cx| {
                                let id = id.clone();
                                shell
                                    .update(cx, |shell, cx| shell.rename_of(&id, window, cx))
                                    .ok();
                            },
                        );
                    }
                    if let Some(by) = detach {
                        let (shell, id) = (shell.clone(), id.clone());
                        menu = menu
                            .entry(
                                session_detach_id(&id),
                                crate::session::DETACH,
                                Some(Icon::Detach),
                                move |_, cx| {
                                    let id = id.clone();
                                    shell.update(cx, |shell, cx| shell.detach_of(&id, cx)).ok();
                                },
                            )
                            .detail(by.to_string());
                    }
                    match offered {
                        Actions::End => menu.entry(
                            session_end_id(&id),
                            "End session…",
                            Some(glyphs::STOP),
                            move |_, cx| {
                                let id = id.clone();
                                shell.update(cx, |shell, cx| shell.ask_end_of(&id, cx)).ok();
                            },
                        ),
                        _ => menu.entry(
                            session_resume_id(&id),
                            "Resume",
                            Some(Icon::Play),
                            move |_, cx| {
                                let id = id.clone();
                                shell.update(cx, |shell, cx| shell.resume_of(&id, cx)).ok();
                            },
                        ),
                    }
                });
                // The row keeps the menu in sight while it is open: it draws again as it changes.
                let watch = cx.observe(&menu, |_, _, cx| cx.notify());
                Some((menu, watch))
            }
        };
        cx.notify();
    }

    pub fn mark(&self) -> Option<AgentState> {
        self.mark.shown
    }

    /// How many commands its agents run now.
    pub fn commands(&self) -> usize {
        self.commands
    }

    pub fn set_commands(&mut self, commands: usize, cx: &mut Context<Self>) {
        if self.commands != commands {
            self.commands = commands;
            cx.notify();
        }
    }

    /// What is new in the session since it was last opened, in words.
    pub fn unread(&self) -> Option<&str> {
        self.unread.as_deref()
    }

    pub fn set_unread(&mut self, unread: Option<&str>, cx: &mut Context<Self>) {
        if self.unread() != unread {
            self.unread = unread.map(|said| SharedString::from(said.to_owned()));
            cx.notify();
        }
    }

    /// Whether its composer holds a draft, in the session being open or not.
    pub fn draft(&self) -> bool {
        self.draft
    }

    pub fn set_draft(&mut self, draft: bool, cx: &mut Context<Self>) {
        if self.draft != draft {
            self.draft = draft;
            cx.notify();
        }
    }

    /// What its state mark means, in words.
    pub fn said(&self) -> &str {
        &self.said
    }

    pub fn set_said(&mut self, said: &str, cx: &mut Context<Self>) {
        if self.said() != said {
            self.said = SharedString::from(said.to_owned());
            cx.notify();
        }
    }

    /// What it waits for, in a few words: empty unless it waits.
    pub fn waits(&self) -> &str {
        &self.waits
    }

    pub fn set_waits(&mut self, waits: &str, cx: &mut Context<Self>) {
        if self.waits() != waits {
            self.waits = SharedString::from(waits.to_owned());
            if !waits.is_empty() {
                self.waits_shown = self.waits.clone();
            }
            cx.notify();
        }
    }

    /// What it runs on, the model and the effort by name: empty unless an agent leads it.
    pub fn runs(&self) -> &str {
        &self.runs
    }

    pub fn set_runs(&mut self, runs: &str, cx: &mut Context<Self>) {
        if self.runs() != runs {
            self.runs = SharedString::from(runs.to_owned());
            if !runs.is_empty() {
                self.runs_shown = self.runs.clone();
            }
            cx.notify();
        }
    }

    /// The one mark the row ends with: its state, else the unread dot, else the time ago.
    pub fn trailing(&self) -> Trailing {
        match (self.mark.shown, &self.unread) {
            (Some(state), _) => Trailing::State(state),
            (None, Some(_)) => Trailing::Unread,
            (None, None) => Trailing::Ago,
        }
    }

    /// The session's root agent.
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn renders(&self) -> usize {
        self.renders
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    /// The model the row's mark reads; the session page's pill reads it too.
    pub fn status(&self) -> Entity<Status> {
        self.mark.status.clone()
    }

    /// Whether it shows this already.
    pub fn holds(&self, title: &str, mark: Option<AgentState>, active_at: u64) -> bool {
        self.title.as_ref() == title && self.own == mark && self.active_at == active_at
    }

    pub fn set(
        &mut self,
        title: &str,
        mark: Option<AgentState>,
        active_at: u64,
        now: u64,
        cx: &mut Context<Self>,
    ) {
        self.own = mark;
        let mut changed = self.mark.set(self.shown_mark(), cx);
        if self.title.as_ref() != title {
            self.title = SharedString::from(title.to_owned());
            changed = true;
        }
        // Time passing alone is the clock's to show (`tick`): a read of the daemon that changed
        // nothing of this session draws nothing.
        if self.active_at != active_at || self.ago.is_empty() {
            self.active_at = active_at;
            changed |= self.set_ago(now);
        }
        if changed {
            cx.notify();
        }
    }

    /// Whether the time ago it shows is no longer the right one.
    pub fn ago_changes(&self, now: u64) -> bool {
        self.mark.shown.is_none() && self.ago.as_ref() != ago(now, self.active_at)
    }

    /// The time ago moves on by itself: drawn again only when its text changes.
    pub fn tick(&mut self, now: u64, cx: &mut Context<Self>) {
        if self.mark.shown.is_none() && self.set_ago(now) {
            cx.notify();
        }
    }

    fn set_ago(&mut self, now: u64) -> bool {
        let ago = ago(now, self.active_at);
        let changed = self.ago.as_ref() != ago;
        if changed {
            self.ago = ago.into();
        }
        changed && self.mark.shown.is_none()
    }

    pub fn set_selected(&mut self, selected: bool, cx: &mut Context<Self>) {
        if self.selected != selected {
            self.selected = selected;
            cx.notify();
        }
    }
}

impl Render for SessionRow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let shell = self.shell.clone();
        let id = self.id.clone();
        let elsewhere = self.elsewhere.clone();
        let mut row = TreeLevel::Session
            .row(session_id(&self.id), self.first_line())
            // Nothing in hand: its words have nothing to say.
            .faint(self.first_line() == crate::model::FREE)
            .selected(self.selected)
            .on_click(move |_, window, cx| {
                let session = id.clone();
                let elsewhere = elsewhere.clone();
                shell
                    .update(cx, |shell, cx| match elsewhere {
                        // Its machine is off: nothing of it can be read now.
                        Some(there) if there.offline => {}
                        // This machine's, listed while another is in front: this one comes back.
                        Some(there) if there.here => shell.open_here(there.agent, window, cx),
                        // Another machine's: its page is that machine's, on that session.
                        Some(there) => shell.open_elsewhere(there.machine, there.agent, window, cx),
                        None => shell.open(Selection::Session(session), window, cx),
                    })
                    .ok();
            })
            .when_some(self.menu.as_ref(), |row, (menu, _)| {
                row.actions(menu.clone(), menu.read(cx).is_open())
            })
            .when(self.leads > 0, |row| {
                row.expanded(self.open)
                    .on_toggle(cx.listener(|this, _, _, cx| this.toggle(cx)))
            });
        let when = match &self.scope {
            Some(scope) => SharedString::from(format!("{scope} · {}", self.ago)),
            None => self.ago.clone(),
        };
        let id = session_commands_id(&self.id);
        let trailing_now = self.trailing();
        // The mark keeps the last words it said while it folds away.
        let words = match trailing_now {
            Trailing::State(_) => Some(self.said.clone()),
            Trailing::Unread => self.unread.clone(),
            Trailing::Ago => None,
        };
        if let Some(words) = words.filter(|words| !words.is_empty()) {
            self.words = words;
        }
        let p = palette(cx);
        let unread = presence(
            ElementId::Name(format!("session-unread-fade-{}", self.id).into()),
            trailing_now == Trailing::Unread,
            div()
                .id(session_unread_id(&self.id))
                .flex()
                .items_center()
                .justify_center()
                .w(px(14.))
                .h(px(MARK))
                .child(div().size(px(6.)).rounded_full().bg(p.acc))
                .test_support(),
        );
        let draft = presence(
            ElementId::Name(format!("session-draft-fade-{}", self.id).into()),
            self.draft && !self.selected,
            self.draft_mark(window, cx).into_any_element(),
        );
        // One mark: the state, the dot or the time take each other's place, one folding away as
        // the other comes. The state and the dot share the tooltip that says what they mean.
        let mark = gpui_design_system::tipped(
            div()
                .id(session_mark_id(&self.id))
                .flex()
                .items_center()
                .h(px(MARK))
                .aria_label(self.words.clone())
                .child(self.mark.icon(&self.id))
                .child(unread),
            self.words.clone(),
            MARK,
            window,
            cx,
        );
        let last = div()
            .flex()
            .items_center()
            .child(draft)
            .child(mark)
            .child(presence(
                ElementId::Name(format!("meta-{}", self.id).into()),
                trailing_now == Trailing::Ago,
                meta(when, cx),
            ))
            .into_any_element();
        if let Some(key) = self.key_chip(false, window, cx) {
            row = row.leading(key);
        }
        if self.scope.is_none() || self.blocked_line() {
            row = row.second(self.second_line(window, cx));
        }
        // The connector to the row above, and the indent it runs into, fade with the row.
        let tied = fade(
            ElementId::Name(format!("session-tie-fade-{}", self.id).into()),
            self.tie.is_some(),
            PANEL,
            window,
            cx,
        );
        let first_line = match self.scope {
            None => (TWO_LINES - (FIRST + LINE)) / 2. + FIRST / 2.,
            Some(_) if self.blocked_line() => (TWO_LINES - (FIRST + LINE)) / 2. + FIRST / 2.,
            Some(_) => 14.,
        };
        // A session whose machine is off fades its ink, and takes it back when the machine is.
        let greyed = fade(
            ElementId::Name(format!("session-greyed-fade-{}", self.id).into()),
            self.offline(),
            PANEL,
            window,
            cx,
        );
        div()
            .relative()
            .opacity(1.0 - GREYED * greyed)
            .pl(space::XL * (self.tie_shown.depth() as f32 * tied))
            .when(tied > 0.0, |d| {
                d.child(tree_tie(
                    session_id(&self.id),
                    &self.tie_shown,
                    first_line,
                    tied,
                    cx,
                ))
            })
            .child(row.trailing(trailing(id, self.commands, last, cx)))
    }
}

/// The words the pencil of a draft says.
const DRAFT_SAID: &str = "Draft not sent";

impl SessionRow {
    /// The pencil of an unsent draft, beside the row's own mark: its ink warms on the pointer.
    fn draft_mark(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let pencil = hoverable(
            div()
                .id(ElementId::Name(format!("draft-ink-{}", self.id).into()))
                .flex()
                .items_center()
                .justify_center()
                .w(px(14.))
                .h(px(MARK)),
            move |element, t| {
                element.child(
                    gpui_design_system::icon(Icon::Edit)
                        .size(px(12.))
                        .text_color(mix(p.mute, p.text, t)),
                )
            },
        );
        gpui_design_system::tipped(
            div()
                .id(session_draft_id(&self.id))
                .flex_none()
                .h(px(MARK))
                .aria_label(DRAFT_SAID)
                .child(pencil),
            DRAFT_SAID,
            MARK,
            window,
            cx,
        )
    }
}

/// How wide what a session waits for may unfold on its row, more than the sidebar holds: the
/// row cuts it.
const WAITS_WIDTH: f32 = 400.;
/// How wide the chip of a key may unfold: a tag is cut at 160 px.
const KEY_WIDTH: f32 = 160.;
/// The gap a session's row puts between its key and its title (the kit's `TreeRow`).
const ROW_GAP: f32 = 7.;
/// The height of a session's second line.
const LINE: f32 = 16.;
/// The height of a row's first line, above its second.
const FIRST: f32 = 18.;

impl SessionRow {
    /// Whether it is a row of the "Blocked by others" group, which says on its second line who it
    /// waits for: a scoped row (its project is in its meta) that waits. An ended row waits for
    /// nothing and has no second line.
    fn blocked_line(&self) -> bool {
        self.scope.is_some() && !self.waits.is_empty()
    }

    /// The second line of a row of the "Blocked by others" group: `waits for Maya · review of
    /// #123` in the `away` ink, cut with an ellipsis inside the row and whole in a tooltip; its
    /// words fade as they change.
    fn blocked_line_of(&self, window: &mut Window, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let p = palette(cx);
        let waits = self.waits.clone();
        let line = div()
            .id(session_line_id(&self.id))
            .flex()
            .items_center()
            .h(px(LINE))
            .overflow_hidden()
            .text_size(px(11.5))
            .text_color(p.away)
            .child(
                div()
                    .id(session_waits_id(&self.id))
                    .aria_label(waits.clone())
                    .min_w_0()
                    .h(px(LINE))
                    .child(changing(
                        ElementId::Name(format!("session-waits-words-{}", self.id).into()),
                        waits.clone(),
                        move |waits| {
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(waits)
                                .into_any_element()
                        },
                    ))
                    .test_support(),
            );
        tipped(line, waits, LINE, window, cx).into_any_element()
    }

    /// Under the title, faint: the place's icon and name, then, when there is room, its pull
    /// request and its checks' mark. The line is one line high and wraps: what does not fit goes
    /// under it, out of sight, and the name is cut with an ellipsis. The whole is in a tooltip.
    fn second_line(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        // The line is swapped for another when the row's machine tag comes (its pilot's machine
        // lists it after it was drawn), goes, or changes: the new one fades in, as the kit's
        // `changing` fades words.
        let kind = match (&self.elsewhere, &self.tag) {
            _ if self.blocked_line() => "blocked".to_owned(),
            (Some(there), _) if !there.listed => format!("machine {}", there.machine),
            (_, Some(tag)) => format!("tag {tag}"),
            _ => "place".to_owned(),
        };
        let swap = changed(
            &ElementId::Name(format!("session-line-swap-{}", self.id).into()),
            &SharedString::from(kind),
            window,
            cx,
        );
        let line = self.line_of(window, cx);
        div().min_w_0().opacity(swap).child(line).into_any_element()
    }

    /// The line itself, [`SessionRow::second_line`] without its fade.
    fn line_of(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        if self.blocked_line() {
            return self.blocked_line_of(window, cx);
        }
        let p = palette(cx);
        let (name, icon, said) = match &self.place {
            None => ("main".to_owned(), Icon::Folder, "Project folder"),
            Some(name) => (name.clone(), Icon::Branch, "Workspace"),
        };
        // It fades in when the pull request becomes known, and out when it is gone.
        let shown = fade(
            ElementId::Name(format!("session-pull-fade-{}", self.id).into()),
            self.pull.is_some(),
            ENTER,
            window,
            cx,
        );
        let pull = self
            .pull_shown
            .clone()
            .filter(|_| self.pull.is_some() || shown > 0.0);
        // What it waits for unfolds and fades in after the place, and folds away when it stops.
        let waiting = fade(
            ElementId::Name(format!("session-waits-fade-{}", self.id).into()),
            !self.waits.is_empty(),
            PANEL,
            window,
            cx,
        );
        let waits = Some(self.waits_shown.clone())
            .filter(|waits| !waits.is_empty() && (!self.waits.is_empty() || waiting > 0.0));
        // What a led session runs on unfolds and fades in after the place too, its words fading
        // as they change.
        let running = fade(
            ElementId::Name(format!("session-runs-fade-{}", self.id).into()),
            !self.runs.is_empty(),
            PANEL,
            window,
            cx,
        );
        let runs = Some(self.runs_shown.clone())
            .filter(|runs| !runs.is_empty() && (!self.runs.is_empty() || running > 0.0));
        // The session's own title stays in the tooltip when what it is about took its line.
        let own = (self.first_line() != self.title).then(|| self.title.to_string());
        let tip = [
            own,
            Some(name.clone()),
            waits.as_ref().map(|w| w.to_string()),
        ]
        .into_iter()
        .chain([runs.as_ref().map(|r| r.to_string())])
        .flatten()
        .chain(pull.as_ref().map(pull_said))
        .collect::<Vec<_>>()
        .join(" · ");
        if let Some(there) = self.elsewhere.clone().filter(|there| !there.listed) {
            return self.machine_line(there, window, cx);
        }
        if let Some(tag) = self.tag.clone() {
            return self.tag_line(tag, cx);
        }
        let place = div()
            .id(session_place_id(&self.id))
            .aria_label(SharedString::from(name.clone()))
            .flex()
            .items_center()
            .gap(space::XS)
            .max_w_full()
            .min_w_0()
            .h(px(LINE))
            .child(
                div()
                    .id(session_place_icon_id(&self.id))
                    .aria_label(said)
                    .flex_none()
                    .child(
                        gpui_design_system::icon(icon)
                            .size(px(11.))
                            .text_color(p.faint),
                    )
                    .test_support(),
            )
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(SharedString::from(name)),
            )
            .test_support();
        let line = div()
            .id(session_line_id(&self.id))
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(space::SM)
            .h(px(LINE))
            .overflow_hidden()
            .text_size(px(11.5))
            .text_color(p.faint)
            .child(
                // One line that never wraps, so that a long wait is cut with an ellipsis instead
                // of going under the line, out of sight.
                div()
                    .flex()
                    .items_center()
                    .gap(space::SM)
                    .min_w_0()
                    .max_w_full()
                    .h(px(LINE))
                    .children(self.key_chip(true, window, cx))
                    .child(place)
                    .children(waits.map(|waits| {
                        div()
                            .id(session_waits_id(&self.id))
                            .aria_label(waits.clone())
                            .flex()
                            .items_center()
                            .gap(space::SM)
                            .min_w_0()
                            .h(px(LINE))
                            .overflow_hidden()
                            .max_w(px(WAITS_WIDTH * waiting))
                            .opacity(waiting)
                            .child(div().flex_none().child("·"))
                            .child(changing(
                                ElementId::Name(format!("session-waits-words-{}", self.id).into()),
                                waits.clone(),
                                move |waits| {
                                    div()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .child(waits)
                                        .into_any_element()
                                },
                            ))
                            .test_support()
                    }))
                    .children(runs.map(|runs| {
                        div()
                            .id(session_runs_id(&self.id))
                            .aria_label(runs.clone())
                            .flex()
                            .items_center()
                            .gap(space::SM)
                            .min_w_0()
                            .h(px(LINE))
                            .overflow_hidden()
                            .max_w(px(WAITS_WIDTH * running))
                            .opacity(running)
                            .child(div().flex_none().child("·"))
                            .child(changing(
                                ElementId::Name(format!("session-runs-words-{}", self.id).into()),
                                runs.clone(),
                                move |runs| {
                                    div()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .child(runs)
                                        .into_any_element()
                                },
                            ))
                            .test_support()
                    })),
            )
            .children(pull.map(|pull| {
                let (mark, color) = pull_mark(&pull, &p);
                div()
                    .id(session_pull_id(&self.id))
                    .aria_label(SharedString::from(pull_said(&pull)))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(space::XS)
                    .h(px(LINE))
                    .opacity(shown)
                    .child(SharedString::from(format!("#{}", pull.number)))
                    .children(mark.map(|mark| {
                        gpui_design_system::icon(mark)
                            .size(px(11.))
                            .text_color(color)
                    }))
                    .test_support()
            }));
        tipped(line, SharedString::from(tip), LINE, window, cx).into_any_element()
    }

    /// The mark of the machine a session runs on (the kit's `MachineMark`, at its small size): its
    /// icon in its colour with the badge of its state, saying its name and state in its tooltip; a
    /// plain mark of unknown state while the member list is not read. The element around it says
    /// the machine's look in words, as the member list signs it.
    fn machine_mark(&self, p: &gpui_design_system::Palette) -> gpui_kit::AnyElement {
        let look = self.elsewhere.as_ref().and_then(|there| there.look.clone());
        let presence = ElementId::Name(format!("session-machine-presence-{}", self.id).into());
        let mark = match &self.home {
            Some(home) => machine_mark(presence, home, MarkSize::Small, None),
            None => {
                let machine = self
                    .elsewhere
                    .as_ref()
                    .map(|there| there.machine.clone())
                    .or_else(|| self.tag.as_ref().map(|tag| tag.to_string()))
                    .unwrap_or_default();
                div()
                    .flex_none()
                    .pr(space::XS)
                    .child(
                        MachineMark::new(
                            presence,
                            Icon::Service,
                            p.faint,
                            gpui_design_system::MachineState::Unknown,
                            machine,
                        )
                        .size(MarkSize::Small),
                    )
                    .into_any_element()
            }
        };
        div()
            .id(ElementId::Name(
                format!("session-machine-icon-{}", self.id).into(),
            ))
            .aria_label(SharedString::from(look.unwrap_or_default()))
            .flex_none()
            .child(mark)
            .test_support()
            .into_any_element()
    }

    /// The second line of a session of another machine in the one list: the machine's mark alone,
    /// its name and state (since when it is off) in the mark's words.
    fn tag_line(&self, tag: SharedString, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let p = palette(cx);
        let said = SharedString::from(match &self.home {
            Some(home) => home.said(),
            None => tag.to_string(),
        });
        div()
            .id(session_machine_id(&self.id))
            .aria_label(said)
            .flex()
            .items_center()
            .h(px(LINE))
            .child(self.machine_mark(&p))
            .test_support()
            .into_any_element()
    }

    /// The second line of a session on another machine: the machine's mark, then the session's
    /// state as last known, whose words fade as they change.
    fn machine_line(
        &self,
        there: crate::model::Elsewhere,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let p = palette(cx);
        let said = SharedString::from(format!("{} · {}", there.machine, there.state));
        let line = div()
            .id(session_machine_id(&self.id))
            .aria_label(said.clone())
            .flex()
            .items_center()
            .gap(space::XS)
            .min_w_0()
            .h(px(LINE))
            .overflow_hidden()
            .text_size(px(11.5))
            .text_color(p.faint)
            .child(self.machine_mark(&p))
            .child(changing(
                ElementId::Name(format!("session-machine-state-{}", self.id).into()),
                SharedString::from(there.state.clone()),
                move |state| {
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(state)
                        .into_any_element()
                },
            ))
            .test_support();
        let tip = div()
            .id(ElementId::Name(
                format!("session-machine-tip-{}", self.id).into(),
            ))
            .h(px(LINE))
            .child(line);
        tipped(tip, said, LINE, window, cx).into_any_element()
    }
}

/// A pull request's number and the state of its checks, said: `#12, checks passing`.
fn pull_said(pull: &PullRequest) -> String {
    let checks = match pull.mark.as_deref() {
        Some("passing") => ", checks passing",
        Some("failing") => ", checks failing",
        Some("stopped") => ", checks stopped",
        Some(_) => ", checks pending",
        None => "",
    };
    format!("#{}{checks}", pull.number)
}

/// The mark of a pull request's checks, drawn by shape as well as by colour: a check, a cross, a
/// dot; none without checks.
fn pull_mark(
    pull: &PullRequest,
    p: &gpui_design_system::Palette,
) -> (Option<Icon>, gpui_kit::Hsla) {
    match pull.mark.as_deref() {
        Some("passing") => (Some(Icon::Check), p.ok),
        Some("failing") => (Some(Icon::Close), p.fail),
        Some("stopped") => (Some(Icon::Stopped), p.faint),
        Some(_) => (Some(Icon::Running), p.run),
        None => (None, p.faint),
    }
}
