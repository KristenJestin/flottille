//! Throwaway probe of #1101 (reverted before the pull request ends): a test that fails on its first
//! attempt only. nextest runs each attempt in its own process: the marker in the temporary folder
//! is what the second attempt sees.

use std::fs;

#[test]
fn flaky_probe_fails_on_its_first_attempt_only() {
    let run = std::env::var("GITHUB_RUN_ID").unwrap_or_else(|_| "local".into());
    let marker = std::env::temp_dir().join(format!("flottille-flaky-probe-{run}"));
    if marker.exists() {
        return;
    }
    fs::write(&marker, b"seen").expect("the marker is written");
    panic!("the first attempt of the probe fails on purpose");
}
