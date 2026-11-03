//! The end of the log of each failed job of a pull request, for `forge show` (`docs/forges.md`,
//! "Why a check failed"): an agent that is told "failing" can do nothing, one that reads the last
//! lines of the job's log can often fix it. Bounded in lines and in bytes, per job and in total,
//! cleaned of what makes a log unreadable, and masked before it is kept.

use super::bitbucket::{self, Contexts};
use super::{Forge, Job, Log, Pull, gh_text};

/// The lines of a log that are given, counted from its end.
const LINES: usize = 40;
/// The bytes of a log that are given (its lines' text).
const JOB_BYTES: usize = 4000;
/// The bytes of all the logs of a pull request.
const TOTAL_BYTES: usize = 12_000;
/// The logs asked of the forge for a pull request.
const JOBS: usize = 5;
/// The characters of a line that says why a job failed, in a wake.
const ERROR_CHARS: usize = 160;

/// Reads the log of each failed job of `pull` that the forge gives, within the bounds above; a job
/// whose log is not given says why. Only `forge show` asks for it: the state a window watches is
/// never made of logs.
pub(super) async fn attach(forge: &Forge, pull: &mut Pull) {
    let mut left = TOTAL_BYTES;
    let mut asked = 0;
    // Only Bitbucket Cloud has logs to read, and a context to name for them.
    let context = match forge {
        Forge::Bitbucket { .. } if bitbucket::cloud(forge) && !pull.jobs.is_empty() => {
            Contexts::default().of(forge).await
        }
        _ => Err(String::new()),
    };
    for job in &mut pull.jobs {
        job.log = Some(match forge {
            Forge::Bitbucket { .. } if !bitbucket::cloud(forge) => {
                Err("bkt gives no job logs, read it on Bitbucket".into())
            }
            Forge::Bitbucket { .. } => match bitbucket::build_of(forge, job.details.as_deref()) {
                None => Err(match &job.details {
                    Some(link) => format!("it is not a Bitbucket Pipelines build, see {link}"),
                    None => "it is not a Bitbucket Pipelines build".into(),
                }),
                Some(_) if asked == JOBS || left == 0 => Err(format!(
                    "at most {JOBS} logs and {TOTAL_BYTES} bytes are given"
                )),
                Some(build) => match &context {
                    Err(why) => Err(why.clone()),
                    Ok(context) => {
                        asked += 1;
                        let log = bitbucket::failed_step_log(forge, context, build)
                            .await
                            .and_then(|raw| {
                                tail_with(&raw, JOB_BYTES.min(left), plain, bitbucket_part)
                            });
                        if let Ok(log) = &log {
                            left -= log.text.len().min(left);
                        }
                        log
                    }
                },
            },
            Forge::GitHub { .. } => match actions_job(job.details.as_deref()) {
                None => Err(match &job.details {
                    Some(link) => format!("it is not a GitHub Actions job, see {link}"),
                    None => "it is not a GitHub Actions job".into(),
                }),
                Some(_) if asked == JOBS || left == 0 => Err(format!(
                    "at most {JOBS} logs and {TOTAL_BYTES} bytes are given"
                )),
                Some(id) => {
                    asked += 1;
                    let log = gh_text(&[
                        "run",
                        "view",
                        "--job",
                        &id.to_string(),
                        "--log-failed",
                        "-R",
                        &forge.gh_repository(),
                    ])
                    .await
                    .and_then(|raw| tail_with(&raw, JOB_BYTES.min(left), clean, github_part));
                    if let Ok(log) = &log {
                        left -= log.text.len().min(left);
                    }
                    log
                }
            },
        });
    }
}

/// The job a check's link names: `…/actions/runs/<run>/job/<job>`.
fn actions_job(link: Option<&str>) -> Option<u64> {
    let (_, rest) = link?.split_once("/actions/runs/")?;
    let (run, job) = rest.split_once("/job/")?;
    run.bytes().all(|b| b.is_ascii_digit()).then_some(())?;
    let digits: String = job.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// The end of `raw`, cleaned: at most `LINES` lines and `bytes` bytes of text, the oldest lines
/// going first and whole; a single line longer than `bytes` keeps its end. The failing part is
/// what `part_of` finds in the lines, the whole log when it finds none. Each line is made plain
/// by `line_of`: GitHub's lines start with names and a timestamp, Bitbucket's do not.
fn tail_with(
    raw: &str,
    bytes: usize,
    line_of: fn(&str) -> String,
    part_of: fn(&[String]) -> Option<Vec<String>>,
) -> Result<Log, String> {
    let mut lines: Vec<String> = raw.lines().map(line_of).collect();
    trim(&mut lines);
    // The failing command's lines when the source's markers find them, else the whole log.
    let part = part_of(&lines).map(|mut part| {
        trim(&mut part);
        part
    });
    let found = part.as_ref().is_some_and(|part| !part.is_empty());
    if let Some(part) = part.filter(|part| !part.is_empty()) {
        lines = part;
    }
    let total = lines.len();
    if total == 0 {
        return Err("its log is empty".into());
    }
    lines.drain(..total.saturating_sub(LINES));
    // Each line costs its text and the line break that follows it.
    while lines.len() > 1 && lines.iter().map(|l| l.len() + 1).sum::<usize>() > bytes {
        lines.remove(0);
    }
    if let [only] = lines.as_mut_slice()
        && only.len() + 1 > bytes
    {
        let mut from = only.len().saturating_sub(bytes.saturating_sub(4));
        while !only.is_char_boundary(from) {
            from += 1;
        }
        *only = format!("…{}", &only[from..]);
    }
    Ok(Log {
        shown: lines.len(),
        total,
        part: found,
        text: lines.join("\n"),
    })
}

/// `lines` without the empty ones at both ends.
fn trim(lines: &mut Vec<String>) {
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    while lines.first().is_some_and(String::is_empty) {
        lines.remove(0);
    }
}

/// What Bitbucket Pipelines prints once the script is over, by the start of the line
/// (`docs/forges.md`, "Why a check failed": only the first is documented by Atlassian).
const TEARDOWN: [&str; 4] = [
    "Searching for test report files in directories named",
    "Searching for files matching artifact pattern",
    "Uploading cache",
    "Uploading artifact",
];
/// The commands that only list the environment.
const ENVIRONMENT: [&str; 4] = ["env", "printenv", "export", "set"];
/// How many `NAME=value` lines in a row are an environment listing.
const LISTING: usize = 3;

/// The lines of the failing command in a Bitbucket Pipelines step log (`docs/forges.md`, "Why a
/// check failed"): from the last command echoed (`+ <command>`) that is not an environment
/// listing, to the teardown or to a command that lists the environment. The `NAME=value` listings
/// in between are left out. None when no command is echoed.
fn bitbucket_part(lines: &[String]) -> Option<Vec<String>> {
    fn command(line: &str) -> Option<&str> {
        line.strip_prefix("+ ").map(str::trim)
    }
    let lists =
        |command: &str| ENVIRONMENT.contains(&command.split_whitespace().next().unwrap_or(""));
    let from = lines
        .iter()
        .rposition(|line| command(line).is_some_and(|command| !lists(command)))?;
    let to = lines[from + 1..]
        .iter()
        .position(|line| {
            TEARDOWN.iter().any(|mark| line.starts_with(mark)) || command(line).is_some_and(lists)
        })
        .map_or(lines.len(), |at| from + 1 + at);
    let part = &lines[from..to];
    let mut kept = Vec::with_capacity(part.len());
    let mut at = 0;
    while at < part.len() {
        let run = part[at..]
            .iter()
            .take_while(|line| is_variable(line))
            .count();
        if run >= LISTING {
            at += run;
        } else {
            kept.push(part[at].clone());
            at += 1;
        }
    }
    Some(kept)
}

/// `NAME=value`, a line of an environment listing.
fn is_variable(line: &str) -> bool {
    line.split_once('=').is_some_and(|(name, _)| {
        name.starts_with(|c: char| c.is_ascii_uppercase() || c == '_')
            && name
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    })
}

/// The lines of the failing step's command in a GitHub Actions log: after the group that echoes
/// the command and its `env:` (`##[group]Run …` to `##[endgroup]`), up to the last `##[error]`
/// line, which is kept. None without an `##[error]` line.
fn github_part(lines: &[String]) -> Option<Vec<String>> {
    let error = lines
        .iter()
        .rposition(|line| line.starts_with("##[error]"))?;
    let start = lines[..error]
        .iter()
        .rposition(|line| line.starts_with("##[group]Run "))
        .and_then(|group| {
            lines[group..error]
                .iter()
                .position(|line| line == "##[endgroup]")
                .map(|at| group + at + 1)
        })
        .unwrap_or(0);
    Some(lines[start..=error].to_vec())
}

/// One line of `gh run view --log-failed` (`<job>\t<step>\t<timestamp> <text>`) as its text: without
/// the names and the timestamp that start every line, nor the colours and the carriage returns of
/// a terminal's progress.
fn clean(line: &str) -> String {
    let line = line.trim_start_matches('\u{feff}');
    let mut parts = line.splitn(3, '\t');
    let text = match (parts.next(), parts.next(), parts.next()) {
        (Some(_), Some(_), Some(text)) => text,
        _ => line,
    };
    let text = match text.split_once(' ') {
        Some((stamp, rest)) if is_timestamp(stamp) => rest,
        _ => text,
    };
    plain(text)
}

/// A line of a log without the colours and the carriage returns of a terminal.
fn plain(text: &str) -> String {
    let text = text
        .rsplit('\r')
        .find(|part| !part.is_empty())
        .unwrap_or("");
    let mut clean = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            // A colour or a cursor move: `ESC [`, then up to its final letter.
            '\u{1b}' => {
                if chars.as_str().starts_with('[') {
                    chars.next();
                    for c in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&c) {
                            break;
                        }
                    }
                }
            }
            c if c.is_control() && c != '\t' => {}
            c => clean.push(c),
        }
    }
    clean.trim_end().to_owned()
}

/// `2026-01-01T00:00:00.1234567Z`, how GitHub starts every line of a log.
fn is_timestamp(word: &str) -> bool {
    let bytes = word.as_bytes();
    bytes.len() >= 20
        && bytes.ends_with(b"Z")
        && bytes[4] == b'-'
        && bytes[10] == b'T'
        && bytes[..4].iter().all(u8::is_ascii_digit)
}

/// The lines of `log` that say why its job failed, for a wake: the first and the last line that
/// look like an error (`error`, `FAILED`, `panicked`), the same one once; the runner's exit code
/// says nothing of why and is no candidate. The last line of the log when none looks like one.
/// The log's text is already the failing part and masked.
pub(super) fn why(log: &Log) -> Vec<String> {
    let lines = log
        .text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let looks_like_error = |line: &&str| {
        let line = line.to_lowercase();
        !line.starts_with("##[error]process completed")
            && ["error", "failed", "panicked"]
                .iter()
                .any(|word| line.contains(word))
    };
    let found: Vec<&str> = lines.clone().filter(looks_like_error).collect();
    let mut said: Vec<&str> = match (found.first(), found.last()) {
        (Some(first), Some(last)) if first != last => vec![first, last],
        (Some(only), _) => vec![only],
        _ => lines.clone().next_back().into_iter().collect(),
    };
    said.dedup();
    said.into_iter().map(cut).collect()
}

/// `line` without the runner's mark, at most `ERROR_CHARS` characters long.
pub(super) fn cut(line: &str) -> String {
    let line = line.strip_prefix("##[error]").unwrap_or(line);
    match line.char_indices().nth(ERROR_CHARS - 1) {
        Some((at, _)) if line[at..].chars().nth(1).is_some() => format!("{}…", &line[..at]),
        _ => line.to_owned(),
    }
}

/// The failed jobs of a pull request, as `forge show` says them: the end of each log, or why there
/// is none. Nothing for a pull request with no failed job.
pub(super) fn text(jobs: &[Job]) -> String {
    if jobs.is_empty() {
        return String::new();
    }
    let mut text = format!("  failed jobs ({}):\n", jobs.len());
    for job in jobs {
        match &job.log {
            Some(Ok(log)) => {
                let of = if log.part {
                    "of the failing command's output"
                } else {
                    "of its log"
                };
                text.push_str(&format!(
                    "  - {}: the last {} of {} lines {of}:\n",
                    job.name, log.shown, log.total
                ));
                for line in log.text.lines() {
                    text.push_str(&format!("    {line}\n"));
                }
            }
            Some(Err(why)) => text.push_str(&format!("  - {}: no log: {why}\n", job.name)),
            None => text.push_str(&format!("  - {}\n", job.name)),
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tail(raw: &str, bytes: usize) -> Result<Log, String> {
        tail_with(raw, bytes, clean, |_| None)
    }

    #[test]
    fn a_log_line_loses_its_names_timestamp_and_colours() {
        assert_eq!(
            clean("test\tRun cargo\t2026-01-01T00:00:01.1234567Z \u{1b}[31merror\u{1b}[0m: boom  "),
            "error: boom"
        );
        assert_eq!(clean("compiling 10%\rcompiling 100%"), "compiling 100%");
        assert_eq!(clean("a\tb\tkept\twith a tab"), "kept\twith a tab");
    }

    #[test]
    fn a_job_is_named_by_the_link_of_its_check() {
        for (link, job) in [
            (
                "https://github.com/acme/api/actions/runs/900/job/456",
                Some(456),
            ),
            (
                "https://github.com/acme/api/actions/runs/900/job/456?pr=12",
                Some(456),
            ),
            ("https://github.com/acme/api/actions/runs/900", None),
            ("https://ci.example.com/builds/5", None),
        ] {
            assert_eq!(actions_job(Some(link)), job, "{link}");
        }
        assert_eq!(actions_job(None), None);
    }

    #[test]
    fn the_tail_keeps_the_last_lines_within_its_bytes() {
        let raw: String = (1..=100).map(|n| format!("line {n:03}\n")).collect();

        let log = tail(&raw, 90).unwrap();
        // 8 bytes and a line break each: ten lines make 90 bytes.
        assert_eq!((log.shown, log.total), (10, 100));
        assert!(log.text.starts_with("line 091") && log.text.ends_with("line 100"));
        let log = tail(&raw, 10_000).unwrap();
        assert_eq!((log.shown, log.total), (LINES, 100));
        assert_eq!(tail("\n\n", 100), Err("its log is empty".into()));
    }

    #[test]
    fn a_single_line_longer_than_the_bytes_keeps_its_end() {
        let log = tail(&format!("{}END", "x".repeat(500)), 20).unwrap();

        assert_eq!(log.shown, 1);
        assert!(log.text.starts_with('…') && log.text.ends_with("xEND"));
        assert!(log.text.len() <= 20, "{}", log.text.len());
    }

    /// A Bitbucket Pipelines step log as `bkt pipeline logs --step` prints it: its setup, each
    /// script command echoed with `+ `, the environment listing of a command, then the teardown.
    fn bitbucket_log(failing_output: &[&str]) -> String {
        let mut lines = vec![
            "Images used:",
            "    build : docker.io/library/rust@sha256:0000",
            "Cloning into '/opt/atlassian/pipelines/agent/build'...",
            "+ cargo build",
            "   Compiling app v0.1.0",
            "+ cargo test",
        ];
        lines.extend(failing_output);
        lines.extend([
            "Searching for test report files in directories named [test-results, failsafe-reports, test-reports, TestResults, surefire-reports] down to a depth of 4",
            "Finished scanning for test reports. Found 0 test report files.",
            "Merged test suites, total number tests is 0, with 0 failures and 0 errors.",
            "Uploading cache cargo",
            "Uploading artifact target",
        ]);
        lines.join("\n") + "\n"
    }

    const FAILURE: [&str; 3] = [
        "test login::form ... FAILED",
        "thread 'login::form' panicked at src/login.rs:42:9:",
        "error: test failed, to rerun pass `--lib`",
    ];

    #[test]
    fn a_bitbucket_log_keeps_the_failing_commands_output() {
        let log = tail_with(&bitbucket_log(&FAILURE), 4000, plain, bitbucket_part).unwrap();

        assert_eq!(
            log.text.lines().collect::<Vec<_>>(),
            ["+ cargo test", FAILURE[0], FAILURE[1], FAILURE[2]]
        );
        assert_eq!((log.shown, log.total, log.part), (4, 4, true));
    }

    #[test]
    fn the_environment_listing_and_teardown_are_left_out() {
        let environment = [
            "HOME=/root",
            "BITBUCKET_BUILD_NUMBER=31",
            "BITBUCKET_REPO_SLUG=api",
            "CI=true",
            "PATH=/usr/local/bin:/usr/bin",
        ];
        // The listing after the failing command, then a command that lists it again.
        let output: Vec<&str> = FAILURE
            .into_iter()
            .chain(environment)
            .chain(["+ env"])
            .chain(environment)
            .collect();
        let log = tail_with(&bitbucket_log(&output), 4000, plain, bitbucket_part).unwrap();

        for left_out in [
            "HOME=",
            "BITBUCKET_",
            "Uploading",
            "Searching for test report",
            "Merged test suites",
            "Images used",
        ] {
            assert!(!log.text.contains(left_out), "{left_out}: {}", log.text);
        }
        assert!(log.text.starts_with("+ cargo test\n"), "{}", log.text);
        assert!(log.text.ends_with(FAILURE[2]), "{}", log.text);
    }

    #[test]
    fn a_long_failing_output_keeps_its_end() {
        let output: Vec<String> = (1..=100).map(|n| format!("output {n:03}")).collect();
        let output: Vec<&str> = output.iter().map(String::as_str).collect();

        let log = tail_with(&bitbucket_log(&output), 4000, plain, bitbucket_part).unwrap();

        assert_eq!((log.shown, log.total), (LINES, 101));
        assert!(log.text.ends_with("output 100"), "{}", log.text);
        assert!(!log.text.contains("Uploading"), "{}", log.text);
    }

    #[test]
    fn a_log_without_markers_keeps_its_tail() {
        let raw: String = (1..=100).map(|n| format!("line {n:03}\n")).collect();

        for log in [
            tail_with(&raw, 4000, plain, bitbucket_part).unwrap(),
            tail_with(&raw, 4000, clean, github_part).unwrap(),
            // Teardown lines and no command echoed: nothing says which part failed.
            tail_with(
                "setup\nUploading cache x\nSearching for test report files in directories named []\n",
                4000,
                plain,
                bitbucket_part,
            )
            .unwrap(),
        ] {
            assert!(!log.part);
            assert_eq!(log.shown, log.total.min(LINES));
        }
        let log = tail_with(&raw, 4000, plain, bitbucket_part).unwrap();
        assert!(log.text.starts_with("line 061") && log.text.ends_with("line 100"));
    }

    #[test]
    fn a_github_log_keeps_the_failing_commands_output() {
        let raw = [
            "test\tRun cargo test\t2026-01-01T00:00:00.0000000Z ##[group]Run cargo test",
            "test\tRun cargo test\t2026-01-01T00:00:00.0000000Z cargo test",
            "test\tRun cargo test\t2026-01-01T00:00:00.0000000Z shell: /usr/bin/bash -e {0}",
            "test\tRun cargo test\t2026-01-01T00:00:00.0000000Z env:",
            "test\tRun cargo test\t2026-01-01T00:00:00.0000000Z   CARGO_TERM_COLOR: always",
            "test\tRun cargo test\t2026-01-01T00:00:00.0000000Z ##[endgroup]",
            "test\tRun cargo test\t2026-01-01T00:00:01.0000000Z test login::form ... FAILED",
            "test\tRun cargo test\t2026-01-01T00:00:01.0000000Z ##[error]Process completed with exit code 101.",
            "",
        ]
        .join("\n");

        let log = tail_with(&raw, 4000, clean, github_part).unwrap();

        assert_eq!(
            log.text.lines().collect::<Vec<_>>(),
            [
                "test login::form ... FAILED",
                "##[error]Process completed with exit code 101."
            ]
        );
        assert!(log.part);
    }

    fn log_of(text: &str) -> Log {
        Log {
            text: text.to_owned(),
            shown: text.lines().count(),
            total: text.lines().count(),
            part: true,
        }
    }

    #[test]
    fn the_error_lines_are_the_first_and_the_last_that_look_like_one() {
        let log = log_of(&FAILURE.join("\n"));
        assert_eq!(
            why(&log),
            [
                "test login::form ... FAILED",
                "error: test failed, to rerun pass `--lib`"
            ]
        );
        // One alone is said once; the runner's exit code says nothing of why.
        let log = log_of(
            "compiling\nerror[E0425]: no value `x`\n##[error]Process completed with exit code 101.",
        );
        assert_eq!(why(&log), ["error[E0425]: no value `x`"]);
    }

    #[test]
    fn a_log_with_no_error_line_says_its_last_line() {
        assert_eq!(why(&log_of("one\ntwo\nthree")), ["three"]);
        assert_eq!(why(&log_of("")), Vec::<String>::new());
    }

    #[test]
    fn an_error_line_is_cut_at_a_length_a_wake_can_afford() {
        let line = why(&log_of(&format!("error: {}", "x".repeat(500))));

        assert_eq!(line.len(), 1);
        assert!(line[0].chars().count() <= ERROR_CHARS, "{}", line[0]);
        assert!(line[0].ends_with('…'));
    }
}
