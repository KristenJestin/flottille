//! The fake development server does what its options say. Its being tested here is also what makes
//! `cargo test` build its binary, which the tests of the tasks that say when they are ready run.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
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
fn it_answers_every_request_on_the_port_it_takes() {
    // Port 0: the server takes a free port itself and says which, so that no other test can take
    // it between a check and the server's bind.
    let mut child = server()
        .args(["--listen", "0"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut announced = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut announced)
        .unwrap();
    let port: u16 = announced
        .trim_end()
        .strip_prefix("listening on http://127.0.0.1:")
        .and_then(|port| port.parse().ok())
        .unwrap_or_else(|| panic!("no port announced: {announced:?}"));
    assert_ne!(port, 0);

    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    child.kill().unwrap();
    child.wait().unwrap();

    assert!(
        response.starts_with("HTTP/1.1 204 No Content"),
        "{response}"
    );
}
