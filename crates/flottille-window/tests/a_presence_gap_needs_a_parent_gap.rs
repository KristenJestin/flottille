//! `presence(..).gap(g)` is not a margin: `g` is the gap the parent flex puts between its
//! children, and the block only folds it away with itself. A parent without a `.gap(..)` draws
//! nothing there, and the space the code asked for is never seen (#849, #859).
//!
//! The guard reads the window's sources as syntax trees. A `presence(..)` / `presence_of(..)`
//! given a `.gap(..)`, and a `problem_line(.., gap, ..)`, must be handed to a `.child(..)` /
//! `.children(..)` whose chain (`div().flex().flex_col().gap(..)`) holds a `.gap(..)` of the same
//! value, or starts from a kit component that lays its children out with that gap (`Section`,
//! `Modal`, `Confirm`). A block bound by a `let`, or pushed into a vector, is followed to the
//! `.child(..)` that takes it. What a scan cannot follow (a block a helper returns, a tuple) is
//! on `UNSEEN`, each with who checks it; a space a block asks of itself is given as a `.pt(..)`
//! inside it, never as a gap. The screens whose space was lost are measured, by the tests that
//! name the block, on GPUI's test platform (`crates/flottille/tests`).

use std::fs;
use std::path::{Path, PathBuf};

use syn::visit::Visit;
use syn::{Expr, ExprMethodCall};

/// The components of the kit that lay their children out with a gap of their own, and which.
const KIT_GAPS: &[(&str, &str)] = &[
    ("Section::new", "space::MD"),
    ("Modal::new", "space::LG"),
    ("Confirm::new", "space::LG"),
];

/// The blocks whose parent the scan cannot see: the file, the function or the id, and why it is
/// right. Each is read by hand; one that matches nothing any more is refused, so the list cannot
/// outlive its sites.
const UNSEEN: &[(&str, &str, &str)] = &[
    (
        "claude.rs",
        "found_block",
        "its callers put it in a `Section`, which gaps `space::MD`",
    ),
    (
        "extensions/mod.rs",
        "projects_line",
        "`feature_page` puts it in a column that gaps `space::MD`",
    ),
    (
        "commands.rs",
        "problem_line",
        "its callers give the gap of their own column, and are sites of the scan",
    ),
    (
        "tasks.rs",
        "ready_and_url",
        "its blocks are children of the two columns the function builds, each with the gap",
    ),
    (
        "programs.rs",
        "line_of",
        "`program_block` puts its lines in a column that gaps `space::XS`",
    ),
    (
        "rules.rs",
        "global-block",
        "returned in a tuple, then a child of the rules' `Section` (`space::MD`)",
    ),
];

/// How the parent of a block was found.
#[derive(Clone, Debug, PartialEq)]
enum Kind {
    /// The chain holds a gap, or starts from a kit component that has one.
    Gap,
    /// A chain that starts with a call (`div()`) and holds none.
    NoGap,
    /// A chain that starts with a variable nothing is known of, or no `.child(..)` at all.
    Unseen,
}

#[derive(Clone, Debug)]
struct Parent {
    kind: Kind,
    /// What the chain starts from (`div`, `Section::new`).
    start: String,
    /// Its gaps, as written, comma separated.
    gap: String,
}

impl Parent {
    fn unseen() -> Self {
        Parent {
            kind: Kind::Unseen,
            start: "-".into(),
            gap: String::new(),
        }
    }
}

#[derive(Debug)]
struct Site {
    func: String,
    /// The first string of the block's arguments: its id.
    id: String,
    /// The gap the block asks for, as written.
    own: String,
    parent: Parent,
}

#[derive(Default)]
struct Finder {
    /// The `.child(..)` / `.children(..)` calls whose arguments are being visited.
    parents: Vec<Parent>,
    sites: Vec<Site>,
    /// The blocks bound by a `let` (or pushed to a vector) of the function being visited, until a
    /// `.child(..)` takes them: the name, and the index of its site.
    pending: Vec<(String, usize)>,
    /// What each `let` of the function being visited starts from (`let modal = Modal::new(..)`).
    started: Vec<(String, String)>,
    /// The vectors a `NAME.push(..)` fills while its argument is visited.
    filling: Vec<String>,
    func: String,
}

#[derive(Default)]
struct FirstString(Option<String>);

impl<'ast> Visit<'ast> for FirstString {
    /// An id built by `format!(..)`: its text, as the macro was written.
    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        self.0.get_or_insert_with(|| mac.tokens.to_string());
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

/// A path or a call as written; anything else is `?`.
fn describe(expr: &Expr) -> String {
    match expr {
        Expr::Path(path) => path
            .path
            .segments
            .iter()
            .map(|part| part.ident.to_string())
            .collect::<Vec<_>>()
            .join("::"),
        Expr::Call(call) => describe(&call.func),
        Expr::Lit(_) => "lit".into(),
        _ => "?".into(),
    }
}

/// `px(0.)`: a gap that asks for no space is no gap to lose.
fn asks_nothing(expr: &Expr) -> bool {
    let Expr::Call(call) = expr else { return false };
    describe(&call.func) == "px"
        && matches!(call.args.first(), Some(Expr::Lit(lit))
            if matches!(&lit.lit, syn::Lit::Float(n) if n.base10_parse::<f32>().is_ok_and(|n| n == 0.0)))
}

fn is_gap(call: &ExprMethodCall) -> bool {
    call.method.to_string().starts_with("gap")
}

fn presence_call(expr: &Expr) -> Option<&syn::ExprCall> {
    let Expr::Call(call) = expr else { return None };
    let Expr::Path(path) = &*call.func else {
        return None;
    };
    let last = path.path.segments.last()?;
    (last.ident == "presence" || last.ident == "presence_of").then_some(call)
}

impl Finder {
    /// What a chain starts from: `Modal::new` for `modal.child(..)` after `let modal =
    /// Modal::new(..)`, or the variable itself when nothing is known of it.
    fn start_of(&self, expr: &Expr) -> String {
        let mut root = expr;
        loop {
            match root {
                Expr::MethodCall(call) => root = &call.receiver,
                Expr::Paren(inner) => root = &inner.expr,
                _ => break,
            }
        }
        match root {
            Expr::Call(call) => describe(&call.func),
            Expr::Path(path) => match path.path.get_ident().map(ToString::to_string) {
                Some(name) => self
                    .started
                    .iter()
                    .rev()
                    .find(|(held, _)| *held == name)
                    .map_or(name, |(_, start)| start.clone()),
                None => describe(root),
            },
            _ => "?".into(),
        }
    }

    fn record(&mut self, id: String, own: String) {
        let at = self.sites.len();
        if self.parents.is_empty()
            && let Some(vector) = self.filling.last()
        {
            self.pending.push((vector.clone(), at));
        }
        self.sites.push(Site {
            func: self.func.clone(),
            id,
            own,
            parent: self.parents.last().cloned().unwrap_or_else(Parent::unseen),
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
        if self.sites.len() > before && self.sites[before].parent.kind == Kind::Unseen {
            self.pending.push((name.clone(), before));
        }
        if let Some(init) = &local.init {
            let start = self.start_of(&init.expr);
            // `let section = match .. { .. }` says nothing new about a name already known.
            if start != "?" || !self.started.iter().any(|(held, _)| *held == name) {
                self.started.push((name, start));
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

    /// `problem_line(id, line, gap, cx)` is a `presence_of(..).gap(gap)`.
    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if describe(&call.func) == "problem_line"
            && call.args.len() == 4
            && !asks_nothing(&call.args[2])
        {
            self.record(
                first_string(call.args.iter().take(1)),
                describe(&call.args[2]),
            );
        }
        syn::visit::visit_expr_call(self, call);
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
        let start = self.start_of(&Expr::MethodCall(outer.clone()));
        let gaps: Vec<String> = calls
            .iter()
            .filter(|call| is_gap(call))
            .map(|call| describe(&call.args[0]))
            .collect();
        let kit = KIT_GAPS.iter().find(|(name, _)| *name == start);
        let parent = match (gaps.is_empty(), kit, root) {
            (_, Some((_, gap)), _) => Parent {
                kind: Kind::Gap,
                start: start.clone(),
                gap: (*gap).to_owned(),
            },
            (false, ..) => Parent {
                kind: Kind::Gap,
                start: start.clone(),
                gap: gaps.join(","),
            },
            (true, None, Expr::Call(_)) => Parent {
                kind: Kind::NoGap,
                start: start.clone(),
                gap: String::new(),
            },
            (true, None, Expr::Path(path))
                if self
                    .started
                    .iter()
                    .any(|(held, _)| path.path.is_ident(held)) =>
            {
                Parent {
                    kind: Kind::NoGap,
                    start: start.clone(),
                    gap: String::new(),
                }
            }
            _ => Parent::unseen(),
        };
        if let Some(presence) = presence_call(root)
            && let Some(gap) = calls.iter().find(|call| call.method == "gap")
            && !asks_nothing(&gap.args[0])
        {
            self.record(first_string(&presence.args), describe(&gap.args[0]));
        }
        self.visit_expr(root);
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
                self.parents.push(Parent {
                    kind: Kind::NoGap,
                    start: "kept".into(),
                    gap: String::new(),
                });
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

/// Why a site is refused, when it is, leaving out the blocks `UNSEEN` answers for.
fn refusal(site: &Site) -> Option<String> {
    match site.parent.kind {
        Kind::NoGap => Some(format!(
            "its parent (`{}`) has no `.gap({})`: the space is never drawn",
            site.parent.start, site.own
        )),
        Kind::Unseen => Some(
            "its parent cannot be seen: build it in a `.child(..)` of a column \
                              with a gap, or list it in `UNSEEN` with why it is right"
                .to_owned(),
        ),
        Kind::Gap => {
            let parent = &site.parent.gap;
            let same =
                site.own == "?" || parent.split(',').any(|gap| gap == site.own || gap == "?");
            (!same).then(|| {
                format!(
                    "it asks for `{}` and its parent gaps `{parent}`: the fold is not the space",
                    site.own
                )
            })
        }
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
            let Some(why) = refusal(&site) else { continue };
            let allowed = UNSEEN.iter().position(|(file, held, _)| {
                *file == name && (*held == site.func || *held == site.id)
            });
            match allowed {
                Some(at) if site.parent.kind == Kind::Unseen => used[at] = true,
                _ => offenders.push(format!("{name}, fn {}, `{}`: {why}", site.func, site.id)),
            }
        }
    }
    assert!(seen > 60, "the blocks with a gap were found: {seen}");
    let stale: Vec<_> = UNSEEN
        .iter()
        .zip(&used)
        .filter(|(_, used)| !**used)
        .map(|((file, held, _), _)| format!("{file}: {held}"))
        .collect();
    assert!(
        offenders.is_empty(),
        "a gap given to a `presence` is the gap its parent puts between its children, and it \
         folds with the block (docs/reference/gpui.md, \"Rules of the window\"):\n{}",
        offenders.join("\n")
    );
    assert!(
        stale.is_empty(),
        "`UNSEEN` lists blocks that are no longer there or no longer unseen:\n{}",
        stale.join("\n")
    );
}

fn refused(source: &str) -> Vec<String> {
    sites_of(source).iter().filter_map(refusal).collect()
}

#[test]
fn a_presence_gap_in_a_column_without_gap_is_refused() {
    let found = refused(
        "fn f() { div().flex().flex_col().child(a).child(presence(\"x\", true, b).gap(space::MD)); }",
    );
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("has no `.gap(space::MD)`"), "{found:?}");
    let of = refused("fn f() { div().child(presence_of(\"x\", None, |l| l).gap(space::SM)); }");
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
fn a_gap_that_is_not_the_parents_is_refused() {
    let found =
        refused("fn f() { div().gap(space::LG).child(presence(\"x\", true, b).gap(space::MD)); }");
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("`space::MD`") && found[0].contains("`space::LG`"));
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
}

#[test]
fn a_block_bound_by_a_let_is_followed_to_the_column_that_takes_it() {
    let bare =
        "fn f() { let block = presence(\"x\", true, b).gap(space::MD); div().child(block); }";
    assert_eq!(refused(bare).len(), 1);
    let gapped = "fn f() { let block = presence(\"x\", true, b).gap(space::MD); div().gap(space::MD).child(block); }";
    assert!(refused(gapped).is_empty());
}

#[test]
fn a_block_pushed_to_a_vector_is_followed_to_the_column_that_takes_it() {
    let bare = "fn f() { let mut all = Vec::new(); all.push(presence(\"x\", true, b).gap(space::MD)); div().children(all); }";
    assert_eq!(refused(bare).len(), 1);
    let gapped = "fn f() { let mut all = Vec::new(); all.push(presence(\"x\", true, b).gap(space::MD)); div().gap(space::MD).children(all); }";
    assert!(refused(gapped).is_empty());
}

#[test]
fn a_problem_line_gap_is_a_presence_gap() {
    let bare = "fn f() { div().child(problem_line(\"e\", None, space::LG, cx)); }";
    assert_eq!(refused(bare).len(), 1);
    let gapped = "fn f() { div().gap(space::LG).child(problem_line(\"e\", None, space::LG, cx)); }";
    assert!(refused(gapped).is_empty());
}

#[test]
fn a_presence_that_asks_for_no_gap_is_no_site() {
    assert!(sites_of("fn f() { div().child(presence(\"x\", true, b)); }").is_empty());
}

#[test]
fn what_a_modal_keeps_in_sight_has_no_gap_and_asks_for_none() {
    let asked =
        "fn f() { Modal::new(\"m\", \"M\").kept(problem_line(\"e\", None, space::LG, cx)); }";
    assert_eq!(refused(asked).len(), 1, "{:?}", refused(asked));
    let none = "fn f() { Modal::new(\"m\", \"M\").kept(problem_line(\"e\", None, px(0.), cx)); }";
    assert!(refused(none).is_empty(), "{:?}", refused(none));
}
