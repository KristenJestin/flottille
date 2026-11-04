//! `flottille config`: what the user keeps in Flottille's configuration folder, from the command
//! line. Today that is the user's own instructions for every agent (`docs/agents.md`, "Roles").

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

use clap::{Args, Subcommand};
use flottille_core::paths::Paths;
use serde_json::{Value, json};

use crate::client::Client;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Your own instructions, added to every agent's in every project (instructions.md)
    Instructions(Instructions),
}

#[derive(Args)]
pub struct Instructions {
    /// Print them (the default)
    #[arg(long, group = "what")]
    show: bool,
    /// Change them in your editor ($VISUAL, else $EDITOR), started without a shell
    #[arg(long, group = "what")]
    edit: bool,
    /// Replace them with the text of this file; an empty file removes them
    #[arg(long, value_name = "FILE", group = "what")]
    file: Option<PathBuf>,
    #[arg(long)]
    json: bool,
}

pub fn run(paths: &Paths, action: ConfigAction) -> Result<ExitCode> {
    let ConfigAction::Instructions(args) = action;
    let mut client = Client::connect_or_start(paths)?;
    let mut ask = |method: &str, params: Value| client.request(method, params);
    let changed = if let Some(file) = &args.file {
        let text = std::fs::read_to_string(file)
            .map_err(|error| format!("cannot read {}: {error}", file.display()))?;
        Some(text)
    } else if args.edit {
        let shown = ask("instructions.show", json!({}))?;
        edited(shown["text"].as_str().unwrap_or_default())?
    } else {
        None
    };
    let shown = match changed {
        Some(text) => ask("instructions.set", json!({"text": text}))?,
        None => ask("instructions.show", json!({}))?,
    };
    if args.json {
        println!("{shown}");
    } else {
        print_instructions(&shown, args.file.is_some() || args.edit);
    }
    Ok(ExitCode::SUCCESS)
}

fn print_instructions(shown: &Value, changed: bool) {
    let path = shown["path"].as_str().unwrap_or_default();
    let text = shown["text"].as_str().unwrap_or_default();
    if changed {
        println!("Saved {path}: agents created from now on get it.");
    } else if text.trim().is_empty() {
        println!(
            "No instructions yet ({path}). Write them with `flottille config instructions --edit` \
             or `--file <path>`."
        );
    } else {
        println!("{path}:\n\n{}", text.trim_end());
    }
    if let Some(cut) = shown["cut"].as_str() {
        eprintln!("Cut: {cut}.");
    }
}

/// The text after the user's editor worked on `text` in a private temporary file; none when it
/// is unchanged. The editor is started without a shell: `$VISUAL` or `$EDITOR`, split at
/// spaces, the file last.
fn edited(text: &str) -> Result<Option<String>> {
    let editor = ["VISUAL", "EDITOR"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.trim().is_empty())
        .ok_or("no editor: set $VISUAL or $EDITOR, or use --file")?;
    let mut parts = editor.split_whitespace();
    let program = parts.next().ok_or("no editor")?;
    // Private to the user, and named after this process so that no other file is taken for it.
    let file =
        std::env::temp_dir().join(format!("flottille-instructions-{}.md", std::process::id()));
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&file)?
        .write_all(text.as_bytes())?;
    let result = edit_in(program, parts, &file, text);
    let _ = std::fs::remove_file(&file);
    result
}

fn edit_in<'a>(
    program: &str,
    args: impl Iterator<Item = &'a str>,
    file: &std::path::Path,
    text: &str,
) -> Result<Option<String>> {
    let status = Command::new(program)
        .args(args)
        .arg(file)
        .status()
        .map_err(|error| format!("cannot start {program}: {error}"))?;
    if !status.success() {
        return Err(format!("{program} failed ({status}): nothing was saved").into());
    }
    let after = std::fs::read_to_string(file)?;
    Ok((after != text).then_some(after))
}
