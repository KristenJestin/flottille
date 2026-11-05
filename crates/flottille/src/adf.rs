//! Atlassian Document Format (ADF), Jira Cloud's text: the Markdown an agent writes is turned into
//! it on every Jira write, and the tree Jira keeps is turned back into Markdown when it is read
//! (`docs/tickets.md`, "Jira's text"). The two directions meet for the kinds converted, so what an
//! agent reads back is what it wrote, and the text Flottille recorded for a post can be found
//! again (`posts.rs`).

use std::collections::HashMap;

use markdown::mdast::Node;
use markdown::{Constructs, ParseOptions};
use serde_json::{Value, json};

/// The ADF document of a Markdown text. What has no ADF node (raw HTML, an image, a table Jira
/// would not take) stays in the document as the text it was written as, never dropped.
pub(crate) fn from_markdown(text: &str) -> Value {
    let options = ParseOptions {
        constructs: Constructs {
            gfm_table: true,
            gfm_strikethrough: true,
            gfm_autolink_literal: true,
            ..Constructs::default()
        },
        ..ParseOptions::default()
    };
    let mut content = match markdown::to_mdast(text, &options) {
        Ok(root) => {
            let mut definitions = HashMap::new();
            definitions_of(&root, &mut definitions);
            let from = Converter { text, definitions };
            let mut content = Vec::new();
            from.blocks(
                root.children().map_or(&[][..], Vec::as_slice),
                false,
                &mut content,
            );
            content
        }
        // Parsing plain Markdown does not fail; were it to, the text still goes through.
        Err(_) => vec![as_text(text)],
    };
    if content.is_empty() {
        content.push(json!({"type": "paragraph"}));
    }
    json!({"type": "doc", "version": 1, "content": content})
}

/// The link definitions (`[id]: url`) of a document, by their identifier.
fn definitions_of(node: &Node, found: &mut HashMap<String, (String, Option<String>)>) {
    if let Node::Definition(definition) = node {
        found
            .entry(definition.identifier.clone())
            .or_insert((definition.url.clone(), definition.title.clone()));
    }
    for child in node.children().into_iter().flatten() {
        definitions_of(child, found);
    }
}

/// A paragraph holding `text` as it is, its lines apart by line breaks.
fn as_text(text: &str) -> Value {
    let mut content = Vec::new();
    push_text(&mut content, text, &[]);
    match content.is_empty() {
        true => json!({"type": "paragraph"}),
        false => json!({"type": "paragraph", "content": content}),
    }
}

/// Adds `text` to inline `content` under `marks`: its lines are joined by line breaks, and a text
/// next to one of the same marks is the same node.
fn push_text(content: &mut Vec<Value>, text: &str, marks: &[Value]) {
    for (at, line) in text.split('\n').enumerate() {
        if at > 0 {
            content.push(json!({"type": "hardBreak"}));
        }
        if line.is_empty() {
            continue;
        }
        let same = |last: &Value| {
            last["type"] == "text"
                && last.get("marks").cloned().unwrap_or(json!([])) == json!(marks)
        };
        match content.last_mut() {
            Some(last) if same(last) => {
                let joined = format!("{}{line}", last["text"].as_str().unwrap_or_default());
                last["text"] = Value::from(joined);
            }
            _ => {
                let mut node = json!({"type": "text", "text": line});
                if !marks.is_empty() {
                    node["marks"] = json!(marks);
                }
                content.push(node);
            }
        }
    }
}

struct Converter<'a> {
    text: &'a str,
    definitions: HashMap<String, (String, Option<String>)>,
}

impl Converter<'_> {
    /// What `node` was written as.
    fn source(&self, node: &Node) -> &str {
        node.position()
            .and_then(|at| self.text.get(at.start.offset..at.end.offset))
            .unwrap_or_default()
    }

    /// Blocks into `out`. `inner` is a quote or a list item, whose ADF holds paragraphs, lists
    /// and code blocks only: the rest is written as paragraphs there.
    fn blocks(&self, nodes: &[Node], inner: bool, out: &mut Vec<Value>) {
        for node in nodes {
            match node {
                Node::Paragraph(paragraph) => {
                    let content = self.inline(&paragraph.children, &[]);
                    out.push(paragraph_of(content));
                }
                Node::Heading(heading) if !inner => {
                    let content = self.inline(&heading.children, &[]);
                    out.push(
                        json!({"type": "heading", "attrs": {"level": heading.depth.clamp(1, 6)},
                        "content": content}),
                    );
                }
                Node::Heading(heading) => {
                    let content = self.inline(&heading.children, &[json!({"type": "strong"})]);
                    out.push(paragraph_of(content));
                }
                Node::Blockquote(quote) if !inner => {
                    let mut content = Vec::new();
                    self.blocks(&quote.children, true, &mut content);
                    out.push(json!({"type": "blockquote", "content": filled(content)}));
                }
                Node::Blockquote(quote) => self.blocks(&quote.children, true, out),
                Node::List(list) => {
                    let items: Vec<Value> = list
                        .children
                        .iter()
                        .map(|item| {
                            let mut content = Vec::new();
                            self.blocks(
                                item.children().map_or(&[][..], Vec::as_slice),
                                true,
                                &mut content,
                            );
                            json!({"type": "listItem", "content": filled(content)})
                        })
                        .collect();
                    out.push(match (list.ordered, list.start) {
                        (true, Some(start)) if start != 1 => {
                            json!({"type": "orderedList", "attrs": {"order": start}, "content": items})
                        }
                        (true, _) => json!({"type": "orderedList", "content": items}),
                        (false, _) => json!({"type": "bulletList", "content": items}),
                    });
                }
                Node::Code(code) => {
                    let mut block = json!({"type": "codeBlock"});
                    if let Some(language) = code.lang.as_deref().filter(|l| !l.is_empty()) {
                        block["attrs"] = json!({"language": language});
                    }
                    if !code.value.is_empty() {
                        block["content"] = json!([{"type": "text", "text": code.value}]);
                    }
                    out.push(block);
                }
                Node::ThematicBreak(_) if !inner => out.push(json!({"type": "rule"})),
                Node::ThematicBreak(_) => out.push(as_text("---")),
                Node::Table(table) if !inner && self.simple(table.children.as_slice()) => {
                    out.push(self.table(&table.children));
                }
                Node::Html(html) => out.push(as_text(&html.value)),
                // A definition is no text of its own: the links that use it carry its address.
                Node::Definition(_) => {}
                other => out.push(as_text(self.source(other))),
            }
        }
    }

    /// Whether the rows of a Markdown table are a table Jira takes: as wide as the header.
    fn simple(&self, rows: &[Node]) -> bool {
        let width = rows.first().and_then(Node::children).map_or(0, Vec::len);
        width > 0
            && rows
                .iter()
                .all(|row| row.children().map_or(0, Vec::len) == width)
    }

    fn table(&self, rows: &[Node]) -> Value {
        let rows: Vec<Value> = rows
            .iter()
            .enumerate()
            .map(|(at, row)| {
                let kind = if at == 0 { "tableHeader" } else { "tableCell" };
                let cells: Vec<Value> = row
                    .children()
                    .into_iter()
                    .flatten()
                    .map(|cell| {
                        let content =
                            self.inline(cell.children().map_or(&[][..], Vec::as_slice), &[]);
                        json!({"type": kind, "attrs": {}, "content": [paragraph_of(content)]})
                    })
                    .collect();
                json!({"type": "tableRow", "content": cells})
            })
            .collect();
        json!({"type": "table", "attrs": {"isNumberColumnEnabled": false, "layout": "default"},
            "content": rows})
    }

    /// Inline content under `marks`.
    fn inline(&self, nodes: &[Node], marks: &[Value]) -> Vec<Value> {
        let mut out = Vec::new();
        self.inline_into(nodes, marks, &mut out);
        out
    }

    fn inline_into(&self, nodes: &[Node], marks: &[Value], out: &mut Vec<Value>) {
        let with = |mark: Value| [marks, &[mark]].concat();
        for node in nodes {
            match node {
                Node::Text(text) => push_text(out, &text.value, marks),
                Node::Break(_) => out.push(json!({"type": "hardBreak"})),
                Node::Emphasis(e) => {
                    self.inline_into(&e.children, &with(json!({"type": "em"})), out)
                }
                Node::Strong(s) => {
                    self.inline_into(&s.children, &with(json!({"type": "strong"})), out)
                }
                Node::Delete(d) => {
                    self.inline_into(&d.children, &with(json!({"type": "strike"})), out)
                }
                // Jira holds a code mark beside a link and no other.
                Node::InlineCode(code) => {
                    let kept: Vec<Value> = marks
                        .iter()
                        .filter(|m| m["type"] == "link")
                        .cloned()
                        .collect();
                    push_text(
                        out,
                        &code.value,
                        &[kept, vec![json!({"type": "code"})]].concat(),
                    );
                }
                Node::Link(link) => {
                    let mark = link_mark(&link.url, link.title.as_deref());
                    self.inline_into(&link.children, &with(mark), out);
                }
                Node::LinkReference(reference) => match self.definitions.get(&reference.identifier)
                {
                    Some((url, title)) => {
                        let mark = link_mark(url, title.as_deref());
                        self.inline_into(&reference.children, &with(mark), out);
                    }
                    None => push_text(out, self.source(node), marks),
                },
                Node::Html(html) => push_text(out, &html.value, marks),
                // An image, a footnote, anything with no ADF node: the text it was written as.
                other => push_text(out, self.source(other), marks),
            }
        }
    }
}

fn paragraph_of(content: Vec<Value>) -> Value {
    match content.is_empty() {
        true => json!({"type": "paragraph"}),
        false => json!({"type": "paragraph", "content": content}),
    }
}

/// A list item or a quote holds at least one block.
fn filled(content: Vec<Value>) -> Vec<Value> {
    match content.is_empty() {
        true => vec![json!({"type": "paragraph"})],
        false => content,
    }
}

fn link_mark(url: &str, title: Option<&str>) -> Value {
    let mut attrs = json!({"href": url});
    if let Some(title) = title {
        attrs["title"] = Value::from(title);
    }
    json!({"type": "link", "attrs": attrs})
}

/// The Markdown of an ADF tree: its blocks apart by an empty line.
pub(crate) fn to_markdown(node: &Value) -> String {
    let mut blocks = Vec::new();
    block(node, &mut blocks);
    blocks.join("\n\n").trim().to_owned()
}

fn content(node: &Value) -> &[Value] {
    node["content"].as_array().map_or(&[], Vec::as_slice)
}

/// The blocks of `node`, each as Markdown, into `out`.
fn block(node: &Value, out: &mut Vec<String>) {
    match node["type"].as_str().unwrap_or_default() {
        "paragraph" => {
            let text = inline(content(node), "\n");
            if !text.trim().is_empty() {
                out.push(text.lines().map(line_start).collect::<Vec<_>>().join("\n"));
            }
        }
        "heading" => {
            let level = node["attrs"]["level"].as_u64().unwrap_or(1).clamp(1, 6) as usize;
            let text = inline(content(node), " ");
            if !text.trim().is_empty() {
                out.push(format!("{} {}", "#".repeat(level), text.trim()));
            }
        }
        "blockquote" => {
            let mut inner = Vec::new();
            content(node)
                .iter()
                .for_each(|child| block(child, &mut inner));
            let quoted = inner.join("\n\n");
            let lines = quoted
                .lines()
                .map(|l| format!("> {l}").trim_end().to_owned());
            if !quoted.is_empty() {
                out.push(lines.collect::<Vec<_>>().join("\n"));
            }
        }
        "codeBlock" => {
            let code: String = content(node)
                .iter()
                .filter_map(|t| t["text"].as_str())
                .collect();
            let longest = code
                .split(|c| c != '`')
                .map(str::len)
                .max()
                .unwrap_or_default();
            let fence = "`".repeat((longest + 1).max(3));
            let language = node["attrs"]["language"].as_str().unwrap_or_default();
            out.push(format!(
                "{fence}{language}\n{}\n{fence}",
                code.trim_end_matches('\n')
            ));
        }
        "bulletList" | "orderedList" => {
            let ordered = node["type"] == "orderedList";
            let first = node["attrs"]["order"].as_u64().unwrap_or(1);
            let items: Vec<String> = content(node)
                .iter()
                .zip(first..)
                .map(|(item, number)| {
                    let marker = if ordered {
                        format!("{number}. ")
                    } else {
                        "- ".to_owned()
                    };
                    let mut body = String::new();
                    for child in content(item) {
                        let mut blocks = Vec::new();
                        block(child, &mut blocks);
                        for text in blocks {
                            let lists = matches!(
                                child["type"].as_str(),
                                Some("bulletList" | "orderedList")
                            );
                            match (body.is_empty(), lists) {
                                (true, _) => {}
                                (false, true) => body.push('\n'),
                                (false, false) => body.push_str("\n\n"),
                            }
                            body.push_str(&text);
                        }
                    }
                    let indent = " ".repeat(marker.len());
                    let mut lines = body.lines();
                    let mut text = format!("{marker}{}", lines.next().unwrap_or_default());
                    for line in lines {
                        text.push('\n');
                        if !line.is_empty() {
                            text.push_str(&indent);
                            text.push_str(line);
                        }
                    }
                    text
                })
                .collect();
            if !items.is_empty() {
                out.push(items.join("\n"));
            }
        }
        "rule" => out.push("---".to_owned()),
        "table" => {
            let rows: Vec<Vec<String>> = content(node)
                .iter()
                .map(|row| {
                    content(row)
                        .iter()
                        .map(|cell| {
                            let mut blocks = Vec::new();
                            content(cell)
                                .iter()
                                .for_each(|child| block(child, &mut blocks));
                            blocks.join(" ").replace('\n', " ").replace('|', "\\|")
                        })
                        .collect()
                })
                .collect();
            let width = rows.first().map_or(0, Vec::len);
            if width > 0 {
                let line = |cells: &[String]| format!("| {} |", cells.join(" | "));
                let mut lines = vec![line(&rows[0]), line(&vec!["---".to_owned(); width])];
                lines.extend(rows[1..].iter().map(|row| line(row)));
                out.push(lines.join("\n"));
            }
        }
        "blockCard" => out.push(node["attrs"]["url"].as_str().unwrap_or_default().to_owned()),
        // An image or a file has no text: a line says where it stands and that it is not seen.
        "mediaSingle" | "mediaGroup" | "media" => {
            let lines: Vec<String> = match node["type"] == "media" {
                true => vec![media_line(node)],
                false => content(node)
                    .iter()
                    .filter(|child| child["type"] == "media")
                    .map(media_line)
                    .collect(),
            };
            if !lines.is_empty() {
                out.push(lines.join("\n"));
            }
        }
        "text" | "hardBreak" | "mention" | "emoji" | "inlineCard" => {
            out.push(inline(std::slice::from_ref(node), "\n"));
        }
        // The document, a panel, an expand and anything unknown: the blocks it holds, if any.
        _ => content(node).iter().for_each(|child| block(child, out)),
    }
}

/// What an agent that cannot see an image is told of it.
const NOT_SHOWN: &str = "not shown: ask the user to paste it if it matters";

/// A line saying that a `media` is there: an image (its type, `attrs.mimeType`, or failing that
/// its name's extension says so; an external one always is) or a file, by its name, which is the
/// attachment's file name (`alt`), or its address when it is external. Never fetched: the
/// attachment's address needs the user's credentials.
fn media_line(node: &Value) -> String {
    let attrs = &node["attrs"];
    let text = |key: &str| {
        attrs[key]
            .as_str()
            .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|t| !t.is_empty())
    };
    let (alt, url) = (text("alt"), text("url"));
    let external = attrs["type"] == "external";
    let name = match (external, alt, url) {
        (true, Some(alt), Some(url)) => format!("{alt} ({url})"),
        (true, None, Some(url)) => url,
        (_, Some(alt), _) => alt,
        _ => "unnamed".to_owned(),
    };
    let image = match attrs["mimeType"].as_str() {
        Some(mime) => mime.starts_with("image/"),
        None => {
            external
                || name.rsplit_once('.').is_some_and(|(_, extension)| {
                    [
                        "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "heic", "tif", "tiff",
                    ]
                    .iter()
                    .any(|image| extension.eq_ignore_ascii_case(image))
                })
        }
    };
    match image {
        true => format!("[image: {name}, {NOT_SHOWN}]"),
        false => format!("[file: {name}, not shown]"),
    }
}

/// Gives each `media` of `node` named in `types` (attachment file name to media type) the type its
/// attachment has: what the name's extension would only guess.
pub(crate) fn typed(node: &mut Value, types: &HashMap<String, String>) {
    if matches!(node["type"].as_str(), Some("media" | "mediaInline"))
        && let Some(mime) = node["attrs"]["alt"].as_str().and_then(|alt| types.get(alt))
    {
        node["attrs"]["mimeType"] = Value::from(mime.as_str());
    }
    if let Some(children) = node["content"].as_array_mut() {
        for child in children {
            typed(child, types);
        }
    }
}

/// A line of a paragraph that Markdown would take for a block of its own is written so that it
/// stays the text it is.
fn line_start(line: &str) -> String {
    let rest = line.trim_start();
    let digits = rest.trim_start_matches(|c: char| c.is_ascii_digit());
    let numbered =
        digits.len() < rest.len() && digits.starts_with(['.', ')']) && digits[1..].starts_with(' ');
    if numbered {
        let at = line.len() - digits.len();
        return format!("{}\\{}", &line[..at], &line[at..]);
    }
    let blocky = rest.starts_with(['#', '>'])
        || ["- ", "+ "].iter().any(|bullet| rest.starts_with(bullet))
        || matches!(rest, "-" | "+")
        || (rest.len() >= 3 && rest.chars().all(|c| c == '-' || c == '='));
    match blocky {
        true => format!("{}\\{rest}", &line[..line.len() - rest.len()]),
        false => line.to_owned(),
    }
}

/// Inline nodes as Markdown, a line break written `hard`.
fn inline(nodes: &[Value], hard: &str) -> String {
    // Texts of the same marks are one run: `**a****b**` is not what two bold texts mean.
    let mut runs: Vec<(String, Vec<&Value>)> = Vec::new();
    for node in nodes {
        let marks: Vec<&Value> = node["marks"].as_array().into_iter().flatten().collect();
        let text = match node["type"].as_str().unwrap_or_default() {
            "text" => node["text"].as_str().unwrap_or_default().to_owned(),
            "hardBreak" => {
                runs.push((hard.to_owned(), vec![&Value::Null]));
                continue;
            }
            "mention" | "emoji" => node["attrs"]["text"]
                .as_str()
                .or(node["attrs"]["shortName"].as_str())
                .unwrap_or_default()
                .to_owned(),
            "inlineCard" | "blockCard" => {
                node["attrs"]["url"].as_str().unwrap_or_default().to_owned()
            }
            // Written as it is: the brackets are not Markdown to escape.
            "mediaInline" | "media" => {
                runs.push((media_line(node), vec![&Value::Null]));
                continue;
            }
            _ => continue,
        };
        match runs.last_mut() {
            Some((last, last_marks)) if *last_marks == marks && !marks.contains(&&Value::Null) => {
                last.push_str(&text);
            }
            _ => runs.push((text, marks)),
        }
    }
    runs.iter()
        .map(|(text, marks)| match marks.first() {
            Some(Value::Null) => text.clone(),
            _ => run(text, marks),
        })
        .collect()
}

/// A text under its marks, as Markdown.
fn run(text: &str, marks: &[&Value]) -> String {
    let has = |kind: &str| marks.iter().any(|m| m["type"] == kind);
    let mut out = match has("code") {
        true => {
            let longest = text
                .split(|c| c != '`')
                .map(str::len)
                .max()
                .unwrap_or_default();
            let ticks = "`".repeat(longest + 1);
            let pad = if text.starts_with('`') || text.ends_with('`') {
                " "
            } else {
                ""
            };
            format!("{ticks}{pad}{text}{pad}{ticks}")
        }
        false => escaped(text),
    };
    for (kind, marker) in [("strike", "~~"), ("strong", "**"), ("em", "*")] {
        if has(kind) && !out.trim().is_empty() {
            // A marker next to a space opens or closes nothing.
            let (lead, rest) = out.split_at(out.len() - out.trim_start().len());
            let (inside, trail) = rest.split_at(rest.trim_end().len());
            out = format!("{lead}{marker}{inside}{marker}{trail}");
        }
    }
    if let Some(link) = marks.iter().find(|m| m["type"] == "link") {
        let href = link["attrs"]["href"].as_str().unwrap_or_default();
        out = match (
            marks.len() == 1 && text == href,
            href.contains([' ', '(', ')']),
        ) {
            (true, _) => text.to_owned(),
            (false, true) => format!("[{out}](<{href}>)"),
            (false, false) => format!("[{out}]({href})"),
        };
    }
    out
}

/// A text with what Markdown would take for markup written as the text it is.
fn escaped(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (at, c) in chars.iter().enumerate() {
        let word = |c: Option<&char>| c.is_some_and(|c| c.is_alphanumeric());
        let intraword =
            word(at.checked_sub(1).and_then(|at| chars.get(at))) && word(chars.get(at + 1));
        if matches!(c, '\\' | '*' | '`' | '[' | ']' | '<' | '~') || (*c == '_' && !intraword) {
            out.push('\\');
        }
        out.push(*c);
    }
    out
}

/// What a text comes back as once Jira has kept it: its Markdown, as ADF, as Markdown again.
pub(crate) fn round_trip(text: &str) -> String {
    to_markdown(&from_markdown(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paragraph(text: &str) -> Value {
        json!({"type": "paragraph", "content": [{"type": "text", "text": text}]})
    }

    fn doc(content: Value) -> Value {
        json!({"type": "doc", "version": 1, "content": content})
    }

    fn marked(text: &str, marks: Value) -> Value {
        json!({"type": "text", "text": text, "marks": marks})
    }

    #[test]
    fn headings_paragraphs_and_marks_are_converted() {
        let adf = from_markdown(
            "# Goal\n\nLet users **log in**, *fast*, ~~slowly~~ with `code`.\n\n###### Small",
        );
        assert_eq!(
            adf,
            doc(json!([
                {"type": "heading", "attrs": {"level": 1}, "content": [{"type": "text", "text": "Goal"}]},
                {"type": "paragraph", "content": [
                    {"type": "text", "text": "Let users "},
                    marked("log in", json!([{"type": "strong"}])),
                    {"type": "text", "text": ", "},
                    marked("fast", json!([{"type": "em"}])),
                    {"type": "text", "text": ", "},
                    marked("slowly", json!([{"type": "strike"}])),
                    {"type": "text", "text": " with "},
                    marked("code", json!([{"type": "code"}])),
                    {"type": "text", "text": "."},
                ]},
                {"type": "heading", "attrs": {"level": 6}, "content": [{"type": "text", "text": "Small"}]},
            ]))
        );
    }

    #[test]
    fn code_blocks_lists_links_quotes_and_rules_are_converted() {
        let adf = from_markdown(
            "```rust\nfn main() {}\n```\n\n- a\n- b\n  1. deep\n\n[docs](https://example.com/a)\n\n> quoted\n\n---",
        );
        assert_eq!(
            adf,
            doc(json!([
                {"type": "codeBlock", "attrs": {"language": "rust"},
                    "content": [{"type": "text", "text": "fn main() {}"}]},
                {"type": "bulletList", "content": [
                    {"type": "listItem", "content": [paragraph("a")]},
                    {"type": "listItem", "content": [paragraph("b"), {"type": "orderedList", "content": [
                        {"type": "listItem", "content": [paragraph("deep")]}
                    ]}]},
                ]},
                {"type": "paragraph", "content": [
                    marked("docs", json!([{"type": "link", "attrs": {"href": "https://example.com/a"}}]))
                ]},
                {"type": "blockquote", "content": [paragraph("quoted")]},
                {"type": "rule"},
            ]))
        );
    }

    #[test]
    fn a_simple_table_becomes_a_table_and_another_stays_text() {
        let adf = from_markdown("| a | b |\n| --- | --- |\n| 1 | **2** |");
        let cell = |kind: &str, content: Value| json!({"type": kind, "attrs": {}, "content": [{"type": "paragraph", "content": content}]});
        assert_eq!(
            adf,
            doc(
                json!([{"type": "table", "attrs": {"isNumberColumnEnabled": false, "layout": "default"},
                "content": [
                    {"type": "tableRow", "content": [
                        cell("tableHeader", json!([{"type": "text", "text": "a"}])),
                        cell("tableHeader", json!([{"type": "text", "text": "b"}])),
                    ]},
                    {"type": "tableRow", "content": [
                        cell("tableCell", json!([{"type": "text", "text": "1"}])),
                        cell("tableCell", json!([marked("2", json!([{"type": "strong"}]))])),
                    ]},
                ]}])
            )
        );
        // A row with fewer cells than the header is not a table Jira would take.
        let ragged = "| a | b |\n| --- | --- |\n| 1 |";
        assert_eq!(
            from_markdown(ragged),
            doc(json!([{"type": "paragraph", "content": [
                {"type": "text", "text": "| a | b |"}, {"type": "hardBreak"},
                {"type": "text", "text": "| --- | --- |"}, {"type": "hardBreak"},
                {"type": "text", "text": "| 1 |"},
            ]}]))
        );
    }

    #[test]
    fn plain_text_is_sent_as_one_paragraph_per_block() {
        let adf = from_markdown("First line\nsecond line\n\nThird block\n\n\nFourth");
        assert_eq!(
            adf,
            doc(json!([
                {"type": "paragraph", "content": [
                    {"type": "text", "text": "First line"}, {"type": "hardBreak"},
                    {"type": "text", "text": "second line"},
                ]},
                paragraph("Third block"),
                paragraph("Fourth"),
            ]))
        );
        assert_eq!(
            round_trip("First line\nsecond line\n\nThird block"),
            "First line\nsecond line\n\nThird block"
        );
    }

    #[test]
    fn what_cannot_be_converted_is_kept_as_text() {
        let adf = from_markdown(
            "<div class=\"x\">raw</div>\n\nSee ![a logo](https://example.com/l.png) here",
        );
        assert_eq!(
            adf,
            doc(json!([
                paragraph("<div class=\"x\">raw</div>"),
                paragraph("See ![a logo](https://example.com/l.png) here"),
            ]))
        );
    }

    /// A `mediaSingle` as a real site prints it, its media named `alt`.
    fn media_single(alt: &str) -> Value {
        json!({"type": "mediaSingle",
            "attrs": {"layout": "align-start", "localId": "a0f1803bf23b", "width": 1035},
            "content": [{"type": "media", "attrs": {"alt": alt, "collection": "", "height": 852,
                "id": "00000000-0000-4000-8000-000000000001", "type": "file", "width": 1291}}]})
    }

    const NOT_SHOWN: &str = "not shown: ask the user to paste it if it matters";

    #[test]
    fn an_image_in_a_jira_comment_is_named_where_it_stands() {
        let adf = doc(json!([
            paragraph("Claudio proposes this for the mobile version"),
            media_single("image-20260101-120000.png"),
            paragraph("What do you think?"),
        ]));

        assert_eq!(
            to_markdown(&adf),
            format!(
                "Claudio proposes this for the mobile version\n\n\
                 [image: image-20260101-120000.png, {NOT_SHOWN}]\n\nWhat do you think?"
            )
        );
    }

    #[test]
    fn a_file_that_is_not_an_image_is_named_as_a_file() {
        let group = json!({"type": "mediaGroup", "content": [
            {"type": "media", "attrs": {"type": "file", "id": "1", "alt": "spec.pdf"}},
            {"type": "media", "attrs": {"type": "file", "id": "2", "alt": "photo.JPG"}},
        ]});
        let inline = json!({"type": "paragraph", "content": [
            {"type": "text", "text": "See "},
            {"type": "mediaInline", "attrs": {"id": "3", "alt": "notes.txt"}},
        ]});
        // The attachment's own type wins over the name: a `.bin` the site calls an image.
        let typed = json!({"type": "mediaSingle", "content": [
            {"type": "media", "attrs": {"type": "file", "alt": "shot.bin", "mimeType": "image/png"}}]});

        assert_eq!(
            to_markdown(&doc(json!([group, inline, typed]))),
            format!(
                "[file: spec.pdf, not shown]\n[image: photo.JPG, {NOT_SHOWN}]\n\n\
                 See [file: notes.txt, not shown]\n\n[image: shot.bin, {NOT_SHOWN}]"
            )
        );
    }

    #[test]
    fn an_external_image_gives_its_address() {
        let external = json!({"type": "mediaSingle", "content": [
            {"type": "media", "attrs": {"type": "external", "url": "https://example.com/a.png"}}]});

        assert_eq!(
            to_markdown(&doc(json!([external]))),
            format!("[image: https://example.com/a.png, {NOT_SHOWN}]")
        );
    }

    #[test]
    fn an_external_image_with_an_alt_gives_its_address_after_the_name() {
        let external = json!({"type": "mediaSingle", "content": [
            {"type": "media", "attrs": {"type": "external", "alt": "the login page",
                "url": "https://example.com/a.png"}}]});

        assert_eq!(
            to_markdown(&doc(json!([external]))),
            format!("[image: the login page (https://example.com/a.png), {NOT_SHOWN}]")
        );
    }

    #[test]
    fn a_media_with_no_name_is_still_said() {
        let nameless = json!({"type": "mediaSingle", "content": [
            {"type": "media", "attrs": {"type": "file", "id": "1"}}]});

        assert_eq!(
            to_markdown(&doc(json!([nameless]))),
            "[file: unnamed, not shown]"
        );
    }

    #[test]
    fn a_text_with_no_content_is_one_empty_paragraph() {
        assert_eq!(from_markdown(" \n"), doc(json!([{"type": "paragraph"}])));
    }

    #[test]
    fn the_markdown_written_is_the_markdown_read_back_for_each_converted_kind() {
        for text in [
            "# One\n\n## Two\n\n###### Six",
            "A paragraph.\n\nAnother one.",
            "line one\nline two",
            "**bold** and *italic* and ~~strike~~ and `code`",
            "***both***",
            "```rust\nfn main() {}\n```",
            "```\nplain\n\n\nthree lines apart\n```",
            "- a\n- b\n  - nested\n  - more\n- c",
            "1. one\n2. two\n   1. inner\n3. three",
            "- first\n\n  second paragraph\n- next",
            "[text](https://example.com/a)",
            "[**bold link**](https://example.com/a)",
            "> quoted\n> text",
            "above\n\n---\n\nbelow",
            "| a | b |\n| --- | --- |\n| 1 | **2** |",
            "Some \\*stars\\* and snake_case\n\\# starts a line\n1\\. not a list",
        ] {
            assert_eq!(round_trip(text), text, "{text}");
        }
    }
}
