//! `ModelChoice`, the one way to choose a model (`docs/reference/gpui.md`, rule 15), on GPUI's
//! test platform: its default said with what it gives, no "none" entry, the effort beside a model
//! that has efforts only.

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use common::{Desktop, advance, init, pending_frames};
use flottille_ui::motion::{ENTER, PANEL};
use flottille_ui::{Chosen, ModelChoice, ModelEntry};
use gpui_kit::test::{TestQueryExt as _, TestWindowExt};
use gpui_kit::{
    Context, Entity, IntoElement, ParentElement, Render, SharedString, Styled, TestAppContext,
    VisualTestContext, Window, div, px, size,
};

/// What a choice reported: what was chosen, and its effort.
type Seen = Rc<RefCell<Vec<(Chosen, Option<SharedString>)>>>;

/// A window holding one choice, which keeps what was chosen.
struct Host {
    chosen: Chosen,
    effort: Option<SharedString>,
    picked: bool,
    seen: Seen,
}

fn models() -> Vec<ModelEntry> {
    vec![
        ModelEntry::new("claude", "Claude", Some("opus".into()), "Opus").efforts(["low", "high"]),
        ModelEntry::new("claude", "Claude", Some("haiku".into()), "Haiku"),
        ModelEntry::new("codex", "Codex", Some("gpt-5".into()), "GPT-5").efforts(["medium"]),
    ]
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (this, seen) = (cx.entity().downgrade(), self.seen.clone());
        let choice = ModelChoice::new("judge", "Model", self.chosen.clone())
            .default("Strongest of the provider", "Opus for Claude")
            .models(models())
            .effort(self.effort.clone())
            .picked(self.picked)
            .cost("Each decision is a turn of this model, on your own quota.")
            .on_choose(move |chosen, effort, _, cx| {
                seen.borrow_mut().push((chosen.clone(), effort.clone()));
                this.update(cx, |this, cx| {
                    this.chosen = chosen;
                    this.effort = effort;
                    cx.notify();
                })
                .ok();
            });
        div().size_full().p(px(16.)).child(choice)
    }
}

fn host(cx: &mut TestAppContext, chosen: Chosen) -> (Entity<Host>, &mut VisualTestContext, Seen) {
    init(cx, Desktop::new(false).env());
    let seen = Rc::new(RefCell::new(Vec::new()));
    let given = seen.clone();
    let (view, cx) = cx.add_window_view(move |_, _| Host {
        chosen,
        effort: None,
        picked: false,
        seen: given,
    });
    cx.simulate_resize(size(px(900.), px(700.)));
    cx.update(|window, cx| window.render_frame(cx));
    (view, cx, seen)
}

fn shown(id: &str, cx: &mut VisualTestContext) -> bool {
    let id = id.to_owned();
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.try_find(id).is_some_and(|found| found.visible())
    })
}

fn label(id: &str, cx: &mut VisualTestContext) -> Option<String> {
    let id = id.to_owned();
    cx.update(|window, _| {
        window
            .try_find(id)
            .and_then(|found| found.label().map(str::to_owned))
    })
}

fn open(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.click("judge-model", cx));
    advance(cx, ENTER * 2);
}

#[gpui_kit::test]
fn a_model_choice_says_its_default_and_what_it_gives(cx: &mut TestAppContext) {
    let (_, cx, seen) = host(cx, Chosen::First);
    assert_eq!(
        label("judge-model", cx).as_deref(),
        Some("Strongest of the provider"),
        "the default is chosen and said as its rule"
    );
    assert_eq!(
        label("judge-cost", cx).as_deref(),
        Some("Each decision is a turn of this model, on your own quota.")
    );
    open(cx);
    assert_eq!(
        label("judge-model-first-line", cx).as_deref(),
        Some("Opus for Claude"),
        "its second line says what it gives today"
    );
    // The models, `<provider> · <model>`, grouped by provider.
    assert_eq!(
        label("judge-model-claude-opus", cx).as_deref(),
        Some("Claude · Opus")
    );
    assert_eq!(label("judge-model-group-1", cx).as_deref(), Some("Claude"));
    assert_eq!(label("judge-model-group-2", cx).as_deref(), Some("Codex"));
    assert!(label("judge-model-group-3", cx).is_none());
    cx.update(|window, cx| window.click("judge-model-codex-gpt-5", cx));
    assert_eq!(
        seen.borrow().last(),
        Some(&(Chosen::Model("codex".into(), Some("gpt-5".into())), None))
    );
    advance(cx, PANEL * 2);
    assert_eq!(label("judge-model", cx).as_deref(), Some("Codex · GPT-5"));
    assert_eq!(pending_frames(cx), 0);
}

#[gpui_kit::test]
fn a_model_choice_has_no_none_entry(cx: &mut TestAppContext) {
    let (_, cx, _) = host(cx, Chosen::Model("claude".into(), Some("haiku".into())));
    open(cx);
    let options: Vec<String> = cx.update(|window, _| {
        ["first", "claude-opus", "claude-haiku", "codex-gpt-5"]
            .iter()
            .map(|key| {
                window
                    .find(format!("judge-model-{key}"))
                    .label()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    });
    for option in &options {
        assert!(
            !option.to_lowercase().contains("none") && !option.starts_with("No "),
            "{option}"
        );
    }
    cx.update(|window, _| {
        assert!(window.try_find("judge-model-none").is_none());
        assert!(window.try_find_by_label("None").is_none());
    });
}

#[gpui_kit::test]
fn the_effort_unfolds_only_for_a_model_with_efforts(cx: &mut TestAppContext) {
    let (view, cx, seen) = host(cx, Chosen::Model("claude".into(), Some("haiku".into())));
    assert!(!shown("judge-effort", cx), "Haiku has no effort");
    open(cx);
    cx.update(|window, cx| window.click("judge-model-claude-opus", cx));
    // It unfolds beside the model: narrower than its full width midway, whole at the end.
    advance(cx, PANEL / 3);
    let midway = cx.update(|window, _| window.find("judge-effort-block").bounds().size.width);
    advance(cx, PANEL);
    let whole = cx.update(|window, _| window.find("judge-effort-block").bounds().size.width);
    assert!(midway < whole, "{midway:?} < {whole:?}");
    assert!(shown("judge-effort", cx));
    assert_eq!(label("judge-effort", cx).as_deref(), Some("Default"));
    cx.update(|window, cx| window.click("judge-effort", cx));
    advance(cx, ENTER * 2);
    cx.update(|window, cx| window.click("judge-effort-high", cx));
    assert_eq!(
        seen.borrow().last(),
        Some(&(
            Chosen::Model("claude".into(), Some("opus".into())),
            Some("high".into())
        ))
    );
    advance(cx, PANEL * 2);
    // A model without efforts: it folds away.
    view.update(cx, |host, cx| {
        host.chosen = Chosen::Model("claude".into(), Some("haiku".into()));
        host.effort = None;
        cx.notify();
    });
    advance(cx, PANEL / 3);
    assert!(shown("judge-effort-block", cx), "it folds, not vanishes");
    advance(cx, PANEL);
    assert!(!shown("judge-effort-block", cx));
    assert_eq!(pending_frames(cx), 0);
}

#[gpui_kit::test]
fn the_picked_mark_fades_away_once_seen(cx: &mut TestAppContext) {
    let (view, cx, _) = host(cx, Chosen::Model("claude".into(), Some("haiku".into())));
    view.update(cx, |host, cx| {
        host.picked = true;
        cx.notify();
    });
    advance(cx, PANEL * 2);
    assert_eq!(label("judge-picked-mark", cx).as_deref(), Some("picked"));
    view.update(cx, |host, cx| {
        host.picked = false;
        cx.notify();
    });
    advance(cx, PANEL / 3);
    assert!(shown("judge-picked", cx), "it fades, not vanishes");
    advance(cx, PANEL);
    assert!(!shown("judge-picked", cx));
    assert_eq!(pending_frames(cx), 0);
}
