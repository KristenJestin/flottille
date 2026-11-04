//! A development server that serves nothing, for the tests of the tasks that say when they are
//! ready and where they answer (`docs/projects.md`, "Tasks"): it prints the lines it is given,
//! then listens on a free port of `127.0.0.1` and answers every request with `204`, or never
//! does, and keeps running until it is stopped.
//!
//! ```text
//! flottille-fake-server [--wait <ms>] [--print <line>]... [--listen <port> | --exit <code>]
//! ```
//!
//! Each option is done in its order: `--wait` sleeps, `--print` prints a line. `--listen` listens
//! on that port (a test picks a free one) and prints `listening on http://127.0.0.1:<port>`;
//! without it, nothing listens. `--exit` ends with that code once the rest is done.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::time::Duration;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut listen = None;
    let mut exit = None;
    while let Some(arg) = args.next() {
        let mut value = || args.next().expect("an option without its value");
        match arg.as_str() {
            "--wait" => std::thread::sleep(Duration::from_millis(value().parse().unwrap())),
            "--print" => println!("{}", value()),
            "--listen" => listen = Some(value()),
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
    let listener = TcpListener::bind(format!("127.0.0.1:{port}")).unwrap();
    println!("listening on http://127.0.0.1:{port}");
    std::io::stdout().flush().unwrap();
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        // The request's head, up to its empty line.
        while reader.read_line(&mut line).is_ok_and(|read| read > 2) {
            line.clear();
        }
        let _ = stream.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n");
    }
}
