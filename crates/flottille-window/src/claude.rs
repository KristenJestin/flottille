//! What a project's `.claude` folder holds of skills and commands, offered for import in the
//! project's settings (`docs/skills.md`, `docs/commands.md`, "Importing a `.claude` folder"). The
//! daemon reads the folder (`skill.detect`, `command.detect`) and imports (`skill.import`,
//! `command.import`); the window only offers what it found: each file with Import, Import all,
//! and a file that cannot be imported with why.

use flottille_ui::{Button, Icon, ListCard, ListRow, Listing, icon, palette, presence, space};
use gpui_kit::{
    App, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, TestSupportExt as _, Window, div, px,
};
use serde_json::Value;

use crate::actions::ClaudeKind;

/// A file of `.claude` as the daemon found it.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub name: String,
    pub path: String,
    pub description: String,
    /// It can be imported now.
    pub new: bool,
    /// It is imported already.
    pub imported: bool,
    /// Why it cannot be imported.
    pub problem: Option<String>,
}

impl Found {
    pub fn of(value: &Value) -> Self {
        let state = value["state"].as_str().unwrap_or_default();
        let path = value["path"].as_str().unwrap_or_default().to_owned();
        let name = match value["name"].as_str().unwrap_or_default() {
            // A file whose name cannot be read is called after its place.
            "" => called_after(&path),
            name => name.to_owned(),
        };
        Self {
            name,
            path,
            description: value["description"].as_str().unwrap_or_default().to_owned(),
            new: state == "new",
            imported: state == "imported",
            problem: value["problem"].as_str().map(str::to_owned),
        }
    }
}

/// `.claude/skills/<name>/SKILL.md` or `.claude/commands/<name>.md`: `<name>`.
fn called_after(path: &str) -> String {
    let path = std::path::Path::new(path);
    let named = if path.file_name().is_some_and(|file| file == "SKILL.md") {
        path.parent().and_then(|folder| folder.file_name())
    } else {
        path.file_stem()
    };
    named
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The toast an import says: what was imported, and what was left out.
pub fn imported_toast(kind: ClaudeKind, read: &Result<Value, String>) -> flottille_ui::Toast {
    let noun = match kind {
        ClaudeKind::Skill => ("skill", "skills"),
        ClaudeKind::Command => ("command", "commands"),
    };
    match read {
        Err(why) => flottille_ui::Toast::failure(format!("The {} were not imported", noun.1))
            .detail(why.clone()),
        Ok(value) => {
            let names: Vec<&str> = value["imported"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|entry| entry["name"].as_str())
                .collect();
            let skipped = value["skipped"].as_array().map_or(0, Vec::len);
            let title = match names.as_slice() {
                [] => format!("No {} imported", noun.0),
                [one] => format!("{} {one} imported", capital(noun.0)),
                many => format!("{} {} imported", many.len(), noun.1),
            };
            let toast = flottille_ui::Toast::success(title);
            match skipped {
                0 => toast.detail("Agents created from now on see them."),
                n => toast.detail(format!("{n} left out: the list of .claude says why.")),
            }
        }
    }
}

fn capital(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

type OnImport = std::rc::Rc<dyn Fn(Option<String>, &mut Window, &mut App) + 'static>;

/// Takes what the daemon found into the rows the block draws: the files not imported yet, so
/// that one imported folds away.
pub fn sync(rows: &mut Listing<Found>, found: Option<&[Found]>, cx: &App) {
    let Some(found) = found else {
        return;
    };
    let now = cx.background_executor().now();
    let offered = found
        .iter()
        .filter(|found| !found.imported)
        .map(|found| (found.path.clone(), found.clone()));
    rows.sync(offered, now, cx.reduce_motion());
}

/// The block under a section's list: the files of `.claude/<folder>` not imported yet, each with
/// Import (or why it cannot be), and Import all. It unfolds when there is one and folds away once
/// all are imported. `prefix` is the ids' (`skill`, `command`).
pub fn found_block(
    prefix: &'static str,
    folder: &'static str,
    rows: &Listing<Found>,
    busy: bool,
    on_import: impl Fn(Option<String>, &mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let p = palette(cx);
    let importable = rows
        .rows()
        .filter(|row| !row.placement.is_gone() && row.value.new)
        .count();
    let on_import: OnImport = std::rc::Rc::new(on_import);
    let mut list = ListCard::new(ElementId::Name(format!("{prefix}-found-list").into()));
    for listed in rows.rows() {
        let found = listed.value;
        let id = |part: &str| ElementId::Name(format!("{prefix}-{part}-{}", found.name).into());
        let mut row = ListRow::new(id("found"), SharedString::from(found.name.clone()))
            .mono()
            .leading(icon(Icon::File).size(px(14.)).text_color(p.faint))
            .tag(SharedString::from(found.path.clone()));
        match &found.problem {
            Some(why) => {
                row = row.below(
                    div()
                        .id(id("found-problem"))
                        .aria_label(SharedString::from(why.clone()))
                        .text_size(px(12.5))
                        .text_color(p.fail)
                        .child(SharedString::from(why.clone()))
                        .test_support(),
                );
            }
            None => {
                if !found.description.is_empty() {
                    row = row.line(SharedString::from(found.description.clone()));
                }
            }
        }
        if found.new {
            let (name, on_import) = (found.name.clone(), on_import.clone());
            row = row.trailing(
                Button::secondary(id("import"), "Import")
                    .disabled(busy)
                    .on_click(move |_: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        on_import(Some(name.clone()), window, cx)
                    }),
            );
        }
        list = list.item(id("found-item"), listed.placement, row);
    }
    let head = div()
        .flex()
        .items_center()
        .gap(space::SM)
        .min_h(px(30.))
        .child(
            div()
                .text_size(px(12.5))
                .text_color(p.mute)
                .child(SharedString::from(format!(
                    "Found in the project's .claude/{folder}, not imported yet"
                ))),
        )
        .child(div().flex_1())
        .child({
            let on_import = on_import.clone();
            Button::quiet(
                ElementId::Name(format!("{prefix}-import-all").into()),
                "Import all",
            )
            .disabled(busy || importable == 0)
            .on_click(move |_, window, cx| on_import(None, window, cx))
        });
    presence(
        ElementId::Name(format!("{prefix}-found-block").into()),
        !rows.is_empty(),
        div()
            .id(ElementId::Name(format!("{prefix}-found").into()))
            .flex()
            .flex_col()
            .gap(space::SM)
            .child(head)
            .child(list)
            .test_support(),
    )
    .gap(space::MD)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_found_file_reads_its_state_and_its_problem() {
        let found = Found::of(
            &json!({"name": "react", "path": ".claude/skills/react/SKILL.md",
            "state": "new", "description": "React"}),
        );
        assert!(found.new && !found.imported && found.problem.is_none());
        let broken = Found::of(&json!({"name": "bad", "path": ".claude/commands/bad.md",
            "state": "unusable", "problem": "no description"}));
        assert!(!broken.new);
        assert_eq!(broken.problem.as_deref(), Some("no description"));
        let unnamed = Found::of(
            &json!({"name": "", "path": ".claude/skills/broken/SKILL.md",
            "state": "unusable", "problem": "bad front matter"}),
        );
        assert_eq!(unnamed.name, "broken");
    }
}
