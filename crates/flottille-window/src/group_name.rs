//! The user makes a group of projects (`docs/design.md` §10, the sidebar): from the Projects
//! header's "⋯" menu, a dialog with one labelled field. The group is the user's view of their
//! projects, kept by the daemon with the rest of the sidebar's state.

use gpui_design_system::{Button, Field, Modal, space};
use gpui_kit::base::input::{Input, InputEvent, InputState};
use gpui_kit::{
    Context, Entity, Focusable as _, IntoElement, ParentElement, Render, SharedString, Styled,
    Subscription, WeakEntity, Window, div,
};

use crate::arrange::NAME_LIMIT;
use crate::commands::{boxed, line_field, problem_line};
use crate::shell::Shell;

/// The dialog's id; its field is `group-name`, its button `group-name-save`.
pub const GROUP_NAME: &str = "group-name";
/// What the Projects header's "⋯" menu says.
pub const NEW_GROUP_LABEL: &str = "New group…";

pub struct GroupName {
    shell: WeakEntity<Shell>,
    open: bool,
    field: Entity<InputState>,
    error: Option<SharedString>,
    _subscription: Subscription,
}

impl GroupName {
    pub fn new(shell: WeakEntity<Shell>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let field = line_field("for example Work", window, cx);
        let subscription = cx.subscribe_in(&field, window, |this, _, event, _, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.save(cx);
            }
        });
        Self {
            shell,
            open: false,
            field,
            error: None,
            _subscription: subscription,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn field(&self) -> &Entity<InputState> {
        &self.field
    }

    /// Opens the dialog, its field empty.
    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = true;
        self.error = None;
        self.field
            .update(cx, |field, cx| field.set_value("", window, cx));
        let focus = self.field.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        cx.notify();
    }

    pub fn save(&mut self, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        let name = self.field.read(cx).value().trim().to_owned();
        if name.is_empty() {
            self.error = Some("Give the group a name.".into());
            cx.notify();
            return;
        }
        if name.chars().count() > NAME_LIMIT {
            self.error = Some(format!("A group's name is {NAME_LIMIT} characters at most.").into());
            cx.notify();
            return;
        }
        let taken = self
            .shell
            .read_with(cx, |shell, _| shell.group_named(&name))
            .unwrap_or(false);
        if taken {
            self.error = Some("A group of that name exists.".into());
            cx.notify();
            return;
        }
        let shell = self.shell.clone();
        // Once this update is over: the shell is told while it may be updating this dialog.
        cx.defer(move |cx| {
            shell
                .update(cx, |shell, cx| shell.add_group(&name, cx))
                .ok();
        });
        self.close(cx);
    }
}

impl Render for GroupName {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        Modal::new(GROUP_NAME, "New group")
            .open(self.open)
            .sentence("Drag projects into it. Removing a group never removes its projects.")
            .on_close(move |_, cx| {
                this.update(cx, |this, cx| this.close(cx)).ok();
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(Field::new(
                        "group-name-field",
                        "Name",
                        boxed("group-name-input", Input::new(&self.field), cx),
                    ))
                    .child(problem_line(
                        "group-name-error",
                        self.error.clone(),
                        space::SM,
                        cx,
                    )),
            )
            .action(
                Button::primary("group-name-save", "Create")
                    .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
            )
    }
}
