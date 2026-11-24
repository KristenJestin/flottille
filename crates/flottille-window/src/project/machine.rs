//! The sessions of a shared project on its other machines, on the project's page (#1266,
//! `docs/remote.md`, "One list of every machine"): one block a machine, its icon, its name and its
//! state on its heading, then each of its sessions with that machine's icon, as the list has them.
//! A session opens its page there; a machine that is off is greyed and its sessions do not open.

use gpui_design_system::motion::PANEL;
use gpui_design_system::{FlatRow, Item, Listing, Type, fade, palette, space};
use gpui_kit::{
    Context, ElementId, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, TestSupportExt as _, Window, div, px,
};

use super::{
    SessionLine, machine_block_id, machine_session_ago_id, machine_session_mark_id, session_row_id,
};
use crate::model::ago;
use crate::shell::{Selection, now_ms};

/// What a machine's block shows: the machine, as the sidebar marks it, and its sessions of the
/// project, the latest exchange first.
#[derive(Clone, Debug, PartialEq)]
pub struct MachineSessions {
    pub machine: String,
    /// The icon and the colour its member signs, as words (`server`, `teal`).
    pub icon: String,
    pub colour: String,
    /// `online`, `offline since 14:02`, `this machine`.
    pub state: String,
    pub offline: bool,
    pub sessions: Vec<SessionLine>,
}

pub struct MachineBlock {
    shell: crate::commands::ShellOf,
    data: MachineSessions,
    /// The sessions as last drawn: one that comes unfolds, one that goes folds away.
    rows: Listing<SessionLine>,
}

impl MachineBlock {
    pub(super) fn new(shell: crate::commands::ShellOf, data: MachineSessions) -> Self {
        Self {
            shell,
            data,
            rows: Listing::default(),
        }
    }

    pub fn data(&self) -> &MachineSessions {
        &self.data
    }

    pub(super) fn set(&mut self, data: MachineSessions, cx: &mut Context<Self>) {
        if self.data != data {
            self.data = data;
            cx.notify();
        }
    }
}

impl Render for MachineBlock {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let data = self.data.clone();
        let name = data.machine.clone();
        // Its machine going off greys the block, and coming back gives the ink back.
        let ink = fade(
            ElementId::Name(format!("machine-block-ink-{name}").into()),
            !data.offline,
            PANEL,
            window,
            cx,
        );
        let said = SharedString::from(format!("{name} · {}", data.state));
        let heading = div()
            .h(px(36.))
            .px(space::SM)
            .flex()
            .items_center()
            .gap(space::SM)
            .child(div().w(px(16.)).flex_none().flex().justify_center().child(
                crate::machines::mark(
                    ElementId::Name(format!("machine-block-mark-{name}").into()),
                    &data.icon,
                    &data.colour,
                    14.,
                    window,
                    cx,
                ),
            ))
            .child(
                div()
                    .flex_none()
                    .medium()
                    .text_size(px(14.))
                    .text_color(p.text)
                    .child(SharedString::from(name.clone())),
            )
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(12.5))
                    .text_color(p.faint)
                    .child(SharedString::from(data.state.clone())),
            );
        let (at, reduce) = (cx.background_executor().now(), cx.reduce_motion());
        self.rows.sync(
            data.sessions.iter().map(|s| (s.id.clone(), s.clone())),
            at,
            reduce,
        );
        let now = now_ms();
        let rows: Vec<_> = self
            .rows
            .rows()
            .map(|row| (row.value.clone(), row.placement))
            .collect();
        let rows = rows.into_iter().map(|(session, placement)| {
            let (shell, open) = (self.shell.clone(), session.id.clone());
            let said_ago = SharedString::from(ago(now, session.active_at));
            let mark = crate::machines::mark(
                machine_session_mark_id(&session.id),
                &data.icon,
                &data.colour,
                13.,
                window,
                cx,
            );
            let row = FlatRow::new(
                session_row_id(&session.id),
                SharedString::from(session.title.clone()),
            )
            .leading(mark)
            .meta(
                div()
                    .id(machine_session_ago_id(&session.id))
                    .aria_label(said_ago.clone())
                    .child(said_ago)
                    .test_support(),
            )
            .on_click(move |_, window, cx| {
                let session = open.clone();
                shell
                    .update(cx, |shell, cx| {
                        shell.open(Selection::Session(session), window, cx)
                    })
                    .ok();
            });
            Item::new(
                ElementId::Name(format!("machine-session-item-{}", session.id).into()),
                placement,
                row,
            )
        });
        div()
            .id(machine_block_id(&name))
            .aria_label(said)
            .rounded(px(10.))
            .py(space::XS)
            .flex()
            .flex_col()
            .opacity(0.5 + 0.5 * ink)
            .child(heading)
            .children(rows)
            .test_support()
    }
}
