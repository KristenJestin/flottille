//! The file tools: `fs_read`, `fs_list`, `search`, `fs_write`, `fs_edit` (`docs/tools.md`,
//! `docs/reference/hemera.md` §3).
//!
//! Writing is optimistic per session: a file the agent has not read, or that changed since it
//! read it, is never written over. Results are cut to stay well under what the agent keeps in its
//! context (`docs/reference/mcp.md` §4 rules 5 and 8), and each says how to get the next part.

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{BufRead, BufReader, ErrorKind, Read, Seek, SeekFrom};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use flottille_core::glob;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::daemon::Daemon;
use crate::gate::{self, Class, Verdict};
use crate::mcp::{Outcome, arguments};
use crate::place::{Place, Resolved};

/// A page of `fs_read`, at most.
pub const READ_PAGE_BYTES: u64 = 32 * 1024;
/// What a result's body may hold, in bytes, so that the result stays under 40,000 characters with
/// its last lines.
const BODY_BUDGET: usize = 32_000;
const LIST_DEPTH_MAX: u8 = 5;
const LIST_ENTRIES_MAX: usize = 1000;
const SEARCH_MATCH_LIMIT: usize = 100;
const SEARCH_SCAN_BYTES: usize = 1024 * 1024;
const SEARCH_SKIPS_LISTED: usize = 20;
const HIT_CHARS: usize = 200;
const EDITS_MAX: usize = 50;
/// The text of a file read is kept for the diff of a later refusal, when it is this small, for so
/// many files per agent.
const KEPT_BYTES: u64 = 256 * 1024;
const KEPT_FILES: usize = 32;
const DIFF_LINES: usize = 20;
const DIFF_LINE_CHARS: usize = 120;
const CHUNK: usize = 64 * 1024;

/// The texts of the files each agent read, for the diffs of the refusals: the last ones only.
#[derive(Default)]
pub struct Kept {
    texts: Mutex<HashMap<String, VecDeque<(PathBuf, String)>>>,
}

impl Kept {
    fn keep(&self, agent: &str, path: &Path, text: Option<String>) {
        let mut texts = self.texts.lock().unwrap();
        let files = texts.entry(agent.to_owned()).or_default();
        files.retain(|(kept, _)| kept != path);
        if let Some(text) = text {
            files.push_back((path.to_owned(), text));
            if files.len() > KEPT_FILES {
                files.pop_front();
            }
        }
    }

    fn text(&self, agent: &str, path: &Path) -> Option<String> {
        let texts = self.texts.lock().unwrap();
        texts
            .get(agent)?
            .iter()
            .find(|(kept, _)| kept == path)
            .map(|(_, text)| text.clone())
    }
}

/// What a file tool needs to know of its call.
pub struct Context<'a> {
    pub daemon: &'a Daemon,
    pub agent: &'a str,
    pub place: &'a Place,
    /// The user or the judge allowed the call, of this class: its path may be outside the place.
    pub approved: Option<Class>,
}

impl Context<'_> {
    /// The path the call names, resolved and decided.
    fn path(&self, repository: Option<&str>, named: &str) -> Result<Resolved, Outcome> {
        let (_, base) = self.place.base(repository).map_err(Outcome::Refused)?;
        let resolved = self.place.resolve(base, named);
        if self.approved == Some(Class::Judge)
            && !resolved.inside
            && let Some(why) = self.base_now(&resolved)
        {
            return Err(changed(&resolved, &why));
        }
        if self.approved.is_some() {
            return Ok(resolved);
        }
        match gate::contained(std::slice::from_ref(&resolved)) {
            Verdict::Deny(why) => Err(Outcome::Refused(why)),
            _ => Ok(resolved),
        }
    }

    /// Why the gate would now hold a path outside the place for the user, if it would.
    fn base_now(&self, path: &Resolved) -> Option<String> {
        let paths = &self.daemon.paths;
        let own = [
            paths.data.clone(),
            paths.config.clone(),
            paths.state.clone(),
            paths.runtime.clone(),
        ];
        let groups = crate::runs::groups_of(self.daemon, self.agent);
        gate::process(&groups, path).or_else(|| {
            gate::sensitive(&own, path)
                .map(|place| format!("{} is a sensitive place ({place})", path.shown))
        })
    }

    /// Refuses a path outside the place that no longer leads where it did when the call resolved
    /// it, or, allowed as a call to judge, now leads where the gate holds for the user: a folder
    /// of it may have become a link meanwhile. Checked just before the file is opened or renamed.
    fn still(&self, path: &Resolved) -> Result<(), Outcome> {
        if path.inside {
            return Ok(());
        }
        let now = self.place.at(&path.path);
        let why = if now.path != path.path || now.unfollowed.is_some() {
            Some(format!("it now leads to {}", now.shown))
        } else if self.approved == Some(Class::Judge) {
            self.base_now(&now)
        } else {
            None
        };
        match why {
            Some(why) => Err(changed(path, &why)),
            None => Ok(()),
        }
    }

    fn key(path: &Path) -> String {
        path.to_string_lossy().into_owned()
    }

    fn fingerprint(&self, path: &Path) -> Result<Option<String>, Outcome> {
        self.daemon
            .read(|store| store.file_read(self.agent, &Context::key(path)))
            .map_err(|e| Outcome::Failed(e.to_string()))
    }

    /// What the session now knows of the file: its fingerprint, and its text when small.
    fn remember(&self, path: &Path, sha: &str, text: Option<String>) -> Result<(), Outcome> {
        self.daemon
            .read(|store| store.set_file_read(self.agent, &Context::key(path), sha))
            .map_err(|e| Outcome::Failed(e.to_string()))?;
        self.daemon.mcp.kept.keep(self.agent, path, text);
        Ok(())
    }
}

/// The path a file tool's call names, resolved, for the gate: a call whose arguments do not fit
/// reaches none, and its tool refuses it.
pub fn reaches(place: &Place, tool: &str, args: &Value) -> Vec<Resolved> {
    #[derive(Deserialize)]
    struct Named {
        repository: Option<String>,
        path: Option<String>,
    }
    let args = if args.is_null() {
        json!({})
    } else {
        args.clone()
    };
    let Ok(named) = serde_json::from_value::<Named>(args) else {
        return Vec::new();
    };
    // A search starts at the top of its repository.
    let path = if tool == "search" {
        Some(".".to_owned())
    } else {
        named.path
    };
    match (place.base(named.repository.as_deref()), path) {
        (Ok((_, base)), Some(path)) => vec![place.resolve(base, &path)],
        _ => Vec::new(),
    }
}

/// The refusal of a path that changed between the moment it was allowed and its use.
fn changed(path: &Resolved, why: &str) -> Outcome {
    Outcome::Refused(format!(
        "refused: {}: the path changed since it was allowed ({why}); nothing was done",
        path.shown
    ))
}

fn not_regular(path: &Resolved) -> Outcome {
    Outcome::Refused(format!(
        "refused: {} is not a regular file (a pipe, a socket or a device): this tool reads files",
        path.shown
    ))
}

/// Opens a regular file to read. Opening a pipe waits until someone writes to it, which would hold
/// the call's thread forever: it is opened without waiting, and refused (`InvalidInput`, which
/// `failed` words). The path is a real one: its last part become a link since is not followed.
fn open_regular(path: &Path) -> std::io::Result<File> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(path)?;
    if file.metadata()?.is_file() {
        Ok(file)
    } else {
        Err(std::io::Error::from(ErrorKind::InvalidInput))
    }
}

fn read_regular(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    open_regular(path)?.read_to_end(&mut bytes)?;
    Ok(bytes)
}

pub fn failed(path: &Resolved, error: std::io::Error) -> Outcome {
    match error.kind() {
        ErrorKind::InvalidInput => not_regular(path),
        ErrorKind::NotFound => Outcome::Refused(format!("refused: {} does not exist", path.shown)),
        _ => Outcome::Failed(format!("{}: {error}", path.shown)),
    }
}

pub fn sha256(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = open_regular(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; CHUNK];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

// Reading.

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArguments {
    repository: Option<String>,
    path: String,
    range: Option<Range>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Range {
    offset: Option<u64>,
    length: Option<u64>,
}

/// Where a page starts and how long it is, checked.
pub fn page_of(range: Option<&Range>) -> Result<(u64, u64), Outcome> {
    let (offset, length) = match range {
        None => (0, READ_PAGE_BYTES),
        Some(range) => (
            range.offset.unwrap_or(0),
            range.length.unwrap_or(READ_PAGE_BYTES),
        ),
    };
    if !(1..=READ_PAGE_BYTES).contains(&length) {
        return Err(Outcome::Refused(format!(
            "refused: range.length must be 1 to {READ_PAGE_BYTES} bytes: read a longer file a \
             page at a time, each page names the next offset"
        )));
    }
    Ok((offset, length))
}

pub fn fs_read(cx: &Context, args: Value) -> Outcome {
    let args: ReadArguments = match arguments("fs_read", args) {
        Ok(args) => args,
        Err(refusal) => return refusal,
    };
    let (offset, length) = match page_of(args.range.as_ref()) {
        Ok(page) => page,
        Err(refusal) => return refusal,
    };
    let path = match cx.path(args.repository.as_deref(), &args.path) {
        Ok(path) => path,
        Err(refusal) => return refusal,
    };
    match read_page(Some(cx), &path, offset, length) {
        Ok(outcome) => outcome,
        Err(error) => failed(&path, error),
    }
}

/// A page of a file. `cx` is the session that read it, which then knows its fingerprint (what
/// lets it write the file); `None` for a read that grants nothing, such as a reference's.
pub fn read_page(
    cx: Option<&Context>,
    path: &Resolved,
    offset: u64,
    length: u64,
) -> std::io::Result<Outcome> {
    if let Some(Err(outcome)) = cx.map(|cx| cx.still(path)) {
        return Ok(outcome);
    }
    let metadata = std::fs::metadata(&path.path)?;
    if metadata.is_dir() {
        return Ok(Outcome::Refused(format!(
            "refused: {} is a folder: list it with fs_list",
            path.shown
        )));
    }
    if !metadata.is_file() {
        return Ok(not_regular(path));
    }
    let size = metadata.len();
    if offset > size {
        return Ok(Outcome::Refused(format!(
            "refused: offset {offset} is past the end of {} ({size} bytes)",
            path.shown
        )));
    }
    if let Some(cx) = cx {
        let sha = sha256_file(&path.path)?;
        let kept = (size <= KEPT_BYTES)
            .then(|| read_regular(&path.path).ok())
            .flatten()
            .and_then(|bytes| String::from_utf8(bytes).ok());
        if let Err(outcome) = cx.remember(&path.path, &sha, kept) {
            return Ok(outcome);
        }
    }

    let mut file = open_regular(&path.path)?;
    // A few bytes more than the page: an offset inside a character skips its continuation bytes,
    // and a page that ends inside one ends before it.
    file.seek(SeekFrom::Start(offset))?;
    let mut raw = Vec::new();
    file.by_ref().take(length + 3).read_to_end(&mut raw)?;
    let skipped = raw.iter().take_while(|b| continuation(**b)).count();
    let start = offset + skipped as u64;
    let mut raw = raw.split_off(skipped);
    if raw.len() > length as usize {
        let mut cut = length as usize;
        while cut > 0 && continuation(raw[cut]) {
            cut -= 1;
        }
        raw.truncate(cut.max(1));
    }
    let mut window = raw;
    if start + (window.len() as u64) < size
        && let Some(newline) = window.iter().rposition(|b| *b == b'\n')
    {
        window.truncate(newline + 1);
    }
    let first_line = 1 + newlines_before(&mut file, start)?;

    let mut body = String::new();
    let mut taken = 0;
    for (n, line) in window.split_inclusive(|b| *b == b'\n').enumerate() {
        let text = String::from_utf8_lossy(line);
        let rendered = format!("{:6}\t{}", first_line + n as u64, text);
        if body.len() + rendered.len() > BODY_BUDGET {
            if taken == 0 {
                // One line longer than a page: its start, cut on a character.
                let mut cut = BODY_BUDGET.min(line.len());
                while cut > 1 && continuation(line[cut]) {
                    cut -= 1;
                }
                body.push_str(&format!(
                    "{:6}\t{}",
                    first_line,
                    String::from_utf8_lossy(&line[..cut])
                ));
                taken = cut;
            }
            break;
        }
        body.push_str(&rendered);
        taken += line.len();
    }
    let end = start + taken as u64;
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    let next = (end < size).then_some(end);
    let tail = match next {
        Some(next) if taken > 0 => format!(
            "(bytes {start}-{} of {size}; the next page starts at offset {next})",
            end - 1
        ),
        _ if taken > 0 => format!("(bytes {start}-{} of {size}, the end of the file)", end - 1),
        _ => format!("(no bytes at offset {start} of {size}: the end of the file)"),
    };
    let range = json!({"offset": start, "end": end, "size": size, "next": next});
    Ok(Outcome::Done(format!("{body}{tail}\n{range}")))
}

fn continuation(byte: u8) -> bool {
    byte & 0b1100_0000 == 0b1000_0000
}

fn newlines_before(file: &mut File, offset: u64) -> std::io::Result<u64> {
    file.seek(SeekFrom::Start(0))?;
    let mut count = 0;
    let mut left = offset;
    let mut buffer = vec![0; CHUNK];
    while left > 0 {
        let want = (left as usize).min(CHUNK);
        let read = file.read(&mut buffer[..want])?;
        if read == 0 {
            break;
        }
        count += buffer[..read].iter().filter(|b| **b == b'\n').count() as u64;
        left -= read as u64;
    }
    Ok(count)
}

// Listing.

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListArguments {
    repository: Option<String>,
    path: String,
    depth: Option<u8>,
}

pub fn fs_list(cx: &Context, args: Value) -> Outcome {
    let args: ListArguments = match arguments("fs_list", args) {
        Ok(args) => args,
        Err(refusal) => return refusal,
    };
    let depth = args.depth.unwrap_or(1);
    if !(1..=LIST_DEPTH_MAX).contains(&depth) {
        return Outcome::Refused(format!(
            "refused: depth must be 1 to {LIST_DEPTH_MAX}: list a folder below to go deeper"
        ));
    }
    let path = match cx.path(args.repository.as_deref(), &args.path) {
        Ok(path) => path,
        Err(refusal) => return refusal,
    };
    let metadata = match std::fs::symlink_metadata(&path.path) {
        Ok(metadata) => metadata,
        Err(error) => return failed(&path, error),
    };
    let mut lines = Vec::new();
    if !metadata.is_dir() {
        lines.push(entry_line(&path.shown, &path.path, &metadata));
        return Outcome::Done(lines.join("\n"));
    }
    let mut size = 0;
    let stopped = list(&path.path, Path::new(""), depth, &mut lines, &mut size);
    let mut text = if lines.is_empty() {
        format!("{} is empty", path.shown)
    } else {
        lines.join("\n")
    };
    match stopped {
        Some(Stop::Entries) => {
            text.push_str(&format!(
                "\n… stopped at {LIST_ENTRIES_MAX} entries: list a folder below, or a smaller depth"
            ));
        }
        Some(Stop::Budget) => text.push_str(&format!(
            "\n… stopped at {} entries to stay short: list a folder below, or a smaller depth",
            lines.len()
        )),
        None => {}
    }
    Outcome::Done(text)
}

enum Stop {
    Entries,
    Budget,
}

fn entry_line(shown: &str, path: &Path, metadata: &std::fs::Metadata) -> String {
    if metadata.file_type().is_symlink() {
        let target = std::fs::read_link(path)
            .map(|t| t.display().to_string())
            .unwrap_or_default();
        format!("{shown}  link  -> {target}")
    } else if metadata.is_dir() {
        format!("{shown}/  folder")
    } else {
        format!("{shown}  file  {}", metadata.len())
    }
}

/// The entries under `folder`, sorted, a folder's own after it, `.git` left out; why it stopped
/// early, if it did.
fn list(
    folder: &Path,
    relative: &Path,
    depth: u8,
    lines: &mut Vec<String>,
    size: &mut usize,
) -> Option<Stop> {
    let entries = match sorted_entries(&folder.join(relative)) {
        Ok(entries) => entries,
        Err(error) => {
            lines.push(format!("{}/  unreadable: {error}", relative.display()));
            return None;
        }
    };
    for name in entries {
        if name == ".git" {
            continue;
        }
        let path = relative.join(&name);
        let Ok(metadata) = std::fs::symlink_metadata(folder.join(&path)) else {
            continue;
        };
        if lines.len() == LIST_ENTRIES_MAX {
            return Some(Stop::Entries);
        }
        let line = entry_line(&path.display().to_string(), &folder.join(&path), &metadata);
        if *size + line.len() > BODY_BUDGET {
            return Some(Stop::Budget);
        }
        *size += line.len() + 1;
        lines.push(line);
        if metadata.is_dir()
            && depth > 1
            && let Some(stop) = list(folder, &path, depth - 1, lines, size)
        {
            return Some(stop);
        }
    }
    None
}

fn sorted_entries(folder: &Path) -> std::io::Result<Vec<String>> {
    let mut names: Vec<String> = std::fs::read_dir(folder)?
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .collect();
    names.sort();
    Ok(names)
}

// Searching.

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArguments {
    pattern: String,
    repository: Option<String>,
    glob: Option<String>,
    cursor: Option<String>,
}

pub fn search(cx: &Context, args: Value) -> Outcome {
    let args: SearchArguments = match arguments("search", args) {
        Ok(args) => args,
        Err(refusal) => return refusal,
    };
    if args.pattern.is_empty() {
        return Outcome::Refused("refused: the pattern is empty: name the text to find".into());
    }
    let base = match cx.path(args.repository.as_deref(), ".") {
        Ok(base) => base,
        Err(refusal) => return refusal,
    };
    let cursor = match args.cursor.as_deref().map(parse_cursor).transpose() {
        Ok(cursor) => cursor,
        Err(refusal) => return refusal,
    };
    let ignored = std::fs::read_to_string(base.path.join(".gitignore"))
        .map(|text| ignore_patterns(&text))
        .unwrap_or_default();
    let mut files = Vec::new();
    walk(&base.path, Path::new(""), &ignored, &mut files);

    let needle = args.pattern.to_lowercase();
    let mut hits = Vec::new();
    let mut hits_size = 0;
    let mut scanned = 0;
    let mut skipped = Vec::new();
    let mut stop: Option<(String, String)> = None;
    'files: for relative in files {
        let shown = cx.place.shown(&base.path.join(&relative));
        let mut from_line = 1;
        if let Some((path, line)) = &cursor {
            match walk_order(&shown, path) {
                std::cmp::Ordering::Less => continue,
                std::cmp::Ordering::Equal => from_line = *line,
                std::cmp::Ordering::Greater => {}
            }
        }
        if let Some(glob) = &args.glob
            && !path_matches(glob, &relative)
        {
            continue;
        }
        // Line by line: a file far larger than what one call scans is never read whole.
        let Ok(file) = open_regular(&base.path.join(&relative)) else {
            continue;
        };
        let mut reader = BufReader::with_capacity(CHUNK, file);
        if reader.fill_buf().is_ok_and(|start| start.contains(&0)) {
            skipped.push(shown);
            continue;
        }
        let mut line = Vec::new();
        let mut number = 0;
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => number += 1,
            }
            if number < from_line {
                continue;
            }
            let reason = if hits.len() == SEARCH_MATCH_LIMIT {
                Some(format!("stopped at {SEARCH_MATCH_LIMIT} matches"))
            } else if scanned >= SEARCH_SCAN_BYTES {
                Some(format!("stopped at {SEARCH_SCAN_BYTES} bytes scanned"))
            } else if hits_size > BODY_BUDGET {
                Some("stopped to stay short".to_owned())
            } else {
                None
            };
            if let Some(reason) = reason {
                stop = Some((reason, format!("{shown}:{number}")));
                break 'files;
            }
            scanned += line.len();
            let text = String::from_utf8_lossy(&line);
            if text.to_lowercase().contains(&needle) {
                let hit = format!("{shown}:{number}: {}", cut_chars(text.trim(), HIT_CHARS));
                hits_size += hit.len() + 1;
                hits.push(hit);
            }
        }
    }

    let mut out = vec![format!(
        "{} match(es) for {:?} in {scanned} bytes scanned",
        hits.len(),
        args.pattern
    )];
    out.push(match &stop {
        Some((reason, cursor)) => {
            format!("{reason}: go on with the cursor {cursor:?}")
        }
        None => "every file was searched".to_owned(),
    });
    out.push(format!(
        "limits: {SEARCH_MATCH_LIMIT} matches and {SEARCH_SCAN_BYTES} bytes scanned per call, \
         lines cut at {HIT_CHARS} characters; .git, node_modules and the .gitignore's patterns \
         are left out"
    ));
    if !skipped.is_empty() {
        let more = skipped.len().saturating_sub(SEARCH_SKIPS_LISTED);
        let mut line = format!(
            "binary files skipped: {}",
            skipped[..skipped.len().min(SEARCH_SKIPS_LISTED)].join(", ")
        );
        if more > 0 {
            line.push_str(&format!(" and {more} more"));
        }
        out.push(line);
    }
    out.extend(hits);
    Outcome::Done(out.join("\n"))
}

fn parse_cursor(cursor: &str) -> Result<(String, u64), Outcome> {
    cursor
        .rsplit_once(':')
        .and_then(|(path, line)| Some((path.to_owned(), line.parse().ok()?)))
        .ok_or_else(|| {
            Outcome::Refused(format!(
                "refused: the cursor {cursor:?} is not one a search gave: it reads path:line"
            ))
        })
}

/// The order of the walk, depth first and sorted by name: a comparison of the parts.
fn walk_order(a: &str, b: &str) -> std::cmp::Ordering {
    a.split('/').cmp(b.split('/'))
}

fn cut_chars(text: &str, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_owned(),
    }
}

/// A `.gitignore` pattern, the simple kinds only: no `!`.
struct Ignore {
    parts: Vec<String>,
    anchored: bool,
    folder_only: bool,
}

fn ignore_patterns(text: &str) -> Vec<Ignore> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with('!'))
        .map(|line| {
            let folder_only = line.ends_with('/');
            let line = line.trim_end_matches('/');
            let anchored = line.contains('/');
            Ignore {
                parts: line
                    .trim_start_matches('/')
                    .split('/')
                    .map(str::to_owned)
                    .collect(),
                anchored,
                folder_only,
            }
        })
        .collect()
}

fn is_ignored(patterns: &[Ignore], relative: &Path, folder: bool) -> bool {
    let parts: Vec<&str> = relative.iter().filter_map(|p| p.to_str()).collect();
    let name = parts.last().copied().unwrap_or_default();
    patterns.iter().any(|pattern| {
        if pattern.folder_only && !folder {
            return false;
        }
        if pattern.anchored {
            parts_match(
                &pattern.parts.iter().map(String::as_str).collect::<Vec<_>>(),
                &parts,
            )
        } else {
            glob::matches(&pattern.parts[0], name)
        }
    })
}

/// Whether the parts of a pattern match the parts of a path, `**` taking any number of them.
fn parts_match(pattern: &[&str], path: &[&str]) -> bool {
    match (pattern.first(), path.first()) {
        (None, None) => true,
        (Some(&"**"), _) => {
            parts_match(&pattern[1..], path)
                || (!path.is_empty() && parts_match(pattern, &path[1..]))
        }
        (Some(part), Some(name)) => {
            glob::matches(part, name) && parts_match(&pattern[1..], &path[1..])
        }
        _ => false,
    }
}

/// The `glob` filter of a search: on the file's name alone when it has no `/`.
fn path_matches(glob: &str, relative: &Path) -> bool {
    let parts: Vec<&str> = relative.iter().filter_map(|p| p.to_str()).collect();
    if !glob.contains('/') {
        return parts.last().is_some_and(|name| glob::matches(glob, name));
    }
    parts_match(&glob.split('/').collect::<Vec<_>>(), &parts)
}

/// The files under `base`, depth first and sorted, never in `.git` nor `node_modules`, nor what
/// the `.gitignore` names; links are not followed, so that nothing outside is read.
fn walk(base: &Path, relative: &Path, ignored: &[Ignore], files: &mut Vec<PathBuf>) {
    let Ok(names) = sorted_entries(&base.join(relative)) else {
        return;
    };
    for name in names {
        if name == ".git" || name == "node_modules" {
            continue;
        }
        let path = relative.join(&name);
        let Ok(metadata) = std::fs::symlink_metadata(base.join(&path)) else {
            continue;
        };
        if metadata.file_type().is_symlink() || is_ignored(ignored, &path, metadata.is_dir()) {
            continue;
        }
        if metadata.is_dir() {
            walk(base, &path, ignored, files);
        } else if metadata.is_file() {
            files.push(path);
        }
    }
}

// Writing.

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteArguments {
    repository: Option<String>,
    path: String,
    content: String,
    #[allow(dead_code)]
    why: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditArguments {
    repository: Option<String>,
    path: String,
    edits: Vec<Change>,
    #[allow(dead_code)]
    why: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    old: String,
    new: String,
}

pub fn fs_write(cx: &Context, args: Value) -> Outcome {
    let args: WriteArguments = match arguments("fs_write", args) {
        Ok(args) => args,
        Err(refusal) => return refusal,
    };
    let path = match cx.path(args.repository.as_deref(), &args.path) {
        Ok(path) => path,
        Err(refusal) => return refusal,
    };
    let existed = match fresh(cx, &path) {
        Ok(existed) => existed,
        Err(refusal) => return refusal,
    };
    match write(cx, &path, args.content) {
        Ok(size) => Outcome::Done(format!(
            "{} {} ({size} bytes)",
            if existed { "wrote" } else { "made" },
            path.shown
        )),
        Err(outcome) => outcome,
    }
}

pub fn fs_edit(cx: &Context, args: Value) -> Outcome {
    let args: EditArguments = match arguments("fs_edit", args) {
        Ok(args) => args,
        Err(refusal) => return refusal,
    };
    if !(1..=EDITS_MAX).contains(&args.edits.len()) {
        return Outcome::Refused(format!(
            "refused: an fs_edit holds 1 to {EDITS_MAX} edits: split it"
        ));
    }
    if let Some(n) = args.edits.iter().position(|e| e.old.is_empty()) {
        return Outcome::Refused(format!(
            "refused: the anchor of edit {} is empty: give the text it replaces",
            n + 1
        ));
    }
    let path = match cx.path(args.repository.as_deref(), &args.path) {
        Ok(path) => path,
        Err(refusal) => return refusal,
    };
    match std::fs::symlink_metadata(&path.path) {
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Outcome::Refused(format!(
                "refused: {} does not exist: fs_edit changes a file that exists, fs_write makes \
                 one",
                path.shown
            ));
        }
        Ok(metadata) if !metadata.is_file() && !metadata.is_dir() => return not_regular(&path),
        _ => {}
    }
    if let Err(refusal) = fresh(cx, &path) {
        return refusal;
    }
    let mut text = match read_regular(&path.path).map(String::from_utf8) {
        Ok(Ok(text)) => text,
        Ok(Err(_)) => {
            return Outcome::Refused(format!(
                "refused: {} is not UTF-8 text: fs_edit changes text, fs_write replaces a file",
                path.shown
            ));
        }
        Err(error) => return failed(&path, error),
    };
    for (n, change) in args.edits.iter().enumerate() {
        let (count, at) = occurrences(&text, &change.old);
        let Some(at) = at.filter(|_| count == 1) else {
            return Outcome::Refused(format!(
                "refused: the anchor of edit {} appears {count} time(s) in {}; it must appear \
                 exactly once. Nothing was changed",
                n + 1,
                path.shown
            ));
        };
        // Spliced: the new text is taken as it is, never as a pattern.
        text.replace_range(at..at + change.old.len(), &change.new);
    }
    match write(cx, &path, text) {
        Ok(_) => Outcome::Done(format!(
            "edited {}: {} change(s)",
            path.shown,
            args.edits.len()
        )),
        Err(outcome) => outcome,
    }
}

/// How many times `anchor` appears in `text`, overlapping ones counted, and where it first does.
fn occurrences(text: &str, anchor: &str) -> (usize, Option<usize>) {
    let mut count = 0;
    let mut first = None;
    let mut from = 0;
    while let Some(found) = text[from..].find(anchor) {
        let at = from + found;
        count += 1;
        first.get_or_insert(at);
        from = at + text[at..].chars().next().map_or(1, char::len_utf8);
    }
    (count, first)
}

/// Whether the session may write the file as it is now: it read it as it is, or there is none
/// and it never read one. Whether the file exists.
fn fresh(cx: &Context, path: &Resolved) -> Result<bool, Outcome> {
    let known = cx.fingerprint(&path.path)?;
    let exists = match std::fs::metadata(&path.path) {
        Ok(metadata) if metadata.is_dir() => {
            return Err(Outcome::Refused(format!(
                "refused: {} is a folder",
                path.shown
            )));
        }
        Ok(_) => true,
        Err(error) if error.kind() == ErrorKind::NotFound => false,
        Err(error) => return Err(failed(path, error)),
    };
    match (exists, known) {
        (false, None) => Ok(false),
        (false, Some(_)) => Err(Outcome::Refused(format!(
            "refused: {} was removed since this session read it; nothing was written",
            path.shown
        ))),
        (true, None) => Err(Outcome::Refused(format!(
            "refused: {} exists and this session has not read it: read it with fs_read first",
            path.shown
        ))),
        (true, Some(known)) => {
            let now = sha256_file(&path.path).map_err(|e| failed(path, e))?;
            if now == known {
                return Ok(true);
            }
            let what = match (
                cx.daemon.mcp.kept.text(cx.agent, &path.path),
                std::fs::read_to_string(&path.path),
            ) {
                (Some(before), Ok(after)) if sha256(before.as_bytes()) == known => {
                    crate::mcp::masked(&diff(&before, &after), cx.place)
                }
                _ => "the version it read is no longer kept, so what changed cannot be shown"
                    .to_owned(),
            };
            Err(Outcome::Refused(format!(
                "refused: {} changed since this session read it; read it again before writing. \
                 What changed:\n{what}",
                path.shown
            )))
        }
    }
}

/// The lines that differ between two texts, at most `DIFF_LINES`: what is between their common
/// start and their common end.
fn diff(before: &str, after: &str) -> String {
    let (old, new): (Vec<&str>, Vec<&str>) = (before.lines().collect(), after.lines().collect());
    let start = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let end = old[start..]
        .iter()
        .rev()
        .zip(new[start..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let lines: Vec<String> = old[start..old.len() - end]
        .iter()
        .map(|line| format!("-{}", cut_chars(line, DIFF_LINE_CHARS)))
        .chain(
            new[start..new.len() - end]
                .iter()
                .map(|line| format!("+{}", cut_chars(line, DIFF_LINE_CHARS))),
        )
        .collect();
    let mut shown = lines[..lines.len().min(DIFF_LINES)].join("\n");
    if lines.len() > DIFF_LINES {
        shown.push_str(&format!("\n… and {} more lines", lines.len() - DIFF_LINES));
    }
    if shown.is_empty() {
        shown = "only the ends of its lines".to_owned();
    }
    format!("@@ line {} @@\n{shown}", start + 1)
}

/// Refuses unless the folder the file goes in is still inside the place on the disk: a folder
/// swapped for a link since the path was resolved would take the write out of it.
fn check_parent(place: &Place, parent: &Path, path: &Resolved) -> Result<(), Outcome> {
    // Outside the place, the user or the judge allowed it: `Context::still` checks it.
    if !path.inside || place.holds(parent) {
        return Ok(());
    }
    Err(Outcome::Refused(format!(
        "refused: {}: its folder is no longer inside this agent's place (it changed while the \
         call ran); nothing was written",
        path.shown
    )))
}

/// Writes the file through a temporary file renamed over it (which replaces a link rather than
/// writing through it), keeping its mode; then the session knows it as written. The folder is
/// checked to be inside the place (or, outside, to lead where it did) after it is made and again
/// just before the rename.
fn write(cx: &Context, path: &Resolved, content: String) -> Result<usize, Outcome> {
    let failed = |error: std::io::Error| Outcome::Failed(format!("{}: {error}", path.shown));
    let parent = path.path.parent().unwrap_or(Path::new("/"));
    // Outside the place, no folder is made where the path no longer leads.
    cx.still(path)?;
    std::fs::create_dir_all(parent).map_err(failed)?;
    check_parent(cx.place, parent, path)?;
    let name = path
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let id = flottille_core::agent::new_id().map_err(failed)?;
    let temporary = parent.join(format!(".{name}.{id}.flottille"));
    let mode = std::fs::metadata(&path.path).ok().map(|m| m.permissions());
    let written = std::fs::write(&temporary, content.as_bytes())
        .and_then(|()| match mode {
            Some(mode) => std::fs::set_permissions(&temporary, mode),
            None => std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o644)),
        })
        .map_err(failed)
        .and_then(|()| check_parent(cx.place, parent, path))
        .and_then(|()| cx.still(path))
        .and_then(|()| std::fs::rename(&temporary, &path.path).map_err(failed));
    if let Err(outcome) = written {
        let _ = std::fs::remove_file(&temporary);
        return Err(outcome);
    }
    let size = content.len();
    let sha = sha256(content.as_bytes());
    let kept = (size as u64 <= KEPT_BYTES).then_some(content);
    cx.remember(&path.path, &sha, kept)?;
    Ok(size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_replaced_by_a_link_out_is_no_longer_inside() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::create_dir(dir.path().join("elsewhere")).unwrap();
        let place = Place {
            project: "app".into(),
            workspace: None,
            root: root.clone(),
            repositories: vec![(".".into(), root.clone())],
            branch: String::new(),
            variables: Default::default(),
            home: None,
        };
        let resolved = place.resolve(&root, "sub/new.txt");
        let parent = resolved.path.parent().unwrap();
        assert!(resolved.inside);
        assert!(check_parent(&place, parent, &resolved).is_ok());

        // Between the resolution and the write.
        std::fs::remove_dir(root.join("sub")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("elsewhere"), root.join("sub")).unwrap();

        let Err(Outcome::Refused(refusal)) = check_parent(&place, parent, &resolved) else {
            panic!("a folder swapped for a link out must be refused");
        };
        assert!(
            refusal.starts_with("refused: sub/new.txt: its folder is no longer inside"),
            "{refusal}"
        );
    }

    #[test]
    fn overlapping_anchors_are_counted() {
        assert_eq!(occurrences("aaa", "aa"), (2, Some(0)));
        assert_eq!(occurrences("éaé", "é"), (2, Some(0)));
        assert_eq!(occurrences("abc", "x"), (0, None));
    }

    #[test]
    fn a_diff_shows_only_what_changed() {
        assert_eq!(
            diff("one\ntwo\nthree\n", "one\nTWO\nthree\n"),
            "@@ line 2 @@\n-two\n+TWO"
        );
        let long: String = (0..30).map(|n| format!("{n}\n")).collect();
        let shown = diff("", &long);
        assert!(shown.ends_with("… and 10 more lines"), "{shown}");
    }

    #[test]
    fn simple_gitignore_patterns_apply_and_negations_do_not() {
        let patterns = ignore_patterns("# c\n*.log\n/build/\ndocs/out\n!keep.log\n");
        assert!(is_ignored(&patterns, Path::new("a/b.log"), false));
        assert!(is_ignored(&patterns, Path::new("build"), true));
        assert!(!is_ignored(&patterns, Path::new("build"), false));
        assert!(!is_ignored(&patterns, Path::new("src/build"), true));
        assert!(is_ignored(&patterns, Path::new("docs/out"), true));
        assert!(!is_ignored(&patterns, Path::new("src/main.rs"), false));
    }

    #[test]
    fn a_search_glob_takes_any_depth_with_two_stars() {
        assert!(path_matches("*.rs", Path::new("src/deep/b.rs")));
        assert!(path_matches("**/b.rs", Path::new("src/deep/b.rs")));
        assert!(path_matches("**/b.rs", Path::new("b.rs")));
        assert!(path_matches("src/*.rs", Path::new("src/a.rs")));
        assert!(!path_matches("src/*.rs", Path::new("src/deep/b.rs")));
    }

    #[test]
    fn the_walk_order_compares_parts() {
        use std::cmp::Ordering::*;
        assert_eq!(walk_order("a/b", "a.txt"), Less);
        assert_eq!(walk_order("many/02.txt", "many/02.txt"), Equal);
        assert_eq!(walk_order("z", "a/b"), Greater);
    }
}
