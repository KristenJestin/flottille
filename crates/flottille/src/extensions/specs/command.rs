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
    /// Start a spec, a draft, in a project where Specs is on; from an idea, a session is started
    /// in the project's main checkout to write it
    New {
        project: String,
        /// Its title; the idea's first line by default
        #[arg(long, required_unless_present = "idea")]
        title: Option<String>,
        /// What to build, in a sentence: a lead session reads the code, asks you its questions
        /// and writes the spec
        #[arg(long)]
        idea: Option<String>,
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
    /// Write the spec of a ticket of the project's tracker: a lead session is started in the
    /// project's main checkout to write it
    FromTicket {
        project: String,
        ticket: String,
        /// internal (the ticket read, never written) or ticket (the ticket is the spec); internal
        /// by default
        #[arg(long, value_parser = ["internal", "ticket"])]
        mode: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Answer a question the session writing the spec asked: it becomes a decision (D)
    Answer {
        spec: u64,
        question: u64,
        /// The option chosen, from 1
        #[arg(
            long,
            value_name = "N",
            required_unless_present = "text",
            conflicts_with = "text"
        )]
        option: Option<u64>,
        /// An answer in your own words, setting every option aside
        #[arg(long)]
        text: Option<String>,
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
    /// Launch the development of a ready spec: a session for you in a new workspace linked to
    /// its ticket, its first message the spec
    Develop {
        spec: u64,
        #[arg(long)]
        json: bool,
    },
    /// A ticket changed since Flottille last read it, in the mode ticket: take the change into
    /// the spec (it goes back to Draft and its session is told), or overwrite the ticket with it
    Ticket {
        spec: u64,
        #[arg(value_parser = ["take", "overwrite"])]
        choice: String,
    },
    /// Where an internal spec departs from its tickets, offered at Done; --post posts it on them
    /// as a comment, once
    Departures {
        spec: u64,
        #[arg(long)]
        post: bool,
        #[arg(long)]
        json: bool,
    },
}

pub fn run(paths: &Paths, action: SpecAction) -> Result<ExitCode> {
    let mut client = Client::connect_or_start(paths)?;
    let (answer, json) = match action {
        SpecAction::New {
            project,
            title,
            idea,
            mode,
            tickets,
            json,
        } => {
            let answer = client.request(
                "spec.new",
                json!({"project": project, "title": title, "idea": idea, "mode": mode,
                    "tickets": tickets}),
            )?;
            if answer.get("agent").is_none() {
                (answer, json)
            } else {
                return started(&answer, json);
            }
        }
        SpecAction::FromTicket {
            project,
            ticket,
            mode,
            json,
        } => {
            let answer = client.request(
                "spec.from_ticket",
                json!({"project": project, "ticket": ticket, "mode": mode}),
            )?;
            return started(&answer, json);
        }
        SpecAction::Answer {
            spec,
            question,
            option,
            text,
        } => (
            client.request(
                "spec.answer",
                json!({"spec": spec, "question": question, "option": option, "text": text}),
            )?,
            false,
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
        SpecAction::Develop { spec, json } => {
            let answer = client.request("spec.develop", json!({"spec": spec}))?;
            if json {
                println!("{answer}");
            } else {
                let agent = answer["agent"].as_str().unwrap_or_default();
                println!(
                    "{}\nSession {agent} develops it in workspace {}: follow it with `flottille \
                     agent show {agent}`.",
                    line(&answer["spec"]),
                    answer["workspace"].as_str().unwrap_or_default(),
                );
            }
            return Ok(ExitCode::SUCCESS);
        }
        SpecAction::Ticket { spec, choice } => (
            client.request("spec.ticket", json!({"spec": spec, "choice": choice}))?,
            false,
        ),
        SpecAction::Departures { spec, post, json } => {
            let answer = client.request("spec.departures", json!({"spec": spec, "post": post}))?;
            if json {
                println!("{answer}");
            } else {
                print!("{}", departures(&answer));
            }
            return Ok(ExitCode::SUCCESS);
        }
    };
    if json {
        println!("{answer}");
    } else {
        print!("{}", text(&answer));
    }
    Ok(ExitCode::SUCCESS)
}

/// A spec and the session started to write it.
fn started(answer: &Value, json: bool) -> Result<ExitCode> {
    if json {
        println!("{answer}");
    } else {
        println!(
            "{}\nSession {} writes it: follow it with `flottille agent show {}`.",
            line(&answer["spec"]),
            answer["agent"].as_str().unwrap_or_default(),
            answer["agent"].as_str().unwrap_or_default()
        );
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

/// What `spec.departures` answered, as a text.
fn departures(answer: &Value) -> String {
    let said = answer["departures"].as_str().unwrap_or_default();
    let spec = &answer["spec"];
    let tickets: Vec<&str> = answer["tickets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let state = if said.trim().is_empty() {
        format!("Spec {spec} departs from its tickets nowhere.")
    } else if answer["posted_at"].is_u64() {
        format!("Posted on {}.", tickets.join(", "))
    } else if answer["offered"] == true {
        format!(
            "Not posted: post it on {} with `flottille spec departures {spec} --post`.",
            tickets.join(", ")
        )
    } else {
        "Offered at Done, to post or not.".to_owned()
    };
    match said.trim().is_empty() {
        true => format!("{state}\n"),
        false => format!("{said}\n\n{state}\n"),
    }
}

/// A spec as a text, its sections in their order.
fn text(spec: &Value) -> String {
    let field = |value: &Value, key: &str| value[key].as_str().unwrap_or_default().to_owned();
    let mut text = format!("{}\nLanguage: {}\n", line(spec), field(spec, "language"));
    let write = &spec["ticket_write"];
    let problem = field(write, "problem");
    match write["state"].as_str() {
        Some("written") => text.push_str(&format!("Ticket: {problem}\n")),
        Some("changed") => text.push_str(&format!(
            "Ticket: {problem} (`flottille spec ticket {} take` or `overwrite`)\n",
            spec["spec"]
        )),
        Some("failed") => text.push_str(&format!("Not written to its ticket: {problem}\n")),
        _ => {}
    }
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
    let departures = field(spec, "departures");
    if !departures.is_empty() {
        text.push_str(&format!("\nDepartures from the tickets\n{departures}\n"));
    }
    for question in list("asked") {
        let state = match question["decision"].as_str() {
            Some(decision) => format!("answered, {decision}"),
            None => "open".to_owned(),
        };
        text.push_str(&format!(
            "Question {} ({state}): {}\n",
            question["question"],
            field(&question, "text")
        ));
        for (at, option) in question["options"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let option = option.as_str().unwrap_or_default();
            let recommended = match option == field(&question, "recommended") {
                true => format!(" (recommended: {})", field(&question, "reason")),
                false => String::new(),
            };
            text.push_str(&format!("  {}. {option}{recommended}\n", at + 1));
        }
    }
    text
}
