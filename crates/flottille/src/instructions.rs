//! The user's own instructions for every agent in every project (`docs/agents.md`, "Roles"):
//! `instructions.md` in Flottille's configuration folder, added to each agent's instructions
//! after the cache boundary and before the project's own files. Bare mode keeps the providers'
//! own global files out; this one is Flottille's, and it never reads theirs.
//!
//! The text is read when an agent is created and frozen in its instructions, so a change reaches
//! the agents created after it. It is not the rules (`rules.md`, `docs/projects.md`): those
//! replace Flottille's default text and a project's own replace them, while these are added to
//! whatever an agent is told.

use flottille_core::paths::Paths;
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::projects::Failure;
use crate::rules::LIMIT;

/// The text, cut to the limit when a hand edit went past it, and why it was cut.
pub struct Instructions {
    pub text: String,
    pub cut: Option<String>,
}

/// The user's instructions; empty while the file is absent. A file that cannot be read is no
/// instructions, said in the daemon's log: no agent is kept from starting.
pub fn effective(paths: &Paths) -> Instructions {
    match read(paths) {
        Ok(instructions) => {
            if let Some(cut) = &instructions.cut {
                eprintln!("flottille daemon: instructions: {cut}");
            }
            instructions
        }
        Err(failure) => {
            eprintln!("flottille daemon: instructions: {}", failure.message);
            Instructions {
                text: String::new(),
                cut: None,
            }
        }
    }
}

/// `effective`, with a file that cannot be read said to the caller.
fn read(paths: &Paths) -> Result<Instructions, Failure> {
    let file = paths.instructions();
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(Failure::new(
                "unavailable",
                format!("{}: {error}", file.display()),
            ));
        }
    };
    let count = text.chars().count();
    if count <= LIMIT {
        return Ok(Instructions { text, cut: None });
    }
    let cut = format!(
        "your instructions for all projects ({}) are {count} characters long and may hold \
         {LIMIT}: agents get the first {LIMIT} only, since they ride in every agent's \
         instructions; shorten them",
        file.display()
    );
    Ok(Instructions {
        text: text.chars().take(LIMIT).collect(),
        cut: Some(cut),
    })
}

/// What is wrong with a text offered as the instructions.
fn check(text: &str) -> Result<(), String> {
    let count = text.chars().count();
    if count > LIMIT {
        return Err(format!(
            "the instructions are {count} characters long and may hold {LIMIT}: they ride in \
             every agent's instructions, so keep what matters"
        ));
    }
    Ok(())
}

/// Answers an `instructions.*` request of the user: `instructions.show` and `instructions.set`.
/// No tool of an agent reaches them.
pub fn call(daemon: &Daemon, method: &str, params: Value) -> Result<Value, Failure> {
    match method {
        "instructions.show" => {
            serde_json::from_value::<Empty>(params)?;
            show(daemon)
        }
        "instructions.set" => {
            let Set { text } = serde_json::from_value(params)?;
            check(&text).map_err(Failure::refused)?;
            let file = daemon.paths.instructions();
            if text.is_empty() {
                match std::fs::remove_file(&file) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                        return Err(Failure::new("unavailable", format!("instructions.md: {e}")));
                    }
                    _ => {}
                }
            } else {
                std::fs::create_dir_all(&daemon.paths.config).map_err(|e| {
                    Failure::new(
                        "unavailable",
                        format!("{}: {e}", daemon.paths.config.display()),
                    )
                })?;
                crate::providers::write_in_place(&file, &text)
                    .map_err(|e| Failure::new("unavailable", format!("instructions.md: {e}")))?;
            }
            daemon
                .emit("instructions.changed", json!({}))
                .map_err(|error| Failure::new("unavailable", error.to_string()))?;
            show(daemon)
        }
        method => Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        )),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Set {
    text: String,
}

/// `{"path", "text", "cut"}`: the file, the text agents created from now on get, and why it was
/// cut to the limit (`null` when it was not).
fn show(daemon: &Daemon) -> Result<Value, Failure> {
    let Instructions { text, cut } = read(&daemon.paths)?;
    Ok(json!({"path": daemon.paths.instructions(), "text": text, "cut": cut}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instructions_longer_than_the_limit_are_refused_with_the_limit() {
        assert!(check(&"é".repeat(LIMIT)).is_ok());
        let why = check(&"é".repeat(LIMIT + 1)).unwrap_err();
        assert!(why.contains("8000"), "{why}");
    }
}
