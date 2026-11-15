//! A block that comes and goes (`presence(..)`, `presence_of(..)`) holds the space above it itself,
//! with `.space(..)`: it folds with the block whatever its parent is. `.gap(..)` is the gap of the
//! parent column, which only a parent that has it draws: the kit keeps it working, and the window
//! no longer uses it (#884; the class of #849 and #859).
//!
//! The scan reads the window's sources as syntax trees: a `.gap(..)` called on a chain that starts
//! from `presence(..)` or `presence_of(..)` is refused, unless the block is on `KEPT`.

use std::fs;
use std::path::{Path, PathBuf};

use syn::visit::Visit;
use syn::{Expr, ExprMethodCall};

/// The blocks that keep the gap of their parent, by id, and why: their column also holds rows
/// that are `Item`s of a `Listing`, whose own `.gap(..)` is the column's too, so the column has to
/// keep its gap.
const KEPT: &[&str] = &["slash-builtin-label", "slash-project-label", "slash-note"];

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

#[derive(Default)]
struct Finder {
    found: Vec<String>,
}

impl<'ast> Visit<'ast> for Finder {
    fn visit_expr_method_call(&mut self, call: &'ast ExprMethodCall) {
        if call.method == "gap"
            && let Some(id) = block_of(&call.receiver)
            && !KEPT.contains(&id.as_str())
        {
            self.found.push(format!("block `{id}`"));
        }
        syn::visit::visit_expr_method_call(self, call);
    }
}

fn gaps_of_blocks(source: &str) -> Vec<String> {
    let file = syn::parse_file(source).expect("a source parses");
    let mut finder = Finder::default();
    finder.visit_file(&file);
    finder.found
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
fn no_block_borrows_the_gap_of_its_parent() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&root, &mut files);
    assert!(files.len() > 20, "the sources were found: {}", files.len());
    let mut offenders = Vec::new();
    for file in files {
        let name = file
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        for place in gaps_of_blocks(&fs::read_to_string(&file).unwrap()) {
            offenders.push(format!("{name}, {place}"));
        }
    }
    assert!(
        offenders.is_empty(),
        "a block holds its space with `.space(..)`, in a parent column without a gap \
         (docs/reference/gpui.md, \"Rules of the window\"):\n{}",
        offenders.join("\n")
    );
}

#[test]
fn the_scan_refuses_a_block_with_a_gap() {
    assert_eq!(
        gaps_of_blocks(
            "fn f() { v_flex().child(presence(\"a\", true, div()).gap(space::MD)); \
             v_flex().child(presence_of(\"b\", None, |x| x).gap(g)); }"
        ),
        ["block `a`", "block `b`"]
    );
}

#[test]
fn the_scan_lets_a_space_a_column_gap_and_a_kept_block_through() {
    assert!(
        gaps_of_blocks(
            "fn f() { v_flex().gap(space::MD).child(presence(\"a\", true, div()).space(space::MD)); \
             presence(\"slash-note\", true, div()).gap(space::XS); }"
        )
        .is_empty()
    );
}
