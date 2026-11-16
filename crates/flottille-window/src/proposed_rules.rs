//! The rules proposed for a project from the failures its agents keep making (`docs/projects.md`,
//! "Proposed rules"), in the Instructions tab of the project's Settings, under its rules: each
//! the rule the judge wrote, in a field the user edits, how often the failure was seen, the
//! sessions it was seen in (each opens its session), and two actions, Add to the project's rules
//! and Dismiss. The window decides nothing: what is proposed is what the daemon says
//! (`rules.proposals`, read with the shell's snapshot), and the daemon adds or dismisses
//! (`rules.proposal.accept`, `rules.proposal.dismiss`).

use std::collections::{HashMap, HashSet};

use gpui_design_system::{
    Button, Field, ListCard, ListRow, Listing, Section, Toast, follow_theme, palette, presence,
    space, toast,
};
use gpui_kit::base::input::{InputEvent, TextareaState};
use gpui_kit::{
    AppContext as _, ClickEvent, Context, ElementId, Entity, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, Styled, Subscription, TestSupportExt as _, WeakEntity,
    Window, div, px,
};

use crate::actions::Command;
use crate::commands::perform_later;
use crate::model::ProposedRule;
use crate::shell::{Selection, Shell};

/// What the section says they are.
pub const SENTENCE: &str = "Your agents kept failing the same way, and the judge wrote a rule \
                            that would spare them. Edit it, then add it to this project's rules: \
                            it reaches the sessions started from then on.";

/// The element id of a part of a proposal's row.
pub fn part_id(id: u64, part: &str) -> ElementId {
    ElementId::Name(format!("proposed-rule-{id}-{part}").into())
}

pub struct ProposedRules {
    shell: WeakEntity<Shell>,
    project: String,
    rows: Listing<ProposedRule>,
    /// Each proposal's field, made when first drawn, since a text field needs a window.
    fields: HashMap<u64, Entity<TextareaState>>,
    /// The proposals whose answer waits for the daemon.
    waiting: HashSet<u64>,
    /// The proposals answered: they fold away at once, before the daemon's list says so.
    answered: HashSet<u64>,
    /// Why an answer was refused, under its field until the text changes.
    problems: HashMap<u64, SharedString>,
    /// Each field's watch of its text, kept as long as the field.
    _subscriptions: HashMap<u64, Subscription>,
}

impl ProposedRules {
    pub fn new(shell: WeakEntity<Shell>, project: String) -> Self {
        Self {
            shell,
            project,
            rows: Listing::default(),
            fields: HashMap::new(),
            waiting: HashSet::new(),
            answered: HashSet::new(),
            problems: HashMap::new(),
            _subscriptions: HashMap::new(),
        }
    }

    /// The project's proposals that wait for the user, as the shell last read them.
    pub fn proposals(&self, cx: &gpui_kit::App) -> Vec<ProposedRule> {
        let Some(shell) = self.shell.upgrade() else {
            return Vec::new();
        };
        shell
            .read(cx)
            .snapshot()
            .proposals
            .iter()
            .filter(|proposal| proposal.project == self.project)
            .filter(|proposal| !self.answered.contains(&proposal.id))
            .cloned()
            .collect()
    }

    /// The field of a proposal, once drawn.
    pub fn field(&self, id: u64) -> Option<&Entity<TextareaState>> {
        self.fields.get(&id)
    }

    pub fn problem(&self, id: u64) -> Option<&str> {
        self.problems.get(&id).map(|why| why.as_ref())
    }

    /// Adds the rule as the field holds it to the project's rules.
    pub fn accept(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(text) = self
            .fields
            .get(&id)
            .map(|field| field.read(cx).value().to_string())
        else {
            return;
        };
        self.answer(
            id,
            Command::ProposalAnswer {
                project: self.project.clone(),
                proposal: id,
                text: Some(text),
            },
            cx,
        );
    }

    pub fn dismiss(&mut self, id: u64, cx: &mut Context<Self>) {
        self.answer(
            id,
            Command::ProposalAnswer {
                project: self.project.clone(),
                proposal: id,
                text: None,
            },
            cx,
        );
    }

    fn answer(&mut self, id: u64, command: Command, cx: &mut Context<Self>) {
        if !self.waiting.insert(id) {
            return;
        }
        self.problems.remove(&id);
        perform_later(&self.shell, command, cx);
        cx.notify();
    }

    /// The daemon's answer: the proposal folds away, and a toast says what became of it; a
    /// refusal is said under its field.
    pub fn answered(
        &mut self,
        id: u64,
        added: bool,
        result: Result<(), String>,
        cx: &mut Context<Self>,
    ) {
        self.waiting.remove(&id);
        match result {
            Ok(()) => {
                self.answered.insert(id);
                let said = match added {
                    true => Toast::success(format!("Rule added to the rules of {}", self.project))
                        .detail("It reaches the sessions started from now on."),
                    false => Toast::success("Proposed rule dismissed")
                        .detail("This failure will not be proposed again."),
                };
                toast(said, cx);
            }
            Err(why) => {
                self.problems.insert(id, why.into());
            }
        }
        cx.notify();
    }

    fn make_field(&mut self, proposal: &ProposedRule, window: &mut Window, cx: &mut Context<Self>) {
        let id = proposal.id;
        let field = cx.new(|cx| {
            let mut field = TextareaState::new(window, cx).auto_grow(2, 8);
            follow_theme(&mut field, cx);
            field
        });
        field.update(cx, |field, cx| field.set_value(&proposal.rule, window, cx));
        self._subscriptions.insert(
            id,
            cx.subscribe(&field, move |this, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    this.problems.remove(&id);
                    cx.notify();
                }
            }),
        );
        self.fields.insert(id, field);
    }

    /// One proposal: how often its failure was seen, its rule in a field, the sessions it was
    /// seen in, and its two actions.
    fn row(&self, proposal: &ProposedRule, cx: &mut Context<Self>) -> ListRow {
        let p = palette(cx);
        let id = proposal.id;
        let waiting = self.waiting.contains(&id);
        let field = self.fields.get(&id).cloned();
        let text = div()
            .id(part_id(id, "text"))
            .px(space::MD)
            .py(space::SM)
            .rounded(px(8.))
            .bg(p.field)
            .border_1()
            .border_color(p.line2)
            .text_size(px(13.))
            .line_height(px(20.))
            .children(field)
            .test_support();
        let sessions: Vec<_> = proposal
            .calls
            .iter()
            .enumerate()
            .map(|(n, call)| {
                let shell = self.shell.clone();
                let session = call.session.clone();
                Button::quiet(
                    part_id(id, &format!("call-{n}")),
                    format!("Failure {}", n + 1),
                )
                .tooltip(SharedString::from(call.error.clone()))
                .on_click(move |_: &ClickEvent, window, cx| {
                    let session = session.clone();
                    shell
                        .update(cx, |shell, cx| {
                            shell.open(Selection::Session(session), window, cx)
                        })
                        .ok();
                })
            })
            .collect();
        let this = cx.entity().downgrade();
        let dismissing = this.clone();
        let problem = self.problems.get(&id).cloned();
        let below = div()
            .flex()
            .flex_col()
            .gap(space::SM)
            .pt(space::SM)
            .child(Field::new(part_id(id, "field"), "Rule", text).error(problem))
            .child(
                div()
                    .id(part_id(id, "calls"))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(space::XS)
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(p.mute)
                            .child("Seen in:"),
                    )
                    .children(sessions)
                    .test_support(),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(space::SM)
                    .child(
                        Button::quiet(part_id(id, "dismiss"), "Dismiss")
                            .disabled(waiting)
                            .on_click(move |_: &ClickEvent, _, cx| {
                                dismissing.update(cx, |this, cx| this.dismiss(id, cx)).ok();
                            }),
                    )
                    .child(
                        Button::secondary(part_id(id, "add"), "Add to the project's rules")
                            .disabled(waiting)
                            .on_click(move |_: &ClickEvent, _, cx| {
                                this.update(cx, |this, cx| this.accept(id, cx)).ok();
                            }),
                    ),
            );
        ListRow::new(
            part_id(id, "row"),
            SharedString::from(format!(
                "{} refused {} times in {} sessions",
                proposal.tool, proposal.count, proposal.sessions
            )),
        )
        .below(below)
    }
}

impl Render for ProposedRules {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let proposals = self.proposals(cx);
        for proposal in &proposals {
            if !self.fields.contains_key(&proposal.id) {
                self.make_field(proposal, window, cx);
            }
        }
        let now = cx.background_executor().now();
        let reduce = cx.reduce_motion();
        self.rows.sync(
            proposals
                .into_iter()
                .map(|proposal| (proposal.id.to_string(), proposal)),
            now,
            reduce,
        );
        let drawn: Vec<_> = self
            .rows
            .rows()
            .map(|row| (row.placement, row.value.clone()))
            .collect();
        let present = !self.rows.is_empty();
        // A proposal no row shows any more keeps neither its field nor its watch.
        let shown: HashSet<u64> = drawn.iter().map(|(_, proposal)| proposal.id).collect();
        self.fields.retain(|id, _| shown.contains(id));
        self._subscriptions.retain(|id, _| shown.contains(id));
        let mut list = ListCard::new("proposed-rules-list");
        for (placement, proposal) in &drawn {
            let row = self.row(proposal, cx);
            list = list.item(part_id(proposal.id, "item"), *placement, row);
        }
        let project = self.project.clone();
        presence(
            ElementId::Name(format!("proposed-rules-{project}").into()),
            present,
            Section::new(
                format!("proposed-rules-section-{project}"),
                "Proposed rules",
            )
            .sentence(SENTENCE)
            .child(list),
        )
        .space(space::XXL)
    }
}
