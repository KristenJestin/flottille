//! Every component and every state of `flottille-ui` on one page, still: nothing needs a pointer
//! to be seen, so the page can be captured in a headless compositor.
//!
//! ```text
//! cargo run -p flottille-ui --example gallery [-- --os-release <path>] [--drawer]
//! ```
//!
//! The theme is chosen as the window chooses it: Omarchy's when the os-release says `ID=omarchy`
//! (`--os-release` points at another file than `/etc/os-release`), read from
//! `$XDG_STATE_HOME/omarchy/current/theme/colors.toml`. `--drawer` shows the right drawer open.

use std::path::PathBuf;

use flottille_ui::omarchy::Environment;
use flottille_ui::rows::TreeLevel;
use flottille_ui::{
    AgentState, Badge, Button, Card, Drawer, Fold, Icon, Icons, ListRow, Pill, PulsingLabel, Side,
    StateIcon, Status, TreeRow, Type, enter, icon, meta, palette, section_label, tree_children,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, Context, Entity, FontWeight, Hsla, IntoElement,
    ParentElement, Render, Styled, Window, WindowBounds, WindowOptions, div, px, size,
};

fn main() {
    let mut env = Environment::from_system();
    let mut drawer = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--os-release" => env.os_release = PathBuf::from(args.next().expect("a path")),
            "--drawer" => drawer = true,
            other => panic!("unknown argument {other}"),
        }
    }
    gpui_kit::application().with_assets(Icons).run(move |cx| {
        flottille_ui::init_with(env, cx);
        let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Maximized(bounds)),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |_, cx| cx.new(|cx| Gallery::new(drawer, cx)))
            .expect("a window");
    });
}

struct Agent {
    name: &'static str,
    model: &'static str,
    doing: &'static str,
    icon: Entity<StateIcon>,
    pill: Entity<Pill>,
}

struct Gallery {
    states: Vec<(AgentState, Entity<StateIcon>, Entity<Pill>)>,
    agents: Vec<Agent>,
    session_marks: Vec<Entity<StateIcon>>,
    thinking: Entity<PulsingLabel>,
    open_fold: Entity<Fold>,
    shut_fold: Entity<Fold>,
    drawer: Entity<Drawer>,
}

struct Steps;

impl Render for Steps {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        div()
            .px(px(14.))
            .py(px(8.))
            .flex()
            .flex_col()
            .gap(px(4.))
            .text_color(p.mute)
            .ui_text()
            .text_size(px(13.))
            .child("01  The timeout is set once; something overrides it.")
            .child("02  The value is in seconds, read as milliseconds.")
    }
}

struct DrawerContent {
    rows: Vec<(Entity<StateIcon>, Entity<Pill>, &'static str, &'static str)>,
}

impl Render for DrawerContent {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .p(px(12.))
            .flex()
            .flex_col()
            .child(section_label("Agents", cx))
            .children(
                self.rows
                    .iter()
                    .enumerate()
                    .map(|(i, (icon, pill, name, model))| {
                        ListRow::new(("drawer-agent", i), *name)
                            .leading(icon.clone())
                            .detail(*model)
                            .trailing(pill.clone())
                            .selected(i == 0)
                    }),
            )
    }
}

impl Gallery {
    fn new(drawer_open: bool, cx: &mut Context<Self>) -> Self {
        let states = AgentState::ALL
            .iter()
            .map(|&state| {
                let status = cx.new(|_| Status::new(state));
                let icon = cx.new(|cx| StateIcon::new(status.clone(), cx));
                let pill = cx.new(|cx| Pill::new(status, cx));
                (state, icon, pill)
            })
            .collect();
        let agent = |name, model, doing, state, cx: &mut Context<Self>| {
            let status = cx.new(|_| Status::new(state));
            Agent {
                name,
                model,
                doing,
                icon: cx.new(|cx| StateIcon::new(status.clone(), cx)),
                pill: cx.new(|cx| Pill::new(status, cx)),
            }
        };
        let agents = vec![
            agent(
                "api-tests",
                "Sonnet",
                "app-api · TTL units",
                AgentState::Completed,
                cx,
            ),
            agent(
                "web-form",
                "Sonnet",
                "asked lead · #3",
                AgentState::Waiting,
                cx,
            ),
            agent(
                "docs-pass",
                "Haiku",
                "docs · timeout default",
                AgentState::Running,
                cx,
            ),
        ];
        let mark = |state, cx: &mut Context<Self>| {
            let status = cx.new(|_| Status::new(state));
            cx.new(|cx| StateIcon::new(status, cx))
        };
        let session_marks = vec![
            mark(AgentState::Running, cx),
            mark(AgentState::NeedsYou, cx),
            mark(AgentState::NeedsYou, cx),
        ];
        let thinking = cx.new(|cx| PulsingLabel::new("Thinking", true, cx));
        let steps = cx.new(|_| Steps);
        let open_fold = cx.new(|_| Fold::new("fold-open", steps.clone(), true));
        let shut_fold = cx.new(|_| Fold::new("fold-shut", steps, false));
        let rows = [
            ("lead", "Opus", AgentState::NeedsYou),
            ("web-form", "Sonnet", AgentState::Waiting),
            ("docs-pass", "Haiku", AgentState::Completed),
        ]
        .into_iter()
        .map(|(name, model, state)| {
            let status = cx.new(|_| Status::new(state));
            (
                cx.new(|cx| StateIcon::new(status.clone(), cx)),
                cx.new(|cx| Pill::new(status, cx)),
                name,
                model,
            )
        })
        .collect();
        let content = cx.new(|_| DrawerContent { rows });
        let drawer = cx.new(|cx| {
            let mut drawer = Drawer::new("rail-drawer", Side::Right, px(290.), content);
            if drawer_open {
                drawer.open(cx);
            }
            drawer
        });
        Self {
            states,
            agents,
            session_marks,
            thinking,
            open_fold,
            shut_fold,
            drawer,
        }
    }
}

fn panel(title: &str, cx: &App) -> gpui_kit::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(section_label(title, cx))
}

fn swatch(name: &'static str, color: Hsla, cx: &App) -> AnyElement {
    let p = palette(cx);
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(3.))
        .child(
            div()
                .size(px(26.))
                .rounded(px(6.))
                .bg(color)
                .border_1()
                .border_color(p.line2),
        )
        .child(div().mono(9.5).text_color(p.faint).child(name))
        .into_any_element()
}

impl Render for Gallery {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let tokens = [
            ("bg", p.bg),
            ("rail", p.rail),
            ("side", p.side),
            ("card", p.card),
            ("card2", p.card2),
            ("line", p.line),
            ("line2", p.line2),
            ("hover", p.hover),
            ("sel", p.sel),
            ("text", p.text),
            ("mute", p.mute),
            ("faint", p.faint),
            ("acc", p.acc),
            ("run", p.run),
            ("wait", p.wait),
            ("ok", p.ok),
            ("fail", p.fail),
            ("add", p.add),
            ("del", p.del),
        ];

        let states = panel("States", cx).child(div().flex().flex_col().gap(px(6.)).children(
            self.states.iter().map(|(state, icon, pill)| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(icon.clone())
                    .child(div().w(px(84.)).flex().child(pill.clone()))
                    .child(
                        div()
                            .mono(11.)
                            .text_color(p.faint)
                            .child(format!("{state:?}").to_lowercase()),
                    )
            }),
        ));

        let buttons = panel("Buttons", cx)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(8.))
                    .items_center()
                    .child(Button::primary("b-new", "New session").leading(Icon::Plus))
                    .child(Button::secondary("b-ws", "New workspace").leading(Icon::Plus))
                    .child(Button::quiet("b-deny", "Deny"))
                    .child(Button::primary("b-once", "Allow once")),
            )
            .child(
                div()
                    .flex()
                    .gap(px(4.))
                    .items_center()
                    .child(Button::icon("i-search", Icon::Search, "Search"))
                    .child(
                        Button::icon("i-bell", Icon::Bell, "Waiting for you")
                            .badge(Badge::count(2)),
                    )
                    .child(Button::icon(
                        "i-edit",
                        Icon::OpenInEditor,
                        "Open the workspace in your editor",
                    ))
                    .child(Button::icon("i-agents", Icon::Agents, "Agents").badge(Badge::alert()))
                    .child(Button::icon("i-menu", Icon::Menu, "Projects"))
                    .child(div().ml(px(8.)).child(flottille_ui::Tooltip::view(
                        "Open the workspace in your editor".into(),
                        cx,
                    ))),
            )
            .child(
                div().flex().gap(px(10.)).children(
                    Icon::ALL
                        .iter()
                        .map(|&glyph| icon(glyph).size(px(16.)).text_color(p.mute)),
                ),
            )
            .child(section_label("Tokens", cx))
            .child(
                div().w(px(300.)).flex().flex_wrap().gap(px(6.)).children(
                    tokens
                        .into_iter()
                        .map(|(name, color)| swatch(name, color, cx)),
                ),
            );

        let tree = panel("Tree", cx).child(
            div()
                .w(px(270.))
                .p(px(8.))
                .rounded(px(10.))
                .bg(p.side)
                .flex()
                .flex_col()
                .child(section_label("Projects", cx))
                .child(TreeRow::new("t-atlas", TreeLevel::Project, "atlas").expanded(true))
                .child(tree_children(
                    [
                        TreeRow::new("t-main", TreeLevel::Main, "main").into_any_element(),
                        tree_children(
                            [
                                TreeRow::new("t-s1", TreeLevel::Session, "Release notes for 2.4")
                                    .trailing(meta("2d", cx))
                                    .into_any_element(),
                            ],
                            cx,
                        )
                        .into_any_element(),
                        TreeRow::new("t-ws", TreeLevel::Workspace, "fix-session-timeout")
                            .into_any_element(),
                        tree_children(
                            [
                                TreeRow::new(
                                    "t-s2",
                                    TreeLevel::Session,
                                    "Sessions expire too early",
                                )
                                .selected(true)
                                .trailing(self.session_marks[1].clone())
                                .into_any_element(),
                                TreeRow::new("t-s3", TreeLevel::Session, "CSV export by tenant")
                                    .trailing(self.session_marks[0].clone())
                                    .into_any_element(),
                                TreeRow::new("t-s4", TreeLevel::Session, "Review the refresh flow")
                                    .trailing(meta("1h", cx))
                                    .into_any_element(),
                            ],
                            cx,
                        )
                        .into_any_element(),
                    ],
                    cx,
                ))
                .child(
                    TreeRow::new("t-notes", TreeLevel::Project, "notes-app")
                        .expanded(false)
                        .trailing(self.session_marks[2].clone()),
                ),
        );

        let place =
            Card::new()
                .title(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(icon(Icon::Branch).text_color(p.faint))
                        .child("fix-session-timeout"),
                )
                .meta("from develop · +2 −1")
                .child(div().p(px(4.)).flex().flex_col().children(
                    self.agents.iter().enumerate().map(|(i, a)| {
                        enter(("enter-agent", i), i, {
                            ListRow::new(("agent", i), a.name)
                                .leading(a.icon.clone())
                                .detail(a.model)
                                .meta(a.doing)
                                .trailing(a.pill.clone())
                        })
                    }),
                ));

        let approval = Card::new()
            .attention()
            .child(
                div()
                    .px(px(16.))
                    .pt(px(14.))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.session_marks[1].clone())
                            .child("Run npm run e2e?"),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(p.mute)
                            .child("The suite is the only check of the refresh timer."),
                    )
                    .child(meta("asked by web-form · forwarded by lead · #3", cx)),
            )
            .action(Button::quiet("a-deny", "Deny"))
            .action(Button::quiet("a-tree", "Allow for this tree"))
            .action(Button::primary("a-once", "Allow once"));

        let diff_line = |text: &'static str, bg: Option<Hsla>, ink: Hsla| {
            div()
                .px(px(14.))
                .mono(12.)
                .line_height(px(21.))
                .text_color(ink)
                .when_some(bg, |d, bg| d.bg(bg))
                .child(text)
        };
        let diff = Card::new()
            .title(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(icon(Icon::Repository).size(px(13.)).text_color(p.faint))
                    .child(
                        div()
                            .mono(12.5)
                            .font_weight(FontWeight::NORMAL)
                            .child("app-api/src/auth/session.ts"),
                    ),
            )
            .meta("+2 −1")
            .child(
                div()
                    .py(px(6.))
                    .child(diff_line(
                        "export function sessionTtlMs(env: Env) {",
                        None,
                        p.mute,
                    ))
                    .child(diff_line(
                        "-  return env.SESSION_TTL / 6;",
                        Some(p.del),
                        p.fail,
                    ))
                    .child(diff_line(
                        "+  return env.SESSION_TTL * 1000;",
                        Some(p.add),
                        p.ok,
                    ))
                    .child(diff_line("}", None, p.mute)),
            );

        let folds = Card::new()
            .title(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(self.thinking.clone())
                    .child(meta("2.0s", cx)),
            )
            .child(self.open_fold.clone())
            .child(
                div()
                    .px(px(14.))
                    .py(px(6.))
                    .border_t_1()
                    .border_color(p.line)
                    .text_color(p.faint)
                    .text_size(px(13.))
                    .child("Thought for 2.0s (folded)"),
            )
            .child(self.shut_fold.clone());

        let type_sample = panel("Type", cx).child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(px(24.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("atlas"),
                )
                .child(div().child("Geist 13.5 — Sessions expire after 5 minutes instead of 30."))
                .child(
                    div()
                        .mono(12.)
                        .text_color(p.mute)
                        .child("Geist Mono — ~/projects/atlas · develop · 0123456789"),
                ),
        );

        div()
            .size_full()
            .relative()
            .bg(p.bg)
            .text_color(p.text)
            .ui_text()
            .child(
                div()
                    .size_full()
                    .p(px(24.))
                    .flex()
                    .gap(px(28.))
                    .child(
                        div()
                            .w(px(230.))
                            .flex()
                            .flex_col()
                            .gap(px(18.))
                            .child(states)
                            .child(type_sample),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(18.))
                            .child(tree)
                            .child(buttons),
                    )
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(12.))
                            .child(section_label("Cards and rows", cx))
                            .child(place)
                            .child(approval)
                            .child(diff)
                            .child(folds),
                    ),
            )
            .child(self.drawer.clone())
    }
}
