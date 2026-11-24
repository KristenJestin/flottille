//! Each repository's commit checks, in the Instructions tab of the project's Settings
//! (`docs/projects.md`, "Commit checks"): one command line each, run before an agent's commit in
//! the repository (pre-commit) and on its message (commit-msg). What the repository's hooks hold
//! is proposed under the fields, each line with Add, and runs only once it is saved. The window
//! writes and reads nothing itself: the daemon reads the hooks (`project.commit_checks`) and saves
//! (`project.set_commit_checks`), and the fields show what it says next.

use std::collections::HashMap;

use gpui_design_system::{
    Button, Field, Icon, ListCard, ListRow, Listing, Section, Skeleton, Toast, Type as _, awaiting,
    follow_theme, growing, icon, palette, presence, presence_of, space, toast,
};
use gpui_kit::base::input::{InputEvent, TextareaState};
use gpui_kit::{
    App, AppContext as _, ClickEvent, Context, ElementId, Entity, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement as _, Styled, Subscription,
    TestSupportExt as _, Window, div, px,
};
use serde_json::Value;

use crate::actions::Command;
use crate::commands::{perform_later, section_blocks, stack};
use crate::project::{only_at_root, repository_label};

/// What the section says they are for.
pub const SENTENCE: &str = "Command lines run before an agent commits in a repository: before \
                            the commit, then on its message. One that fails refuses the commit, \
                            and the agent is told why. They are yours: the repository's own \
                            hooks never run, and what they hold is only proposed here.";

/// The two hooks a check stands for, as the daemon names them, with the key of their lines.
const HOOKS: [(&str, &str); 2] = [("pre-commit", "pre_commit"), ("commit-msg", "commit_msg")];

/// A line of the repository's hooks: proposed to be added, or left out with why.
#[derive(Clone, Debug, PartialEq)]
pub struct Proposal {
    pub hook: String,
    pub line: String,
    pub from: String,
    /// Why it cannot be a check (shell syntax); none for a line that can be added.
    pub why: Option<String>,
}

impl Proposal {
    fn key(&self) -> String {
        format!("{}\n{}\n{}", self.hook, self.from, self.line)
    }

    /// What its row's ids are made of, the same for as long as it is proposed.
    pub fn id(&self) -> String {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.key().hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }
}

/// One repository's checks: its two fields, what was saved, what its hooks propose.
pub struct RepositoryChecks {
    pub path: String,
    /// Made when first drawn, since a text field needs a window: pre-commit, then commit-msg.
    fields: [Option<Entity<TextareaState>>; 2],
    /// The lines as the daemon last said them, by hook.
    saved: [Vec<String>; 2],
    proposals: Vec<Proposal>,
    rows: Listing<Proposal>,
    waiting: bool,
    /// Why the last save was refused, under the fields until the text changes.
    problem: Option<SharedString>,
}

impl RepositoryChecks {
    fn of(read: &Value) -> RepositoryChecks {
        let lines = |key: &str| -> Vec<String> {
            read[key]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        };
        let mut proposals: Vec<Proposal> = Vec::new();
        for (key, left) in [("proposed", false), ("left_out", true)] {
            for found in read[key].as_array().into_iter().flatten() {
                let text = |key: &str| found[key].as_str().unwrap_or_default().to_owned();
                proposals.push(Proposal {
                    hook: text("hook"),
                    line: text("line"),
                    from: text("from"),
                    why: left.then(|| text("why")),
                });
            }
        }
        RepositoryChecks {
            path: read["path"].as_str().unwrap_or_default().to_owned(),
            fields: [None, None],
            saved: [lines(HOOKS[0].1), lines(HOOKS[1].1)],
            proposals,
            rows: Listing::default(),
            waiting: false,
            problem: None,
        }
    }

    /// The text in the field of the hook at `at`.
    pub fn typed(&self, at: usize, cx: &App) -> Option<String> {
        self.fields[at]
            .as_ref()
            .map(|field| field.read(cx).value().to_string())
    }

    /// The checks a field holds: one a line, blank lines left out.
    fn lines(&self, at: usize, cx: &App) -> Option<Vec<String>> {
        self.typed(at, cx).map(|typed| {
            typed
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect()
        })
    }

    /// Whether a field differs from what the daemon last said.
    fn typing(&self, cx: &App) -> bool {
        (0..2).any(|at| {
            self.lines(at, cx)
                .is_some_and(|lines| lines != self.saved[at])
        })
    }

    pub fn savable(&self, cx: &App) -> bool {
        !self.waiting && self.typing(cx)
    }

    pub fn saved(&self, at: usize) -> &[String] {
        &self.saved[at]
    }

    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    /// What its hooks propose, the lines left out included, as last read.
    pub fn proposals(&self) -> &[Proposal] {
        &self.proposals
    }

    /// What its hooks propose that the fields do not hold yet, and what they hold that cannot be
    /// a check.
    pub fn shown(&self, cx: &App) -> Vec<Proposal> {
        self.proposals
            .iter()
            .filter(|proposal| {
                proposal.why.is_some()
                    || !HOOKS.iter().enumerate().any(|(at, (hook, _))| {
                        *hook == proposal.hook
                            && self
                                .lines(at, cx)
                                .unwrap_or_else(|| self.saved[at].clone())
                                .contains(&proposal.line)
                    })
            })
            .cloned()
            .collect()
    }
}

pub struct CommitChecks {
    shell: crate::commands::ShellOf,
    project: String,
    /// None until the daemon answered the first read.
    repositories: Option<Vec<RepositoryChecks>>,
    /// Why the checks could not be read.
    unread: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl CommitChecks {
    pub fn new(shell: crate::commands::ShellOf, project: String) -> Self {
        Self {
            shell,
            project,
            repositories: None,
            unread: None,
            _subscriptions: Vec::new(),
        }
    }

    pub fn read(&mut self, cx: &mut Context<Self>) {
        perform_later(
            &self.shell,
            Command::ReadCommitChecks(self.project.clone()),
            cx,
        );
    }

    pub fn repositories(&self) -> Option<&[RepositoryChecks]> {
        self.repositories.as_deref()
    }

    fn repository(&self, path: &str) -> Option<&RepositoryChecks> {
        self.repositories
            .as_ref()?
            .iter()
            .find(|repository| repository.path == path)
    }

    /// The field of `path`'s hook `hook` (`pre-commit`, `commit-msg`), once drawn.
    pub fn field(&self, path: &str, hook: &str) -> Option<Entity<TextareaState>> {
        let at = HOOKS.iter().position(|(name, _)| *name == hook)?;
        self.repository(path)?.fields[at].clone()
    }

    /// Adds a proposed line at the end of its field: its row folds away, since the field holds it.
    pub fn add(
        &mut self,
        path: &str,
        proposal: &Proposal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(field) = self.field(path, &proposal.hook) else {
            return;
        };
        let mut text = field.read(cx).value().to_string();
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&proposal.line);
        field.update(cx, |field, cx| field.set_value(&text, window, cx));
        cx.notify();
    }

    pub fn save(&mut self, path: &str, cx: &mut Context<Self>) {
        let project = self.project.clone();
        let Some(repository) = self
            .repositories
            .as_mut()
            .and_then(|all| all.iter_mut().find(|repository| repository.path == path))
        else {
            return;
        };
        let (Some(pre_commit), Some(commit_msg)) =
            (repository.lines(0, cx), repository.lines(1, cx))
        else {
            return;
        };
        if !repository.savable(cx) {
            return;
        }
        repository.waiting = true;
        repository.problem = None;
        let command = Command::SaveCommitChecks {
            project,
            repository: path.to_owned(),
            pre_commit,
            commit_msg,
        };
        perform_later(&self.shell, command, cx);
        cx.notify();
    }

    /// The daemon's answer to a read, or to the save of `saved`'s repository: every repository's
    /// checks and proposals, or why nothing was read or written. A field follows what was saved
    /// unless something is being typed in its repository; a saved one takes what was written.
    pub fn answered(
        &mut self,
        read: Result<Value, String>,
        saved: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let read = match read {
            Ok(read) => read,
            Err(why) => {
                match saved.as_deref().and_then(|path| {
                    self.repositories
                        .as_mut()?
                        .iter_mut()
                        .find(|r| r.path == path)
                }) {
                    Some(repository) => {
                        repository.waiting = false;
                        repository.problem = Some(why.into());
                    }
                    None => self.unread = Some(why.into()),
                }
                cx.notify();
                return;
            }
        };
        self.unread = None;
        let mut old: HashMap<String, RepositoryChecks> = self
            .repositories
            .take()
            .into_iter()
            .flatten()
            .map(|repository| (repository.path.clone(), repository))
            .collect();
        let mut repositories = Vec::new();
        for answer in read["repositories"].as_array().into_iter().flatten() {
            let mut new = RepositoryChecks::of(answer);
            if let Some(kept) = old.remove(&new.path) {
                let written = saved.as_deref() == Some(new.path.as_str());
                let typing = kept.typing(cx) && !written;
                new.fields = kept.fields;
                new.rows = kept.rows;
                new.problem = if written { None } else { kept.problem };
                new.waiting = kept.waiting && !written;
                if !typing {
                    for at in 0..2 {
                        if let Some(field) = new.fields[at].clone() {
                            let text = new.saved[at].join("\n");
                            field.update(cx, |field, cx| field.set_value(&text, window, cx));
                        }
                    }
                }
                if written {
                    let count = new.saved[0].len() + new.saved[1].len();
                    let detail = match count {
                        0 => "No check runs on your agents' commits in it.".to_owned(),
                        _ => "They run on your agents' next commits in it.".to_owned(),
                    };
                    let label = repository_label(&self.project, &new.path);
                    toast(
                        Toast::success(format!("Commit checks of {label} saved")).detail(detail),
                        cx,
                    );
                }
            }
            repositories.push(new);
        }
        self.repositories = Some(repositories);
        cx.notify();
    }

    fn make_field(
        &mut self,
        path: &str,
        at: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<TextareaState>> {
        let placeholder = match at {
            0 => "lint-staged",
            _ => "npx --no -- commitlint --edit {message_file}",
        };
        let field = cx.new(|cx| {
            let mut field = TextareaState::new(window, cx)
                .auto_grow(2, 8)
                .placeholder(placeholder);
            follow_theme(&mut field, cx);
            field
        });
        let changed = path.to_owned();
        self._subscriptions
            .push(cx.subscribe(&field, move |this, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    if let Some(repository) = this
                        .repositories
                        .as_mut()
                        .and_then(|all| all.iter_mut().find(|r| r.path == changed))
                    {
                        repository.problem = None;
                    }
                    cx.notify();
                }
            }));
        let repository = self
            .repositories
            .as_mut()?
            .iter_mut()
            .find(|repository| repository.path == path)?;
        let text = repository.saved[at].join("\n");
        field.update(cx, |field, cx| field.set_value(&text, window, cx));
        repository.fields[at] = Some(field.clone());
        Some(field)
    }

    /// One repository's block: its name when the project has several, its two fields, Save, and
    /// what its hooks propose.
    fn repository_block(
        &mut self,
        path: &str,
        named: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let p = palette(cx);
        let project = self.project.clone();
        let id =
            |part: &str| ElementId::Name(format!("commit-checks-{part}-{project}-{path}").into());
        let mut texts = Vec::new();
        for (at, _) in HOOKS.iter().enumerate() {
            let field = match self.repository(path).and_then(|r| r.fields[at].clone()) {
                Some(field) => Some(field),
                None => self.make_field(path, at, window, cx),
            };
            let hook = HOOKS[at].0;
            let text = div()
                .id(id(&format!("{hook}-text")))
                .px(space::MD)
                .py(space::SM)
                .rounded(px(8.))
                .bg(p.field)
                .border_1()
                .border_color(p.line2)
                .mono(12.5)
                .line_height(px(20.))
                .children(field)
                .test_support();
            // The field grows by a line as a check is added, and its height eases there.
            texts.push(growing(id(&format!("{hook}-height")), text));
        }
        let Some(repository) = self.repository(path) else {
            return div().into_any_element();
        };
        let savable = repository.savable(cx);
        // A refusal is said under the field of the hook it names, the commit-msg one otherwise.
        let problem = repository.problem.clone();
        let pre_commit = problem.as_ref().is_some_and(|why| {
            why.split('`')
                .next()
                .is_some_and(|start| start.contains("pre-commit check"))
        });
        let (pre_problem, msg_problem) = match pre_commit {
            true => (problem, None),
            false => (None, problem),
        };
        let now = cx.background_executor().now();
        let reduce = cx.reduce_motion();
        let shown = repository.shown(cx);
        let mut list = ListCard::new(id("proposals-list"));
        if let Some(repository) = self
            .repositories
            .as_mut()
            .and_then(|all| all.iter_mut().find(|r| r.path == path))
        {
            repository.rows.sync(
                shown.into_iter().map(|proposal| (proposal.key(), proposal)),
                now,
                reduce,
            );
        }
        let Some(repository) = self.repository(path) else {
            return div().into_any_element();
        };
        let drawn: Vec<_> = repository
            .rows
            .rows()
            .map(|row| (row.value.id(), row.placement, row.value.clone()))
            .collect();
        let rows = drawn.len();
        for (n, placement, proposal) in drawn {
            let row_id = |part: &str| id(&format!("{part}-{n}"));
            let mut row = ListRow::new(
                row_id("proposal"),
                SharedString::from(proposal.line.clone()),
            )
            .mono()
            .leading(icon(Icon::File).size(px(14.)).text_color(p.faint))
            .tag(SharedString::from(proposal.hook.clone()));
            match &proposal.why {
                Some(why) => {
                    row = row.below(
                        div()
                            .id(row_id("left-out"))
                            .aria_label(SharedString::from(format!(
                                "Left out of {}: {why}",
                                proposal.from
                            )))
                            .text_size(px(12.5))
                            .text_color(p.mute)
                            .child(SharedString::from(format!(
                                "Left out of {}: {why}",
                                proposal.from
                            )))
                            .test_support(),
                    );
                }
                None => {
                    let (this, path, added) =
                        (cx.entity().downgrade(), path.to_owned(), proposal.clone());
                    row = row
                        .line(SharedString::from(format!("From {}", proposal.from)))
                        .trailing(Button::secondary(row_id("add"), "Add").on_click(
                            move |_: &ClickEvent, window, cx| {
                                cx.stop_propagation();
                                this.update(cx, |this, cx| this.add(&path, &added, window, cx))
                                    .ok();
                            },
                        ));
                }
            }
            list = list.item(row_id("proposal-item"), placement, row);
        }
        let [pre_text, msg_text]: [_; 2] = match texts.try_into() {
            Ok(texts) => texts,
            Err(_) => return div().into_any_element(),
        };
        let saving = path.to_owned();
        let label = repository_label(&self.project, path);
        div()
            .id(id("repository"))
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(space::MD)
                    .children(named.then(|| {
                        div()
                            .id(id("name"))
                            .aria_label(SharedString::from(label.clone()))
                            .flex()
                            .items_center()
                            .gap(space::SM)
                            .text_size(px(13.))
                            .child(icon(Icon::Repository).size(px(14.)).text_color(p.faint))
                            .child(SharedString::from(label.clone()))
                            .test_support()
                    }))
                    .child(
                        Field::new(
                            id("pre-commit-field"),
                            "Before the commit (pre-commit)",
                            pre_text,
                        )
                        .optional()
                        .hint("One command line per line, run in the repository's folder.")
                        .error(pre_problem),
                    )
                    .child(
                        Field::new(
                            id("commit-msg-field"),
                            "On its message (commit-msg)",
                            msg_text,
                        )
                        .optional()
                        .hint(
                            "One command line per line; {message_file} stands for the file \
                                 that holds the message.",
                        )
                        .error(msg_problem),
                    )
                    .child(div().flex().justify_end().child(
                        presence(
                            id("save-block"),
                            savable,
                            Button::secondary(id("save"), "Save").on_click(
                                cx.listener(move |this, _, _, cx| this.save(&saving, cx)),
                            ),
                        ),
                    )),
            )
            .child(
                presence(
                    id("proposals-block"),
                    rows > 0,
                    div()
                        .id(id("proposals"))
                        .flex()
                        .flex_col()
                        .gap(space::SM)
                        .child(
                            div()
                                .min_h(px(30.))
                                .flex()
                                .items_center()
                                .text_size(px(12.5))
                                .text_color(p.mute)
                                .child("Read in the repository's hooks, not saved yet"),
                        )
                        .child(list)
                        .test_support(),
                )
                .space(space::MD),
            )
            .test_support()
            .into_any_element()
    }
}

impl Render for CommitChecks {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let project = self.project.clone();
        let paths: Vec<String> = self
            .repositories
            .iter()
            .flatten()
            .map(|repository| repository.path.clone())
            .collect();
        let named = !only_at_root(&paths);
        let blocks: Vec<gpui_kit::AnyElement> = paths
            .iter()
            .map(|path| self.repository_block(path, named, window, cx))
            .collect();
        let pending = self.repositories.is_none() && self.unread.is_none();
        let id = |part: &str| ElementId::Name(format!("commit-checks-{part}-{project}").into());
        Section::new(format!("commit-checks-{project}"), "Commit checks")
            .sentence(SENTENCE)
            .child(awaiting(
                id("read"),
                pending,
                Skeleton::new(id("skeleton")).h(px(96.)),
                div()
                    .id(id("repositories"))
                    .flex()
                    .flex_col()
                    .gap(space::XL)
                    .children(blocks)
                    .test_support(),
            ))
            .child(section_blocks(
                "commit-checks-blocks",
                self.unread.is_some(),
                space::MD,
                stack(space::MD).child(
                    presence_of(id("unread-block"), self.unread.clone(), move |why| {
                        div()
                            .id("commit-checks-unread")
                            .aria_label(why.clone())
                            .text_size(px(12.5))
                            .text_color(p.fail)
                            .child(why)
                            .test_support()
                            .into_any_element()
                    })
                    .space(space::MD),
                ),
            ))
    }
}
