//! A development server that serves nothing, for the tests of the tasks that say when they are
//! ready and where they answer (`docs/projects.md`, "Tasks"): it prints the lines it is given,
//! then listens on a free port of `127.0.0.1` and answers every request with `204`, or never
//! does, and keeps running until it is stopped.
//!
//! ```text
//! flottille-fake-server [--child] [--wait <ms>] [--print <line>]... [--host <address>] [--serve <n>]
//!                       [--listen <port> | --exit <code>]
//! ```
//!
//! `--child`, first, starts a copy of itself with the other options, in its own session (what a
//! package manager's script does), prints `leader <pid>` and `child <pid>` and waits forever.
//!
//! Each option is done in its order: `--wait` sleeps, `--print` prints a line. `--listen` listens
//! on that port (a test picks a free one) and prints `listening on http://127.0.0.1:<port>`;
//! without it, nothing listens. `--host` is the address it listens on (`127.0.0.1` when it is not
//! given; `::1` listens on the IPv6 loopback). `--serve` closes the listener, the process staying,
//! once that many requests are answered. `--exit` ends with that code once the rest is done.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::time::Duration;

fn main() {
    let mut args = std::env::args().skip(1).peekable();
    if args.peek().map(String::as_str) == Some("--child") {
        args.next();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(args)
            .spawn()
            .unwrap();
        println!("leader {}", std::process::id());
        println!("child {}", child.id());
        std::io::stdout().flush().unwrap();
        // It stays, whether its child does or not, as a package manager waiting on its script.
        let _ = child.wait();
        loop {
            std::thread::sleep(Duration::from_secs(60));
        }
    }
    let mut listen = None;
    let mut exit = None;
    let mut host = "127.0.0.1".to_owned();
    let mut serve = None;
    while let Some(arg) = args.next() {
        let mut value = || args.next().expect("an option without its value");
        match arg.as_str() {
            "--wait" => std::thread::sleep(Duration::from_millis(value().parse().unwrap())),
            "--print" => println!("{}", value()),
            "--listen" => listen = Some(value()),
            "--host" => host = value(),
            "--serve" => serve = Some(value().parse::<u64>().unwrap()),
            "--exit" => exit = Some(value().parse::<i32>().unwrap()),
            other => panic!("unknown option {other}"),
        }
    }
    std::io::stdout().flush().unwrap();
    if let Some(code) = exit {
        std::process::exit(code);
    }
    let Some(port) = listen else {
        loop {
            std::thread::sleep(Duration::from_secs(60));
        }
    };
    let at = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.clone()
    };
    let listener = TcpListener::bind(format!("{at}:{port}")).unwrap();
    // The port it took: port 0 asks the system for a free one, which no other test can take first.
    let port = listener.local_addr().unwrap().port();
    println!("listening on http://{at}:{port}");
    std::io::stdout().flush().unwrap();
    let mut answered = 0;
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        // The request's head, up to its empty line.
        while reader.read_line(&mut line).is_ok_and(|read| read > 2) {
            line.clear();
        }
        let _ = stream.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n");
        answered += 1;
        if serve == Some(answered) {
            break;
        }
    }
    drop(listener);
    loop {
        std::thread::sleep(Duration::from_secs(60));
    }
}
