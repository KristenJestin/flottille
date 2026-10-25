//! `flottille agent chat`: a loop in the terminal on one root agent (`docs/cli.md`).

use std::io::{BufRead, IsTerminal, Write};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use flottille_core::paths::Paths;
use flottille_core::protocol::{AgentName, AgentSend};
use tokio::signal::unix::{SignalKind, signal};

use crate::agent_command::{place, text};
use crate::client::Client;
use crate::follow::{Turn, follow};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// How long a Ctrl+C keeps asking to cancel a turn that has not started to play yet.
const CANCEL_WAIT: Duration = Duration::from_secs(3);

pub fn run(paths: &Paths, agent: &str) -> Result<ExitCode> {
    let mut client = Client::connect_or_start(paths)?;
    let status = client.request(
        "agent.status",
        serde_json::to_value(AgentName {
            agent: agent.to_owned(),
            after: None,
        })?,
    )?;
    let shown = &status["agent"];
    let id = text(shown, "id").to_owned();
    if !shown["parent"].is_null() || matches!(text(shown, "state"), "ended" | "failed") {
        refuse(&mut client, &id)?;
    }
    let interrupt = Interrupt::install(paths.clone(), id.clone())?;

    println!(
        "Chat with agent {id}, {} in {}. Type /exit or press Ctrl+D to leave: the agent goes on \
         without you. Ctrl+C cancels the turn that runs.",
        text(shown, "project"),
        place(shown)
    );
    if status["needs_user"] == true {
        println!(
            "A request or a question of its tree waits for you: `{} approvals`, `{} inbox`.",
            crate::program::name(),
            crate::program::name()
        );
    }
    let stdin = std::io::stdin();
    let prompting = stdin.is_terminal();
    loop {
        if prompting {
            print!("> ");
            std::io::stdout().flush()?;
        }
        let mut line = String::new();
        if stdin.lock().read_line(&mut line)? == 0 {
            if prompting {
                println!();
            }
            return Ok(ExitCode::SUCCESS);
        }
        let message = match Typed::of(&line) {
            Typed::Exit => return Ok(ExitCode::SUCCESS),
            Typed::Nothing => continue,
            Typed::Message(message) => message,
        };
        interrupt.in_turn.store(true, Ordering::SeqCst);
        let turn = send_and_follow(paths, &mut client, &id, message);
        interrupt.in_turn.store(false, Ordering::SeqCst);
        let code = turn?;
        if code != ExitCode::SUCCESS {
            return Ok(code);
        }
    }
}

fn send_and_follow(
    paths: &Paths,
    client: &mut Client,
    agent: &str,
    message: String,
) -> Result<ExitCode> {
    let sent = client.request(
        "agent.send",
        serde_json::to_value(AgentSend {
            agent: agent.to_owned(),
            message,
        })?,
    )?;
    follow(
        paths,
        client,
        agent,
        sent["after"].as_u64(),
        Turn::Sent(text(&sent, "delivery").to_owned()),
        false,
    )
}

/// What a line typed means. Only `/exit` is a command of the chat: any other line, one that
/// starts with a slash too, is the user's message.
#[derive(Debug, PartialEq)]
enum Typed {
    Exit,
    Nothing,
    Message(String),
}

impl Typed {
    fn of(line: &str) -> Typed {
        let line = line.trim_end_matches(['\r', '\n']);
        match line.trim() {
            "/exit" => Typed::Exit,
            "" => Typed::Nothing,
            _ => Typed::Message(line.to_owned()),
        }
    }
}

/// Fails with the daemon's own sentence for an agent no one can write to: a child (it names its
/// root) or one that ended. An empty message is refused whatever the agent is, after those
/// checks, so asking with one only fetches the sentence and delivers nothing.
fn refuse(client: &mut Client, agent: &str) -> Result<()> {
    match client.request(
        "agent.send",
        serde_json::to_value(AgentSend {
            agent: agent.to_owned(),
            message: String::new(),
        })?,
    ) {
        Err(error) => Err(error),
        Ok(_) => Err(format!("agent {agent} cannot be written to").into()),
    }
}

/// Ctrl+C. A thread of its own listens for it: during a turn it asks the daemon to cancel the
/// turn and the chat goes on; at the prompt it leaves, the agent untouched.
struct Interrupt {
    in_turn: Arc<AtomicBool>,
}

impl Interrupt {
    /// Returns once the thread listens, so that no Ctrl+C after it is missed.
    fn install(paths: Paths, agent: String) -> Result<Interrupt> {
        let in_turn = Arc::new(AtomicBool::new(false));
        let during_turn = in_turn.clone();
        let (ready, listening) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let listen = || -> std::io::Result<_> {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                let signals = {
                    let _entered = runtime.enter();
                    signal(SignalKind::interrupt())?
                };
                Ok((runtime, signals))
            };
            let (runtime, mut signals) = match listen() {
                Ok(listening) => {
                    let _ = ready.send(Ok(()));
                    listening
                }
                Err(error) => {
                    let _ = ready.send(Err(error));
                    return;
                }
            };
            runtime.block_on(async {
                while signals.recv().await.is_some() {
                    if during_turn.load(Ordering::SeqCst) {
                        cancel(&paths, &agent, &during_turn);
                    } else {
                        // Not the standard output's lock: the chat may hold it.
                        eprintln!();
                        std::process::exit(0);
                    }
                }
            });
        });
        listening.recv()??;
        Ok(Interrupt { in_turn })
    }
}

/// Asks the daemon to cancel the turn. A turn that has not started to play yet (the message is
/// queued) is not cancelled by the first ask: it is asked again while the chat waits for it.
fn cancel(paths: &Paths, agent: &str, in_turn: &AtomicBool) {
    let deadline = Instant::now() + CANCEL_WAIT;
    while in_turn.load(Ordering::SeqCst) && Instant::now() < deadline {
        let asked = Client::connect(paths)
            .ok()
            .flatten()
            .map(|mut client| client.request("agent.cancel", serde_json::json!({"agent": agent})));
        match asked {
            Some(Ok(answer)) if answer["cancelled"] == true => return,
            Some(Err(error)) => {
                eprintln!("{}: {error}", crate::program::name());
                return;
            }
            _ => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_exit_is_a_command_and_any_other_line_is_a_message() {
        assert_eq!(Typed::of("/exit\n"), Typed::Exit);
        assert_eq!(Typed::of("  /exit  \n"), Typed::Exit);
        assert_eq!(Typed::of("\n"), Typed::Nothing);
        assert_eq!(Typed::of("   \r\n"), Typed::Nothing);
        assert_eq!(Typed::of("/exit now\n"), Typed::Message("/exit now".into()));
        assert_eq!(
            Typed::of("/etc/hosts looks odd\n"),
            Typed::Message("/etc/hosts looks odd".into())
        );
        assert_eq!(Typed::of("/help\n"), Typed::Message("/help".into()));
    }
}
