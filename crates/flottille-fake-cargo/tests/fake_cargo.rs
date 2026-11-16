//! The fake `cargo` prints recorded outputs and runs nothing. Its being tested here is also what
//! makes `cargo test` build its binary, which the tests of the filters start.

use std::process::Command;

fn cargo(args: &[&str]) -> (String, Option<i32>) {
    let out = Command::new(env!("CARGO_BIN_EXE_flottille-fake-cargo"))
        .args(args)
        .output()
        .unwrap();
    (String::from_utf8(out.stdout).unwrap(), out.status.code())
}

#[test]
fn a_test_run_fails_once_with_its_progress_and_its_passing_tests() {
    let (text, code) = cargo(&["test"]);

    assert_eq!(code, Some(101));
    assert!(text.contains("Compiling app"), "{text}");
    assert!(text.contains("reads_a_number ... ok"), "{text}");
    assert!(text.contains("adds_two_numbers ... FAILED"), "{text}");
}

#[test]
fn a_green_test_run_and_a_build_work() {
    assert_eq!(cargo(&["test", "--green"]).1, Some(0));
    let (text, code) = cargo(&["build"]);
    assert_eq!(code, Some(0));
    assert!(text.contains("Finished"), "{text}");
}

#[test]
fn another_subcommand_is_refused() {
    assert_eq!(cargo(&["publish"]).1, Some(1));
}
