//! The file tools: `fs_read`, `fs_list`, `search`, `fs_write`, `fs_edit` (`docs/tools.md`,
//! `docs/reference/hemera.md` §3).
//!
//! Writing is optimistic per session: a file the agent has not read, or that changed since it
//! read it, is never written over. Results are cut to stay well under what the agent keeps in its
//! context (`docs/reference/mcp.md` §4 rules 5 and 8), and each says how to get the next part.

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{BufRead, BufReader, ErrorKind, Read, Seek, SeekFrom};
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use flottille_core::glob;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::daemon::Daemon;
use crate::gate::{self, Class, Verdict};
use crate::mcp::{Outcome, arguments};
use crate::nofollow::{self, Entry};
use crate::place::{Place, Resolved, Unfollowed};

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
        if let Some(refusal) = never_opened(&resolved) {
            return Err(refusal);
        }
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

/// The refusal of a path the file tools never open, whoever allowed it: one that goes up out of a
/// folder that is not there, or through a process's folder in `/proc`, which leads anywhere. A
/// path that could not be followed for another reason is opened as written, without following a
/// link (`failed` says it).
fn never_opened(path: &Resolved) -> Option<Outcome> {
    match path.unfollowed.as_ref()? {
        Unfollowed::UpFromMissing => Some(Outcome::Refused(gate::malformed(
            std::slice::from_ref(path),
        )?)),
        through @ Unfollowed::Through(_) => Some(Outcome::Refused(format!(
            "refused: {} {through}: Flottille's file tools never open a path through a \
             process's folder in /proc; nothing was done",
            path.shown
        ))),
        Unfollowed::Failed(_) => None,
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

/// Opens a regular file to read. A pipe, a socket or a device is never opened, only looked at,
/// and refused (`InvalidInput`, which `failed` words): opening one can wait for a writer or act
/// on the device. The path is a real one: no link on its way is followed (`nofollow`).
fn open_regular(path: &Path) -> std::io::Result<File> {
    nofollow::open_file(path)
}

/// `relative` below the folder `folder`, opened as `open_regular` does.
fn open_regular_beneath(folder: BorrowedFd, relative: &Path) -> std::io::Result<File> {
    nofollow::open_file_beneath(folder, relative)
}

fn read_regular(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    open_regular(path)?.read_to_end(&mut bytes)?;
    Ok(bytes)
}

pub fn failed(path: &Resolved, error: std::io::Error) -> Outcome {
    // A link met where the gate found none: a folder became one since the check.
    if matches!(error.raw_os_error(), Some(libc::ELOOP | libc::EXDEV)) {
        return Outcome::Refused(match &path.unfollowed {
            Some(why) => format!(
                "refused: {} {why}: the file tools follow no link the gate could not follow; \
                 nothing was done",
                path.shown
            ),
            // `/proc/self`, `/proc/mounts`, `/proc/net`: the reader's own, here the daemon's.
            None if path.path.starts_with("/proc") => format!(
                "refused: {} goes through /proc/self, which is the daemon's own: the file tools \
                 do not follow it; nothing was done",
                path.shown
            ),
            None => format!(
                "refused: {}: the path changed since it was checked; nothing was done",
                path.shown
            ),
        });
    }
    match error.kind() {
        ErrorKind::InvalidInput => not_regular(path),
        ErrorKind::IsADirectory => Outcome::Refused(format!("refused: {} is a folder", path.shown)),
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

fn sha256_of(file: &mut File) -> std::io::Result<String> {
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
    let mut file = match open_regular(&path.path) {
        Err(error) if error.kind() == ErrorKind::IsADirectory => {
            return Ok(Outcome::Refused(format!(
                "refused: {} is a folder: list it with fs_list",
                path.shown
            )));
        }
        Err(error) if error.kind() == ErrorKind::InvalidInput => return Ok(not_regular(path)),
        opened => opened?,
    };
    let size = file.metadata()?.len();
    if offset > size {
        return Ok(Outcome::Refused(format!(
            "refused: offset {offset} is past the end of {} ({size} bytes)",
            path.shown
        )));
    }
    if let Some(cx) = cx {
        let (sha, kept) = if size <= KEPT_BYTES {
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            (sha256(&bytes), String::from_utf8(bytes).ok())
        } else {
            (sha256_of(&mut file)?, None)
        };
        if let Err(outcome) = cx.remember(&path.path, &sha, kept) {
            return Ok(outcome);
        }
    }

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
    match cx.path(args.repository.as_deref(), &args.path) {
        Ok(path) => list_path(&path, depth),
        Err(refusal) => refusal,
    }
}

/// What `fs_list` says of a path it may list.
fn list_path(path: &Resolved, depth: u8) -> Outcome {
    let looked = match nofollow::open(&path.path, nofollow::LOOK) {
        Ok(looked) => looked,
        Err(error) => return failed(path, error),
    };
    let metadata = match looked.metadata() {
        Ok(metadata) => metadata,
        Err(error) => return failed(path, error),
    };
    if !metadata.is_dir() {
        let entry = Entry {
            name: path.shown.clone(),
            kind: rustix::fs::FileType::from_raw_mode(metadata.mode()),
            size: metadata.len(),
            target: None,
        };
        return Outcome::Done(entry_line(&path.shown, &entry));
    }
    let folder = match nofollow::folder_beneath(looked.as_fd(), Path::new(".")) {
        Ok(folder) => folder,
        Err(error) => return failed(path, error),
    };
    let mut lines = Vec::new();
    let mut size = 0;
    let stopped = list(folder.as_fd(), Path::new(""), depth, &mut lines, &mut size);
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

fn entry_line(shown: &str, entry: &Entry) -> String {
    match entry.kind {
        rustix::fs::FileType::Symlink => format!(
            "{shown}  link  -> {}",
            entry.target.as_deref().unwrap_or_default()
        ),
        rustix::fs::FileType::Directory => format!("{shown}/  folder"),
        _ => format!("{shown}  file  {}", entry.size),
    }
}

/// The entries under `folder`, sorted, a folder's own after it, `.git` left out; why it stopped
/// early, if it did.
/// `folder` is open; `relative` is how its entries are shown. Each folder below is opened in the
/// one above it, never through a link.
fn list(
    folder: BorrowedFd,
    relative: &Path,
    depth: u8,
    lines: &mut Vec<String>,
    size: &mut usize,
) -> Option<Stop> {
    let entries = match nofollow::entries(folder) {
        Ok(entries) => entries,
        Err(error) => {
            lines.push(format!("{}/  unreadable: {error}", relative.display()));
            return None;
        }
    };
    for entry in entries {
        if entry.name == ".git" {
            continue;
        }
        let path = relative.join(&entry.name);
        if lines.len() == LIST_ENTRIES_MAX {
            return Some(Stop::Entries);
        }
        let line = entry_line(&path.display().to_string(), &entry);
        if *size + line.len() > BODY_BUDGET {
            return Some(Stop::Budget);
        }
        *size += line.len() + 1;
        lines.push(line);
        if entry.kind == rustix::fs::FileType::Directory && depth > 1 {
            let stop = match nofollow::folder_beneath(folder, Path::new(&entry.name)) {
                Ok(below) => list(below.as_fd(), &path, depth - 1, lines, size),
                Err(error) => {
                    lines.push(format!("{}/  unreadable: {error}", path.display()));
                    None
                }
            };
            if stop.is_some() {
                return stop;
            }
        }
    }
    None
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
    match cx.path(args.repository.as_deref(), ".") {
        Ok(base) => search_in(cx.place, &base, &args),
        Err(refusal) => refusal,
    }
}

/// A search below `base`, whose files are each opened below its folder, never through a link.
fn search_in(place: &Place, base: &Resolved, args: &SearchArguments) -> Outcome {
    let cursor = match args.cursor.as_deref().map(parse_cursor).transpose() {
        Ok(cursor) => cursor,
        Err(refusal) => return refusal,
    };
    let root = match nofollow::open(&base.path, nofollow::LOOK) {
        Ok(root) => root,
        Err(error) => return failed(base, error),
    };
    let ignored = open_regular_beneath(root.as_fd(), Path::new(".gitignore"))
        .and_then(std::io::read_to_string)
        .map(|text| ignore_patterns(&text))
        .unwrap_or_default();
    let mut files = Vec::new();
    match nofollow::folder_beneath(root.as_fd(), Path::new(".")) {
        Ok(folder) => walk(folder.as_fd(), Path::new(""), &ignored, &mut files),
        Err(error) => return failed(base, error),
    }

    let needle = args.pattern.to_lowercase();
    let mut hits = Vec::new();
    let mut hits_size = 0;
    let mut scanned = 0;
    let mut skipped = Vec::new();
    let mut stop: Option<(String, String)> = None;
    'files: for relative in files {
        let shown = place.shown(&base.path.join(&relative));
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
        let Ok(file) = open_regular_beneath(root.as_fd(), &relative) else {
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

/// The files below the open `folder`, depth first and sorted, never in `.git` nor
/// `node_modules`, nor what the `.gitignore` names; links are not followed, so that nothing
/// outside is read.
fn walk(folder: BorrowedFd, relative: &Path, ignored: &[Ignore], files: &mut Vec<PathBuf>) {
    let Ok(entries) = nofollow::entries(folder) else {
        return;
    };
    for entry in entries {
        if entry.name == ".git" || entry.name == "node_modules" {
            continue;
        }
        let path = relative.join(&entry.name);
        let directory = entry.kind == rustix::fs::FileType::Directory;
        if is_ignored(ignored, &path, directory) {
            continue;
        }
        if directory {
            if let Ok(below) = nofollow::folder_beneath(folder, Path::new(&entry.name)) {
                walk(below.as_fd(), &path, ignored, files);
            }
        } else if entry.kind == rustix::fs::FileType::RegularFile {
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
    if let Err(refusal) = exists(&path) {
        return refusal;
    }
    if let Err(refusal) = fresh(cx, &path) {
        return refusal;
    }
    let mut text = match text_of(&path) {
        Ok(text) => text,
        Err(refusal) => return refusal,
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

/// Refuses a path `fs_edit` cannot change: missing, or neither a file nor a folder.
fn exists(path: &Resolved) -> Result<(), Outcome> {
    match nofollow::open(&path.path, nofollow::LOOK).and_then(|looked| looked.metadata()) {
        Err(error) if error.kind() == ErrorKind::NotFound => Err(Outcome::Refused(format!(
            "refused: {} does not exist: fs_edit changes a file that exists, fs_write makes one",
            path.shown
        ))),
        Err(error) => Err(failed(path, error)),
        Ok(metadata) if !metadata.is_file() && !metadata.is_dir() => Err(not_regular(path)),
        Ok(_) => Ok(()),
    }
}

/// The text `fs_edit` changes.
fn text_of(path: &Resolved) -> Result<String, Outcome> {
    match read_regular(&path.path).map(String::from_utf8) {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(_)) => Err(Outcome::Refused(format!(
            "refused: {} is not UTF-8 text: fs_edit changes text, fs_write replaces a file",
            path.shown
        ))),
        Err(error) => Err(failed(path, error)),
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
    let kept = cx.daemon.mcp.kept.text(cx.agent, &path.path);
    fresh_against(cx.place, path, known, kept)
}

/// `fresh`, given what the session knows of the file: its fingerprint, and its text when kept.
/// The file is opened once, without following a link, and what changed is read from it: a path
/// that changed since the check shows nothing of where it now leads.
fn fresh_against(
    place: &Place,
    path: &Resolved,
    known: Option<String>,
    kept: Option<String>,
) -> Result<bool, Outcome> {
    let file = match open_regular(&path.path) {
        Ok(file) => Some(file),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => return Err(failed(path, error)),
    };
    match (file, known) {
        (None, None) => Ok(false),
        (None, Some(_)) => Err(Outcome::Refused(format!(
            "refused: {} was removed since this session read it; nothing was written",
            path.shown
        ))),
        (Some(_), None) => Err(Outcome::Refused(format!(
            "refused: {} exists and this session has not read it: read it with fs_read first",
            path.shown
        ))),
        (Some(mut file), Some(known)) => {
            let now = sha256_of(&mut file).map_err(|e| failed(path, e))?;
            if now == known {
                return Ok(true);
            }
            let after = file
                .seek(SeekFrom::Start(0))
                .and_then(|_| std::io::read_to_string(&mut file));
            let what = match (kept, after) {
                (Some(before), Ok(after)) if sha256(before.as_bytes()) == known => {
                    crate::mcp::masked(&diff(&before, &after), place)
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

/// Writes the file, then the session knows it as written.
fn write(cx: &Context, path: &Resolved, content: String) -> Result<usize, Outcome> {
    put(path, content.as_bytes())?;
    let size = content.len();
    let sha = sha256(content.as_bytes());
    let kept = (size as u64 <= KEPT_BYTES).then_some(content);
    cx.remember(&path.path, &sha, kept)?;
    Ok(size)
}

/// Writes the file through a temporary file renamed over it, keeping its mode. Its folder is
/// opened (and the missing ones made) a folder at a time without following a link, and the
/// temporary file is made and renamed in that open folder: what is written is where the gate
/// checked, whatever became a link since.
fn put(path: &Resolved, content: &[u8]) -> Result<(), Outcome> {
    let failed = |error: std::io::Error| failed(path, error);
    let parent = path.path.parent().unwrap_or(Path::new("/"));
    let Some(name) = path.path.file_name() else {
        return Err(Outcome::Refused(format!(
            "refused: {} is a folder",
            path.shown
        )));
    };
    let folder = nofollow::folder_made(parent).map_err(failed)?;
    let mode = rustix::fs::statat(&folder, name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW)
        .ok()
        .filter(|stat| {
            rustix::fs::FileType::from_raw_mode(stat.st_mode) == rustix::fs::FileType::RegularFile
        })
        .map_or(0o644, |stat| stat.st_mode & 0o7777);
    let id = flottille_core::agent::new_id().map_err(failed)?;
    let temporary = format!(".{}.{id}.flottille", name.to_string_lossy());
    let made = rustix::fs::openat(
        &folder,
        temporary.as_str(),
        rustix::fs::OFlags::WRONLY
            | rustix::fs::OFlags::CREATE
            | rustix::fs::OFlags::EXCL
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::from_raw_mode(0o600),
    )
    .map_err(|e| failed(e.into()))?;
    let mut file = File::from(made);
    let written = std::io::Write::write_all(&mut file, content)
        .and_then(|()| {
            Ok(rustix::fs::fchmod(
                &file,
                rustix::fs::Mode::from_raw_mode(mode),
            )?)
        })
        .and_then(|()| {
            Ok(rustix::fs::renameat(
                &folder,
                temporary.as_str(),
                &folder,
                name,
            )?)
        });
    if let Err(error) = written {
        let _ = rustix::fs::unlinkat(&folder, temporary.as_str(), rustix::fs::AtFlags::empty());
        return Err(failed(error));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    /// A place `app` with `sub/f.txt`, a folder outside it with the same, and a folder of secrets
    /// the swapped folders lead to.
    struct Scene {
        _dir: tempfile::TempDir,
        base: PathBuf,
        root: PathBuf,
        place: Place,
    }

    impl Scene {
        fn new() -> Scene {
            let dir = tempfile::tempdir().unwrap();
            let base = std::fs::canonicalize(dir.path()).unwrap();
            let root = base.join("app");
            for folder in [root.join("sub"), base.join("out/sub")] {
                std::fs::create_dir_all(&folder).unwrap();
                std::fs::write(folder.join("f.txt"), "plain text\n").unwrap();
            }
            std::fs::create_dir_all(base.join("secret")).unwrap();
            std::fs::write(base.join("secret/f.txt"), "SECRET\n").unwrap();
            std::fs::write(base.join("secret/hidden.txt"), "SECRET\n").unwrap();
            let place = Place {
                project: "app".into(),
                workspace: None,
                root: root.clone(),
                repositories: vec![(".".into(), root.clone()), ("sub".into(), root.join("sub"))],
                branch: String::new(),
                variables: Default::default(),
                home: None,
            };
            Scene {
                _dir: dir,
                base,
                root,
                place,
            }
        }

        /// `sub` inside the place, or outside it.
        fn sub(&self, inside: bool) -> PathBuf {
            if inside {
                self.root.join("sub")
            } else {
                self.base.join("out/sub")
            }
        }

        fn resolve(&self, path: &Path) -> Resolved {
            self.place.resolve(&self.root, &path.display().to_string())
        }

        /// The folder made a link to the secrets, after the check.
        fn swap(&self, folder: &Path) {
            std::fs::remove_dir_all(folder).unwrap();
            std::os::unix::fs::symlink(self.base.join("secret"), folder).unwrap();
        }

        fn secrets_untouched(&self) {
            let mut names: Vec<String> = std::fs::read_dir(self.base.join("secret"))
                .unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect();
            names.sort();
            assert_eq!(names, ["f.txt", "hidden.txt"]);
            assert_eq!(
                std::fs::read_to_string(self.base.join("secret/f.txt")).unwrap(),
                "SECRET\n"
            );
        }
    }

    fn said(outcome: &Outcome) -> &str {
        match outcome {
            Outcome::Done(text)
            | Outcome::Refused(text)
            | Outcome::Failed(text)
            | Outcome::Held(text) => text,
        }
    }

    fn changed_refusal(outcome: Outcome, what: &str) {
        let Outcome::Refused(refusal) = outcome else {
            panic!("{what}: not refused: {}", said(&outcome));
        };
        assert!(
            refusal.contains("the path changed since it was checked; nothing was done"),
            "{what}: {refusal}"
        );
        assert!(!refusal.contains("SECRET"), "{what}: {refusal}");
        assert!(!refusal.contains("hidden"), "{what}: {refusal}");
    }

    #[test]
    fn a_folder_swapped_for_a_link_between_the_check_and_the_use_is_refused() {
        for inside in [true, false] {
            let side = if inside { "inside" } else { "outside" };

            let scene = Scene::new();
            let path = scene.resolve(&scene.sub(inside).join("f.txt"));
            assert_eq!(path.inside, inside);
            scene.swap(&scene.sub(inside));
            changed_refusal(
                read_page(None, &path, 0, 100).unwrap_or_else(|e| failed(&path, e)),
                &format!("fs_read {side}"),
            );

            let scene = Scene::new();
            let path = scene.resolve(&scene.sub(inside));
            scene.swap(&scene.sub(inside));
            changed_refusal(list_path(&path, 2), &format!("fs_list {side}"));

            let scene = Scene::new();
            let path = scene.resolve(&scene.sub(inside).join("new/deeper.txt"));
            scene.swap(&scene.sub(inside));
            changed_refusal(
                put(&path, b"written").unwrap_err(),
                &format!("fs_write {side}"),
            );
            scene.secrets_untouched();

            let scene = Scene::new();
            let path = scene.resolve(&scene.sub(inside).join("f.txt"));
            scene.swap(&scene.sub(inside));
            changed_refusal(exists(&path).unwrap_err(), &format!("fs_edit {side}"));
            changed_refusal(text_of(&path).unwrap_err(), &format!("fs_edit {side}"));
            changed_refusal(
                put(&path, b"edited").unwrap_err(),
                &format!("fs_edit {side}"),
            );
            scene.secrets_untouched();

            let scene = Scene::new();
            let base = scene.resolve(&scene.sub(inside));
            scene.swap(&scene.sub(inside));
            let args = SearchArguments {
                pattern: "SECRET".into(),
                repository: None,
                glob: None,
                cursor: None,
            };
            changed_refusal(
                search_in(&scene.place, &base, &args),
                &format!("search {side}"),
            );
        }
    }

    #[test]
    fn a_search_never_opens_a_folder_swapped_for_a_link_below_it() {
        let scene = Scene::new();
        let base = scene.resolve(&scene.root);
        scene.swap(&scene.root.join("sub"));
        let args = SearchArguments {
            pattern: "SECRET".into(),
            repository: None,
            glob: None,
            cursor: None,
        };
        let Outcome::Done(found) = search_in(&scene.place, &base, &args) else {
            panic!("the search failed");
        };
        assert!(found.starts_with("0 match(es)"), "{found}");
        assert!(!found.contains("f.txt"), "{found}");
    }

    #[test]
    fn the_fresh_check_of_a_swapped_path_does_not_show_where_it_now_leads() {
        let scene = Scene::new();
        let path = scene.resolve(&scene.root.join("sub/f.txt"));
        let read = "plain text\n".to_owned();
        scene.swap(&scene.root.join("sub"));
        let refusal = fresh_against(
            &scene.place,
            &path,
            Some(sha256(read.as_bytes())),
            Some(read),
        )
        .unwrap_err();
        changed_refusal(refusal, "fresh");
    }

    #[test]
    fn a_path_going_up_out_of_a_missing_folder_is_never_opened_and_makes_nothing() {
        let scene = Scene::new();
        std::fs::create_dir_all(scene.base.join("nvim")).unwrap();
        std::os::unix::fs::symlink(scene.base.join("nvim"), scene.root.join("cfg")).unwrap();
        let path = scene.place.resolve(&scene.root, "m/../cfg/new/init.lua");
        assert_eq!(path.unfollowed, Some(Unfollowed::UpFromMissing));
        let Some(Outcome::Refused(refusal)) = never_opened(&path) else {
            panic!("a path up out of a missing folder must be refused");
        };
        assert!(
            refusal.contains("names a folder that does not exist and then goes up out of it"),
            "{refusal}"
        );
        // Even written, it opens nothing through the link and makes nothing.
        assert!(put(&path, b"x").is_err());
        assert!(!scene.root.join("m").exists());
        assert!(!scene.base.join("nvim/new").exists());
    }

    #[test]
    fn an_approved_path_that_could_not_be_followed_is_refused_for_what_it_is() {
        let scene = Scene::new();
        // A process's folder in /proc under another name: never opened, and said so.
        let path = scene.place.resolve(&scene.root, "/dev/fd/3");
        let Some(Outcome::Refused(refusal)) = never_opened(&path) else {
            panic!("/dev/fd/3 must never be opened: {path:?}");
        };
        assert!(
            refusal.contains("never open a path through a process's folder in /proc"),
            "{refusal}"
        );

        // More links than Linux follows: opened as written, which follows none.
        let mut target = scene.base.join("out/sub");
        for n in (0..41).rev() {
            let link = scene.base.join(format!("l{n}"));
            std::os::unix::fs::symlink(&target, &link).unwrap();
            target = link;
        }
        let path = scene.resolve(&target.join("f.txt"));
        assert!(
            matches!(path.unfollowed, Some(Unfollowed::Failed(_))),
            "{path:?}"
        );
        assert!(never_opened(&path).is_none());
        let Outcome::Refused(refusal) =
            read_page(None, &path, 0, 100).unwrap_or_else(|e| failed(&path, e))
        else {
            panic!("a path of 41 links must not be read");
        };
        assert!(refusal.contains("more than 40 links"), "{refusal}");
        assert!(!refusal.contains("changed since"), "{refusal}");

        // A folder it may not look into: opened, and the system says why it cannot be. Root
        // looks into any folder.
        let locked = scene.base.join("locked");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::write(locked.join("f.txt"), "x").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read_dir(&locked).is_ok() {
            return;
        }
        let path = scene.resolve(&locked.join("f.txt"));
        let outcome = read_page(None, &path, 0, 100).unwrap_or_else(|e| failed(&path, e));
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            matches!(path.unfollowed, Some(Unfollowed::Failed(_))),
            "{path:?}"
        );
        assert!(never_opened(&path).is_none());
        let Outcome::Failed(why) = outcome else {
            panic!("{}", said(&outcome));
        };
        assert!(why.contains("ermission denied"), "{why}");
    }

    #[test]
    fn a_pipe_or_a_device_is_refused_without_being_opened_for_reading() {
        let scene = Scene::new();
        let fifo = scene.root.join("sub/pipe");
        let name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o644) }, 0);
        // Any open for reading or writing is an `IN_OPEN` event; a look (`O_PATH`) is none.
        let watch = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        assert!(watch >= 0);
        assert!(unsafe { libc::inotify_add_watch(watch, name.as_ptr(), libc::IN_OPEN) } >= 0);
        let path = scene.resolve(Path::new("sub/pipe"));

        let read = read_page(None, &path, 0, 100).unwrap_or_else(|e| failed(&path, e));
        let fresh = fresh_against(&scene.place, &path, Some("sha".into()), None).unwrap_err();
        let text = text_of(&path).unwrap_err();
        let root = nofollow::open(&scene.root, nofollow::LOOK).unwrap();
        let beneath = open_regular_beneath(root.as_fd(), Path::new("sub/pipe")).unwrap_err();
        for outcome in [read, fresh, text, failed(&path, beneath)] {
            assert!(
                said(&outcome).contains("is not a regular file"),
                "{}",
                said(&outcome)
            );
        }
        let mut event = [0u8; 256];
        let got = unsafe { libc::read(watch, event.as_mut_ptr().cast(), event.len()) };
        unsafe { libc::close(watch) };
        assert_eq!(got, -1, "the pipe was opened");
    }

    #[test]
    fn making_a_folder_needs_only_to_go_through_the_folders_above() {
        let scene = Scene::new();
        // A folder above the place that may be gone through but not read.
        let locked = scene.base.join("locked");
        std::fs::create_dir_all(locked.join("app")).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o311)).unwrap();
        if std::fs::read_dir(&locked).is_ok() {
            // Root reads any folder: nothing to show.
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
            return;
        }
        let place = Place {
            root: locked.join("app"),
            ..scene.place.clone()
        };
        let path = place.resolve(&place.root, "newdir/f.txt");
        let written = put(&path, b"x");
        let read = read_page(None, &path, 0, 10).map_err(|e| failed(&path, e));
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(written.is_ok(), "{}", said(&written.unwrap_err()));
        assert!(read.is_ok());
        assert_eq!(
            std::fs::read_to_string(locked.join("app/newdir/f.txt")).unwrap(),
            "x"
        );
    }

    #[test]
    fn a_link_of_proc_to_the_daemon_s_own_is_refused_as_such() {
        let scene = Scene::new();
        for named in ["/proc/mounts", "/proc/net/dev"] {
            let path = scene.place.resolve(&scene.root, named);
            let outcome = read_page(None, &path, 0, 100).unwrap_or_else(|e| failed(&path, e));
            let Outcome::Refused(refusal) = outcome else {
                panic!("{named}: {}", said(&outcome));
            };
            assert!(
                refusal.contains("goes through /proc/self, which is the daemon's own"),
                "{refusal}"
            );
        }
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
