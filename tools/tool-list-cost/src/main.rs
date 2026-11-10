use std::process::ExitCode;

use serde_json::Value;
use tool_list_cost::{Counter, misuse_report, report, usage_report};

const USAGE: &str = "usage:
  tool-list-cost lists <dump.json> [tool,tool,...]
                                       the cost of each kind of agent's list, per tool and per level;
                                       with tools named, what slimming only those would save
  tool-list-cost misuse <dump.json>    what the server answers to a call with wrong arguments
  tool-list-cost usage <events.jsonl>  which tools are called, from `flottille watch --after 0`
  tool-list-cost text <file>           the tokens of a text file (a project's rules, an instructions file)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (command, file, slim) = match args.as_slice() {
        [command, file] => (command, file, ""),
        [command, file, slim] if command == "lists" => (command, file, slim.as_str()),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let slim: Vec<&str> = slim.split(',').filter(|name| !name.is_empty()).collect();
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("tool-list-cost: {file}: {error}");
            return ExitCode::FAILURE;
        }
    };
    let dump = |text: &str| -> Result<Value, String> {
        serde_json::from_str(text).map_err(|error| format!("{file} is not JSON: {error}"))
    };
    let out = match command.as_str() {
        "lists" => dump(&text).map(|dump| report(&Counter::new(), &dump, &slim)),
        "misuse" => dump(&text).map(|dump| misuse_report(&Counter::new(), &dump)),
        "usage" => Ok(usage_report(&text)),
        "text" => Ok(format!("{}\n", Counter::new().tokens(&text))),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match out {
        Ok(out) => {
            print!("{out}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("tool-list-cost: {error}");
            ExitCode::FAILURE
        }
    }
}
