//! The end of the log of each failed job of a pull request, for `forge show` (`docs/forges.md`,
//! "Why a check failed"): an agent that is told "failing" can do nothing, one that reads the last
//! lines of the job's log can often fix it. Bounded in lines and in bytes, per job and in total,
//! cleaned of what makes a log unreadable, and masked before it is kept.

use super::bitbucket::{self, Contexts};
use serde_json::Value;

use super::ending::Ending;
use super::{Forge, Job, Log, gh_text};

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

/// Reads the log of each failed job of a pull request that the forge gives, within the bounds
/// above, all at once; a job whose log is not given says why. Only `forge show` asks for it: the
/// state a window watches is never made of logs.
pub(super) async fn attach(forge: &Forge, jobs: &mut [Job]) {
    // Only Bitbucket Cloud has logs to read, and a context to name for them.
    let context = match forge {
        Forge::Bitbucket { .. } if bitbucket::cloud(forge) && !jobs.is_empty() => {
            Contexts::default().of(forge).await
        }
        _ => Err(String::new()),
    };
    // What each job needs is decided before anything is read, from the bounds that do not depend
    // on a log's size, so that the logs can be read together.
    let mut asked = 0;
    let mut plans = Vec::new();
    for job in jobs.iter() {
        plans.push(match forge {
            Forge::Bitbucket { .. } if !bitbucket::cloud(forge) => {
                Plan::Said(Err("bkt gives no job logs, read it on Bitbucket".into()))
            }
            Forge::Bitbucket { .. } => match bitbucket::build_of(forge, job.details.as_deref()) {
                None => Plan::Said(Err(match &job.details {
                    Some(link) => format!("it is not a Bitbucket Pipelines build, see {link}"),
                    None => "it is not a Bitbucket Pipelines build".into(),
                })),
                Some(_) if asked == JOBS => Plan::Said(Err(too_many())),
                Some(build) => match &context {
                    Err(why) => Plan::Said(Err(why.clone())),
                    Ok(context) => {
                        asked += 1;
                        Plan::Read(Source::Bitbucket(context.clone(), build))
                    }
                },
            },
            // A job the CI did not run to its end has no failing command's log to read.
            Forge::GitHub { .. } if job.ending != Ending::Code => {
                Plan::Said(Err("it did not fail on a command".into()))
            }
            Forge::GitHub { .. } => match actions_job(job.details.as_deref()) {
                None => Plan::Said(Err(match &job.details {
                    Some(link) => format!("it is not a GitHub Actions job, see {link}"),
                    None => "it is not a GitHub Actions job".into(),
                })),
                Some(_) if asked == JOBS => Plan::Said(Err(too_many())),
                Some((run, job)) => {
                    asked += 1;
                    Plan::Read(Source::GitHub(run, job))
                }
            },
        });
    }
    let raws = futures::future::join_all(plans.iter().map(|plan| async move {
        match plan {
            Plan::Said(_) => None,
            Plan::Read(Source::Bitbucket(context, build)) => {
                let built = bitbucket::failed_step_log(forge, context, *build).await;
                Some((built.ending, built.log))
            }
            Plan::Read(Source::GitHub(_, job)) => Some((
                None,
                gh_text(&[
                    "run",
                    "view",
                    "--job",
                    &job.to_string(),
                    "--log-failed",
                    "-R",
                    &forge.gh_repository(),
                ])
                .await
                .map(|log| (job.to_string(), log)),
            )),
        }
    }))
    .await;
    // The total is shared out in the jobs' order, as it is when they are read one after the other.
    let mut left = TOTAL_BYTES;
    for ((job, plan), raw) in jobs.iter_mut().zip(plans).zip(raws) {
        let (ending, raw) = match raw {
            Some((ending, raw)) => (ending, Some(raw)),
            None => (None, None),
        };
        // What the build says of its end refines what the check's status said: a failed status
        // is the CI's when the build died of its own, and a stop may say who made it.
        match (&job.ending, ending) {
            (Ending::Code, Some(ending)) => job.ending = ending,
            (Ending::Stopped(None), Some(ending @ Ending::Stopped(Some(_)))) => job.ending = ending,
            _ => {}
        }
        let log = match (plan, raw) {
            (Plan::Said(said), _) => said,
            (Plan::Read(_), _) if left == 0 => Err(too_many()),
            (Plan::Read(source), Some(raw)) => raw.and_then(|(step, raw)| {
                let (run, line_of, part_of): (_, fn(&str) -> String, PartOf) = match source {
                    Source::Bitbucket(_, build) => (build, plain, bitbucket_part),
                    Source::GitHub(run, _) => (run, clean, github_part),
                };
                let mut log = tail_with(&raw, JOB_BYTES.min(left), line_of, part_of)?;
                log.whole = Some((run, step));
                Ok(log)
            }),
            (Plan::Read(_), None) => unreachable!("a log to read was read"),
        };
        if let Ok(log) = &log {
            left -= log.text.len().min(left);
        }
        job.log = Some(log);
    }
}

/// What a job's log needs: it is said already, or it is read from here.
enum Plan {
    Said(Result<Log, String>),
    Read(Source),
}

enum Source {
    /// The context to name and the build.
    Bitbucket(String, u64),
    /// The run and the job's id.
    GitHub(u64, u64),
}

/// What finds the failing command's lines among a log's.
type PartOf = fn(&[String]) -> Option<Vec<String>>;

fn too_many() -> String {
    format!("at most {JOBS} logs and {TOTAL_BYTES} bytes are given")
}

/// The run and the job a check's link names: `…/actions/runs/<run>/job/<job>`.
fn actions_job(link: Option<&str>) -> Option<(u64, u64)> {
    let (_, rest) = link?.split_once("/actions/runs/")?;
    let (run, job) = rest.split_once("/job/")?;
    let digits: String = job.chars().take_while(char::is_ascii_digit).collect();
    Some((run.parse().ok()?, digits.parse().ok()?))
}

/// The end of `raw`, cleaned: at most `LINES` lines and `bytes` bytes of text, the oldest lines
/// going first and whole; a single line longer than `bytes` keeps its end. The failing part is
/// what `part_of` finds in the lines, the whole log when it finds none. Each line is made plain
/// by `line_of`: GitHub's lines start with names and a timestamp, Bitbucket's do not.
fn tail_with(
    raw: &str,
    bytes: usize,
    line_of: fn(&str) -> String,
    part_of: PartOf,
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
        whole: None,
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
/// check failed"): the output of the first command echoed (`+ <command>`, an environment listing
/// excepted) that printed an error-looking line, else of the last one, from the command line to
/// the next command, the teardown or the end. The commands of an `after-script` are echoed after
/// the failed one, so the last command is not the failing one; the log does not tell them apart.
/// The `NAME=value` listings in between are left out. None when no command is echoed.
fn bitbucket_part(lines: &[String]) -> Option<Vec<String>> {
    fn command(line: &str) -> Option<&str> {
        line.strip_prefix("+ ").map(str::trim)
    }
    let lists =
        |command: &str| ENVIRONMENT.contains(&command.split_whitespace().next().unwrap_or(""));
    // Each command and where its output ends.
    let commands: Vec<(usize, usize)> = (0..lines.len())
        .filter(|&at| command(&lines[at]).is_some_and(|command| !lists(command)))
        .map(|from| {
            let to = lines[from + 1..]
                .iter()
                .position(|line| {
                    TEARDOWN.iter().any(|mark| line.starts_with(mark)) || command(line).is_some()
                })
                .map_or(lines.len(), |at| from + 1 + at);
            (from, to)
        })
        .collect();
    let (from, to) = commands
        .iter()
        .copied()
        .find(|&(from, to)| {
            lines[from + 1..to]
                .iter()
                .any(|line| looks_like_error(line))
        })
        .or(commands.last().copied())?;
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
pub(super) fn clean(line: &str) -> String {
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

/// Whether a line looks like the one that says why a command failed: the words `error`, `failed`
/// or `panicked`, whole, in any case. Passing lines (`... ok`, `0 failed`, `thiserror`) and the
/// runner's exit code, which says nothing of why, do not.
fn looks_like_error(line: &str) -> bool {
    let line = line.trim();
    if line.ends_with("... ok") || line.starts_with("##[error]Process completed") {
        return false;
    }
    let words: Vec<String> = line
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect();
    words
        .iter()
        .enumerate()
        .any(|(at, word)| match word.as_str() {
            "error" | "panicked" => true,
            // "0 failed" counts what did not fail.
            "failed" => at == 0 || words[at - 1] != "0",
            _ => false,
        })
}

/// The line of `log` that says why its job failed, for a wake: the first that looks like an error
/// (the words `error`, `failed` or `panicked`, whole, in any case), else the last line of the log.
/// Passing lines are no candidate: `... ok`, `0 failed`, `thiserror`; nor is the runner's exit
/// code, which says nothing of why. The log's text is already the failing part and masked.
pub(super) fn why(log: &Log) -> Option<String> {
    let lines = log
        .text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    lines
        .clone()
        .find(|line| looks_like_error(line))
        .or_else(|| lines.clone().next_back())
        .map(cut)
}

/// `line` without the runner's mark, at most `ERROR_CHARS` characters long.
fn cut(line: &str) -> String {
    let line = line.strip_prefix("##[error]").unwrap_or(line);
    match line.char_indices().nth(ERROR_CHARS - 1) {
        Some((at, _)) if line[at..].chars().nth(1).is_some() => format!("{}…", &line[..at]),
        _ => line.to_owned(),
    }
}

/// How to read a job's whole log: the exact call, since the excerpt is only its end.
pub(super) fn read_whole(repository: &str, (run, step): &(u64, String)) -> String {
    format!(
        "Read the whole log with forge {{\"action\": \"run_logs\", \"repository\": {}, \"run\": {run}, \"step\": {}}}.",
        Value::from(repository),
        Value::from(step.as_str())
    )
}

/// The failed jobs of a pull request, as `forge show` says them: how each ended when it was not
/// the code's failure, the end of each log, or why there is none. Nothing for a pull request with
/// no failed job.
pub(super) fn text(jobs: &[Job], repository: &str) -> String {
    if jobs.is_empty() {
        return String::new();
    }
    let mut text = format!("  failed jobs ({}):\n", jobs.len());
    for job in jobs {
        let said = job.ending.said(job.took);
        // A job the CI did not run to its end has no failing command's output to show.
        let log = match (&job.log, &job.ending) {
            (_, Ending::Stopped(_) | Ending::TimedOut) => None,
            (log, _) => log.as_ref(),
        };
        match (log, said) {
            (Some(Ok(log)), said) => {
                let of = if log.part {
                    "of the failing command's output"
                } else {
                    "of its log"
                };
                let tail = format!("the last {} of {} lines {of}", log.shown, log.total);
                match said {
                    Some(said) => text.push_str(&format!("  - {}: {said}; {tail}:\n", job.name)),
                    None => text.push_str(&format!("  - {}: {tail}:\n", job.name)),
                }
                for line in log.text.lines() {
                    text.push_str(&format!("    {line}\n"));
                }
                if let Some(whole) = &log.whole {
                    text.push_str(&format!("    {}\n", read_whole(repository, whole)));
                }
            }
            (_, Some(said)) => text.push_str(&format!("  - {}: {said}\n", job.name)),
            (Some(Err(why)), None) => text.push_str(&format!("  - {}: no log: {why}\n", job.name)),
            (None, None) => text.push_str(&format!("  - {}\n", job.name)),
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
                Some((900, 456)),
            ),
            (
                "https://github.com/acme/api/actions/runs/900/job/456?pr=12",
                Some((900, 456)),
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
    fn a_bitbucket_step_with_an_after_script_shows_the_failed_script_command() {
        // The after-script runs once the script has failed: its commands are echoed after the
        // failed one, and one of them prints a line that looks like an error too.
        let output: Vec<&str> = FAILURE
            .into_iter()
            .chain([
                "+ ./cleanup.sh",
                "removing the scratch folder",
                "+ curl -X POST https://hooks.example.com/notify",
                "error: the notification was refused",
            ])
            .collect();

        let log = tail_with(&bitbucket_log(&output), 4000, plain, bitbucket_part).unwrap();

        assert_eq!(
            log.text.lines().collect::<Vec<_>>(),
            ["+ cargo test", FAILURE[0], FAILURE[1], FAILURE[2]]
        );
        assert!(log.part);
    }

    #[test]
    fn a_bitbucket_step_without_an_after_script_is_read_as_before() {
        // The failing command is the last one echoed, whether or not an earlier one said "error".
        let log = tail_with(&bitbucket_log(&FAILURE), 4000, plain, bitbucket_part).unwrap();
        assert_eq!(log.text.lines().next(), Some("+ cargo test"));
        assert_eq!(log.shown, 4);

        let quiet = ["exit status 2"];
        let log = tail_with(&bitbucket_log(&quiet), 4000, plain, bitbucket_part).unwrap();
        assert_eq!(
            log.text.lines().collect::<Vec<_>>(),
            ["+ cargo test", "exit status 2"]
        );
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
            whole: None,
        }
    }

    #[test]
    fn the_error_line_is_the_first_that_looks_like_one() {
        let log = log_of(&FAILURE.join("\n"));
        assert_eq!(why(&log).as_deref(), Some("test login::form ... FAILED"));
        // The runner's exit code says nothing of why.
        let log = log_of(
            "compiling\nerror[E0425]: no value `x`\n##[error]Process completed with exit code 101.",
        );
        assert_eq!(why(&log).as_deref(), Some("error[E0425]: no value `x`"));
    }

    #[test]
    fn passing_lines_are_no_error_line() {
        let log = log_of(
            "   Compiling thiserror v1.0.69\n\
             test parse_error_is_reported ... ok\n\
             test result: ok. 12 passed; 0 failed; 0 ignored\n\
             thread 'x' panicked at src/x.rs:1:1:\n\
             assertion failed: left == right",
        );
        assert_eq!(
            why(&log).as_deref(),
            Some("thread 'x' panicked at src/x.rs:1:1:")
        );
        // Nothing but passing lines: the last one.
        let log = log_of("Compiling thiserror v1.0.69\ntest result: ok. 12 passed; 0 failed");
        assert_eq!(
            why(&log).as_deref(),
            Some("test result: ok. 12 passed; 0 failed")
        );
    }

    #[test]
    fn a_log_with_no_error_line_says_its_last_line() {
        assert_eq!(why(&log_of("one\ntwo\nthree")).as_deref(), Some("three"));
        assert_eq!(why(&log_of("")), None);
    }

    #[test]
    fn an_error_line_is_cut_at_a_length_a_wake_can_afford() {
        let line = why(&log_of(&format!("error: {}", "x".repeat(500)))).unwrap();

        assert!(line.chars().count() <= ERROR_CHARS, "{line}");
        assert!(line.ends_with('…'));
    }
}
