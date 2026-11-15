//! A block that comes and goes (`presence(..)`, `presence_of(..)`) holds the space above it itself,
//! with `.space(..)`: it folds with the block whatever its parent is. `.gap(..)` is the gap of the
//! parent column, which only a parent that has it draws: the kit keeps it working, and the window
//! no longer uses it (#884; the class of #849 and #859).
//!
//! The scan reads the window's sources as syntax trees and refuses two things: a `.gap(..)` called
//! on a chain that starts from `presence(..)` or `presence_of(..)`, unless the block is on `KEPT`
//! (and a `KEPT` entry that matches nothing any more is refused too); and a block with
//! `.space(..)` that is a `.child(..)` of a column whose chain still has `.gap(..)`, which would
//! draw the space twice.

use std::fs;
use std::path::{Path, PathBuf};

use syn::visit::Visit;
use syn::{Expr, ExprMethodCall};

/// The blocks that keep the gap of their parent, by file and id, and why. The slash list's: the
/// column also holds rows that are `Item`s of a `Listing`, whose own `.gap(..)` is the column's
/// too, so the column has to keep its gap.
const KEPT: &[(&str, &str)] = &[
    // The column of blocks of a `Section`, whose own gap (the kit's) is its parent's: the id is the
    // helper's argument, not a string, so the scan names it `""`.
    ("commands.rs", ""),
    ("session.rs", "slash-builtin-label"),
    ("session.rs", "slash-project-label"),
    ("session.rs", "slash-note"),
];

#[derive(Default)]
struct FirstString(Option<String>);

impl<'ast> Visit<'ast> for FirstString {
    fn visit_lit_str(&mut self, lit: &'ast syn::LitStr) {
        self.0.get_or_insert_with(|| lit.value());
    }
}

/// The id of the block a chain of method calls starts from (`presence(..)` or `presence_of(..)`):
/// its first string, as written; `None` when the chain starts from something else.
fn block_of(mut expr: &Expr) -> Option<String> {
    loop {
        match expr {
            Expr::MethodCall(call) => expr = &call.receiver,
            Expr::Paren(inner) => expr = &inner.expr,
            Expr::Call(call) => {
                let Expr::Path(path) = &*call.func else {
                    return None;
                };
                let name = &path.path.segments.last()?.ident;
                if name != "presence" && name != "presence_of" {
                    return None;
                }
                let mut first = FirstString::default();
                for arg in &call.args {
                    first.visit_expr(arg);
                }
                return Some(first.0.unwrap_or_default());
            }
            _ => return None,
        }
    }
}

/// Whether a chain of method calls calls `method` somewhere.
fn chain_has(mut expr: &Expr, method: &str) -> bool {
    loop {
        match expr {
            Expr::MethodCall(call) if call.method == method => return true,
            Expr::MethodCall(call) => expr = &call.receiver,
            Expr::Paren(inner) => expr = &inner.expr,
            _ => return false,
        }
    }
}

/// The id of the block `expr` is, when it holds its space with `.space(..)`.
fn spaced_block(expr: &Expr) -> Option<String> {
    block_of(expr).filter(|_| chain_has(expr, "space"))
}

/// What a source holds that the guard has to know about.
#[derive(Default, Debug, PartialEq)]
struct Scan {
    /// The blocks that borrow the gap of their parent: `presence(..).gap(..)`.
    gaps: Vec<String>,
    /// The blocks with `.space(..)` in a column that still has `.gap(..)`: the space drawn twice.
    doubled: Vec<String>,
}

impl<'ast> Visit<'ast> for Scan {
    fn visit_expr_method_call(&mut self, call: &'ast ExprMethodCall) {
        if call.method == "gap" {
            if let Some(id) = block_of(&call.receiver) {
                self.gaps.push(id);
            }
            // The column's children that came before its gap.
            let mut chain = &*call.receiver;
            while let Expr::MethodCall(inner) = chain {
                if inner.method == "child" {
                    self.doubled
                        .extend(inner.args.iter().filter_map(spaced_block));
                }
                chain = &inner.receiver;
            }
        }
        // A child of a column whose gap came before it.
        if call.method == "child" && chain_has(&call.receiver, "gap") {
            self.doubled
                .extend(call.args.iter().filter_map(spaced_block));
        }
        syn::visit::visit_expr_method_call(self, call);
    }
}

fn scan(source: &str) -> Scan {
    let file = syn::parse_file(source).expect("a source parses");
    let mut found = Scan::default();
    found.visit_file(&file);
    found.doubled.sort();
    found.doubled.dedup();
    found
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

/// What the guard refuses in the sources `files` (name, text), with `kept` the blocks allowed to
/// keep their gap.
fn refused(files: &[(String, String)], kept: &[(&str, &str)]) -> Vec<String> {
    let mut offenders = Vec::new();
    let mut used = Vec::new();
    for (name, source) in files {
        let found = scan(source);
        for id in found.gaps {
            match kept
                .iter()
                .find(|(file, block)| file == name && *block == id)
            {
                Some(entry) => used.push(*entry),
                None => offenders.push(format!("{name}, block `{id}` has `.gap(..)`")),
            }
        }
        for id in found.doubled {
            offenders.push(format!(
                "{name}, block `{id}` has `.space(..)` in a column that has `.gap(..)`"
            ));
        }
    }
    for (file, id) in kept {
        if !used.contains(&(*file, *id)) {
            offenders.push(format!(
                "{file}, `{id}` is on KEPT and no longer needs to be"
            ));
        }
    }
    offenders
}

#[test]
fn no_block_borrows_the_gap_of_its_parent() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut paths = Vec::new();
    sources(&root, &mut paths);
    assert!(paths.len() > 20, "the sources were found: {}", paths.len());
    let files: Vec<(String, String)> = paths
        .iter()
        .map(|path| {
            let name = path
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            (name, fs::read_to_string(path).unwrap())
        })
        .collect();
    let offenders = refused(&files, KEPT);
    assert!(
        offenders.is_empty(),
        "a block holds its space with `.space(..)`, in a parent column without a gap \
         (docs/reference/gpui.md, \"Rules of the window\"):\n{}",
        offenders.join("\n")
    );
}

fn file(source: &str) -> Vec<(String, String)> {
    vec![("a.rs".to_owned(), source.to_owned())]
}

#[test]
fn the_scan_refuses_a_block_with_a_gap() {
    assert_eq!(
        scan(
            "fn f() { v_flex().child(presence(\"a\", true, div()).gap(space::MD)); \
             v_flex().child(presence_of(\"b\", None, |x| x).gap(g)); }"
        )
        .gaps,
        ["a", "b"]
    );
}

#[test]
fn the_scan_lets_a_space_in_a_column_without_gap_and_a_kept_block_through() {
    let spaced = "fn f() { v_flex().child(presence(\"a\", true, div()).space(space::MD)); }";
    assert_eq!(scan(spaced), Scan::default());
    // A block that keeps its gap is the column's: a column with a gap holds it, on `KEPT`.
    let kept = "fn f() { v_flex().gap(g).child(presence(\"b\", true, div()).gap(space::XS)); }";
    assert_eq!(scan(kept).gaps, ["b"]);
    assert!(refused(&file(kept), &[("a.rs", "b")]).is_empty());
}

#[test]
fn the_scan_refuses_a_stale_entry_of_kept() {
    let source = "fn f() { v_flex().child(presence(\"a\", true, div()).space(space::MD)); }";
    // `a` has no gap any more, and another file's `a` is not this one's.
    assert_eq!(
        refused(&file(source), &[("a.rs", "a")]),
        ["a.rs, `a` is on KEPT and no longer needs to be"]
    );
    let gapped = "fn f() { v_flex().child(presence(\"a\", true, div()).gap(g)); }";
    assert_eq!(
        refused(&file(gapped), &[("b.rs", "a")]),
        [
            "a.rs, block `a` has `.gap(..)`",
            "b.rs, `a` is on KEPT and no longer needs to be"
        ]
    );
}

#[test]
fn the_scan_refuses_a_space_in_a_column_that_has_a_gap() {
    // The gap before the child, and the gap after it.
    let before = "fn f() { v_flex().gap(g).child(presence(\"a\", true, div()).space(s)); }";
    let after = "fn f() { v_flex().child(presence(\"a\", true, div()).space(s)).gap(g); }";
    let own = "fn f() { v_flex().gap(g).child(presence_of(\"b\", x, |y| y).space(s).into_any_element()); }";
    for source in [before, after] {
        assert_eq!(scan(source).doubled, ["a"], "{source}");
    }
    assert_eq!(scan(own).doubled, ["b"]);
    assert_eq!(
        refused(&file(before), &[]),
        ["a.rs, block `a` has `.space(..)` in a column that has `.gap(..)`"]
    );
}
