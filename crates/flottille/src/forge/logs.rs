//! The end of the log of each failed job of a pull request, for `forge show` (`docs/forges.md`,
//! "Why a check failed"): an agent that is told "failing" can do nothing, one that reads the last
//! lines of the job's log can often fix it. Bounded in lines and in bytes, per job and in total,
//! cleaned of what makes a log unreadable, and masked before it is kept.

use super::{Forge, Job, Log, Pull, gh_text};

/// The lines of a log that are given, counted from its end.
const LINES: usize = 40;
/// The bytes of a log that are given (its lines' text).
const JOB_BYTES: usize = 4000;
/// The bytes of all the logs of a pull request.
const TOTAL_BYTES: usize = 12_000;
/// The logs asked of the forge for a pull request.
const JOBS: usize = 5;

/// Reads the log of each failed job of `pull` that the forge gives, within the bounds above; a job
/// whose log is not given says why. Only `forge show` asks for it: the state a window watches is
/// never made of logs.
pub(super) async fn attach(forge: &Forge, pull: &mut Pull) {
    let mut left = TOTAL_BYTES;
    let mut asked = 0;
    for job in &mut pull.jobs {
        job.log = Some(match forge {
            Forge::Bitbucket { .. } => Err("bkt gives no job logs, read it on Bitbucket".into()),
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
                    .and_then(|raw| tail(&raw, JOB_BYTES.min(left)));
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
/// going first and whole; a single line longer than `bytes` keeps its end.
fn tail(raw: &str, bytes: usize) -> Result<Log, String> {
    let mut lines: Vec<String> = raw.lines().map(clean).collect();
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    while lines.first().is_some_and(String::is_empty) {
        lines.remove(0);
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
        text: lines.join("\n"),
    })
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
                text.push_str(&format!(
                    "  - {}: the last {} of {} lines of its log:\n",
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
}
