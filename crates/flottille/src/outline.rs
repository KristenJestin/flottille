//! The outline of a source file `fs_read` gives instead of a page (`docs/tools.md`, "Files"): its
//! declarations without their bodies, each with its line and its byte offset, read with
//! tree-sitter. No model is involved, and nothing here opens a file: the caller gives the bytes.

use std::ops::ControlFlow;
use std::time::{Duration, Instant};

use tree_sitter::{Node, ParseOptions, Parser};

/// How far down a declaration inside another is listed (modules, classes, namespaces). Source
/// nested deeper than this is no source a reader wants an outline of, and it bounds the walk.
const DEPTH: usize = 32;
/// How many characters of a declaration are shown: a signature longer than this is cut.
const SHOWN_CHARS: usize = 160;

/// How long the parse of a file may take: a hostile file can make it run for minutes and grow in
/// memory, on a thread nothing can stop.
pub const PARSE_TIME: Duration = Duration::from_secs(2);

/// Why there is no outline.
#[derive(Debug, PartialEq)]
pub enum Failure {
    /// The parse went past its time: refused, not failed (the file is fine, an outline of it is
    /// not worth more of the daemon).
    TooLong,
    Failed(String),
}

/// A language the outline knows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Language {
    Rust,
    TypeScript,
    Tsx,
    JavaScript,
    Python,
    Go,
}

impl Language {
    /// The language of a file by its extension, `None` for any other.
    pub fn of(extension: &str) -> Option<Language> {
        Some(match extension {
            "rs" => Language::Rust,
            "ts" | "mts" | "cts" => Language::TypeScript,
            "tsx" => Language::Tsx,
            "js" | "mjs" | "cjs" | "jsx" => Language::JavaScript,
            "py" | "pyi" => Language::Python,
            "go" => Language::Go,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Language::Rust => "Rust",
            Language::TypeScript => "TypeScript",
            Language::Tsx => "TSX",
            Language::JavaScript => "JavaScript",
            Language::Python => "Python",
            Language::Go => "Go",
        }
    }

    fn grammar(self) -> tree_sitter::Language {
        match self {
            Language::Rust => tree_sitter_rust::LANGUAGE.into(),
            Language::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Language::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Language::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Language::Python => tree_sitter_python::LANGUAGE.into(),
            Language::Go => tree_sitter_go::LANGUAGE.into(),
        }
    }
}

/// One declaration of the outline.
struct Entry {
    depth: usize,
    /// From 1.
    line: usize,
    /// Where the declaration starts, in bytes.
    offset: usize,
    text: String,
}

/// The outline of `source` as `fs_read` answers it: a head that says what it is and is not, then
/// a line per declaration, `line:offset`, then the declaration indented by its nesting.
/// `mask` hides the secrets of each declaration before it is cut: cut first, half of a secret
/// would show that no mask knows.
pub fn describe(
    shown: &str,
    language: Language,
    source: &[u8],
    mask: &dyn Fn(&str) -> String,
    within: Duration,
) -> Result<String, Failure> {
    let entries = declarations(language, source, mask, within)?;
    let mut text = format!(
        "outline of {shown} ({}, {} bytes; line:byte offset, then the declaration; read one with \
         fs_read range.offset). Not a read: fs_write and fs_edit still need a page of it read.\n",
        language.name(),
        source.len(),
    );
    if entries.is_empty() {
        text.push_str("(no declarations)\n");
    }
    for entry in &entries {
        text.push_str(&format!(
            "{}:{}  {}{}\n",
            entry.line,
            entry.offset,
            "  ".repeat(entry.depth),
            entry.text
        ));
    }
    Ok(text)
}

fn declarations(
    language: Language,
    source: &[u8],
    mask: &dyn Fn(&str) -> String,
    within: Duration,
) -> Result<Vec<Entry>, Failure> {
    let mut parser = Parser::new();
    parser.set_language(&language.grammar()).map_err(|error| {
        Failure::Failed(format!(
            "the {} grammar did not load: {error}",
            language.name()
        ))
    })?;
    let deadline = Instant::now() + within;
    let mut stop = |_: &tree_sitter::ParseState| {
        if Instant::now() >= deadline {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    };
    // A parse that is stopped answers `None`: the only other way to is a missing grammar.
    let tree = parser
        .parse_with_options(
            &mut |at, _| source.get(at..).unwrap_or_default(),
            None,
            Some(ParseOptions::new().progress_callback(&mut stop)),
        )
        .ok_or(Failure::TooLong)?;
    let mut walk = Walk {
        source,
        language,
        mask,
        entries: Vec::new(),
    };
    walk.members(tree.root_node(), 0);
    Ok(walk.entries)
}

struct Walk<'a> {
    source: &'a [u8],
    language: Language,
    mask: &'a dyn Fn(&str) -> String,
    entries: Vec<Entry>,
}

impl Walk<'_> {
    /// The declarations among the children of `parent`, at `depth`.
    fn members(&mut self, parent: Node, depth: usize) {
        if depth > DEPTH {
            return;
        }
        let mut cursor = parent.walk();
        for child in parent.named_children(&mut cursor) {
            match self.language {
                Language::Rust => self.rust(child, depth),
                Language::TypeScript | Language::Tsx | Language::JavaScript => {
                    self.script(child, child, depth)
                }
                Language::Python => self.python(child, depth),
                Language::Go => self.go(child, depth),
            }
        }
    }

    /// The source from `from` to `to`, on one line, without what ends a declaration.
    fn squashed(&self, from: usize, to: usize) -> String {
        let text = String::from_utf8_lossy(&self.source[from..to.max(from)]);
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let text = (self.mask)(text.trim_end_matches([' ', '=', ':', ';', ',']));
        if text.chars().count() > SHOWN_CHARS {
            let cut: String = text.chars().take(SHOWN_CHARS).collect();
            format!("{}…", cut.trim_end())
        } else {
            text
        }
    }

    /// Lists `text` as a declaration that starts where `at` does.
    fn push(&mut self, at: Node, depth: usize, text: String) {
        self.entries.push(Entry {
            depth,
            line: at.start_position().row + 1,
            offset: at.start_byte(),
            text,
        });
    }

    /// Lists `node` with its text from where `at` starts to where its body (or, for a constant,
    /// its value) starts: its signature.
    fn declare(&mut self, at: Node, node: Node, depth: usize) {
        let end = node
            .child_by_field_name("body")
            .or_else(|| node.child_by_field_name("value"))
            .map_or(node.end_byte(), |part| part.start_byte());
        let text = self.squashed(at.start_byte(), end);
        self.push(at, depth, text);
    }

    fn rust(&mut self, node: Node, depth: usize) {
        match node.kind() {
            "function_item"
            | "function_signature_item"
            | "struct_item"
            | "enum_item"
            | "union_item"
            | "const_item"
            | "static_item"
            | "type_item"
            | "associated_type" => self.declare(node, node, depth),
            "macro_definition" => {
                let name = node.child_by_field_name("name");
                let name = name.map_or(String::new(), |name| {
                    self.squashed(name.start_byte(), name.end_byte())
                });
                self.push(node, depth, format!("macro_rules! {name}"));
            }
            "trait_item" | "impl_item" | "mod_item" => {
                self.declare(node, node, depth);
                if let Some(body) = node.child_by_field_name("body") {
                    self.members(body, depth + 1);
                }
            }
            _ => {}
        }
    }

    /// TypeScript, TSX and JavaScript. `at` is where the declaration starts: an exported one
    /// starts at its `export`.
    fn script(&mut self, node: Node, at: Node, depth: usize) {
        match node.kind() {
            "export_statement" => {
                if let Some(declaration) = node.child_by_field_name("declaration") {
                    self.script(declaration, node, depth);
                }
            }
            "ambient_declaration" | "expression_statement" => {
                let mut cursor = node.walk();
                for child in node.named_children(&mut cursor) {
                    if matches!(
                        child.kind(),
                        "internal_module" | "module" | "function_signature" | "class_declaration"
                    ) {
                        self.script(child, at, depth);
                    }
                }
            }
            "function_declaration"
            | "generator_function_declaration"
            | "function_signature"
            | "type_alias_declaration"
            | "method_definition"
            | "method_signature"
            | "abstract_method_signature" => self.declare(at, node, depth),
            "class_declaration"
            | "abstract_class_declaration"
            | "interface_declaration"
            | "enum_declaration"
            | "internal_module"
            | "module" => {
                self.declare(at, node, depth);
                let Some(body) = node.child_by_field_name("body") else {
                    return;
                };
                let nested = matches!(
                    node.kind(),
                    "class_declaration"
                        | "abstract_class_declaration"
                        | "interface_declaration"
                        | "internal_module"
                        | "module"
                );
                if nested {
                    self.members(body, depth + 1);
                }
            }
            "lexical_declaration" | "variable_declaration" => {
                let constant = self.source[node.start_byte()..].starts_with(b"const");
                let mut cursor = node.walk();
                for (index, declarator) in node
                    .named_children(&mut cursor)
                    .filter(|declarator| declarator.kind() == "variable_declarator")
                    .enumerate()
                {
                    let value = declarator.child_by_field_name("value");
                    let function = value.filter(|value| {
                        matches!(
                            value.kind(),
                            "arrow_function"
                                | "function_expression"
                                | "function"
                                | "generator_function"
                        )
                    });
                    if !constant && function.is_none() {
                        continue;
                    }
                    // A later declarator is not the statement's: its text starts at itself.
                    let at = if index == 0 { at } else { declarator };
                    let end = function
                        .and_then(|function| function.child_by_field_name("body"))
                        .or(value)
                        .map_or(declarator.end_byte(), |part| part.start_byte());
                    let text = self.squashed(at.start_byte(), end);
                    self.push(at, depth, text);
                }
            }
            _ => {}
        }
    }

    fn python(&mut self, node: Node, depth: usize) {
        match node.kind() {
            "function_definition" => self.declare(node, node, depth),
            "class_definition" => {
                self.declare(node, node, depth);
                if let Some(body) = node.child_by_field_name("body") {
                    self.members(body, depth + 1);
                }
            }
            "decorated_definition" => {
                let Some(definition) = node.child_by_field_name("definition") else {
                    return;
                };
                let mut cursor = node.walk();
                let decorators: Vec<String> = node
                    .named_children(&mut cursor)
                    .filter(|child| child.kind() == "decorator")
                    .map(|decorator| self.squashed(decorator.start_byte(), decorator.end_byte()))
                    .collect();
                let before = self.entries.len();
                self.python(definition, depth);
                // The decorators are part of what a reader starts from, and of the signature.
                if let Some(entry) = self.entries.get_mut(before) {
                    entry.line = node.start_position().row + 1;
                    entry.offset = node.start_byte();
                    entry.text = format!("{} {}", decorators.join(" "), entry.text);
                }
            }
            "expression_statement" => {
                let mut cursor = node.walk();
                let Some(assignment) = node
                    .named_children(&mut cursor)
                    .find(|child| child.kind() == "assignment")
                else {
                    return;
                };
                let Some(name) = assignment.child_by_field_name("left") else {
                    return;
                };
                let named = &self.source[name.start_byte()..name.end_byte()];
                let upper = named
                    .iter()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || *b == b'_')
                    && named.iter().any(u8::is_ascii_uppercase);
                if name.kind() == "identifier" && upper {
                    let end = assignment
                        .child_by_field_name("right")
                        .map_or(assignment.end_byte(), |right| right.start_byte());
                    let text = self.squashed(node.start_byte(), end);
                    self.push(node, depth, text);
                }
            }
            _ => {}
        }
    }

    fn go(&mut self, node: Node, depth: usize) {
        match node.kind() {
            "package_clause" | "function_declaration" | "method_declaration" => {
                self.declare(node, node, depth)
            }
            "type_declaration" => {
                let mut cursor = node.walk();
                let items: Vec<Node> = node
                    .named_children(&mut cursor)
                    .filter(|item| matches!(item.kind(), "type_spec" | "type_alias"))
                    .collect();
                for item in &items {
                    let at = if items.len() == 1 { node } else { *item };
                    let kind = item.child_by_field_name("type").map(|kind| kind.kind());
                    let words = match kind {
                        Some("struct_type") => Some("struct"),
                        Some("interface_type") => Some("interface"),
                        _ => None,
                    };
                    let text = match (words, item.child_by_field_name("type")) {
                        (Some(words), Some(kind)) => {
                            let name = self.squashed(item.start_byte(), kind.start_byte());
                            format!("type {name} {words}")
                        }
                        _ => format!("type {}", self.squashed(item.start_byte(), item.end_byte())),
                    };
                    self.push(at, depth, text);
                    if let Some(kind) = item.child_by_field_name("type")
                        && kind.kind() == "interface_type"
                    {
                        let mut cursor = kind.walk();
                        for method in kind.named_children(&mut cursor) {
                            if matches!(method.kind(), "method_elem" | "method_spec") {
                                self.declare(method, method, depth + 1);
                            }
                        }
                    }
                }
            }
            "const_declaration" => {
                let mut cursor = node.walk();
                let items: Vec<Node> = node
                    .named_children(&mut cursor)
                    .filter(|item| item.kind() == "const_spec")
                    .collect();
                for item in &items {
                    let at = if items.len() == 1 { node } else { *item };
                    let end = item
                        .child_by_field_name("value")
                        .map_or(item.end_byte(), |value| value.start_byte());
                    let text = format!("const {}", self.squashed(item.start_byte(), end));
                    self.push(at, depth, text);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `describe` with no secret to mask and all the time it takes.
    fn described(shown: &str, language: Language, source: &[u8]) -> Result<String, Failure> {
        describe(
            shown,
            language,
            source,
            &|text| text.to_owned(),
            Duration::from_secs(60),
        )
    }

    /// The line and the byte offset where `needle` first appears in `source`.
    fn at(source: &str, needle: &str) -> String {
        let offset = source
            .find(needle)
            .unwrap_or_else(|| panic!("no {needle:?} in the fixture"));
        let line = source[..offset].matches('\n').count() + 1;
        format!("{line}:{offset}")
    }

    /// The declarations of `source` as lines, the head left out, and that no doc comment or body
    /// shows in it.
    fn listed(language: Language, source: &str, left_out: &[&str]) -> Vec<String> {
        let text = described("fixture", language, source.as_bytes()).unwrap();
        let (head, lines) = text.split_once('\n').unwrap();
        assert!(head.contains("Not a read"), "{head}");
        for word in left_out {
            assert!(!text.contains(word), "{word:?} in {text}");
        }
        lines.lines().map(str::to_owned).collect()
    }

    /// `expected` is each declaration's first word(s) as the fixture has them, found in `source`
    /// to give its line and offset, then what is listed of it.
    fn expect(source: &str, expected: &[(&str, usize, &str)]) -> Vec<String> {
        expected
            .iter()
            .map(|(needle, depth, text)| {
                format!("{}  {}{text}", at(source, needle), "  ".repeat(*depth))
            })
            .collect()
    }

    const RUST: &str = "//! Connections.

use std::io;

/// A connection.
pub struct Conn {
    host: String,
}

pub const LIMIT: usize = 8;

pub trait Dial {
    fn dial(&self, host: &str) -> Conn;
}

impl Conn {
    /// Opens it.
    pub fn open(host: &str) -> Conn {
        let secret_body = 1;
        Conn { host: host.to_owned() }
    }
}

mod inner {
    pub fn helper() {}
}

fn main() {}
";

    #[test]
    fn an_outline_lists_declarations_with_their_lines_and_offsets_rust() {
        let listed = listed(
            Language::Rust,
            RUST,
            &["A connection.", "Opens it.", "secret_body", "use std"],
        );

        assert_eq!(
            listed,
            expect(
                RUST,
                &[
                    ("pub struct Conn", 0, "pub struct Conn"),
                    ("pub const LIMIT", 0, "pub const LIMIT: usize"),
                    ("pub trait Dial", 0, "pub trait Dial"),
                    ("fn dial", 1, "fn dial(&self, host: &str) -> Conn"),
                    ("impl Conn", 0, "impl Conn"),
                    ("pub fn open", 1, "pub fn open(host: &str) -> Conn"),
                    ("mod inner", 0, "mod inner"),
                    ("pub fn helper", 1, "pub fn helper()"),
                    ("fn main", 0, "fn main()"),
                ]
            )
        );
    }

    const TYPESCRIPT: &str = "import { x } from \"./x\";

/** A connection. */
export interface Conn {
  host: string;
  close(): void;
}

export type Id = string;

export const LIMIT = 8;

export function open(host: string): Conn {
  const secretBody = 1;
  return { host, close() {} };
}

export class Pool {
  private size = 0;
  take(): Conn | undefined {
    return undefined;
  }
}

export const dial = (host: string): Conn => open(host);

namespace Inner {
  export function helper() {}
}
";

    #[test]
    fn an_outline_lists_declarations_with_their_lines_and_offsets_typescript() {
        let listed = listed(
            Language::TypeScript,
            TYPESCRIPT,
            &["A connection.", "secretBody", "import"],
        );

        assert_eq!(
            listed,
            expect(
                TYPESCRIPT,
                &[
                    ("export interface Conn", 0, "export interface Conn"),
                    ("close()", 1, "close(): void"),
                    ("export type Id", 0, "export type Id"),
                    ("export const LIMIT", 0, "export const LIMIT"),
                    (
                        "export function open",
                        0,
                        "export function open(host: string): Conn"
                    ),
                    ("export class Pool", 0, "export class Pool"),
                    ("take()", 1, "take(): Conn | undefined"),
                    (
                        "export const dial",
                        0,
                        "export const dial = (host: string): Conn =>"
                    ),
                    ("namespace Inner", 0, "namespace Inner"),
                    ("export function helper", 1, "export function helper()"),
                ]
            )
        );
    }

    const TSX: &str = "import React from \"react\";

export function Button(props: Props) {
  return <button>{props.label}</button>;
}

export default class Panel extends React.Component {
  render() {
    return <div />;
  }
}
";

    #[test]
    fn an_outline_lists_declarations_with_their_lines_and_offsets_tsx() {
        let listed = listed(Language::Tsx, TSX, &["<button>"]);

        assert_eq!(
            listed,
            expect(
                TSX,
                &[
                    (
                        "export function Button",
                        0,
                        "export function Button(props: Props)"
                    ),
                    (
                        "export default class",
                        0,
                        "export default class Panel extends React.Component"
                    ),
                    ("render()", 1, "render()"),
                ]
            )
        );
    }

    const JAVASCRIPT: &str = "const LIMIT = 8;
let counter = 0;

export function open(host) {
  return { host };
}

class Pool {
  take() {
    return null;
  }
}

const dial = (host) => open(host);
";

    #[test]
    fn an_outline_lists_declarations_with_their_lines_and_offsets_javascript() {
        let listed = listed(Language::JavaScript, JAVASCRIPT, &["counter", "return"]);

        assert_eq!(
            listed,
            expect(
                JAVASCRIPT,
                &[
                    ("const LIMIT", 0, "const LIMIT"),
                    ("export function open", 0, "export function open(host)"),
                    ("class Pool", 0, "class Pool"),
                    ("take()", 1, "take()"),
                    ("const dial", 0, "const dial = (host) =>"),
                ]
            )
        );
    }

    const PYTHON: &str = "\"\"\"Connections.\"\"\"

import socket

LIMIT = 8


class Conn:
    \"\"\"A connection.\"\"\"

    def __init__(self, host):
        self.host = host

    @property
    def name(self):
        return self.host


def open_conn(host: str) -> Conn:
    secret_body = 1
    return Conn(host)


async def dial(host):
    pass
";

    #[test]
    fn an_outline_lists_declarations_with_their_lines_and_offsets_python() {
        let listed = listed(
            Language::Python,
            PYTHON,
            &["A connection.", "secret_body", "import socket", "self.host"],
        );

        assert_eq!(
            listed,
            expect(
                PYTHON,
                &[
                    ("LIMIT", 0, "LIMIT"),
                    ("class Conn", 0, "class Conn"),
                    ("def __init__", 1, "def __init__(self, host)"),
                    ("@property", 1, "@property def name(self)"),
                    ("def open_conn", 0, "def open_conn(host: str) -> Conn"),
                    ("async def dial", 0, "async def dial(host)"),
                ]
            )
        );
    }

    const GO: &str = "package conn

import \"net\"

// Limit is the most.
const Limit = 8

type Conn struct {
	Host string
}

type Dialer interface {
	Dial(host string) Conn
}

func Open(host string) Conn {
	secretBody := 1
	return Conn{Host: host}
}

func (c *Conn) Close() error {
	return nil
}
";

    #[test]
    fn an_outline_lists_declarations_with_their_lines_and_offsets_go() {
        let listed = listed(
            Language::Go,
            GO,
            &["Limit is the most.", "secretBody", "import"],
        );

        assert_eq!(
            listed,
            expect(
                GO,
                &[
                    ("package conn", 0, "package conn"),
                    ("const Limit", 0, "const Limit"),
                    ("type Conn struct", 0, "type Conn struct"),
                    ("type Dialer interface", 0, "type Dialer interface"),
                    ("Dial(host", 1, "Dial(host string) Conn"),
                    ("func Open", 0, "func Open(host string) Conn"),
                    ("func (c *Conn) Close", 0, "func (c *Conn) Close() error"),
                ]
            )
        );
    }

    #[test]
    fn an_offset_of_the_outline_starts_its_declaration() {
        let text = described("fixture", Language::Rust, RUST.as_bytes()).unwrap();
        let offsets: Vec<usize> = text
            .lines()
            .skip(1)
            .map(|line| line.split(':').nth(1).unwrap().split(' ').next().unwrap())
            .map(|offset| offset.parse().unwrap())
            .collect();

        assert_eq!(offsets.len(), 9);
        for offset in offsets {
            assert!(RUST[offset..].starts_with(|c: char| c.is_alphabetic()));
            assert!(RUST[..offset].ends_with(['\n', ' ']));
        }
    }

    #[test]
    fn a_later_declarator_starts_at_itself_and_shows_nothing_of_an_earlier_one() {
        let source = "let token = \"first-value\", f = () => {};\nconst A = 1, B = 2;\n";

        let listed = listed(Language::JavaScript, source, &["first-value", "let token"]);

        assert_eq!(
            listed,
            expect(
                source,
                &[
                    ("f =", 0, "f = () =>"),
                    ("const A", 0, "const A"),
                    ("B =", 0, "B")
                ]
            )
        );
    }

    #[test]
    fn a_signature_is_masked_before_it_is_cut_so_no_part_of_a_secret_shows() {
        let secret = "hunter2hunter2hunter2";
        // The secret starts a few characters before the 160th of the signature.
        let source = format!(
            "fn f({}, {secret}: u32) {{}}\nfn g() {{}}\n",
            "a".repeat(SHOWN_CHARS - 10)
        );
        let mask = |text: &str| text.replace(secret, "[masked]");

        let text = describe("s.rs", Language::Rust, source.as_bytes(), &mask, PARSE_TIME).unwrap();

        assert!(!text.contains("hun"), "{text}");
        assert!(text.contains("fn g()"), "{text}");
    }

    #[test]
    fn a_parse_past_its_time_is_stopped_and_not_an_outline() {
        // Big enough for the parser to look at the clock (it does so every few thousand steps).
        let source = "a=[".repeat(100_000);

        let said = describe(
            "slow.js",
            Language::JavaScript,
            source.as_bytes(),
            &|text| text.to_owned(),
            Duration::ZERO,
        );

        assert_eq!(said, Err(Failure::TooLong));
    }

    #[test]
    fn a_file_with_no_declaration_says_so() {
        let text = described("empty.py", Language::Python, b"print(1)\n").unwrap();

        assert!(text.contains("(no declarations)"), "{text}");
    }

    #[test]
    fn a_signature_longer_than_a_line_is_cut() {
        let source = format!("fn long({}) {{}}\n", "argument: u32, ".repeat(40));

        let text = described("long.rs", Language::Rust, source.as_bytes()).unwrap();

        let line = text.lines().nth(1).unwrap();
        assert!(line.ends_with('…'), "{line}");
        assert!(line.chars().count() < SHOWN_CHARS + 20, "{line}");
    }

    #[test]
    fn a_source_with_a_syntax_error_still_lists_what_is_there() {
        let text = described(
            "broken.rs",
            Language::Rust,
            b"fn good() {}\nfn broken( {\nfn also_good() {}\n",
        )
        .unwrap();

        assert!(text.contains("fn good()"), "{text}");
    }

    #[test]
    fn the_languages_are_known_by_their_extensions() {
        for (extension, language) in [
            ("rs", Language::Rust),
            ("ts", Language::TypeScript),
            ("mts", Language::TypeScript),
            ("tsx", Language::Tsx),
            ("js", Language::JavaScript),
            ("jsx", Language::JavaScript),
            ("mjs", Language::JavaScript),
            ("py", Language::Python),
            ("go", Language::Go),
        ] {
            assert_eq!(Language::of(extension), Some(language), "{extension}");
        }
        for extension in ["md", "toml", "c", "java", "RS", ""] {
            assert_eq!(Language::of(extension), None, "{extension:?}");
        }
    }
}
