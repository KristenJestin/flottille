//! The fake `pnpm` leaves its shell in a group of its own. Its being tested here is also what makes
//! `cargo test` build its binary, which the tests of the runs start.

use std::process::Command;

#[test]
fn the_shell_is_left_running_in_a_group_of_its_own() {
    let file = std::env::temp_dir().join(format!("flottille-fake-pnpm-{}", std::process::id()));

    let status = Command::new(env!("CARGO_BIN_EXE_flottille-fake-pnpm"))
        .arg(&file)
        .arg("sleep 5")
        .status()
        .unwrap();

    assert!(status.success());
    let pid: i32 = std::fs::read_to_string(&file).unwrap().parse().unwrap();
    std::fs::remove_file(&file).unwrap();
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    // After the name in parentheses: state, parent, then the process group.
    let group: i32 = stat
        .rsplit_once(") ")
        .unwrap()
        .1
        .split(' ')
        .nth(2)
        .unwrap()
        .parse()
        .unwrap();
    Command::new("kill").arg(pid.to_string()).status().unwrap();
    assert_eq!(group, pid, "the shell leads a group of its own");
}
