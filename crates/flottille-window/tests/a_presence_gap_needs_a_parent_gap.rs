//! `presence(..).gap(g)` is not a margin: `g` is the gap the parent flex puts between its
//! children, and the block only folds it away with itself. A parent without a `.gap(..)` draws
//! nothing there, and the space the code asked for is never seen (#849, #859). The other way
//! round, a block without the gap of a column that has one makes the column's gap appear at once
//! when the block comes and leave with a jump when it goes: the gap does not fold with it.
//!
//! The guard reads the window's sources as syntax trees. Every `presence(..)`, `presence_of(..)`
//! and `problem_line(.., gap, ..)` is a block, and is checked against the `.child(..)` /
//! `.children(..)` that takes it:
//! - its parent must be a column (`flex_col()`, `v_flex()`, or a kit component that is one);
//! - a block without a gap may not be in a column that has one;
//! - a block with a gap must have the gap of its column, the same literal;
//! - a gap the scan cannot read (anything but a name, a call of names and numbers) is refused;
//! - a block wrapped in a call (`enter(..)`, `awaiting(..)`, any function) is a child of the
//!   wrapper, not of the column around it, and is refused where it has a gap.
//!
//! A kit `Section` gaps `MD`, a `Modal`/`Confirm` body `LG`; what a modal keeps in sight is a
//! column without a gap. A block bound by a `let` or pushed into a vector is followed to the
//! `.child(..)` that takes it. What a scan cannot follow (a block a helper returns, a tuple) is on
//! `UNSEEN`, one entry per block, with who checks it. The screens whose space was lost are
//! measured by the tests that name the block, on GPUI's test platform (`crates/flottille/tests`).

use std::fs;
use std::path::{Path, PathBuf};

use syn::visit::Visit;
use syn::{Expr, ExprMethodCall};

/// The components of the kit that are a column with a gap of their own (a `Modal` with `.search()`
/// is one without: see `chain_of`).
const KIT_COLUMNS: &[(&str, &str)] = &[
    ("Section::new", "space::MD"),
    ("Modal::new", "space::LG"),
    ("Confirm::new", "space::LG"),
];

/// The blocks whose parent the scan cannot see: the file, the block's id as written, and why it
/// is right. Each is read by hand. One that matches nothing any more is refused, so the list
/// cannot outlive its blocks; a new block next to one of them is checked.
const UNSEEN: &[(&str, &str, &str)] = &[
    (
        "claude.rs",
        "{prefix}-found-block",
        "its callers put it in a `Section`, which gaps `space::MD`",
    ),
    (
        "extensions/mod.rs",
        "{id}-block",
        "`feature_page` puts it in a column that gaps `space::MD`",
    ),
    (
        "commands.rs",
        "{id}-block",
        "its callers give `problem_line` the gap of their own column, and are blocks of the scan",
    ),
    (
        "tasks.rs",
        "{id}-block",
        "a child of the two columns `ready_and_url` builds, each gapping `space::MD`",
    ),
    (
        "programs.rs",
        "{id}-block",
        "`program_block` puts its lines in a column that gaps `space::XS`",
    ),
    (
        "rules.rs",
        "global-block",
        "returned in a tuple, then a child of the rules' `Section` (`space::MD`)",
    ),
];

/// What a chain of calls is: where it starts, whether it is a column, and its vertical gap (the
/// last one set wins).
#[derive(Clone, Debug)]
struct Chain {
    start: String,
    known: bool,
    column: bool,
    gap: Option<String>,
}

/// Where a block sits.
#[derive(Clone, Debug)]
enum Parent {
    /// A `.child(..)` of a chain.
    Chain(Chain),
    /// What a modal keeps in sight: a column without a gap.
    Kept,
    /// An argument of a call (`enter(..)`): a child of what the call draws, which has no gap.
    Wrapper(String),
    /// Nothing known: no `.child(..)` around it, or a chain that starts from a variable.
    Unseen,
}

#[derive(Debug)]
struct Site {
    func: String,
    /// The first string of the block's arguments: its id.
    id: String,
    /// The gap the block asks for, as written; none for a block without one or asking for none.
    own: Option<String>,
    parent: Parent,
}

#[derive(Default)]
struct Finder {
    /// The `.child(..)` / `.children(..)` calls and the calls whose arguments are being visited.
    parents: Vec<Parent>,
    sites: Vec<Site>,
    /// The blocks bound by a `let` (or pushed to a vector) of the function being visited, until a
    /// `.child(..)` takes them: the name, and the index of its site.
    pending: Vec<(String, usize)>,
    /// What each `let` of the function being visited is (`let modal = Modal::new(..)`).
    started: Vec<(String, Chain)>,
    /// The vectors a `NAME.push(..)` fills while its argument is visited.
    filling: Vec<String>,
    /// The gap a chain gave the call it starts from, for that call to take.
    root_gap: Option<String>,
    func: String,
}

#[derive(Default)]
struct FirstString(Option<String>);

impl<'ast> Visit<'ast> for FirstString {
    /// An id built by `format!(..)`: its first string, as written.
    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        let text = mac.tokens.to_string();
        let first = text.split('"').nth(1).map(str::to_owned);
        self.0.get_or_insert_with(|| first.unwrap_or(text));
    }

    fn visit_lit_str(&mut self, lit: &'ast syn::LitStr) {
        self.0.get_or_insert_with(|| lit.value());
    }
}

fn first_string<'a>(args: impl IntoIterator<Item = &'a Expr>) -> String {
    let mut found = FirstString::default();
    for arg in args {
        found.visit_expr(arg);
    }
    found.0.unwrap_or_default()
}

/// A gap as written, by what it is: a path, a call of such with its numbers (`px(8)` is not
/// `px(12)`), a number. Anything else is `?`, which no gap may be.
fn describe(expr: &Expr) -> String {
    match expr {
        Expr::Path(path) => path
            .path
            .segments
            .iter()
            .map(|part| part.ident.to_string())
            .collect::<Vec<_>>()
            .join("::"),
        Expr::Call(call) => {
            let args: Vec<String> = call.args.iter().map(describe).collect();
            format!("{}({})", describe(&call.func), args.join(","))
        }
        Expr::Lit(lit) => match &lit.lit {
            syn::Lit::Float(n) => n
                .base10_parse::<f64>()
                .map_or("?".into(), |n| format!("{n}")),
            syn::Lit::Int(n) => n
                .base10_parse::<f64>()
                .map_or("?".into(), |n| format!("{n}")),
            _ => "?".into(),
        },
        Expr::Paren(inner) => describe(&inner.expr),
        _ => "?".into(),
    }
}

/// `px(0.)`: a gap that asks for no space is no gap to lose.
fn asks_nothing(gap: &str) -> bool {
    gap == "px(0)"
}

/// What a call `f(..)` is named, by its last segment.
fn callee(call: &syn::ExprCall) -> String {
    let Expr::Path(path) = &*call.func else {
        return String::new();
    };
    path.path
        .segments
        .last()
        .map(|part| part.ident.to_string())
        .unwrap_or_default()
}

fn is_presence(call: &syn::ExprCall) -> bool {
    matches!(callee(call).as_str(), "presence" | "presence_of")
}

impl Finder {
    /// What a chain is, following a variable to the `let` that made it.
    fn chain_of(&self, expr: &Expr) -> Chain {
        let mut calls = Vec::new();
        let mut root = expr;
        loop {
            match root {
                Expr::MethodCall(call) => {
                    calls.push(call);
                    root = &call.receiver;
                }
                Expr::Paren(inner) => root = &inner.expr,
                _ => break,
            }
        }
        let unknown = |start: String| Chain {
            start,
            known: false,
            column: false,
            gap: None,
        };
        let mut chain = match root {
            Expr::Call(call) => {
                let start = describe(&call.func);
                let kit = KIT_COLUMNS.iter().find(|(name, _)| *name == start);
                Chain {
                    column: start == "v_flex" || kit.is_some(),
                    gap: kit.map(|(_, gap)| (*gap).to_owned()),
                    start,
                    known: true,
                }
            }
            Expr::Path(path) => self
                .started
                .iter()
                .rev()
                .find(|(held, _)| path.path.is_ident(held))
                .map_or_else(|| unknown(describe(root)), |(_, chain)| chain.clone()),
            _ => unknown("?".into()),
        };
        for call in calls.into_iter().rev() {
            match call.method.to_string().as_str() {
                "flex_col" => chain.column = true,
                "gap" | "gap_y" => chain.gap = call.args.first().map(describe),
                // A search modal's results are a column without a gap, each child in a wrapper
                // of its own (the kit's `modal.rs`); `large` and `wide` keep the body's `LG`.
                "search" if chain.start == "Modal::new" => chain.gap = None,
                _ => {}
            }
        }
        chain
    }

    fn record(&mut self, id: String, own: Option<String>) {
        let at = self.sites.len();
        if self.parents.is_empty()
            && let Some(vector) = self.filling.last()
        {
            self.pending.push((vector.clone(), at));
        }
        self.sites.push(Site {
            func: self.func.clone(),
            id,
            own: own.filter(|gap| !asks_nothing(gap)),
            parent: self.parents.last().cloned().unwrap_or(Parent::Unseen),
        });
    }
}

impl<'ast> Visit<'ast> for Finder {
    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        let outer = (
            std::mem::take(&mut self.pending),
            std::mem::take(&mut self.started),
        );
        let func = std::mem::replace(&mut self.func, item.sig.ident.to_string());
        syn::visit::visit_item_fn(self, item);
        self.func = func;
        (self.pending, self.started) = outer;
    }

    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        let outer = (
            std::mem::take(&mut self.pending),
            std::mem::take(&mut self.started),
        );
        let func = std::mem::replace(&mut self.func, item.sig.ident.to_string());
        syn::visit::visit_impl_item_fn(self, item);
        self.func = func;
        (self.pending, self.started) = outer;
    }

    fn visit_local(&mut self, local: &'ast syn::Local) {
        let before = self.sites.len();
        syn::visit::visit_local(self, local);
        let syn::Pat::Ident(name) = &local.pat else {
            return;
        };
        let name = name.ident.to_string();
        if self.sites.len() > before && matches!(self.sites[before].parent, Parent::Unseen) {
            self.pending.push((name.clone(), before));
        }
        if let Some(init) = &local.init {
            let chain = self.chain_of(&init.expr);
            // `let section = match .. { .. }` says nothing new about a name already known.
            if chain.known || !self.started.iter().any(|(held, _)| *held == name) {
                self.started.push((name, chain));
            }
        }
    }

    /// A block bound by a `let` (or pushed to a vector) and given to a `.child(..)` later.
    fn visit_expr_path(&mut self, path: &'ast syn::ExprPath) {
        let (Some(name), Some(parent)) = (path.path.get_ident(), self.parents.last()) else {
            return;
        };
        let mut taken = Vec::new();
        self.pending.retain(|(held, site)| {
            let hit = name == held;
            if hit {
                taken.push(*site);
            }
            !hit
        });
        for site in taken {
            self.sites[site].parent = parent.clone();
        }
    }

    /// A block is a `presence(..)` / `presence_of(..)` (the gap its chain gave it, if any) or a
    /// `problem_line(id, line, gap, cx)`. Any call that takes a block as an argument is the
    /// block's parent, which has no gap of its own to fold.
    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        let own = self.root_gap.take();
        let name = callee(call);
        if is_presence(call) {
            self.record(first_string(&call.args), own);
        } else if name == "problem_line" && call.args.len() == 4 {
            self.record(
                first_string(call.args.iter().take(1)),
                Some(describe(&call.args[2])),
            );
        }
        // The callee is not inside the call's arguments: `block(..)` calling a closure is not a
        // wrapper of the block the closure builds.
        self.visit_expr(&call.func);
        self.parents.push(Parent::Wrapper(name));
        for arg in &call.args {
            self.visit_expr(arg);
        }
        self.parents.pop();
    }

    /// A whole chain at once (`div().flex_col().child(a).gap(g).child(b)`): a gap anywhere in
    /// it is the parent's gap for every `.child(..)` of the chain.
    fn visit_expr_method_call(&mut self, outer: &'ast ExprMethodCall) {
        let mut calls = vec![outer];
        let mut root = &*outer.receiver;
        loop {
            match root {
                Expr::MethodCall(call) => {
                    calls.push(call);
                    root = &call.receiver;
                }
                Expr::Paren(inner) => root = &inner.expr,
                _ => break,
            }
        }
        let chain = self.chain_of(&Expr::MethodCall(outer.clone()));
        let parent = if chain.known {
            Parent::Chain(chain)
        } else {
            Parent::Unseen
        };
        match root {
            Expr::Call(call) => {
                self.root_gap = calls
                    .iter()
                    .find(|call| call.method == "gap")
                    .map(|gap| describe(&gap.args[0]));
                self.visit_expr_call(call);
                self.root_gap = None;
            }
            other => self.visit_expr(other),
        }
        let filled = match (calls.as_slice(), root) {
            ([call], Expr::Path(path)) if call.method == "push" => {
                path.path.get_ident().map(ToString::to_string)
            }
            _ => None,
        };
        let fills = filled.is_some();
        self.filling.extend(filled);
        for call in calls.into_iter().rev() {
            let holds = call.method == "child" || call.method == "children";
            // What a modal keeps in sight under its body is a column without a gap.
            let kept = call.method == "kept";
            if holds {
                self.parents.push(parent.clone());
            }
            if kept {
                self.parents.push(Parent::Kept);
            }
            for arg in &call.args {
                self.visit_expr(arg);
            }
            if holds || kept {
                self.parents.pop();
            }
        }
        if fills {
            self.filling.pop();
        }
    }
}

fn sites_of(source: &str) -> Vec<Site> {
    let file = syn::parse_file(source).expect("a source parses");
    let mut finder = Finder::default();
    finder.visit_file(&file);
    finder.sites
}

/// What is wrong with a block, if anything; `Unseen` when all that is wrong is that the scan
/// cannot see its parent.
#[derive(Debug, PartialEq)]
enum Refusal {
    Wrong(String),
    Unseen(String),
}

fn refusal(site: &Site) -> Option<Refusal> {
    let own = site.own.as_deref();
    let wrong = |why: String| Some(Refusal::Wrong(why));
    if own.is_some_and(|gap| gap.contains('?')) {
        return wrong(
            "its gap is not a name or a number the scan can read: give the column's constant"
                .into(),
        );
    }
    match &site.parent {
        Parent::Unseen => own.map(|gap| {
            Refusal::Unseen(format!(
                "its parent cannot be seen, and it asks for `{gap}`: build it in a `.child(..)` \
                 of a column with that gap, or list it in `UNSEEN` with why it is right"
            ))
        }),
        Parent::Wrapper(name) => own.map(|gap| {
            Refusal::Wrong(format!(
                "it is an argument of `{name}(..)`, a child of what that draws, which has no \
                 gap to fold: it asks for `{gap}`, so the block goes in the column itself"
            ))
        }),
        Parent::Kept => own.map(|gap| {
            Refusal::Wrong(format!(
                "what a modal keeps in sight has no gap, and it asks for `{gap}`"
            ))
        }),
        Parent::Chain(chain) => match (chain.column, chain.gap.as_deref(), own) {
            (false, _, Some(gap)) => wrong(format!(
                "its parent (`{}`) is not a column (`flex_col()`), and it asks for `{gap}`",
                chain.start
            )),
            (true, None, Some(gap)) => wrong(format!(
                "its parent (`{}`) has no `.gap({gap})`: the space is never drawn",
                chain.start
            )),
            (true, Some(held), None) if held.contains('?') => wrong(format!(
                "its column's gap is not a name or a number the scan can read: `{}`",
                chain.start
            )),
            (true, Some(held), None) if !asks_nothing(held) => wrong(format!(
                "it has no `.gap(..)` and its column (`{}`) gaps `{held}`: the gap appears at \
                 once when it comes and the column jumps when it goes",
                chain.start
            )),
            (true, Some(held), Some(gap)) if gap != held => wrong(format!(
                "it asks for `{gap}` and its column (`{}`) gaps `{held}`: the fold is not the \
                 space",
                chain.start
            )),
            _ => None,
        },
    }
}

fn sources(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, found);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
}

/// The entry of `UNSEEN` that answers for a block of `file`, if one does: by the block's id.
fn answered(file: &str, site: &Site) -> Option<usize> {
    UNSEEN
        .iter()
        .position(|(held, id, _)| *held == file && *id == site.id)
}

#[test]
fn a_presence_gap_needs_a_parent_gap() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&root, &mut files);
    assert!(files.len() > 20, "the sources were found: {}", files.len());
    let mut seen = 0;
    let mut offenders = Vec::new();
    let mut used = vec![false; UNSEEN.len()];
    for file in files {
        let name = file
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        for site in sites_of(&fs::read_to_string(&file).unwrap()) {
            seen += 1;
            match refusal(&site) {
                None => {}
                Some(Refusal::Unseen(_)) if answered(&name, &site).is_some() => {
                    used[answered(&name, &site).unwrap()] = true;
                }
                Some(Refusal::Wrong(why) | Refusal::Unseen(why)) => {
                    offenders.push(format!("{name}, fn {}, `{}`: {why}", site.func, site.id));
                }
            }
        }
    }
    assert!(seen > 150, "the blocks were found: {seen}");
    let stale: Vec<_> = UNSEEN
        .iter()
        .zip(&used)
        .filter(|(_, used)| !**used)
        .map(|((file, id, _), _)| format!("{file}: {id}"))
        .collect();
    assert!(
        offenders.is_empty(),
        "a gap given to a `presence` is the gap its column puts between its children, and it \
         folds with the block, so a block has the gap of its column and a column's blocks all \
         have it (docs/reference/gpui.md, \"Rules of the window\"):\n{}",
        offenders.join("\n")
    );
    assert!(
        stale.is_empty(),
        "`UNSEEN` lists blocks that are no longer there or no longer unseen:\n{}",
        stale.join("\n")
    );
}

fn refused(source: &str) -> Vec<String> {
    sites_of(source)
        .iter()
        .filter_map(refusal)
        .map(|found| match found {
            Refusal::Wrong(why) | Refusal::Unseen(why) => why,
        })
        .collect()
}

#[test]
fn a_presence_gap_in_a_column_without_gap_is_refused() {
    let found = refused(
        "fn f() { div().flex_col().child(a).child(presence(\"x\", true, b).gap(space::MD)); }",
    );
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("has no `.gap(space::MD)`"), "{found:?}");
    let of = refused("fn f() { v_flex().child(presence_of(\"x\", None, |l| l).gap(space::SM)); }");
    assert_eq!(of.len(), 1, "{of:?}");
}

#[test]
fn a_presence_gap_in_a_column_with_that_gap_is_kept() {
    let source = "fn f() { div().flex_col().gap(space::MD).child(a).child(presence(\"x\", true, b).gap(space::MD)); }";
    assert!(refused(source).is_empty());
    // A gap set after the children is still the column's.
    let after = "fn f() { div().flex_col().child(presence(\"x\", true, b).gap(space::MD)).gap(space::MD); }";
    assert!(refused(after).is_empty());
}

#[test]
fn a_block_without_a_gap_in_a_column_that_has_one_is_refused() {
    let found = refused(
        "fn f() { div().flex_col().gap(space::MD).child(a).child(presence(\"x\", true, b)); }",
    );
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("appears at once"), "{found:?}");
    // In a column without one, or beside others in a row, it is as it should be.
    let bare = "fn f() { div().flex_col().child(presence(\"x\", true, b)); }";
    assert!(refused(bare).is_empty());
    let row = "fn f() { div().flex().gap(space::SM).child(presence(\"x\", true, b)); }";
    assert!(refused(row).is_empty(), "{:?}", refused(row));
    // A line that asks for no space is a block without a gap.
    let none =
        "fn f() { div().flex_col().gap(space::MD).child(problem_line(\"e\", None, px(0.), cx)); }";
    assert_eq!(refused(none).len(), 1);
}

#[test]
fn a_gap_is_compared_by_its_number_and_must_be_readable() {
    let other =
        "fn f() { div().flex_col().gap(px(12.)).child(presence(\"x\", true, b).gap(px(8.))); }";
    let found = refused(other);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("`px(8)`") && found[0].contains("`px(12)`"));
    let same =
        "fn f() { div().flex_col().gap(px(12.)).child(presence(\"x\", true, b).gap(px(12.0))); }";
    assert!(refused(same).is_empty());
    let unreadable = "fn f() { div().flex_col().gap(space::MD * t).child(presence(\"x\", true, b).gap(space::MD * t)); }";
    assert!(!refused(unreadable).is_empty());
    let unnamed = "fn f() { div().flex_col().gap(space::MD).child(presence(\"x\", true, b).gap(gap_of(a, b).0)); }";
    assert!(!refused(unnamed).is_empty());
}

#[test]
fn a_row_gap_is_not_a_column_gap() {
    let row =
        "fn f() { div().flex().gap(space::MD).child(presence(\"x\", true, b).gap(space::MD)); }";
    let found = refused(row);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("not a column"), "{found:?}");
    let column = "fn f() { div().flex().flex_col().gap(space::MD).child(presence(\"x\", true, b).gap(space::MD)); }";
    assert!(refused(column).is_empty());
}

#[test]
fn a_block_wrapped_in_enter_or_awaiting_is_a_child_of_the_wrapper() {
    for wrapper in ["enter(\"id\", 0, ", "awaiting(\"id\", false, skeleton, "] {
        let source = format!(
            "fn f() {{ div().flex_col().gap(space::MD).child({wrapper}presence(\"x\", true, b).gap(space::MD))); }}"
        );
        let found = refused(&source);
        assert_eq!(found.len(), 1, "{wrapper}: {found:?}");
        assert!(found[0].contains("argument of"), "{found:?}");
    }
    // The wrapper's own column, built inside it, is the parent again.
    let inside = "fn f() { div().child(enter(\"id\", 0, div().flex_col().gap(space::MD).child(presence(\"x\", true, b).gap(space::MD)))); }";
    assert!(refused(inside).is_empty(), "{:?}", refused(inside));
}

#[test]
fn a_section_and_a_modal_carry_the_gap_of_their_own() {
    let section =
        "fn f() { Section::new(\"s\", \"S\").child(presence(\"x\", true, b).gap(space::MD)); }";
    assert!(refused(section).is_empty());
    let wrong =
        "fn f() { Section::new(\"s\", \"S\").child(presence(\"x\", true, b).gap(space::LG)); }";
    assert_eq!(refused(wrong).len(), 1);
    let modal = "fn f() { let modal = Modal::new(\"m\", \"M\"); modal.child(presence(\"x\", true, b).gap(space::LG)); }";
    assert!(refused(modal).is_empty(), "{:?}", refused(modal));
    // A block of a section without the gap is refused as well.
    let bare = "fn f() { Section::new(\"s\", \"S\").child(presence(\"x\", true, b)); }";
    assert_eq!(refused(bare).len(), 1);
}

#[test]
fn a_search_modal_s_results_have_no_gap_but_a_large_one_keeps_it() {
    let search = "fn f() { Modal::new(\"m\", \"M\").search().child(presence(\"x\", true, b).gap(space::LG)); }";
    let found = refused(search);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("has no `.gap(space::LG)`"), "{found:?}");
    let held = "fn f() { let modal = Modal::new(\"m\", \"M\").search(); modal.child(presence(\"x\", true, b).gap(space::LG)); }";
    assert_eq!(refused(held).len(), 1);
    let bare = "fn f() { Modal::new(\"m\", \"M\").search().child(presence(\"x\", true, b)); }";
    assert!(refused(bare).is_empty(), "{:?}", refused(bare));
    for variant in ["large", "wide"] {
        let kept = format!(
            "fn f() {{ Modal::new(\"m\", \"M\").{variant}().child(presence(\"x\", true, b).gap(space::LG)); }}"
        );
        assert!(refused(&kept).is_empty(), "{variant}: {:?}", refused(&kept));
    }
}

#[test]
fn a_block_bound_by_a_let_is_followed_to_the_column_that_takes_it() {
    let bare = "fn f() { let block = presence(\"x\", true, b).gap(space::MD); div().flex_col().child(block); }";
    assert_eq!(refused(bare).len(), 1);
    let gapped = "fn f() { let block = presence(\"x\", true, b).gap(space::MD); div().flex_col().gap(space::MD).child(block); }";
    assert!(refused(gapped).is_empty());
    // A column bound by a `let` is followed too.
    let column = "fn f() { let col = div().flex_col().gap(space::MD); col.child(presence(\"x\", true, b).gap(space::MD)); }";
    assert!(refused(column).is_empty(), "{:?}", refused(column));
}

#[test]
fn a_block_pushed_to_a_vector_is_followed_to_the_column_that_takes_it() {
    let bare = "fn f() { let mut all = Vec::new(); all.push(presence(\"x\", true, b).gap(space::MD)); div().flex_col().children(all); }";
    assert_eq!(refused(bare).len(), 1);
    let gapped = "fn f() { let mut all = Vec::new(); all.push(presence(\"x\", true, b).gap(space::MD)); div().flex_col().gap(space::MD).children(all); }";
    assert!(refused(gapped).is_empty());
}

#[test]
fn a_problem_line_gap_is_a_presence_gap() {
    let bare = "fn f() { div().flex_col().child(problem_line(\"e\", None, space::LG, cx)); }";
    assert_eq!(refused(bare).len(), 1);
    let gapped = "fn f() { div().flex_col().gap(space::LG).child(problem_line(\"e\", None, space::LG, cx)); }";
    assert!(refused(gapped).is_empty());
}

#[test]
fn what_a_modal_keeps_in_sight_has_no_gap_and_asks_for_none() {
    let asked =
        "fn f() { Modal::new(\"m\", \"M\").kept(problem_line(\"e\", None, space::LG, cx)); }";
    assert_eq!(refused(asked).len(), 1, "{:?}", refused(asked));
    let none = "fn f() { Modal::new(\"m\", \"M\").kept(problem_line(\"e\", None, px(0.), cx)); }";
    assert!(refused(none).is_empty(), "{:?}", refused(none));
}

#[test]
fn a_block_that_cannot_be_followed_is_answered_for_by_its_id_alone() {
    let blocks = sites_of(
        "fn ready_and_url() { let block = |id: String| presence(format!(\"{id}-block\"), true, b).gap(space::MD); \
         let other = |id: String| presence(format!(\"{id}-other\"), true, b).gap(space::MD); }",
    );
    assert_eq!(blocks.len(), 2);
    // Both are in a function that `UNSEEN` knows: only the one whose id it names is answered for.
    let answered_ids: Vec<_> = blocks
        .iter()
        .map(|site| answered("tasks.rs", site).is_some())
        .collect();
    assert_eq!(answered_ids, [true, false]);
    assert!(answered("settings.rs", &blocks[0]).is_none());
    assert!(matches!(refusal(&blocks[1]), Some(Refusal::Unseen(_))));
}

#[test]
fn a_presence_that_asks_for_no_gap_beside_no_gap_is_fine() {
    assert!(refused("fn f() { div().child(presence(\"x\", true, b)); }").is_empty());
}
