//! The filters of a command's output (`docs/tools.md`, "Filtered command output"): what a
//! terminal hides from a person and an agent should not read. Each filter is a TOML file of
//! `crates/flottille-core/filters/`, built into the binary, that holds its own tests.

use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;

use crate::ansi;

/// What a filter made of an output.
#[derive(Debug, PartialEq, Eq)]
pub struct Filtered {
    pub text: String,
    /// How many lines of the output are not in `text`.
    pub removed: usize,
}

/// A test a filter's file holds: read by the tests of this module, never by the daemon.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(not(test), allow(dead_code))]
struct Case {
    name: String,
    input: String,
    expected: String,
    removed: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Spec {
    name: String,
    program: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    drop: Vec<String>,
    max_lines: Option<usize>,
    on_empty: Option<String>,
    #[serde(default)]
    #[cfg_attr(not(test), allow(dead_code))]
    tests: Vec<Case>,
}

const SOURCES: [(&str, &str); 9] = [
    ("generic", include_str!("../filters/generic.toml")),
    ("cargo-build", include_str!("../filters/cargo-build.toml")),
    ("cargo-test", include_str!("../filters/cargo-test.toml")),
    ("git-transfer", include_str!("../filters/git-transfer.toml")),
    ("go-test", include_str!("../filters/go-test.toml")),
    ("npm-install", include_str!("../filters/npm-install.toml")),
    ("pnpm-install", include_str!("../filters/pnpm-install.toml")),
    ("pytest", include_str!("../filters/pytest.toml")),
    ("yarn-install", include_str!("../filters/yarn-install.toml")),
];

/// A line that says what went wrong, or where: a filter never removes one (`docs/tools.md`,
/// "Filtered command output"). Broad on purpose: a line that only looks like one stays.
fn protected(line: &str) -> bool {
    static PROTECTED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)error|warning|failed|panicked|[\w./-]+\.\w+:\d+").expect("a pattern")
    });
    PROTECTED.is_match(line)
}

struct Filter {
    program: Option<String>,
    args: Vec<String>,
    drop: Vec<Regex>,
    max_lines: Option<usize>,
    on_empty: Option<String>,
}

impl Filter {
    fn new(spec: &Spec) -> Result<Filter, String> {
        let drop = spec
            .drop
            .iter()
            .map(|pattern| Regex::new(pattern).map_err(|e| format!("{}: {e}", spec.name)))
            .collect::<Result<_, _>>()?;
        Ok(Filter {
            program: spec.program.clone(),
            args: spec.args.clone(),
            drop,
            max_lines: spec.max_lines,
            on_empty: spec.on_empty.clone(),
        })
    }

    fn matches(&self, program: &str, first: Option<&str>) -> bool {
        self.program.as_deref() == Some(program)
            && (self.args.is_empty() || first.is_some_and(|arg| self.args.iter().any(|a| a == arg)))
    }

    fn run(&self, output: &str) -> Filtered {
        let text = ansi::strip(output);
        let mut removed = 0;
        let mut lines: Vec<&str> = if text.trim().is_empty() {
            Vec::new()
        } else {
            text.trim_end().split('\n').collect()
        };
        // The progress of a tool, never a line that says what went wrong.
        let before = lines.len();
        lines.retain(|line| protected(line) || !self.drop.iter().any(|d| d.is_match(line)));
        removed += before - lines.len();
        // Blank lines: none at the start, one at most in a row.
        let mut kept: Vec<String> = Vec::new();
        let mut blank = true;
        for line in lines {
            if line.trim().is_empty() {
                if blank {
                    removed += 1;
                    continue;
                }
                blank = true;
            } else {
                blank = false;
            }
            kept.push(line.to_owned());
        }
        // A run of three identical lines or more is one line and its count.
        let mut lines: Vec<String> = Vec::new();
        let mut at = 0;
        while at < kept.len() {
            let run = kept[at..]
                .iter()
                .take_while(|line| **line == kept[at] && !line.trim().is_empty())
                .count();
            if run >= 3 {
                lines.push(format!("{} (×{run})", kept[at]));
                removed += run - 1;
                at += run;
            } else {
                lines.push(kept[at].clone());
                at += 1;
            }
        }
        if let Some(max) = self.max_lines
            && lines.len() > max
        {
            let (head, tail) = (max.div_ceil(2), max / 2);
            let middle = head..lines.len() - tail;
            let mut shortened = Vec::new();
            let mut left_out = 0;
            for (n, line) in lines.drain(..).enumerate() {
                if middle.contains(&n) && !protected(&line) {
                    left_out += 1;
                    continue;
                }
                if left_out > 0 {
                    shortened.push(format!("[… {left_out} lines left out …]"));
                    removed += left_out;
                    left_out = 0;
                }
                shortened.push(line);
            }
            lines = shortened;
        }
        if lines.iter().all(|line| line.trim().is_empty()) {
            lines.clear();
            if removed > 0
                && let Some(said) = &self.on_empty
            {
                lines.push(said.clone());
            }
        }
        Filtered {
            text: lines.join("\n"),
            removed,
        }
    }
}

static FILTERS: LazyLock<Vec<Filter>> = LazyLock::new(|| {
    SOURCES
        .iter()
        .filter_map(|(_, source)| Filter::new(&toml::from_str(source).ok()?).ok())
        .collect()
});

/// The output of the command `words` (its program, then its arguments, never a shell line),
/// filtered by the filter of that program and its first argument, or by the generic one.
pub fn apply(words: &[String], output: &str) -> Filtered {
    let program = words.first().map(|word| word.rsplit('/').next().unwrap_or(word));
    // `cargo +nightly test`: the toolchain is not the subcommand.
    let first = words.iter().skip(1).find(|word| !word.starts_with('+'));
    let own = program.and_then(|program| {
        FILTERS
            .iter()
            .find(|filter| filter.matches(program, first.map(String::as_str)))
    });
    let filter = own.or_else(|| FILTERS.iter().find(|filter| filter.program.is_none()));
    match filter {
        Some(filter) => filter.run(output),
        None => Filtered {
            text: output.to_owned(),
            removed: 0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(file: &str) -> Spec {
        let (_, source) = SOURCES.iter().find(|(name, _)| *name == file).unwrap();
        toml::from_str(source).unwrap_or_else(|e| panic!("{file}.toml: {e}"))
    }

    fn words(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_owned).collect()
    }

    /// The tests of a file, each run on the words the filter matches.
    fn run_file(file: &str) {
        let spec = spec(file);
        assert!(!spec.tests.is_empty(), "{file}.toml has no test");
        let line = match (&spec.program, spec.args.first()) {
            (Some(program), Some(arg)) => format!("{program} {arg}"),
            (Some(program), None) => program.clone(),
            (None, _) => "some-program".to_owned(),
        };
        for case in &spec.tests {
            let found = apply(&words(&line), &case.input);
            assert_eq!(
                found.text.trim_end_matches('\n'),
                case.expected,
                "{file}.toml: {}",
                case.name
            );
            if let Some(removed) = case.removed {
                assert_eq!(found.removed, removed, "{file}.toml: {}", case.name);
            }
        }
    }

    fn every_filter() -> Vec<(&'static str, Spec)> {
        SOURCES
            .iter()
            .map(|(file, _)| (*file, spec(file)))
            .collect()
    }

    #[test]
    fn every_file_of_the_folder_is_built_in() {
        let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("filters");
        let mut files: Vec<String> = std::fs::read_dir(folder)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        files.sort();
        let mut built: Vec<String> = SOURCES.iter().map(|(file, _)| format!("{file}.toml")).collect();
        built.sort();
        assert_eq!(files, built);
        assert_eq!(FILTERS.len(), SOURCES.len(), "a file does not load");
    }

    #[test]
    fn an_error_line_is_never_filtered_out() {
        let says = [
            " error: it broke",
            " Warning: careful",
            " FAILED",
            " panicked at the end",
            " src/lib.rs:12:5",
        ];
        for (file, spec) in every_filter() {
            let mut filter = Filter::new(&spec).unwrap();
            // The head and the tail only, so that the middle is cut too.
            filter.max_lines = Some(2);
            for case in &spec.tests {
                let clean = ansi::strip(&case.input);
                let lines: Vec<&str> = clean.lines().collect();
                for (at, line) in lines.iter().enumerate() {
                    for said in says {
                        let wanted = format!("{line}{said}");
                        let mut changed = lines.clone();
                        changed[at] = &wanted;
                        let found = filter.run(&changed.join("\n")).text;
                        assert!(
                            found.contains(&wanted),
                            "{file}.toml ({}): {wanted:?} was filtered out of {found}",
                            case.name
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_cargo_test_run_keeps_its_failures_and_summary() {
        let spec = spec("cargo-test");
        let found = apply(&words("cargo test -p app"), &spec.tests[0].input).text;

        for kept in [
            "test total::adds_two_numbers ... FAILED",
            "thread 'total::adds_two_numbers' panicked at src/total.rs:18:9:",
            "assertion `left == right` failed",
            "left: 4",
            "test result: FAILED. 4 passed; 1 failed",
            "error: test failed, to rerun pass `-p app --lib`",
        ] {
            assert!(found.contains(kept), "{kept} is missing from {found}");
        }
        assert!(!found.contains("Compiling"), "{found}");
        assert!(!found.contains("reads_a_number"), "{found}");
    }

    #[test]
    fn a_command_is_matched_by_its_program_and_its_first_argument() {
        let noise = "   Compiling app v1\nready\n";
        let said = |line: &str| apply(&words(line), noise).text;

        assert_eq!(said("cargo build"), "ready");
        assert_eq!(said("/usr/bin/cargo build --release"), "ready");
        assert_eq!(said("cargo +nightly check"), "ready");
        // Another subcommand, another program and no word at all: the generic filter's.
        assert_eq!(said("cargo run"), noise.trim_end());
        assert_eq!(said("make build"), noise.trim_end());
        assert_eq!(apply(&[], noise).text, noise.trim_end());
    }

    #[test]
    fn the_generic_filter_never_touches_what_a_person_would_read() {
        let output = "one\ntwo\ntwo\nthree\n";

        assert_eq!(
            apply(&words("make"), output),
            Filtered {
                text: "one\ntwo\ntwo\nthree".into(),
                removed: 0
            }
        );
    }

    #[test]
    fn a_long_output_keeps_its_head_and_its_tail_and_every_line_that_matters() {
        let spec: Spec = toml::from_str("name = \"x\"\nmax_lines = 6\n").unwrap();
        let filter = Filter::new(&spec).unwrap();
        let mut lines: Vec<String> = (1..=20).map(|n| format!("line {n}")).collect();
        lines[9] = "src/main.rs:10: where it broke".into();

        let found = filter.run(&lines.join("\n"));

        assert_eq!(
            found.text,
            "line 1\nline 2\nline 3\n[… 6 lines left out …]\nsrc/main.rs:10: where it broke\n\
             [… 7 lines left out …]\nline 18\nline 19\nline 20"
        );
        assert_eq!(found.removed, 13);
    }

    #[test]
    fn an_output_left_empty_says_what_the_filter_says_and_one_that_was_empty_says_nothing() {
        let said = |line: &str, output: &str| apply(&words(line), output);

        assert_eq!(said("cargo build", "   Compiling a v1\n").text, "cargo: ok");
        assert_eq!(said("cargo build", ""), Filtered { text: String::new(), removed: 0 });
        assert_eq!(said("make", "\n\n").text, "");
    }

    #[test]
    fn a_run_of_blank_lines_is_one_and_the_start_has_none() {
        let found = apply(&words("make"), "\n\na\n\n\n\nb\n");

        assert_eq!(found, Filtered { text: "a\n\nb".into(), removed: 4 });
    }

    macro_rules! file_test {
        ($($name:ident: $file:literal),* $(,)?) => {
            $(#[test] fn $name() { run_file($file); })*
        };
    }

    file_test! {
        the_generic_filter_passes_its_tests: "generic",
        the_cargo_build_filter_passes_its_tests: "cargo-build",
        the_cargo_test_filter_passes_its_tests: "cargo-test",
        the_git_transfer_filter_passes_its_tests: "git-transfer",
        the_go_test_filter_passes_its_tests: "go-test",
        the_npm_install_filter_passes_its_tests: "npm-install",
        the_pnpm_install_filter_passes_its_tests: "pnpm-install",
        the_pytest_filter_passes_its_tests: "pytest",
        the_yarn_install_filter_passes_its_tests: "yarn-install",
    }
}
