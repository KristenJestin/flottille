//! The scripted sign-in every fake program shares (#1185, `docs/agents.md`, "Signing in"): the
//! fake `gh`, `glab`, `acli`, `bkt`, `claude` and `codex` each print their own program's words,
//! and all of them keep their state and their record the same way, in the fake's folder:
//!
//! - `sign-in-state`, written by a sign-in (`in`) or a sign-out (`out`): what the status says
//!   from then on; without it, the script's `signed_in` (`true` when it says nothing);
//! - `sign-in.jsonl`, one object per line: `{"login": [args], "pid"}` when a sign-in starts,
//!   `{"pasted": "<line>"}` for the code it read on its input, `{"after": "<text>"}` for whatever
//!   came on its input after that code until the input closed, `{"ended": "signed in" |
//!   "refused"}`, and `{"logout": [args]}`;
//! - `approve` and `refuse`, written by the test: a sign-in that waits for the user's browser ends
//!   signed in once `approve` is there, refused once `refuse` is (60 s at most).
//!
//! The script's `sign_in` shapes the flow: `{"address", "code", "asks_code", "accepts", "fail", "echo_refused", "stdout"}`.
//! With `fail`, the fake says that text on its error output and ends with 1 at once, as a program
//! that cannot sign in without a terminal does (#1529), after the `stdout` text on its output. The
//! address and the code are invented ones by default; with `asks_code: true` the fake reads the
//! code back on its input, as a program that asks for a pasted code does, and is signed in when
//! it is `accepts` (any code without).

use std::io::{BufRead as _, Read as _, Write as _};
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// The address a sign-in shows when the script names none.
pub const ADDRESS: &str = "https://login.example.test/device";
/// The one-time code it shows when the script names none.
pub const CODE: &str = "WDJB-MJHT";

/// Whether the fake is signed in now.
pub fn signed_in(dir: &Path, script: &Value) -> bool {
    match std::fs::read_to_string(dir.join("sign-in-state")) {
        Ok(state) => state.trim() == "in",
        Err(_) => script["signed_in"] != false,
    }
}

/// One line of the sign-in record, written whole.
pub fn record(dir: &Path, entry: Value) {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("sign-in.jsonl"))
        .expect("the sign-in record");
    file.write_all(format!("{entry}\n").as_bytes())
        .expect("the sign-in record");
}

/// A sign-in: `say` prints the program's own words with the address and the code, then the fake
/// waits for the user (a pasted code, or the browser's `approve`); its exit code.
pub fn login(dir: &Path, script: &Value, args: &[String], say: impl FnOnce(&str, &str)) -> i32 {
    let flow = &script["sign_in"];
    let address = flow["address"].as_str().unwrap_or(ADDRESS);
    let code = flow["code"].as_str().unwrap_or(CODE);
    record(dir, json!({"login": args, "pid": std::process::id()}));
    if let Some(failure) = flow["fail"].as_str() {
        // What a program prints on its output before it fails (`stdout`): the dialog's, never a
        // failure's.
        if let Some(said) = flow["stdout"].as_str() {
            println!("{said}");
            let _ = std::io::stdout().flush();
        }
        eprintln!("{failure}");
        record(dir, json!({"ended": "refused"}));
        return 1;
    }
    say(address, code);
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    let signed = if flow["asks_code"] == true {
        let stdin = std::io::stdin();
        let mut line = String::new();
        let _ = stdin.lock().read_line(&mut line);
        let pasted = line.trim_end_matches(['\n', '\r']).to_owned();
        record(dir, json!({"pasted": pasted}));
        // Whatever else comes before the input closes: a code written twice would be here.
        let mut rest = String::new();
        let _ = stdin.lock().read_to_string(&mut rest);
        if !rest.is_empty() {
            record(dir, json!({"after": rest}));
        }
        let accepted = flow["accepts"]
            .as_str()
            .is_none_or(|accepts| accepts == pasted);
        if !accepted && flow["echo_refused"] == true {
            // Some programs say the code they refused in their error.
            eprintln!("Error: the code {pasted} is not valid\nError: the sign-in was refused");
        }
        accepted
    } else {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if dir.join("approve").exists() {
                break true;
            }
            if dir.join("refuse").exists() || Instant::now() > deadline {
                break false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    if signed {
        std::fs::write(dir.join("sign-in-state"), "in").expect("the sign-in state");
        record(dir, json!({"ended": "signed in"}));
        0
    } else {
        record(dir, json!({"ended": "refused"}));
        1
    }
}

/// A sign-out: the state says signed out from now on.
pub fn logout(dir: &Path, args: &[String]) -> i32 {
    record(dir, json!({"logout": args}));
    std::fs::write(dir.join("sign-in-state"), "out").expect("the sign-in state");
    0
}
