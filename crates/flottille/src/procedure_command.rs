//! `flottille procedure`: the procedures of a project, and running one in a workspace once its
//! grants were shown and confirmed (`docs/procedures.md`, `docs/cli.md`).

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;
use flottille_core::paths::Paths;
use flottille_core::procedure::{Grant, Grants, Mode};
use serde_json::{Value, json};

use crate::client::Client;
use crate::project_command::in_full;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Subcommand)]
pub enum ProcedureAction {
    /// The procedures of a project
    List {
        project: String,
        #[arg(long)]
        json: bool,
    },
    /// One procedure: its grants and its instructions
    Show {
        project: String,
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Add a procedure to the project's configuration, or one of Flottille's templates
    Add {
        project: String,
        /// Its name; the template's when it is left out
        #[arg(required_unless_present = "template")]
        name: Option<String>,
        /// Write one of Flottille's procedures: open-prs, merge-stack or close
        #[arg(long, conflicts_with_all = ["description", "instructions", "instructions_file", "grants", "children"])]
        template: Option<String>,
        /// One line: what it does
        #[arg(long, required_unless_present = "template")]
        description: Option<String>,
        /// What the agent of a run is told to do
        #[arg(
            long,
            conflicts_with = "instructions_file",
            required_unless_present_any = ["instructions_file", "template"]
        )]
        instructions: Option<String>,
        /// The instructions, read from this file
        #[arg(long, value_name = "FILE")]
        instructions_file: Option<PathBuf>,
        /// A grant and its mode, `push=allow` or `pr-create=ask`; once per grant
        #[arg(long = "grant", value_name = "GRANT=MODE")]
        grants: Vec<String>,
        /// Whether the run's children get its grants; they do unless it is false
        #[arg(long, value_name = "true|false")]
        children: Option<bool>,
        #[arg(long)]
        json: bool,
    },
    /// Change what is given of a procedure; the rest stays
    Edit {
        project: String,
        name: String,
        #[arg(long)]
        description: Option<String>,
        #[arg(long, conflicts_with = "instructions_file")]
        instructions: Option<String>,
        #[arg(long, value_name = "FILE")]
        instructions_file: Option<PathBuf>,
        /// Its grants become these, `push=allow`; once per grant
        #[arg(
            long = "grant",
            value_name = "GRANT=MODE",
            conflicts_with = "no_grants"
        )]
        grants: Vec<String>,
        /// It grants nothing any more
        #[arg(long)]
        no_grants: bool,
        /// Whether the run's children get its grants
        #[arg(long, value_name = "true|false")]
        children: Option<bool>,
        #[arg(long)]
        json: bool,
    },
    /// Remove a procedure from the project's configuration
    Remove {
        project: String,
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Show a procedure's grants, ask, then start its agent in a workspace
    Run {
        project: String,
        name: String,
        /// The workspace it runs in
        #[arg(long)]
        workspace: String,
        /// Start it without asking
        #[arg(short, long)]
        yes: bool,
        #[arg(long)]
        json: bool,
    },
}

pub fn run(paths: &Paths, action: ProcedureAction) -> Result<ExitCode> {
    let mut client = Client::connect_or_start(paths)?;
    let mut ask = |method: &str, params: Value| {
        client
            .request(method, params)
            .map_err(|error| Box::<dyn std::error::Error>::from(in_full(paths, &error.to_string())))
    };
    match action {
        ProcedureAction::List { project, json } => {
            let list = ask("procedure.list", json!({"project": project}))?;
            if json {
                println!("{list}");
            } else {
                print!("{}", list_text(&list));
            }
        }
        ProcedureAction::Show {
            project,
            name,
            json,
        } => {
            let procedure = ask("procedure.show", json!({"project": project, "name": name}))?;
            if json {
                println!("{procedure}");
            } else {
                print!("{}", show_text(&project, &procedure));
            }
        }
        ProcedureAction::Add {
            project,
            name,
            template,
            description,
            instructions,
            instructions_file,
            grants,
            children,
            json,
        } => {
            let mut params = json!({"project": project, "name": name});
            if let Some(template) = template {
                params["template"] = template.into();
            } else {
                let instructions = instructions_of(instructions, instructions_file)?
                    .ok_or("give the instructions, with --instructions or --instructions-file")?;
                params["description"] = description.into();
                params["instructions"] = instructions.into();
                params["grants"] = serde_json::to_value(grants_of(&grants)?)?;
                if let Some(children) = children {
                    params["children"] = children.into();
                }
            }
            let added = ask("procedure.add", params)?;
            if json {
                println!("{added}");
            } else {
                println!(
                    "Added procedure {} to project {project}: it grants {}. Edit it in the \
                     project's file, or with `flottille procedure edit`.",
                    text(&added["name"]),
                    grants_line(&added)
                );
            }
        }
        ProcedureAction::Edit {
            project,
            name,
            description,
            instructions,
            instructions_file,
            grants,
            no_grants,
            children,
            json,
        } => {
            let mut params = json!({"project": project, "name": name});
            if let Some(description) = description {
                params["description"] = description.into();
            }
            if let Some(instructions) = instructions_of(instructions, instructions_file)? {
                params["instructions"] = instructions.into();
            }
            if no_grants || !grants.is_empty() {
                params["grants"] = serde_json::to_value(grants_of(&grants)?)?;
            }
            if let Some(children) = children {
                params["children"] = children.into();
            }
            let edited = ask("procedure.edit", params)?;
            if json {
                println!("{edited}");
            } else {
                println!("Changed procedure {name} of project {project}.");
            }
        }
        ProcedureAction::Remove {
            project,
            name,
            json,
        } => {
            let removed = ask(
                "procedure.remove",
                json!({"project": project, "name": name}),
            )?;
            if json {
                println!("{removed}");
            } else {
                println!("Removed procedure {name} from project {project}.");
            }
        }
        ProcedureAction::Run {
            project,
            name,
            workspace,
            yes,
            json,
        } => {
            let shown = ask(
                "procedure.run",
                json!({"project": project, "name": name, "workspace": workspace}),
            )?;
            eprint!("{}", run_text(&shown));
            if !yes {
                confirm()?;
            }
            // What was shown is what is granted: the daemon refuses if the grants changed meanwhile.
            let started = ask(
                "procedure.start",
                json!({"project": project, "name": name, "workspace": workspace,
                    "grants": shown["grants"], "children": shown["children"]}),
            )?;
            if json {
                println!("{started}");
            } else {
                println!(
                    "Started agent {} for procedure {name} in workspace {workspace}: follow it \
                     with `flottille agent read {}`.",
                    started["agent"]["id"].as_str().unwrap_or_default(),
                    started["agent"]["id"].as_str().unwrap_or_default()
                );
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn instructions_of(text: Option<String>, file: Option<PathBuf>) -> Result<Option<String>> {
    Ok(match (text, file) {
        (Some(text), _) => Some(text),
        (None, Some(file)) => Some(
            std::fs::read_to_string(&file)
                .map_err(|error| format!("cannot read {}: {error}", file.display()))?,
        ),
        (None, None) => None,
    })
}

/// `push=allow` and the like, each grant once, from the fixed vocabulary.
fn grants_of(words: &[String]) -> Result<Grants> {
    let vocabulary = || {
        Grant::ALL
            .iter()
            .map(|grant| grant.word())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut grants = Grants::new();
    for word in words {
        let (grant, mode) = word
            .split_once('=')
            .ok_or_else(|| format!("--grant {word}: give it as GRANT=MODE, like push=allow"))?;
        let grant = Grant::of(grant).ok_or_else(|| {
            format!(
                "--grant {word}: there is no grant {grant}: the grants are {}",
                vocabulary()
            )
        })?;
        let mode = Mode::of(mode)
            .ok_or_else(|| format!("--grant {word}: the mode is allow or ask, not {mode}"))?;
        if grants.insert(grant, mode).is_some() {
            return Err(format!("--grant {grant} is given twice").into());
        }
    }
    Ok(grants)
}

/// Asks on the standard input; anything but a yes, or no answer at all, refuses.
fn confirm() -> Result<()> {
    eprint!("Run it? [y/N] ");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    if std::io::stdin().lock().read_line(&mut answer)? == 0 {
        eprintln!();
        return Err(
            "not started: nothing to read an answer from, use --yes to run it without asking"
                .into(),
        );
    }
    match answer.trim().to_lowercase().as_str() {
        "y" | "yes" => Ok(()),
        _ => Err("not started".into()),
    }
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

/// The grants, one line each: `  push (allow): push the workspace's branch…`.
fn grants_text(procedure: &Value) -> String {
    let says = procedure["says"].as_array().map_or(&[][..], Vec::as_slice);
    if says.is_empty() {
        return "  none\n".into();
    }
    says.iter()
        .map(|each| {
            format!(
                "  {} ({}): {}\n",
                text(&each["grant"]),
                text(&each["mode"]),
                text(&each["says"])
            )
        })
        .collect()
}

fn children_text(procedure: &Value) -> &'static str {
    if procedure["children"] == true {
        "Its children get these grants too.\n"
    } else {
        "Its children get none of them.\n"
    }
}

/// The grants in one line: `push (allow), pr-create (ask)`, or `none`.
fn grants_line(procedure: &Value) -> String {
    let grants: Vec<String> = procedure["says"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|each| format!("{} ({})", text(&each["grant"]), text(&each["mode"])))
        .collect();
    if grants.is_empty() {
        "none".to_owned()
    } else {
        grants.join(", ")
    }
}

fn list_text(list: &Value) -> String {
    let procedures = list["procedures"].as_array().map_or(&[][..], Vec::as_slice);
    if procedures.is_empty() {
        return format!(
            "Project {} has no procedure: add one with `flottille procedure add`.\n",
            text(&list["project"])
        );
    }
    procedures
        .iter()
        .map(|procedure| {
            format!(
                "{}  {}\n    grants: {}\n",
                text(&procedure["name"]),
                text(&procedure["description"]),
                grants_line(procedure)
            )
        })
        .collect()
}

fn show_text(project: &str, procedure: &Value) -> String {
    let instructions: String = text(&procedure["instructions"])
        .lines()
        .map(|line| format!("  {line}\n"))
        .collect();
    format!(
        "Procedure {} of project {project}: {}\nIt grants, for each run:\n{}{}Instructions:\n{instructions}",
        text(&procedure["name"]),
        text(&procedure["description"]),
        grants_text(procedure),
        children_text(procedure),
    )
}

fn run_text(shown: &Value) -> String {
    format!(
        "Procedure {} of project {}, in workspace {}: {}\nIt grants, for this run only:\n{}{}\
         Everything else is decided as for any agent.\n",
        text(&shown["name"]),
        text(&shown["project"]),
        text(&shown["workspace"]),
        text(&shown["description"]),
        grants_text(shown),
        children_text(shown),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grant_is_given_as_grant_equals_mode_from_the_vocabulary() {
        let grants = grants_of(&["push=allow".into(), "pr-create=ask".into()]).unwrap();
        assert_eq!(
            grants,
            Grants::from([(Grant::Push, Mode::Allow), (Grant::PrCreate, Mode::Ask)])
        );
        for (word, said) in [
            ("push", "GRANT=MODE"),
            (
                "shell=allow",
                "there is no grant shell: the grants are commit, push",
            ),
            ("push=always", "the mode is allow or ask"),
        ] {
            let error = grants_of(&[word.into()]).unwrap_err().to_string();
            assert!(error.contains(said), "{word}: {error}");
        }
        assert!(grants_of(&["push=allow".into(), "push=ask".into()]).is_err());
    }
}
