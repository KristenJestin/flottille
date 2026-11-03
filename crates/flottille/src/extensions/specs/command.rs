//! `flottille spec`: the specs of a project, on the command line.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;
use flottille_core::paths::Paths;
use serde_json::{Value, json};

use crate::client::Client;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Subcommand)]
pub enum SpecAction {
    /// Start a spec, a draft, in a project where Specs is on
    New {
        project: String,
        #[arg(long)]
        title: String,
        /// alone (no ticket), internal (tickets read, never written) or ticket (the ticket is
        /// the spec); alone without --ticket, internal with
        #[arg(long, value_parser = ["alone", "internal", "ticket"])]
        mode: Option<String>,
        /// A ticket of the project's tracker the spec covers; again for each
        #[arg(long = "ticket", value_name = "K")]
        tickets: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// The specs of the projects where Specs is on, or of one
    List {
        #[arg(long, value_name = "P")]
        project: Option<String>,
        /// draft, ready, in_progress, review or done
        #[arg(long, value_name = "S")]
        status: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// One spec, whole
    Show {
        spec: u64,
        #[arg(long)]
        json: bool,
    },
    /// Write one section of a draft whole, from a file: a text for problem and notes, JSON for
    /// the others
    Write {
        spec: u64,
        section: String,
        #[arg(long, value_name = "F")]
        file: PathBuf,
    },
    /// Cover one more ticket of the project's tracker
    Link { spec: u64, ticket: String },
    /// Cover a ticket no more
    Unlink { spec: u64, ticket: String },
    /// Say the spec is ready: refused with an open question or no expected behaviour
    Ready { spec: u64 },
    /// Send a ready spec back to Draft, so that it can be written again
    Draft { spec: u64 },
}

pub fn run(paths: &Paths, action: SpecAction) -> Result<ExitCode> {
    let mut client = Client::connect_or_start(paths)?;
    let (answer, json) = match action {
        SpecAction::New {
            project,
            title,
            mode,
            tickets,
            json,
        } => (
            client.request(
                "spec.new",
                json!({"project": project, "title": title, "mode": mode, "tickets": tickets}),
            )?,
            json,
        ),
        SpecAction::List {
            project,
            status,
            json,
        } => {
            let answer =
                client.request("spec.list", json!({"project": project, "status": status}))?;
            if json {
                println!("{answer}");
            } else {
                for spec in answer["specs"].as_array().into_iter().flatten() {
                    println!("{}", line(spec));
                }
            }
            return Ok(ExitCode::SUCCESS);
        }
        SpecAction::Show { spec, json } => {
            (client.request("spec.show", json!({"spec": spec}))?, json)
        }
        SpecAction::Write {
            spec,
            section,
            file,
        } => {
            let text = std::fs::read_to_string(&file)
                .map_err(|error| format!("{}: {error}", file.display()))?;
            let content = match section.as_str() {
                "problem" | "notes" => Value::String(text),
                _ => serde_json::from_str(&text)
                    .map_err(|error| format!("{}: not JSON: {error}", file.display()))?,
            };
            (
                client.request(
                    "spec.write",
                    json!({"spec": spec, "section": section, "content": content}),
                )?,
                false,
            )
        }
        SpecAction::Link { spec, ticket } => (
            client.request("spec.link", json!({"spec": spec, "ticket": ticket}))?,
            false,
        ),
        SpecAction::Unlink { spec, ticket } => (
            client.request("spec.unlink", json!({"spec": spec, "ticket": ticket}))?,
            false,
        ),
        SpecAction::Ready { spec } => (client.request("spec.ready", json!({"spec": spec}))?, false),
        SpecAction::Draft { spec } => (client.request("spec.draft", json!({"spec": spec}))?, false),
    };
    if json {
        println!("{answer}");
    } else {
        print!("{}", text(&answer));
    }
    Ok(ExitCode::SUCCESS)
}

fn line(spec: &Value) -> String {
    let tickets: Vec<&str> = spec["tickets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    format!(
        "spec {} ({}) {}: {} [{}{}]",
        spec["spec"],
        spec["project"].as_str().unwrap_or_default(),
        spec["status"].as_str().unwrap_or_default(),
        spec["title"].as_str().unwrap_or_default(),
        spec["mode"].as_str().unwrap_or_default(),
        if tickets.is_empty() {
            String::new()
        } else {
            format!(", {}", tickets.join(" "))
        }
    )
}

/// A spec as a text, its sections in their order.
fn text(spec: &Value) -> String {
    let field = |value: &Value, key: &str| value[key].as_str().unwrap_or_default().to_owned();
    let mut text = format!("{}\nLanguage: {}\n", line(spec), field(spec, "language"));
    let list = |key: &str| spec[key].as_array().cloned().unwrap_or_default();
    text.push_str(&format!("\nProblem\n{}\n", field(spec, "problem")));
    text.push_str("\nExpected behaviour\n");
    for c in list("behaviour") {
        text.push_str(&format!("{}: {}\n", field(&c, "id"), field(&c, "text")));
    }
    text.push_str("\nDecisions\n");
    for d in list("decisions") {
        text.push_str(&format!(
            "{}: {}\n  question: {}\n",
            field(&d, "id"),
            field(&d, "decision"),
            field(&d, "question")
        ));
        for option in d["set_aside"].as_array().into_iter().flatten() {
            text.push_str(&format!(
                "  set aside: {}\n",
                option.as_str().unwrap_or_default()
            ));
        }
    }
    text.push_str("\nOut of scope\n");
    for outside in list("out_of_scope") {
        let why = outside["reason"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| outside["ticket"].as_str().map(|t| format!("taken by {t}")))
            .unwrap_or_default();
        text.push_str(&format!("- {}: {why}\n", field(&outside, "text")));
    }
    text.push_str(&format!("\nTechnical notes\n{}\n", field(spec, "notes")));
    text.push_str("\nVerification\n");
    for v in list("verification") {
        text.push_str(&format!("{}: {}\n", field(&v, "id"), field(&v, "text")));
    }
    text.push_str("\nOpen questions\n");
    for question in list("questions") {
        text.push_str(&format!("- {}\n", question.as_str().unwrap_or_default()));
    }
    text
}
