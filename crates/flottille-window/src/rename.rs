//! The user renames a session (`docs/design.md` §10, the sidebar): from its row's "⋯" menu or its
//! page's header, a dialog with one labelled field. The daemon names it (`agent.rename`), its
//! agent can no longer change it, and the result is a toast.

use gpui_design_system::{Button, Field, Modal, space};
use gpui_kit::base::input::{Input, InputEvent, InputState};
use gpui_kit::{
    Context, Entity, Focusable as _, IntoElement, ParentElement, Render, SharedString, Styled,
    Subscription, WeakEntity, Window, div,
};

use crate::actions::Command;
use crate::commands::{boxed, line_field, perform_later, problem_line};
use crate::shell::Shell;

/// The dialog's id; its field is `rename-title`, its button `rename-save`.
pub const RENAME: &str = "rename";
/// What the "⋯" menu and the header's button say.
pub const RENAME_LABEL: &str = "Rename…";

pub struct RenameSession {
    shell: WeakEntity<Shell>,
    /// The session being renamed, while the dialog is open.
    session: Option<String>,
    field: Entity<InputState>,
    error: Option<SharedString>,
    _subscription: Subscription,
}

impl RenameSession {
    pub fn new(shell: WeakEntity<Shell>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let field = line_field("for example Rework the login page", window, cx);
        let subscription = cx.subscribe_in(&field, window, |this, _, event, _, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.save(cx);
            }
        });
        Self {
            shell,
            session: None,
            field,
            error: None,
            _subscription: subscription,
        }
    }

    pub fn is_open(&self) -> bool {
        self.session.is_some()
    }

    pub fn field(&self) -> &Entity<InputState> {
        &self.field
    }

    /// Opens the dialog on `session`, its field holding the title it has now.
    pub fn open(
        &mut self,
        session: &str,
        title: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.session = Some(session.to_owned());
        self.error = None;
        self.field
            .update(cx, |field, cx| field.set_value(title, window, cx));
        let focus = self.field.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.session = None;
        cx.notify();
    }

    pub fn save(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let title = self.field.read(cx).value().trim().to_owned();
        if title.is_empty() {
            self.error = Some("Give the session a title.".into());
            cx.notify();
            return;
        }
        perform_later(&self.shell, Command::Rename { session, title }, cx);
        self.close(cx);
    }
}

impl Render for RenameSession {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        Modal::new(RENAME, "Rename the session")
            .open(self.session.is_some())
            .sentence("Your title stays: the session's agent no longer changes it.")
            .on_close(move |_, cx| {
                this.update(cx, |this, cx| this.close(cx)).ok();
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(Field::new(
                        "rename-title-field",
                        "Title",
                        boxed("rename-title", Input::new(&self.field), cx),
                    ))
                    .child(problem_line(
                        "rename-error",
                        self.error.clone(),
                        space::SM,
                        cx,
                    )),
            )
            .action(
                Button::primary("rename-save", "Save")
                    .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
            )
    }
}
