//! A `cargo` that prints a recorded output and runs nothing, for the tests of the filters of a
//! command's output (`docs/tools.md`, "Filtered command output"): no test ever runs the real one.
//!
//! | Call | Prints | Exit |
//! |---|---|---|
//! | `cargo test` | a run of the tests of a library with one failure, its progress and its passing tests | 101 |
//! | `cargo test --green` | the same run, every test passing | 0 |
//! | `cargo build` | the progress of a build that works | 0 |
//! | `cargo build --many` | 300 lines of progress, then `Finished` | 0 |
//! | `cargo build --fail` | the progress of a build, nothing that says why | 1 |
//! | `cargo build --hang` | one line of progress, then it waits until it is stopped | none |
//! | anything else | `error: no such subcommand` | 1 |

use std::process::exit;

const TEST_HEAD: &str = "\
   Compiling serde v1.0.219
   Compiling app v0.3.1 (/work/app)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 6.31s
     Running unittests src/lib.rs (target/debug/deps/app-3f2a9c1d7b)

running 5 tests
test parse::reads_a_number ... ok
test parse::reads_a_sign ... ok
test parse::rejects_a_letter ... ok
";

const TEST_FAILING: &str = "\
test total::adds_two_numbers ... FAILED
test total::adds_nothing ... ok

failures:

---- total::adds_two_numbers stdout ----
thread 'total::adds_two_numbers' panicked at src/total.rs:18:9:
assertion `left == right` failed
  left: 4
 right: 5
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    total::adds_two_numbers

test result: FAILED. 4 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

error: test failed, to rerun pass `-p app --lib`
";

const TEST_GREEN: &str = "\
test total::adds_two_numbers ... ok
test total::adds_nothing ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
";

const BUILD: &str = "\
    Updating crates.io index
  Downloaded serde v1.0.219
   Compiling serde v1.0.219
   Compiling app v0.3.1 (/work/app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 14.20s
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (text, code) = match args.first().map(String::as_str) {
        Some("test") if args.iter().any(|a| a == "--green") => {
            (format!("{TEST_HEAD}{TEST_GREEN}"), 0)
        }
        Some("test") => (format!("{TEST_HEAD}{TEST_FAILING}"), 101),
        Some("build") if args.iter().any(|a| a == "--many") => {
            let progress: String = (1..=300)
                .map(|n| format!("   Compiling crate-{n} v1.0.{n}\n"))
                .collect();
            (
                format!("{progress}    Finished `dev` profile in 99.00s\n"),
                0,
            )
        }
        Some("build") if args.iter().any(|a| a == "--fail") => (BUILD.to_owned(), 1),
        Some("build") if args.iter().any(|a| a == "--hang") => {
            println!("   Compiling app v0.3.1 (/work/app)");
            let _ = std::io::Write::flush(&mut std::io::stdout());
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        }
        Some("build") => (BUILD.to_owned(), 0),
        _ => ("error: no such subcommand\n".to_owned(), 1),
    };
    print!("{text}");
    exit(code);
}
