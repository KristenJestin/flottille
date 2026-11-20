//! What's new since the version last run (`docs/releases.md`, "What's new"): the page that lists
//! every change between that version and this one, as the daemon gathered them from the release
//! notes (`whatsnew.show`), grouped as the changelog groups them and then by scope, each change
//! with its pull request linked. It is a page of the window, opened by the mark at the foot of the
//! sidebar or from Settings, General, Version, never by itself.
//!
//! What moves: the page's blocks enter one after another, a group folds and unfolds
//! (`Disclosure`), a link's ink fades on hover, the skeleton of a read that waits crossfades into
//! the changes (`awaiting`), and the line of a read that failed unfolds and folds away
//! (`presence_of`).

use std::collections::HashSet;

use gpui_design_system::color::mix;
use gpui_design_system::{
    Button, Disclosure, EmptyLine, Icon, PageHeader, Section, Skeleton, Type, awaiting, enter,
    hoverable, palette, presence, presence_of, space,
};
use gpui_kit::{
    AnyElement, Context, ElementId, InteractiveElement, IntoElement, ParentElement, Render,
    ScrollHandle, SharedString, StatefulInteractiveElement, Styled, TestSupportExt as _,
    WeakEntity, Window, div, px,
};
use serde_json::{Value, json};

use crate::actions::Command;
use crate::commands::{section_blocks, stack};
use crate::shell::Shell;

/// One change: its words and the pull request that made it.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub text: String,
    pub pr: Option<u64>,
    pub url: Option<String>,
}

/// The changes of one scope of a group; none is a change that names no scope.
#[derive(Clone, Debug, PartialEq)]
pub struct Scope {
    pub name: Option<String>,
    pub changes: Vec<Change>,
}

/// A group of the changelog: breaking changes, features, fixes, performance, other changes.
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
    /// The version last run, the changes are counted from: none when nothing is new.
    pub since: Option<String>,
    /// Whether the page was opened since this version runs.
    pub seen: bool,
    /// Why the notes could not be read, when they could not.
    pub failed: bool,
    /// The changes, once read.
    pub groups: Option<Vec<Group>>,
}

impl Notes {
    pub fn of(value: &Value) -> Notes {
        let text = |value: &Value, key: &str| value[key].as_str().map(str::to_owned);
        let groups = value["groups"].as_array().map(|groups| {
            groups
                .iter()
                .map(|group| Group {
                    key: text(group, "key").unwrap_or_default(),
                    title: text(group, "title").unwrap_or_default(),
                    scopes: group["scopes"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|scope| Scope {
                            name: text(scope, "scope"),
                            changes: scope["changes"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .map(|change| Change {
                                    text: text(change, "text").unwrap_or_default(),
                                    pr: change["pr"].as_u64(),
                                    url: text(change, "url"),
                                })
                                .collect(),
                        })
                        .collect(),
                })
                .collect()
        });
        Notes {
            since: text(value, "since"),
            seen: value["seen"] == true,
            failed: value["failed"].is_string(),
            groups,
        }
    }

    /// What the sidebar's mark says: only while there is something new that was not opened.
    pub fn mark(&self) -> Option<String> {
        (self.since.is_some() && !self.seen).then(|| "What's new".to_owned())
    }
}

pub struct WhatsNewPage {
    shell: WeakEntity<Shell>,
    notes: Option<Notes>,
    /// This build's version, channel and commit, as the Version section says them.
    build: Option<SharedString>,
    /// The groups folded by the user: they all start unfolded.
    folded: HashSet<String>,
    scroll: ScrollHandle,
    narrow: bool,
}

impl WhatsNewPage {
    pub fn new(shell: WeakEntity<Shell>) -> Self {
        Self {
            shell,
            notes: None,
            build: None,
            folded: HashSet::new(),
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

    /// Opened afresh: its groups are all unfolded, and its blocks enter again.
    pub fn opened(&mut self, cx: &mut Context<Self>) {
        self.folded.clear();
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
        hoverable(link, move |d, t| d.text_color(mix(p.mute, p.acc, t))).into_any_element()
    }

    fn change(&self, n: usize, change: &Change, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let text = SharedString::from(change.text.clone());
        let pr = change.pr.zip(change.url.clone()).map(|(pr, url)| {
            self.pull_request(
                ElementId::Name(format!("whats-new-change-{n}-pr").into()),
                pr,
                url,
                cx,
            )
        });
        div()
            .id(ElementId::Name(format!("whats-new-change-row-{n}").into()))
            .flex()
            .items_baseline()
            .gap(space::SM)
            .text_size(px(13.))
            .child(
                div()
                    .id(ElementId::Name(format!("whats-new-change-{n}").into()))
                    .aria_label(text.clone())
                    .flex_1()
                    .min_w_0()
                    .text_color(p.text)
                    .child(text)
                    .test_support(),
            )
            .children(pr)
            .into_any_element()
    }

    /// A group: its line folds and unfolds the scopes under it. `first` is the number of the
    /// group's first change on the page.
    fn group(&self, group: &Group, first: usize, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let mut n = first;
        let mut body = div().flex().flex_col().gap(space::MD).pt(space::SM);
        for scope in &group.scopes {
            let mut rows = div().flex().flex_col().gap(space::XS);
            for change in &scope.changes {
                rows = rows.child(self.change(n, change, cx));
                n += 1;
            }
            body = body.child(
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
        let count = group.count();
        let this = cx.entity().downgrade();
        let key = group.key.clone();
        Disclosure::new(
            ElementId::Name(format!("whats-new-group-{}", group.key).into()),
            group.title.clone(),
            !self.folded.contains(&group.key),
        )
        .summary(if count == 1 {
            "1 change".to_owned()
        } else {
            format!("{count} changes")
        })
        .body(body)
        .on_toggle(move |open, _, cx| {
            let key = key.clone();
            this.update(cx, |this, cx| {
                if open {
                    this.folded.remove(&key);
                } else {
                    this.folded.insert(key);
                }
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
        let since = notes.since.clone().unwrap_or_default();
        let header =
            PageHeader::new("whats-new-header", format!("What's new since {since}")).below(
                presence_of("whats-new-build-line", self.build.clone(), move |build| {
                    div()
                        .id("whats-new-build")
                        .aria_label(build.clone())
                        .text_size(px(12.5))
                        .text_color(p.mute)
                        .child(build)
                        .test_support()
                        .into_any_element()
                }),
            );

        // The changes drawn as numbered lines, the group's first number running on.
        let mut blocks = div().flex().flex_col().gap(space::LG);
        let mut total = 0;
        let mut first = 0;
        for group in notes.groups.iter().flatten() {
            let count = group.count();
            blocks = blocks.child(self.group(group, first, cx));
            first += count;
            total += count;
        }
        let empty = notes.groups.as_ref().is_some_and(Vec::is_empty);
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
        let mut section = Section::new("whats-new-changes", "Changes").sentence(
            "Every change between the version you last ran and this one, each once, as the \
             changelog groups them.",
        );
        if notes.groups.is_some() {
            section = section.count(total.to_string());
        }
        let list = presence(
            "whats-new-list-block",
            reading
                || notes
                    .groups
                    .as_ref()
                    .is_some_and(|groups| !groups.is_empty()),
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
            empty,
            EmptyLine::new(
                "whats-new-empty",
                Icon::Repository,
                "No change is listed for these versions.",
            ),
        )
        .space(space::MD);
        // Always something to say: the skeleton, the changes, the failure or the empty line.
        let section = section.child(section_blocks(
            "whats-new-blocks",
            true,
            space::MD,
            stack(space::MD).child(list).child(failed).child(empty),
        ));
        let pad = if self.narrow { space::LG } else { space::XXL };
        let key = |part: &str| ElementId::Name(format!("enter-whats-new-{part}").into());
        let column = div()
            .w_full()
            .max_w(space::COLUMN)
            .flex()
            .flex_col()
            .gap(space::XXL)
            .child(enter(key("header"), 0, header))
            .child(enter(key("changes"), 1, section));
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
