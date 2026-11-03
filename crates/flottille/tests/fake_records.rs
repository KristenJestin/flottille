//! The records of the fake `gh`, `bkt` and `acli` (#325). The checks of a project's programs run
//! at once (`join_all` over program, host and scope), so several fake processes append to the same
//! `received.jsonl` together: each record has to land as one whole line, never mixed with another.

mod common;

use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::Value;
use tempfile::TempDir;

/// How many processes of one fake start together.
const PROCESSES: usize = 16;

/// The longest argument a process sends, so that a record is not a few bytes the kernel could not
/// split.
const PADDING: usize = 4096;

/// Starts `PROCESSES` of `program` at once on `dir`, each with an argument of its own, and waits
/// for all of them.
fn run_together(program: &Path, dir_variable: &str, dir: &Path) {
    let children: Vec<_> = (0..PROCESSES)
        .map(|index| {
            Command::new(program)
                .arg(format!("call-{index}-{}", "x".repeat(PADDING)))
                .env(dir_variable, dir)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap()
        })
        .collect();
    for mut child in children {
        child.wait().unwrap();
    }
}

/// Every line of `dir/received.jsonl`, parsed: one that is not JSON fails the test.
fn lines_of(dir: &Path) -> Vec<Value> {
    let text = std::fs::read_to_string(dir.join("received.jsonl")).unwrap();
    text.lines()
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("a torn record ({error}): {line:.80}"))
        })
        .collect()
}

#[test]
fn concurrent_fake_programs_write_whole_records() {
    let fakes = [
        ("gh", common::fake_gh(), "FLOTTILLE_FAKE_GH_DIR"),
        ("bkt", common::fake_bkt(), "FLOTTILLE_FAKE_BKT_DIR"),
        ("acli", common::fake_acli(), "FLOTTILLE_FAKE_ACLI_DIR"),
    ];
    for (name, program, dir_variable) in fakes {
        let dir = TempDir::new().unwrap();
        run_together(&program, dir_variable, dir.path());

        let records = lines_of(dir.path());
        assert_eq!(records.len(), PROCESSES, "the records of the fake {name}");
        let mut calls: Vec<usize> = records
            .iter()
            .map(|record| {
                let call = record["args"][0].as_str().unwrap();
                call.strip_prefix("call-")
                    .and_then(|rest| rest.split('-').next())
                    .and_then(|index| index.parse().ok())
                    .unwrap()
            })
            .collect();
        calls.sort_unstable();
        assert_eq!(
            calls,
            (0..PROCESSES).collect::<Vec<_>>(),
            "one record per call of the fake {name}"
        );
    }
}
