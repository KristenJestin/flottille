//! The fake development server does what its options say. Its being tested here is also what makes
//! `cargo test` build its binary, which the tests of the tasks that say when they are ready run.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};

fn server() -> Command {
    Command::new(env!("CARGO_BIN_EXE_flottille-fake-server"))
}

#[test]
fn it_prints_its_lines_then_exits_with_its_code() {
    let output = server()
        .args(["--print", "booting", "--exit", "3"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3));
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "booting\n");
}

#[test]
fn it_answers_every_request_on_the_port_it_is_given() {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut child = server()
        .args(["--listen", &port.to_string()])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut announced = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut announced)
        .unwrap();

    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    child.kill().unwrap();
    child.wait().unwrap();

    assert_eq!(
        announced.trim_end(),
        format!("listening on http://127.0.0.1:{port}")
    );
    assert!(
        response.starts_with("HTTP/1.1 204 No Content"),
        "{response}"
    );
}
