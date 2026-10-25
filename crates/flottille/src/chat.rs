//! `flottille agent chat`: a loop in the terminal on one root agent (`docs/cli.md`).

use std::fs::File;
use std::io::{BufRead, IsTerminal, Read, Write};
use std::os::fd::FromRawFd;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::{Duration, Instant};

use flottille_core::paths::Paths;
use flottille_core::protocol::{AgentName, AgentSend};

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
        Turn::Sent,
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

/// Ctrl+C. A signal handler can do almost nothing, so it only writes a byte to a pipe; a thread
/// reads the pipe. During a turn it asks the daemon to cancel it, and the chat goes on; at the
/// prompt it leaves, the agent untouched.
struct Interrupt {
    in_turn: Arc<AtomicBool>,
}

/// The write end of the pipe the handler writes to.
static WAKE: AtomicI32 = AtomicI32::new(-1);

extern "C" fn on_sigint(_: libc::c_int) {
    // SAFETY: write(2) is async-signal-safe; the byte's value does not matter. errno is kept
    // for the code the signal interrupted.
    unsafe {
        let errno = *libc::__errno_location();
        libc::write(WAKE.load(Ordering::Relaxed), b"!".as_ptr().cast(), 1);
        *libc::__errno_location() = errno;
    }
}

impl Interrupt {
    fn install(paths: Paths, agent: String) -> Result<Interrupt> {
        let mut ends = [0; 2];
        // SAFETY: `ends` holds the two descriptors pipe2 fills in.
        if unsafe { libc::pipe2(ends.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        WAKE.store(ends[1], Ordering::SeqCst);
        // SAFETY: a zeroed sigaction is valid; the handler only calls async-signal-safe functions.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = on_sigint as extern "C" fn(libc::c_int) as libc::sighandler_t;
            libc::sigemptyset(&mut action.sa_mask);
            if libc::sigaction(libc::SIGINT, &action, std::ptr::null_mut()) != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        // SAFETY: the read end is ours alone, and nothing else closes it.
        let mut wake = unsafe { File::from_raw_fd(ends[0]) };
        let in_turn = Arc::new(AtomicBool::new(false));
        let during_turn = in_turn.clone();
        std::thread::spawn(move || {
            let mut byte = [0];
            while wake.read(&mut byte).is_ok_and(|read| read > 0) {
                if during_turn.load(Ordering::SeqCst) {
                    cancel(&paths, &agent, &during_turn);
                } else {
                    // Not the standard output's lock: the chat may hold it.
                    eprintln!();
                    std::process::exit(0);
                }
            }
        });
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
