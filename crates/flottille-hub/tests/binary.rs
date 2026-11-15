//! The `flottille-hub` binary: its state folder, its log at a start, and `setup-code`, with a
//! temporary `HOME` and XDG folders.

use std::path::Path;
use std::process::{Command, Output};

use flottille_hub::{Hub, SystemClock};
use flottille_link::{DeviceKey, Keyring, KeyringError, MemberList};
use zeroize::Zeroizing;

/// A keyring that holds one device key, made from a fixed seed.
struct Seeded([u8; 32]);

impl Keyring for Seeded {
    fn read(
        &self,
        _attributes: &[(&str, &str)],
    ) -> Result<Option<Zeroizing<Vec<u8>>>, KeyringError> {
        Ok(Some(Zeroizing::new(self.0.to_vec())))
    }

    fn store(
        &self,
        _label: &str,
        _attributes: &[(&str, &str)],
        _secret: &[u8],
    ) -> Result<(), KeyringError> {
        unreachable!("the key is there")
    }
}

/// The binary, in a temporary home: `XDG_STATE_HOME` is `state` in it unless `xdg` is false.
fn hub(home: &Path, xdg: bool, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_flottille-hub"));
    command.args(args).env("HOME", home.join("home"));
    if xdg {
        command.env("XDG_STATE_HOME", home.join("state"));
    } else {
        command.env_remove("XDG_STATE_HOME");
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success() || !output.stderr.is_empty(),
        "a failure says why"
    );
    output
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[test]
fn the_binary_prints_the_setup_code_once_then_on_demand() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("hub");
    let state = state.to_str().unwrap();

    let first = hub(dir.path(), true, &["--state", state]);
    assert!(first.status.success(), "{}", text(&first.stderr));
    let log = text(&first.stderr);
    let lines: Vec<&str> = log
        .lines()
        .filter(|line| line.contains("setup code:"))
        .collect();
    assert_eq!(lines.len(), 1, "{log}");
    let code = lines[0]
        .strip_prefix("flottille-hub: setup code: ")
        .and_then(|rest| rest.strip_suffix(", valid until a first member joins"))
        .unwrap_or_else(|| panic!("{log}"))
        .to_owned();

    let second = hub(dir.path(), true, &["--state", state]);
    assert!(second.status.success());
    assert!(
        !text(&second.stderr).contains("setup code"),
        "{}",
        text(&second.stderr)
    );

    let again = hub(dir.path(), true, &["setup-code", "--state", state]);
    assert!(again.status.success(), "{}", text(&again.stderr));
    assert_eq!(text(&again.stdout), format!("{code}\n"));

    let reset = hub(
        dir.path(),
        true,
        &["--state", state, "setup-code", "--reset"],
    );
    assert!(reset.status.success(), "{}", text(&reset.stderr));
    let new = text(&reset.stdout).trim_end().to_owned();
    assert_ne!(new, code);
    assert_eq!(new.len(), code.len());
    let after = hub(dir.path(), true, &["--state", state, "setup-code"]);
    assert_eq!(text(&after.stdout), format!("{new}\n"));
}

#[test]
fn the_binary_refuses_to_print_the_code_once_a_member_exists() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("hub");
    let mut opened = Hub::open(&state, SystemClock).unwrap();
    let code = opened.setup_code().unwrap().to_owned();
    let pc = DeviceKey::load_or_make(&Seeded([1; 32]), Path::new("/nonexistent")).unwrap();
    let entry = MemberList::start("pc", &pc).changes()[0].clone();
    opened.setup(&code, &pc.public(), entry).unwrap();
    drop(opened);

    let refused = hub(
        dir.path(),
        true,
        &["--state", state.to_str().unwrap(), "setup-code"],
    );
    assert!(!refused.status.success());
    assert!(refused.stdout.is_empty());
    assert_eq!(
        text(&refused.stderr),
        "flottille-hub: the hub has a member already: a setup code serves only before the first \
         member joins (flottille-hub setup-code --reset empties the list and makes a new one)\n"
    );
}

#[test]
fn the_state_folder_is_under_xdg_state_home_by_default() {
    let dir = tempfile::tempdir().unwrap();
    assert!(hub(dir.path(), true, &[]).status.success());
    assert!(dir.path().join("state/flottille-hub/hub.key").is_file());
    assert!(dir.path().join("state/flottille-hub/hub.json").is_file());

    // Without XDG_STATE_HOME, the specification's fallback in the home.
    let other = tempfile::tempdir().unwrap();
    assert!(hub(other.path(), false, &[]).status.success());
    assert!(
        other
            .path()
            .join("home/.local/state/flottille-hub/hub.key")
            .is_file()
    );
}

#[test]
fn the_binary_refuses_its_state_folder_while_a_hub_uses_it() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("hub");
    let running = Hub::open(&state, SystemClock).unwrap();
    let code = running.setup_code().unwrap().to_owned();

    let state_arg = state.to_str().unwrap();
    for args in [
        &["--state", state_arg][..],
        &["--state", state_arg, "setup-code"],
        &["--state", state_arg, "setup-code", "--reset"],
    ] {
        let refused = hub(dir.path(), true, args);
        assert!(!refused.status.success(), "{args:?}");
        assert!(refused.stdout.is_empty(), "{args:?}");
        assert_eq!(
            text(&refused.stderr),
            format!(
                "flottille-hub: another flottille-hub uses {}: stop it first\n",
                state.display()
            )
        );
    }
    assert_eq!(running.setup_code().unwrap(), code);

    drop(running);
    let after = hub(dir.path(), true, &["--state", state_arg, "setup-code"]);
    assert_eq!(text(&after.stdout), format!("{code}\n"));
}
