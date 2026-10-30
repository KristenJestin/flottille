//! The components that hold the rules of the window (`docs/reference/gpui.md`, "Rules of the
//! window") on one page, still, so that it can be captured in a headless compositor.
//!
//! ```text
//! cargo run -p flottille-ui --example components [-- --dialog | --confirm | --drawer | --menu]
//! ```
//!
//! `--dialog` opens a form dialog, `--confirm` a confirmation, `--drawer` a detail drawer,
//! `--menu` a list row's "⋯" menu. Two toasts are said when the window opens: a failure, which
//! stays, and a success, which leaves after four seconds.

use flottille_ui::{
    Badge, Block, Button, Card, Confirm, Drawer, EmptyLine, Field, Icon, Icons, ListCard, ListRow,
    Menu, Modal, PageHeader, RadioChoices, Section, Segmented, Select, SelectOption, StateIcon,
    Status, Switch, Toast, ToastStack, Type, icon, palette, space, toast,
};
use gpui_kit::base::input::{Input, InputState};
use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, Context, Entity, FontWeight, IntoElement,
    ParentElement, Render, Styled, Window, WindowBounds, WindowOptions, div, px, size,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shown {
    Page,
    Dialog,
    Confirm,
    Drawer,
    Menu,
}

fn main() {
    let mut shown = Shown::Page;
    for arg in std::env::args().skip(1) {
        shown = match arg.as_str() {
            "--dialog" => Shown::Dialog,
            "--confirm" => Shown::Confirm,
            "--drawer" => Shown::Drawer,
            "--menu" => Shown::Menu,
            other => panic!("unknown argument {other}"),
        };
    }
    gpui_kit::application().with_assets(Icons).run(move |cx| {
        flottille_ui::init(cx);
        let bounds = Bounds::centered(None, size(px(1400.), px(1000.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| Components::new(shown, window, cx))
        })
        .expect("a window");
    });
}

struct Components {
    shown: Shown,
    toasts: Entity<ToastStack>,
    header_menu: Entity<Menu>,
    row_menus: Vec<Entity<Menu>>,
    session: Entity<StateIcon>,
    name: Entity<InputState>,
    when: Entity<InputState>,
    file: Entity<InputState>,
    keeps: bool,
    check: bool,
    ask: usize,
    source: usize,
    tracker: usize,
    drawer: Entity<Drawer>,
}

struct RunDetail;

impl Render for RunDetail {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let fact = |key: &'static str, value: &'static str| {
            div()
                .flex()
                .gap(px(16.))
                .text_size(px(12.5))
                .child(div().w(px(96.)).text_color(p.faint).child(key))
                .child(div().mono(12.).text_color(p.text).child(value))
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .ui_text()
            .text_color(p.text)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .pt(px(14.))
                    .pb(px(14.))
                    .pl(px(20.))
                    .pr(px(12.))
                    .border_b_1()
                    .border_color(p.line)
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .child(div().mono(14.).medium().child("pnpm dev"))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(p.faint)
                                    .child("Task dev · running for 4m 12s"),
                            ),
                    )
                    .child(Button::secondary("drawer-stop", "Stop").leading(Icon::Stop))
                    .child(Button::icon("drawer-close", Icon::Close, "Close (Esc)")),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .pt(px(14.))
                    .pb(px(14.))
                    .px(px(20.))
                    .border_b_1()
                    .border_color(p.line)
                    .child(fact("Command", "pnpm dev"))
                    .child(fact("Task", "dev · keeps running"))
                    .child(fact("Where", "fix-login · atlas"))
                    .child(fact("Started", "14:02 · 4m 12s ago")),
            )
            .child(
                div().flex_1().p(px(16.)).child(
                    div()
                        .size_full()
                        .p(px(12.))
                        .rounded(px(8.))
                        .bg(p.field)
                        .border_1()
                        .border_color(p.field_line)
                        .mono(11.5)
                        .text_color(p.mute)
                        .child("> atlas@0.4.0 dev")
                        .child("> vite --port 5173")
                        .child("  VITE v6.0.3  ready in 412 ms")
                        .child("GET /api/items 200 12ms"),
                ),
            )
    }
}

impl Components {
    fn new(shown: Shown, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let toasts = cx.new(ToastStack::new);
        let header_menu = cx.new(|cx| {
            Menu::new("page-more", "More", cx)
                .entry("page-open", "Open in editor", None, |_, _| {})
                .danger("page-remove", "Remove project…", |_, _| {})
        });
        let row_menus = ["style", "release"]
            .into_iter()
            .map(|name| {
                cx.new(|cx| {
                    Menu::new(format!("{name}-more"), "More", cx)
                        .entry(format!("{name}-edit"), "Edit", None, |_, _| {})
                        .entry(format!("{name}-duplicate"), "Duplicate", None, |_, _| {})
                        .danger(format!("{name}-remove"), "Remove…", |_, _| {})
                })
            })
            .collect::<Vec<_>>();
        let status = cx.new(|_| Status::new(flottille_ui::AgentState::Idle));
        let session = cx.new(|cx| StateIcon::new(status, cx));
        let input = |placeholder: &'static str,
                     value: &'static str,
                     window: &mut Window,
                     cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut state = InputState::new(window, cx).placeholder(placeholder);
                if !value.is_empty() {
                    state.set_value(value, window, cx);
                }
                state
            })
        };
        let name = input("style", "style", window, cx);
        let when = input("writing code", "", window, cx);
        let file = input("docs/STYLE.md", "docs/STYLE.md", window, cx);
        let content = cx.new(|_| RunDetail);
        let drawer = cx.new(|cx| Drawer::detail("run", content, cx));
        if shown == Shown::Drawer {
            drawer.update(cx, |d, cx| d.show(window, cx));
        }
        if shown == Shown::Menu {
            let menu = row_menus[1].clone();
            cx.defer_in(window, move |_, window, cx| {
                menu.update(cx, |m, cx| m.open(window, cx));
            });
        }
        cx.defer_in(window, |_, _, cx| {
            toast(
                Toast::failure("The workspace was not removed")
                    .detail("4 files are not committed in atlas."),
                cx,
            );
            toast(
                Toast::success("Forges and tracker saved")
                    .detail("Flottille reads GitHub for api and web from now on."),
                cx,
            );
        });
        Self {
            shown,
            toasts,
            header_menu,
            row_menus,
            session,
            name,
            when,
            file,
            keeps: true,
            check: false,
            ask: 0,
            source: 0,
            tracker: 0,
            drawer,
        }
    }

    fn tile(title: &'static str, cx: &App) -> gpui_kit::Div {
        let p = palette(cx);
        div()
            .w(px(420.))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(14.))
            .p(px(20.))
            .rounded(px(12.))
            .bg(p.bg)
            .border_1()
            .border_color(p.line)
            .child(
                div()
                    .mono(11.)
                    .medium()
                    .text_color(p.faint)
                    .child(title.to_uppercase()),
            )
    }

    fn kind(label: &'static str, cx: &App) -> gpui_kit::Div {
        let p = palette(cx);
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(space::SM)
            .child(div().w(px(76.)).mono(11.).text_color(p.faint).child(label))
    }

    fn buttons(&self, cx: &App) -> gpui_kit::Div {
        let p = palette(cx);
        let note = |text: &'static str| div().text_size(px(12.)).text_color(p.faint).child(text);
        Self::tile("Buttons", cx)
            .child(
                Self::kind("primary", cx)
                    .child(Button::primary("b-save", "Save"))
                    .child(Button::primary("b-new", "New session").leading(Icon::Plus))
                    .child(Button::primary("b-create", "Create").disabled(true)),
            )
            .child(
                Self::kind("secondary", cx)
                    .child(Button::secondary("b-ws", "New workspace").leading(Icon::Plus))
                    .child(Button::secondary("b-settings", "Settings").leading(Icon::Settings))
                    .child(Button::secondary("b-run", "Run").leading(Icon::Play)),
            )
            .child(
                Self::kind("quiet", cx)
                    .child(Button::quiet("b-cancel", "Cancel"))
                    .child(Button::quiet("b-deny", "Deny"))
                    .child(Button::quiet("b-detect", "Detect again").leading(Icon::Search)),
            )
            .child(
                Self::kind("danger", cx)
                    .child(Button::danger("b-remove", "Remove task"))
                    .child(Button::danger("b-end", "End 2 agents"))
                    .child(note("only in a confirmation")),
            )
            .child(
                Self::kind("icon", cx)
                    .child(Button::icon("b-more", Icon::More, "More"))
                    .child(Button::icon("b-add", Icon::Plus, "Add a project"))
                    .child(
                        Button::icon("b-bell", Icon::Bell, "Waiting for you")
                            .badge(Badge::count(2)),
                    )
                    .child(note("always a tooltip")),
            )
    }

    fn lists(&self, cx: &App) -> gpui_kit::Div {
        let p = palette(cx);
        Self::tile("List rows", cx)
            .child(
                ListCard::new("skills-list")
                    .row(
                        ListRow::new("style", "style")
                            .leading(icon(Icon::File).text_color(p.faint))
                            .mono()
                            .tag("docs/STYLE.md")
                            .line("The code style: read before writing code")
                            .more(self.row_menus[0].clone()),
                    )
                    .row(
                        ListRow::new("release", "release")
                            .leading(icon(Icon::Text).text_color(p.faint))
                            .mono()
                            .tag("text")
                            .line("How a release is cut")
                            .more(self.row_menus[1].clone()),
                    ),
            )
            .child(
                EmptyLine::new(
                    "commands-empty",
                    Icon::Slash,
                    "No command yet. Start from a template or write your own.",
                )
                .action(Button::secondary("new-command", "New command").leading(Icon::Plus)),
            )
    }

    fn workspace(&self, cx: &App) -> gpui_kit::Div {
        let p = palette(cx);
        let line = |glyph: Icon, key: &'static str, value: AnyElement| {
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .min_h(px(26.))
                .text_size(px(12.5))
                .child(
                    div()
                        .w(px(112.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_color(p.faint)
                        .child(icon(glyph).size(px(13.)).text_color(p.faint))
                        .child(key),
                )
                .child(value)
        };
        let mono = |text: &'static str| div().mono(12.).text_color(p.mute).child(text);
        let title = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(icon(Icon::Branch).text_color(p.faint))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("help-pantry-file"),
            )
            .child(div().mono(11.5).text_color(p.faint).child("from main"));
        Self::tile("Card blocks", cx)
            .child(
                Card::new()
                    .id("workspace-card")
                    .title(title)
                    .meta("+18 −1")
                    .block(
                        Block::new().child(
                            div()
                                .px(px(14.))
                                .py(px(8.))
                                .flex()
                                .flex_col()
                                .child(line(
                                    Icon::Repository,
                                    "Repository",
                                    mono("pantry  1 ahead · clean").into_any_element(),
                                ))
                                .child(line(
                                    Icon::PullRequest,
                                    "Pull request",
                                    mono("#17 merged").into_any_element(),
                                ))
                                .child(line(
                                    Icon::Ticket,
                                    "Ticket",
                                    mono("#9 Help page for the pantry file").into_any_element(),
                                )),
                        ),
                    )
                    // Setup has nothing to show once ready: no block, no second line.
                    .block(Block::new())
                    .block(
                        Block::new().child(
                            ListRow::new("ws-session", "Write the help page")
                                .leading(self.session.clone())
                                .meta("1h"),
                        ),
                    ),
            )
            .child(
                div().text_size(px(12.5)).text_color(p.mute).child(
                    "Three blocks given, one empty: a line only between the two that exist.",
                ),
            )
    }

    fn controls(&mut self, cx: &mut Context<Self>) -> gpui_kit::Div {
        let p = palette(cx);
        let caption = |text: &'static str| div().mono(11.).text_color(p.faint).child(text);
        let this = cx.entity().downgrade();
        let (a, b, c, d) = (this.clone(), this.clone(), this.clone(), this.clone());
        Self::tile("Which control for which choice", cx)
            .w(px(868.))
            .child(
                div()
                    .flex()
                    .gap(px(24.))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(10.))
                            .child(caption("switch"))
                            .child(
                                Switch::new("keeps", self.keeps, "Keeps running")
                                    .hint("A server: listed under Runs until stopped.")
                                    .on_click(move |_, _, cx| {
                                        a.update(cx, |this, cx| {
                                            this.keeps = !this.keeps;
                                            cx.notify();
                                        })
                                        .ok();
                                    }),
                            )
                            .child(
                                Switch::new("check", self.check, "A check")
                                    .hint("Its result is told to the agent that ran it.")
                                    .on_click(move |_, _, cx| {
                                        b.update(cx, |this, cx| {
                                            this.check = !this.check;
                                            cx.notify();
                                        })
                                        .ok();
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(10.))
                            .child(caption("segmented"))
                            .child(
                                Segmented::new(
                                    "mode",
                                    "Mode",
                                    [("ask", "Ask"), ("auto", "Auto")],
                                    self.ask,
                                )
                                .on_choose(move |index, _, cx| {
                                    c.update(cx, |this, cx| {
                                        this.ask = index;
                                        cx.notify();
                                    })
                                    .ok();
                                }),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(10.))
                            .child(caption("select"))
                            .child(self.tracker_select(cx).fit()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(caption("radio"))
                    .child(self.source_choices(d)),
            )
    }

    fn tracker_select(&self, cx: &mut Context<Self>) -> Select {
        let this = cx.entity().downgrade();
        Select::new(
            "tracker",
            [
                SelectOption::new("github", "GitHub issues")
                    .line("Issues of a repository, read with gh"),
                SelectOption::new("jira", "Jira").line("A Jira site, read only"),
                SelectOption::new("none", "None"),
            ],
            self.tracker,
        )
        .on_choose(move |index, _, cx| {
            this.update(cx, |this, cx| {
                this.tracker = index;
                cx.notify();
            })
            .ok();
        })
    }

    fn source_choices(&self, this: gpui_kit::WeakEntity<Self>) -> RadioChoices {
        RadioChoices::new("source", "What it is")
            .choice(
                "file",
                "A file of the project",
                "Read where the agent works: its workspace's copy when it has one.",
            )
            .leading(Icon::File)
            .choice(
                "text",
                "A text Flottille keeps",
                "Kept in Flottille's data folder, never in the project.",
            )
            .leading(Icon::Text)
            .chosen(self.source)
            .on_choose(move |index, _, cx| {
                this.update(cx, |this, cx| {
                    this.source = index;
                    cx.notify();
                })
                .ok();
            })
    }

    fn page(&self, cx: &App) -> gpui_kit::Div {
        let p = palette(cx);
        let chip = div()
            .h(px(24.))
            .px(px(8.))
            .flex()
            .items_center()
            .gap(px(6.))
            .rounded(px(6.))
            .border_1()
            .border_color(p.line)
            .mono(11.5)
            .text_color(p.mute)
            .child(icon(Icon::Repository).size(px(12.)).text_color(p.faint))
            .child("atlas · main");
        Self::tile("Page header and section", cx)
            .w(px(868.))
            .child(
                PageHeader::new("page", "atlas")
                    .path("~/work/atlas")
                    .action("page-settings", "Settings", Some(Icon::Settings), |_, _| {})
                    .action(
                        "page-new-workspace",
                        "New workspace",
                        Some(Icon::Plus),
                        |_, _| {},
                    )
                    .more(self.header_menu.clone())
                    .below(chip),
            )
            .child(
                Section::new("skills", "Skills")
                    .count("1")
                    .sentence(
                        "Texts an agent reads when its task needs them. It sees their names and \
                         one line; it opens one with a tool.",
                    )
                    .action(Button::secondary("add-skill", "Add skill").leading(Icon::Plus)),
            )
    }

    fn input(state: &Entity<InputState>, cx: &App) -> impl IntoElement {
        let p = palette(cx);
        div()
            .h(px(34.))
            .px(px(10.))
            .rounded(px(8.))
            .bg(p.field)
            .border_1()
            .border_color(p.field_line)
            .flex()
            .items_center()
            .mono(12.5)
            .child(Input::new(state))
    }

    fn dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity().downgrade();
        let source = self.source_choices(this);
        let names = div()
            .flex()
            .gap(px(12.))
            .child(div().flex_1().child(Field::new(
                "skill-name",
                "Name",
                Self::input(&self.name, cx),
            )))
            .child(
                div().flex_1().child(
                    Field::new("skill-when", "Read it when", Self::input(&self.when, cx))
                        .error(Some("Say when an agent should read it.".into())),
                ),
            );
        Modal::new("skill-modal", "Add a skill")
            .sentence(
                "A text an agent reads when its task needs it. It sees the name and the line; \
                 it opens the skill with a tool.",
            )
            .child(names)
            .child(Field::new("skill-source", "What it is", source))
            .child(
                Field::new("skill-file", "File", Self::input(&self.file, cx))
                    .hint("A path in the project folder."),
            )
            .child(Field::new(
                "skill-tracker",
                "Tracker",
                self.tracker_select(cx),
            ))
            .action(Button::primary("skill-add", "Add skill"))
            .on_close(|_, _| {})
            .into_any_element()
    }
}

impl Render for Components {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let overlay: Option<AnyElement> = match self.shown {
            Shown::Dialog => Some(self.dialog(cx)),
            Shown::Confirm => Some(
                Confirm::new("command-removal", "Remove the command open-prs?")
                    .sentence("Typing /open-prs in a composer sends it as text from now on.")
                    .action("Remove command")
                    .on_cancel(|_, _| {})
                    .into_any_element(),
            ),
            _ => None,
        };
        let controls = self.controls(cx);
        div()
            .size_full()
            .relative()
            .bg(p.body)
            .text_color(p.text)
            .ui_text()
            .child(
                div()
                    .size_full()
                    .p(px(32.))
                    .flex()
                    .flex_wrap()
                    .content_start()
                    .gap(px(24.))
                    .child(self.buttons(cx))
                    .child(self.lists(cx))
                    .child(self.workspace(cx))
                    .child(self.page(cx))
                    .child(controls),
            )
            .child(self.drawer.clone())
            .children(overlay)
            .child(self.toasts.clone())
    }
}
