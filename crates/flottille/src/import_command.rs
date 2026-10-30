//! `flottille skill import` and `flottille command import`: what a project's `.claude` folder
//! holds, listed, and imported by name or all at once (`docs/skills.md`, `docs/commands.md`).

use std::process::ExitCode;

use clap::Args;
use serde_json::{Value, json};

use crate::client::Client;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Args)]
pub struct Import {
    project: String,
    /// The names to import, as `import` without any lists them
    #[arg(conflicts_with = "all")]
    names: Vec<String>,
    /// Import every file that can be; the others are said, with why
    #[arg(long)]
    all: bool,
    #[arg(long)]
    json: bool,
}

/// Lists, or imports. `kind` is `skill` or `command`, which is also the prefix of the methods.
pub fn run(
    client: &mut Client,
    kind: &str,
    in_full: impl Fn(&str) -> String,
    import: Import,
) -> Result<ExitCode> {
    let mut ask = |method: &str, params: Value| {
        client
            .request(&format!("{kind}.{method}"), params)
            .map_err(|error| Box::<dyn std::error::Error>::from(in_full(&error.to_string())))
    };
    let project = import.project;
    if import.names.is_empty() && !import.all {
        let detected = ask("detect", json!({"project": project}))?;
        if import.json {
            println!("{detected}");
        } else {
            println!("{}", detected_text(kind, &project, &detected));
        }
        return Ok(ExitCode::SUCCESS);
    }
    let mut params = json!({"project": project});
    if !import.names.is_empty() {
        params["names"] = import.names.into();
    }
    let done = ask("import", params)?;
    if import.json {
        println!("{done}");
    } else {
        println!("{}", done_text(kind, &project, &done));
    }
    Ok(ExitCode::SUCCESS)
}

fn field<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}

fn detected_text(kind: &str, project: &str, detected: &Value) -> String {
    let folder = field(detected, "folder");
    let found = detected["found"].as_array().map_or(&[][..], Vec::as_slice);
    if found.is_empty() {
        return format!("Project {project} has no {kind} in {folder}.");
    }
    let rows: Vec<[String; 4]> = found
        .iter()
        .map(|entry| {
            [
                field(entry, "state").to_owned(),
                entry["name"].as_str().unwrap_or("-").to_owned(),
                field(entry, "path").to_owned(),
                // A file that cannot be imported says why where its description would be.
                match entry["problem"].as_str() {
                    Some(why) => why.to_owned(),
                    None => field(entry, "description").to_owned(),
                },
            ]
        })
        .collect();
    let width = |at: usize, head: &str| {
        rows.iter()
            .map(|row| row[at].chars().count())
            .chain([head.chars().count()])
            .max()
            .unwrap_or_default()
    };
    let (state, name, path) = (width(0, "STATE"), width(1, "NAME"), width(2, "FILE"));
    let mut table = format!(
        "{:state$}  {:name$}  {:path$}  DESCRIPTION OR WHY NOT\n",
        "STATE", "NAME", "FILE"
    );
    for row in &rows {
        table.push_str(&format!(
            "{:state$}  {:name$}  {:path$}  {}\n",
            row[0], row[1], row[2], row[3]
        ));
    }
    if found.iter().any(|entry| entry["state"] == "new") {
        table.push_str(&format!(
            "Import one with `flottille {kind} import {project} <name>`, or every one that can \
             be with --all."
        ));
    }
    table.trim_end().to_owned()
}

fn done_text(kind: &str, project: &str, done: &Value) -> String {
    let imported = done["imported"].as_array().map_or(&[][..], Vec::as_slice);
    let skipped = done["skipped"].as_array().map_or(&[][..], Vec::as_slice);
    let mut lines: Vec<String> = imported
        .iter()
        .map(|entry| {
            format!(
                "Imported {kind} {} into {project}: it reads {} each time it is used.",
                field(entry, "name"),
                field(entry, "path")
            )
        })
        .collect();
    if imported.is_empty() {
        lines.push(format!("Nothing to import into {project}."));
    }
    for entry in skipped {
        lines.push(format!(
            "Not imported: {}: {}",
            field(entry, "path"),
            field(entry, "why")
        ));
    }
    lines.join("\n")
}
