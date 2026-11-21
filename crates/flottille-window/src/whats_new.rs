//! What's new in a version (`docs/releases.md`, "What's new"): the page that says, in the user's
//! words, what they can do now, as the daemon gathered it from the release notes
//! (`whatsnew.show`): the highlights when the release has some, then one section per area (Remote,
//! Window, Agents, Tools, Projects, Other, the fixes last), each note a row of the list with its
//! pull request a quiet link at the end, and the raw list of subjects in a fold closed at the
//! foot. It is a page of the window, opened by the mark at the foot of the sidebar or from
//! Settings, General, Version, never by itself.
//!
//! What moves: the page's blocks enter one after another, the fold of the raw list opens and
//! closes (`Disclosure`), a link's ink fades on hover, a row fades on hover (`ListRow`), the
//! skeleton of a read that waits crossfades into the notes (`awaiting`), and the line of a read
//! that failed unfolds and folds away (`presence_of`).

use gpui_design_system::color::mix;
use gpui_design_system::{
    Button, Disclosure, EmptyLine, Icon, ListCard, ListRow, PageHeader, Section, Skeleton,
    awaiting, enter, hoverable, palette, presence, presence_of, space,
};
use gpui_kit::{
    AnyElement, Context, ElementId, InteractiveElement, IntoElement, ParentElement, Render,
    ScrollHandle, SharedString, StatefulInteractiveElement, Styled, TestSupportExt as _,
    WeakEntity, Window, div, px,
};
use serde_json::{Value, json};

use crate::actions::Command;
use crate::commands::stack;
use crate::shell::Shell;

/// One line: its words and the pull request that made it.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub text: String,
    pub pr: Option<u64>,
    pub url: Option<String>,
}

/// An area of the notes: Remote, Window, Agents, Tools, Projects, Other, Fixed.
#[derive(Clone, Debug, PartialEq)]
pub struct Area {
    pub key: String,
    pub title: String,
    pub changes: Vec<Change>,
}

/// The raw lines of one scope of a group of the changelog; none is a line that names no scope.
#[derive(Clone, Debug, PartialEq)]
pub struct Scope {
    pub name: Option<String>,
    pub changes: Vec<Change>,
}

/// A group of the raw list: breaking changes, features, fixes, performance, other changes.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub key: String,
    pub title: String,
    pub scopes: Vec<Scope>,
}

impl Group {
    fn count(&self) -> usize {
        self.scopes.iter().map(|scope| scope.changes.len()).sum()
    }
}

/// What `whatsnew.show` says.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Notes {
    /// The version that runs: the page is the one of its major and minor.
    pub version: Option<String>,
    /// The version last run, the changes are counted from: none when nothing is new.
    pub since: Option<String>,
    /// Whether the page was opened since this version runs.
    pub seen: bool,
    /// Why the notes could not be read, when they could not.
    pub failed: bool,
    /// The notes by area, once read.
    pub groups: Option<Vec<Area>>,
    /// The lines written by hand for the release; none for a beta.
    pub highlights: Vec<String>,
    /// The raw list of subjects.
    pub all: Vec<Group>,
}

fn change_of(change: &Value) -> Change {
    Change {
        text: change["text"].as_str().unwrap_or_default().to_owned(),
        pr: change["pr"].as_u64(),
        url: change["url"].as_str().map(str::to_owned),
    }
}

fn changes_of(value: &Value) -> Vec<Change> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .map(change_of)
        .collect()
}

impl Notes {
    pub fn of(value: &Value) -> Notes {
        let text = |value: &Value, key: &str| value[key].as_str().map(str::to_owned);
        let groups = value["groups"].as_array().map(|groups| {
            groups
                .iter()
                .map(|group| Area {
                    key: text(group, "key").unwrap_or_default(),
                    title: text(group, "title").unwrap_or_default(),
                    changes: changes_of(&group["changes"]),
                })
                .collect()
        });
        let all = value["all"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|group| Group {
                key: text(group, "key").unwrap_or_default(),
                title: text(group, "title").unwrap_or_default(),
                scopes: group["scopes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|scope| Scope {
                        name: text(scope, "scope"),
                        changes: changes_of(&scope["changes"]),
                    })
                    .collect(),
            })
            .collect();
        Notes {
            version: text(value, "version"),
            since: text(value, "since"),
            seen: value["seen"] == true,
            failed: value["failed"].is_string(),
            groups,
            highlights: value["highlights"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|line| line.as_str().map(str::to_owned))
                .collect(),
            all,
        }
    }

    /// What the sidebar's mark says: only while there is something new that was not opened.
    pub fn mark(&self) -> Option<String> {
        (self.since.is_some() && !self.seen).then(|| "What's new".to_owned())
    }

    /// The page's title: `What's new in 0.4`, the version's major and minor.
    fn title(&self) -> String {
        let line = self
            .version
            .as_deref()
            .map(|version| version.splitn(3, '.').take(2).collect::<Vec<_>>().join("."));
        match line {
            Some(line) => format!("What's new in {line}"),
            None => "What's new".to_owned(),
        }
    }

    fn raw_count(&self) -> usize {
        self.all.iter().map(Group::count).sum()
    }
}

pub struct WhatsNewPage {
    shell: WeakEntity<Shell>,
    notes: Option<Notes>,
    /// This build's version, channel and commit, as the Version section says them.
    build: Option<SharedString>,
    /// Whether the fold of the raw list is open: it starts closed.
    all_open: bool,
    scroll: ScrollHandle,
    narrow: bool,
}

impl WhatsNewPage {
    pub fn new(shell: WeakEntity<Shell>) -> Self {
        Self {
            shell,
            notes: None,
            build: None,
            all_open: false,
            scroll: ScrollHandle::new(),
            narrow: false,
        }
    }

    pub fn set_notes(&mut self, notes: Notes, cx: &mut Context<Self>) {
        if self.notes.as_ref() != Some(&notes) {
            self.notes = Some(notes);
            cx.notify();
        }
    }

    pub fn set_build(&mut self, build: Option<String>, cx: &mut Context<Self>) {
        let build = build.map(SharedString::from);
        if self.build != build {
            self.build = build;
            cx.notify();
        }
    }

    pub fn set_narrow(&mut self, narrow: bool, cx: &mut Context<Self>) {
        if self.narrow != narrow {
            self.narrow = narrow;
            cx.notify();
        }
    }

    /// Opened afresh: the fold of the raw list is closed, and the blocks enter again.
    pub fn opened(&mut self, cx: &mut Context<Self>) {
        self.all_open = false;
        cx.notify();
    }

    /// Check again: the daemon reads the notes once more.
    fn again(&mut self, cx: &mut Context<Self>) {
        self.shell
            .update(cx, |shell, _| {
                shell.perform(Command::Update {
                    method: "whatsnew.fetch",
                    params: json!({}),
                })
            })
            .ok();
    }

    fn open_link(&self, url: &str, cx: &Context<Self>) {
        if let Some(shell) = self.shell.upgrade() {
            shell.read(cx).open_link(url);
        }
    }

    /// A pull request's number, a link: its ink fades on hover, a click opens it with the
    /// system's handler.
    fn pull_request(
        &self,
        id: ElementId,
        pr: u64,
        url: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = palette(cx);
        let this = cx.entity().downgrade();
        let label = SharedString::from(format!("#{pr}"));
        let link = div()
            .id(id)
            .aria_label(label.clone())
            .flex_none()
            .cursor_pointer()
            .mono(12.)
            .child(label)
            .on_click(move |_, _, cx| {
                this.update(cx, |this, cx| this.open_link(&url, cx)).ok();
            });
        hoverable(link, move |d, t| d.text_color(mix(p.faint, p.acc, t))).into_any_element()
    }

    fn link_of(&self, id: &str, change: &Change, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (pr, url) = change.pr.zip(change.url.clone())?;
        Some(self.pull_request(ElementId::Name(format!("{id}-pr").into()), pr, url, cx))
    }

    /// A note, a row of the list: the sentence, the number of its pull request at the end.
    fn note(&self, n: usize, change: &Change, cx: &mut Context<Self>) -> ListRow {
        let id = format!("whats-new-change-{n}");
        let row = ListRow::new(ElementId::Name(id.clone().into()), change.text.clone());
        match self.link_of(&id, change, cx) {
            Some(link) => row.trailing(link),
            None => row,
        }
    }

    /// An area: a section with its count, its notes as the rows of one list. `first` is the number
    /// of its first note on the page.
    fn area(&self, area: &Area, first: usize, cx: &mut Context<Self>) -> Section {
        let rows: Vec<ListRow> = area
            .changes
            .iter()
            .enumerate()
            .map(|(i, change)| self.note(first + i, change, cx))
            .collect();
        Section::new(
            ElementId::Name(format!("whats-new-area-{}", area.key).into()),
            area.title.clone(),
        )
        .count(area.changes.len().to_string())
        .child(
            ListCard::new(ElementId::Name(
                format!("whats-new-area-{}-list", area.key).into(),
            ))
            .rows(rows),
        )
    }

    /// The lines written by hand for a release, a list under a heading of their own.
    fn highlights(&self, lines: &[String]) -> Section {
        Section::new("whats-new-highlights", "Highlights").child(
            ListCard::new("whats-new-highlights-list").rows(lines.iter().enumerate().map(
                |(n, line)| {
                    ListRow::new(
                        ElementId::Name(format!("whats-new-highlight-{n}").into()),
                        line.clone(),
                    )
                },
            )),
        )
    }

    /// One line of the raw list: the subject, its pull request a link at the end.
    fn raw_line(&self, n: usize, change: &Change, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let text = SharedString::from(change.text.clone());
        let id = format!("whats-new-raw-{n}");
        let link = self.link_of(&id, change, cx);
        div()
            .flex()
            .items_baseline()
            .gap(space::SM)
            .text_size(px(13.))
            .child(
                div()
                    .id(ElementId::Name(id.into()))
                    .aria_label(text.clone())
                    .flex_1()
                    .min_w_0()
                    .text_color(p.text)
                    .child(text)
                    .test_support(),
            )
            .children(link)
            .into_any_element()
    }

    /// The raw list of subjects in a fold, closed until it is opened.
    fn all_changes(&self, notes: &Notes, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let mut n = 0;
        let mut body = div().flex().flex_col().gap(space::MD);
        for group in &notes.all {
            let mut scopes = div().flex().flex_col().gap(space::SM);
            for scope in &group.scopes {
                let mut rows = div().flex().flex_col().gap(space::XS);
                for change in &scope.changes {
                    rows = rows.child(self.raw_line(n, change, cx));
                    n += 1;
                }
                scopes = scopes.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(space::XS)
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(p.mute)
                                .child(scope.name.clone().unwrap_or_else(|| "General".to_owned())),
                        )
                        .child(rows),
                );
            }
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(space::SM)
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .text_color(p.text)
                            .child(group.title.clone()),
                    )
                    .child(scopes),
            );
        }
        let count = notes.raw_count();
        let this = cx.entity().downgrade();
        Disclosure::new("whats-new-all", "All changes", self.all_open)
            .summary(if count == 1 {
                "1 change".to_owned()
            } else {
                format!("{count} changes")
            })
            .body(body)
            .on_toggle(move |open, _, cx| {
                this.update(cx, |this, cx| {
                    this.all_open = open;
                    cx.notify();
                })
                .ok();
            })
            .into_any_element()
    }
}

impl Render for WhatsNewPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let notes = self.notes.clone().unwrap_or_default();
        let range = notes.since.clone().map(|since| {
            let said = SharedString::from(format!("since {since}"));
            div()
                .id("whats-new-range")
                .aria_label(said.clone())
                .text_size(px(12.5))
                .text_color(p.mute)
                .child(said)
                .test_support()
        });
        let header = PageHeader::new("whats-new-header", notes.title()).below(
            div()
                .flex()
                .flex_col()
                .gap(space::XS)
                .children(range)
                .child(presence_of(
                    "whats-new-build-line",
                    self.build.clone(),
                    move |build| {
                        div()
                            .id("whats-new-build")
                            .aria_label(build.clone())
                            .text_size(px(12.5))
                            .text_color(p.mute)
                            .child(build)
                            .test_support()
                            .into_any_element()
                    },
                )),
        );

        let key = |part: &str| ElementId::Name(format!("enter-whats-new-{part}").into());
        // The blocks of the list enter one after another: the highlights, each area, the fold.
        let mut blocks = div().flex().flex_col().gap(space::XXL);
        let mut stagger = 0;
        if !notes.highlights.is_empty() {
            blocks = blocks.child(enter(
                key("highlights"),
                stagger,
                self.highlights(&notes.highlights),
            ));
            stagger += 1;
        }
        let mut first = 0;
        for area in notes.groups.iter().flatten() {
            blocks = blocks.child(enter(key(&area.key), stagger, self.area(area, first, cx)));
            first += area.changes.len();
            stagger += 1;
        }
        if !notes.all.is_empty() {
            blocks = blocks.child(enter(key("all"), stagger, self.all_changes(&notes, cx)));
        }

        let nothing = notes.groups.as_ref().is_some_and(Vec::is_empty)
            && notes.highlights.is_empty()
            && notes.all.is_empty();
        let reading = notes.groups.is_none() && !notes.failed;
        let again = cx.entity().downgrade();
        let failed = presence_of(
            "whats-new-failed-line",
            (notes.groups.is_none() && notes.failed)
                .then(|| SharedString::from("The changes of these versions could not be read.")),
            move |why| {
                let again = again.clone();
                div()
                    .flex()
                    .flex_col()
                    .gap(space::SM)
                    .child(
                        div()
                            .id("whats-new-failed")
                            .aria_label(why.clone())
                            .text_size(px(13.))
                            .text_color(p.fail)
                            .child(why)
                            .test_support(),
                    )
                    .child(div().flex().child(
                        Button::secondary("whats-new-again", "Check again").on_click(
                            move |_, _, cx| {
                                again.update(cx, |this, cx| this.again(cx)).ok();
                            },
                        ),
                    ))
                    .into_any_element()
            },
        )
        .space(space::MD);
        let list = presence(
            "whats-new-list-block",
            reading || (notes.groups.is_some() && !nothing),
            awaiting(
                "whats-new-awaiting",
                reading,
                Skeleton::new("whats-new-skeleton").h(px(160.)),
                blocks,
            ),
        )
        .space(space::MD);
        let empty = presence(
            "whats-new-empty-block",
            nothing,
            EmptyLine::new(
                "whats-new-empty",
                Icon::Repository,
                "No change is listed for these versions.",
            ),
        )
        .space(space::MD);
        let pad = if self.narrow { space::LG } else { space::XXL };
        let column = div()
            .w_full()
            .max_w(space::COLUMN)
            .flex()
            .flex_col()
            .gap(space::XXL)
            .child(enter(key("header"), 0, header))
            .child(stack(space::MD).child(list).child(failed).child(empty));
        div()
            .id("whats-new-page")
            .track_scroll(&self.scroll)
            .size_full()
            .overflow_y_scroll()
            .text_color(p.text)
            .flex()
            .flex_col()
            .items_center()
            .px(pad)
            .pt(space::XXL)
            .pb(space::XXXL)
            .child(column)
            .test_support()
    }
}
