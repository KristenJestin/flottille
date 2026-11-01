//! The items (`docs/items.md`): every pull request and every ticket of a project, read by the
//! daemon on its own, incrementally, from the forges and the tracker the project declares, and
//! kept in the store. A round of a source asks only what changed since its cursor (with an
//! overlap), compares it with what is stored, and emits what happened (`item.*`); the first round
//! of a source stores without emitting. Rounds come every minute while a window is connected and
//! every five otherwise (`daemon.toml`), a source that fails waits twice as long each time, and
//! nothing is read of a project that declares nothing.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use flottille_core::declared::ProjectTracker;
use flottille_core::item::{Check, Cursor, Filter, Item};
use flottille_core::project::Project;
use flottille_core::protocol::Event;
use flottille_core::store::now_ms;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::forge::{self, Checkout};
use crate::projects::{self, Failure};
use crate::tickets;

/// How far before its cursor a round asks again: what changed while the last round ran, and a
/// forge's clock a little behind ours, are not missed. What it reads again unchanged emits
/// nothing.
pub const OVERLAP_MS: u64 = 5 * 60_000;
/// The longest wait between two rounds of a source that keeps failing, unless its rhythm is
/// longer.
const LONGEST_MS: u64 = 16 * 60_000;
/// The rhythm when the configuration says none: minutes between two rounds.
const WINDOW_MINUTES: u64 = 1;
const MINUTES: u64 = 5;
/// How many new comments or reviews one event says at most.
const SAID: usize = 5;
/// What an event shows of a comment.
const EXCERPT_CHARS: usize = 200;

/// A comment or a review a round read, as an event says it.
#[derive(Clone, Debug, PartialEq)]
pub struct Said {
    pub author: String,
    pub bot: bool,
    /// Posted by an agent of Flottille (`docs/forges.md`, "What Flottille posts is marked").
    pub by_agent: bool,
    pub state: Option<String>,
    pub excerpt: String,
}

impl Said {
    pub fn of(comment: &forge::Comment) -> Said {
        let (text, by_agent) = comment.unmarked();
        Said {
            author: comment.author.clone(),
            bot: comment.bot,
            by_agent,
            state: comment.state.clone(),
            excerpt: excerpt(text),
        }
    }

    fn to_json(&self) -> Value {
        json!({"author": self.author, "bot": self.bot, "by_agent": self.by_agent,
            "state": self.state, "excerpt": self.excerpt})
    }
}

/// A text on one line, cut.
pub fn excerpt(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match line.chars().count() > EXCERPT_CHARS {
        true => format!("{}…", line.chars().take(EXCERPT_CHARS).collect::<String>()),
        false => line,
    }
}

/// An item as a round read it, with its comments and reviews, whose newest an event says.
#[derive(Clone, Debug)]
pub struct Fetched {
    pub item: Item,
    pub comments: Vec<Said>,
    pub reviews: Vec<Said>,
    /// When it was made, when its source says it: one seen first after the first round but made
    /// before it was not opened since (a list cut at its cap shows it late).
    pub created_at: Option<u64>,
    /// Its checks, reviews and comments were read; a listed one keeps those stored.
    pub detailed: bool,
}

impl Fetched {
    /// An item stored that its forge or tracker no longer has, or that left the repository: it
    /// is closed, and says so.
    pub fn removed(mut item: Item) -> Fetched {
        item.state = "closed".into();
        item.details["removed"] = json!(true);
        Fetched {
            item,
            comments: Vec::new(),
            reviews: Vec::new(),
            created_at: None,
            detailed: true,
        }
    }

    fn open(&self) -> bool {
        matches!(self.item.state.as_str(), "open" | "draft")
    }
}

/// What one list of a source found: the items with their cheap fields, what it could not read
/// (a cap), and whether its items have details to read one by one.
pub struct Listing {
    pub items: Vec<Fetched>,
    pub note: Option<String>,
    pub details: bool,
}

/// A time a source says, as milliseconds: RFC 3339 (with an offset written `+0000` too, as Jira
/// writes it), or milliseconds already (Bitbucket Data Center).
pub fn time_ms(text: &str) -> Option<u64> {
    if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
        return text.parse().ok();
    }
    crate::session_read::form(text).or_else(|| {
        // `…+0000`: the offset without its colon.
        let (head, offset) = text.split_at(text.len().checked_sub(5)?);
        let (sign, digits) = offset.split_at(1);
        (["+", "-"].contains(&sign) && digits.bytes().all(|b| b.is_ascii_digit()))
            .then(|| format!("{head}{sign}{}:{}", &digits[..2], &digits[2..]))
            .and_then(|fixed| crate::session_read::form(&fixed))
    })
}

/// An item's `updated_at` as it is stored: RFC 3339 in UTC, with its milliseconds when it has
/// some (a change within the same second is a change), so that the store sorts by it; a time it
/// cannot read is kept as it was said.
fn normalized(text: &str) -> String {
    let Some(ms) = time_ms(text) else {
        return text.to_owned();
    };
    let shown = crate::session_read::shown_date(ms);
    match ms % 1000 {
        0 => shown,
        millis => format!("{}.{millis:03}Z", shown.trim_end_matches('Z')),
    }
}

/// How many items one write keeps: a round that fails keeps what it read before.
const BATCH: usize = 25;
/// How often a source's open items are compared with its open list, at most: what the search of
/// what changed never shows again (an issue deleted or transferred) is found this way.
const SWEEP_MS: u64 = 60 * 60_000;

/// Each project's sources, as `Store::retain_items` takes them.
type Declared = Vec<(String, Vec<[String; 4]>)>;
/// The rhythm read, with the time its file was changed then.
type Rhythm = (Option<std::time::SystemTime>, (u64, u64));

/// What the daemon keeps of the items in memory: the rounds running, the windows connected, and
/// whom each source is signed in as.
#[derive(Default)]
pub struct Items {
    /// One round of a project at a time.
    rounds: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    windows: AtomicUsize,
    /// Whom a forge says the user is, by host, once it said it.
    me: Mutex<HashMap<String, String>>,
    /// The sources of each project at the last poll: their items are forgotten once they go.
    sources: Mutex<Option<Declared>>,
    /// `daemon.toml`'s rhythm, with the time its file was changed: read again when it changes.
    rhythm: Mutex<Option<Rhythm>>,
    /// When each source's open items were last compared with its open list (`SWEEP_MS`).
    swept: Mutex<HashMap<String, u64>>,
    /// The pull requests whose checks could not be read again, said once each.
    unread_checks: Mutex<std::collections::HashSet<String>>,
}

/// A window connected to the daemon (`events.watch {client: "window"}`): while one is, the rounds
/// come at the window's rhythm. Dropped when its connection closes.
pub struct Window(Arc<Daemon>);

pub fn window(daemon: &Arc<Daemon>) -> Window {
    daemon.items.windows.fetch_add(1, Ordering::SeqCst);
    Window(daemon.clone())
}

impl Drop for Window {
    fn drop(&mut self) {
        self.0.items.windows.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A source of a project: a repository's forge for its pull requests, or its tracker.
#[derive(Clone, Debug)]
enum Source {
    Pulls(Checkout),
    Tickets(ProjectTracker),
}

impl Source {
    fn kind(&self) -> &'static str {
        match self {
            Source::Pulls(_) => "pull",
            Source::Tickets(_) => "ticket",
        }
    }

    fn target(&self) -> String {
        match self {
            Source::Pulls(checkout) => checkout.path.clone(),
            Source::Tickets(_) => "tracker".into(),
        }
    }

    /// The path of the repository its items are of: a Jira work item is of none.
    fn repository(&self) -> String {
        match self {
            Source::Pulls(checkout) => checkout.path.clone(),
            Source::Tickets(ProjectTracker::Github { repository }) => repository.clone(),
            Source::Tickets(ProjectTracker::Jira { .. }) => String::new(),
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Source::Pulls(checkout) => match checkout.declared.as_ref().map(forge::Forge::from) {
                Some(forge::Forge::Bitbucket { .. }) => "bitbucket",
                _ => "github",
            },
            Source::Tickets(tracker) => tracker.kind(),
        }
    }
}

/// The sources a project declares: nothing is read for a repository with no forge, nor for a
/// project with no tracker.
fn sources_of(project: &Project) -> Vec<Source> {
    let mut sources: Vec<Source> = project
        .repositories
        .iter()
        .filter(|r| r.forge.is_some())
        .map(|r| {
            Source::Pulls(Checkout {
                path: r.path.clone(),
                folder: match r.path.as_str() {
                    "." => project.folder.clone(),
                    path => project.folder.join(path),
                },
                remote: r.remote.clone(),
                declared: r.forge.clone(),
            })
        })
        .collect();
    sources.extend(project.tracker.clone().map(Source::Tickets));
    sources
}

/// The rhythm, from `daemon.toml`'s `[items]`: the minutes between two rounds while a window is
/// connected, and otherwise.
fn rhythm(daemon: &Daemon) -> (u64, u64) {
    let file = daemon.paths.config.join("daemon.toml");
    let changed = std::fs::metadata(&file).and_then(|m| m.modified()).ok();
    let mut kept = daemon.items.rhythm.lock().unwrap();
    if let Some((at, rhythm)) = *kept
        && at == changed
    {
        return rhythm;
    }
    let rhythm = read_rhythm(&file);
    *kept = Some((changed, rhythm));
    rhythm
}

fn read_rhythm(file: &std::path::Path) -> (u64, u64) {
    let Ok(text) = std::fs::read_to_string(file) else {
        return (WINDOW_MINUTES, MINUTES);
    };
    let read = |document: &toml_edit::DocumentMut, key: &str, default: u64| {
        document
            .get("items")
            .and_then(|items| items.get(key))
            .and_then(|value| value.as_integer())
            .and_then(|minutes| u64::try_from(minutes).ok())
            .filter(|minutes| *minutes > 0)
            .unwrap_or(default)
    };
    match text.parse::<toml_edit::DocumentMut>() {
        Ok(document) => (
            read(&document, "window_minutes", WINDOW_MINUTES),
            read(&document, "minutes", MINUTES),
        ),
        Err(error) => {
            eprintln!(
                "flottille daemon: {}: {error}; the items are read at their default rhythm",
                file.display()
            );
            (WINDOW_MINUTES, MINUTES)
        }
    }
}

/// How long a source waits after a round: its rhythm, doubled for each round that failed in a row.
fn wait_ms(every_ms: u64, failures: u32) -> u64 {
    (every_ms << failures.min(8)).min(LONGEST_MS.max(every_ms))
}

/// Starts the rounds whose time has come (from the sweep), and forgets the items of the sources
/// and projects that are gone.
pub fn poll(daemon: &Arc<Daemon>) {
    if std::env::var_os("FLOTTILLE_TEST_NO_FETCH").is_some_and(|v| !v.is_empty()) {
        static SAID: std::sync::Once = std::sync::Once::new();
        SAID.call_once(|| {
            eprintln!("flottille daemon: FLOTTILLE_TEST_NO_FETCH is set: no item is read");
        });
        return;
    }
    let all = projects::all(daemon);
    let declared: Declared = all
        .iter()
        .map(|project| {
            let sources = sources_of(project)
                .iter()
                .map(|s| {
                    [
                        s.kind().to_owned(),
                        s.target(),
                        s.name().to_owned(),
                        s.repository(),
                    ]
                })
                .collect();
            (project.name.clone(), sources)
        })
        .collect();
    let changed = daemon.items.sources.lock().unwrap().as_ref() != Some(&declared);
    if changed {
        let kept: Vec<_> = declared
            .iter()
            .filter(|(_, sources)| !sources.is_empty())
            .cloned()
            .collect();
        match daemon.write(|store| Ok((store.retain_items(&kept)?, None::<Event>))) {
            Ok(_) => *daemon.items.sources.lock().unwrap() = Some(declared),
            Err(error) => eprintln!("flottille daemon: items: {error}"),
        }
    }
    let (window, idle) = rhythm(daemon);
    let every_ms = match daemon.items.windows.load(Ordering::SeqCst) {
        0 => idle,
        _ => window,
    } * 60_000;
    let now = now_ms();
    for project in all {
        let sources = sources_of(&project);
        if sources.is_empty() {
            continue;
        }
        let cursors = daemon
            .read(|store| store.cursors(&project.name))
            .unwrap_or_default();
        let due: Vec<Source> = sources
            .into_iter()
            .filter(|source| {
                let cursor = cursors
                    .iter()
                    .find(|c| c.kind == source.kind() && c.target == source.target());
                cursor.and_then(|c| c.round_at).is_none_or(|at| {
                    now >= at + wait_ms(every_ms, cursor.map_or(0, |c| c.failures))
                })
            })
            .collect();
        if due.is_empty() {
            continue;
        }
        let lock = lock_of(daemon, &project.name);
        let Ok(held) = lock.try_lock_owned() else {
            continue;
        };
        let daemon = daemon.clone();
        tokio::spawn(async move {
            round(&daemon, &project, &due).await;
            // Removed while its round wrote: what it wrote goes at the next sweep.
            if !projects::exists(&daemon, &project.name) {
                *daemon.items.sources.lock().unwrap() = None;
            }
            drop(held);
        });
    }
}

fn lock_of(daemon: &Daemon, project: &str) -> Arc<tokio::sync::Mutex<()>> {
    daemon
        .items
        .rounds
        .lock()
        .unwrap()
        .entry(project.to_owned())
        .or_default()
        .clone()
}

/// A round of each of `sources`, one after the other: one that fails stops none of the others.
async fn round(daemon: &Arc<Daemon>, project: &Project, sources: &[Source]) {
    for source in sources {
        source_round(daemon, project, source).await;
    }
}

/// A source ready to read.
enum Reader {
    Pulls(forge::Pulls),
    Tickets(tickets::Tickets),
}

impl Reader {
    async fn open(project: &Project, source: &Source) -> Result<Reader, String> {
        match source {
            Source::Pulls(checkout) => forge::Pulls::open(checkout).await.map(Reader::Pulls),
            Source::Tickets(_) => tickets::Tickets::open(project).await.map(Reader::Tickets),
        }
    }

    async fn list(&self, since: Option<u64>, now: u64, stored: &[Item]) -> Result<Listing, String> {
        match self {
            Reader::Pulls(pulls) => {
                let since = since.map(crate::session_read::shown_date);
                pulls.list(since.as_deref(), stored).await
            }
            Reader::Tickets(tickets) => tickets.list(since, now, stored).await,
        }
    }

    async fn detail(&self, fetched: &mut Fetched) -> Result<bool, String> {
        match self {
            Reader::Pulls(pulls) => pulls.detail(fetched).await,
            Reader::Tickets(tickets) => tickets.detail(fetched).await,
        }
    }

    /// The stored open items its open list no longer holds, each read alone.
    async fn swept(&self, stored: &[Item], listed: &[Fetched]) -> Result<Vec<Fetched>, String> {
        match self {
            Reader::Pulls(pulls) => pulls.swept(stored, listed).await,
            Reader::Tickets(tickets) => tickets.swept(stored, listed).await,
        }
    }

    /// The forge whose user `mine` is said of, when it can say it.
    fn forge(&self) -> Option<&forge::Forge> {
        match self {
            Reader::Pulls(pulls) if pulls.knows_me() => Some(pulls.forge()),
            Reader::Pulls(_) => None,
            Reader::Tickets(tickets) => tickets.forge(),
        }
    }
}

/// Whom `found` says the user is, asked again while it has not said it.
async fn me_of(daemon: &Daemon, found: &forge::Forge) -> Option<String> {
    let host = match found {
        forge::Forge::GitHub { .. } => flottille_core::declared::GITHUB.to_owned(),
        forge::Forge::Bitbucket { host, .. } => host.clone(),
    };
    let known = daemon.items.me.lock().unwrap().get(&host).cloned();
    if known.is_some() {
        return known;
    }
    let me = forge::me(found).await;
    if let Some(me) = &me {
        daemon.items.me.lock().unwrap().insert(host, me.clone());
    }
    me
}

/// Whether `item` was opened by, or is assigned to, `me`.
fn mine_of(item: &Item, me: &str) -> bool {
    item.author.as_deref() == Some(me) || item.assignees.iter().any(|a| a == me)
}

/// The checks of an open pull request whose CI still runs, read again: they move without it.
/// None when there is nothing to read, or when the read failed, which is said once for the pull
/// request and tried again at the next round.
async fn checks_now(daemon: &Daemon, reader: &Reader, item: &Item) -> Option<Vec<Check>> {
    let Reader::Pulls(pulls) = reader else {
        return None;
    };
    let running = item.checks.iter().any(|c| c.state == "pending");
    if !running || !matches!(item.state.as_str(), "open" | "draft") {
        return None;
    }
    let number = item.key.parse::<u64>().ok()?;
    let which = format!("{}\t{}\t{}", item.project, item.repository, item.key);
    match pulls.checks(number).await {
        Ok(checks) => {
            daemon.items.unread_checks.lock().unwrap().remove(&which);
            Some(checks)
        }
        Err(error) => {
            if daemon.items.unread_checks.lock().unwrap().insert(which) {
                eprintln!(
                    "flottille daemon: items of {}: the checks of the pull request #{number} of \
                     {} could not be read: {error}",
                    item.project, item.repository
                );
            }
            None
        }
    }
}

/// Whether an item needs its details read: one open, or one changed in a round after the first
/// (a first round reads the history's details for the open ones alone), unless what is stored of
/// it was read in full at the same update.
fn wants_detail(old: Option<&Item>, new: &Fetched, first: bool) -> bool {
    (new.open() || !first)
        && old.is_none_or(|old| {
            old.updated_at != new.item.updated_at || old.details.get("comments").is_none()
        })
}

/// A listed item that was not read in full keeps what was read in full of it before: its checks
/// and its details, the listed fields over them.
fn keep_details(old: &Item, new: &mut Item) {
    if let (Some(kept), Some(listed)) = (old.details.as_object(), new.details.as_object()) {
        let mut details = kept.clone();
        details.extend(listed.clone());
        new.details = Value::Object(details);
    }
    new.checks = old.checks.clone();
}

/// The cursor after a round that failed, its reading or its writing: it waits twice as long,
/// saying why.
fn failed(cursor: &mut Cursor, problem: String) {
    cursor.failures = cursor.failures.saturating_add(1);
    cursor.problem = Some(problem);
}

/// Writes what a round read so far, with its events, unless the project is gone meanwhile:
/// then nothing of it is kept, and false says to stop.
fn flush(
    daemon: &Daemon,
    project: &str,
    items: &mut Vec<Item>,
    events: &mut Vec<(&'static str, Value)>,
) -> Result<bool, String> {
    if !projects::exists(daemon, project) {
        items.clear();
        events.clear();
        return Ok(false);
    }
    if items.is_empty() && events.is_empty() {
        return Ok(true);
    }
    let (items, events) = (std::mem::take(items), std::mem::take(events));
    daemon
        .write_all(|store| Ok(((), store.put_items(&items, events)?)))
        .map(|()| true)
        .map_err(|error| format!("the items could not be stored: {error}"))
}

async fn source_round(daemon: &Arc<Daemon>, project: &Project, source: &Source) {
    let (kind, target) = (source.kind(), source.target());
    let mut cursor = daemon
        .read(|store| store.cursors(&project.name))
        .unwrap_or_default()
        .into_iter()
        .find(|c| c.kind == kind && c.target == target)
        .unwrap_or_else(|| Cursor {
            project: project.name.clone(),
            kind: kind.into(),
            target: target.clone(),
            ..Cursor::default()
        });
    cursor.source = source.name().into();
    let started = now_ms();
    let previous = cursor.problem.clone();
    let stored = daemon
        .read(|store| {
            store.items(
                &project.name,
                &Filter {
                    kind: Some(kind.into()),
                    ..Filter::default()
                },
            )
        })
        .unwrap_or_default()
        .into_iter()
        .filter(|item| kind == "ticket" || item.repository == target)
        .collect::<Vec<_>>();
    let outcome = read_round(daemon, project, source, &cursor, &stored, started).await;
    match outcome {
        // The project went meanwhile: nothing of it is written back.
        Ok(None) => return,
        Ok(Some(note)) => {
            cursor.first_at = cursor.first_at.or(Some(started));
            cursor.cursor = Some(started);
            cursor.failures = 0;
            cursor.problem = None;
            cursor.note = note;
        }
        Err(problem) => {
            if previous.as_ref() != Some(&problem) {
                eprintln!(
                    "flottille daemon: items of {} ({kind}s of {target}): {problem}",
                    project.name
                );
            }
            failed(&mut cursor, problem);
        }
    }
    cursor.round_at = Some(started);
    if !projects::exists(daemon, &project.name) {
        return;
    }
    if let Err(error) = daemon.read(|store| store.set_cursor(&cursor)) {
        eprintln!("flottille daemon: items of {}: {error}", project.name);
    }
}

/// Lists what changed since the cursor, reads in full what needs it, and stores it as it goes,
/// so that a round that fails keeps what it read; then the checks of the open pull requests
/// whose CI still runs. The note of what could not be read; none when the project went
/// meanwhile.
async fn read_round(
    daemon: &Daemon,
    project: &Project,
    source: &Source,
    cursor: &Cursor,
    stored: &[Item],
    started: u64,
) -> Result<Option<Option<String>>, String> {
    let reader = Reader::open(project, source).await?;
    let since = cursor.cursor.map(|at| at.saturating_sub(OVERLAP_MS));
    let mut listing = reader.list(since, started, stored).await?;
    let first = cursor.cursor.is_none();
    let swept_key = format!("{}\t{}\t{}", project.name, source.kind(), source.target());
    let sweep = !first
        && daemon
            .items
            .swept
            .lock()
            .unwrap()
            .get(&swept_key)
            .is_none_or(|at| started >= at + SWEEP_MS);
    if sweep {
        let swept = reader.swept(stored, &listing.items).await?;
        listing.items.extend(swept);
    }
    // Unknown when the forge cannot say it, or did not this time: then what is stored is kept.
    let knows_me = reader.forge().is_some();
    let me = match reader.forge() {
        Some(found) => me_of(daemon, found).await,
        None => None,
    };
    let (mut items, mut events) = (Vec::new(), Vec::new());
    let mut listed_keys = Vec::new();
    for mut fetched in listing.items {
        let item = &mut fetched.item;
        item.project = project.name.clone();
        item.seen_at = started;
        item.updated_at = item.updated_at.as_deref().map(normalized);
        listed_keys.push((item.repository.clone(), item.key.clone()));
        let old = stored
            .iter()
            .find(|s| s.repository == item.repository && s.key == item.key);
        match &me {
            Some(me) => item.mine = mine_of(item, me),
            None if knows_me => item.mine = old.is_some_and(|old| old.mine),
            None => {}
        }
        if listing.details && !fetched.detailed && wants_detail(old, &fetched, first) {
            match reader.detail(&mut fetched).await {
                Ok(true) => {}
                Ok(false) => {
                    let mut gone = Fetched::removed(fetched.item.clone());
                    gone.item.checks = old.map(|o| o.checks.clone()).unwrap_or_default();
                    fetched = gone;
                }
                Err(error) => {
                    flush(daemon, &project.name, &mut items, &mut events)?;
                    return Err(error);
                }
            }
        }
        if let Some(old) = old
            && !fetched.detailed
        {
            keep_details(old, &mut fetched.item);
            if let Some(checks) = checks_now(daemon, &reader, &fetched.item).await {
                fetched.item.checks = checks;
            }
        }
        if !first {
            let made_before = fetched
                .created_at
                .zip(cursor.first_at)
                .is_some_and(|(made, first_at)| made + OVERLAP_MS < first_at);
            events.extend(
                events_of(old, &fetched)
                    .into_iter()
                    .filter(|(name, _)| !(*name == "item.opened" && made_before)),
            );
        }
        items.push(fetched.item);
        if items.len() >= BATCH && !flush(daemon, &project.name, &mut items, &mut events)? {
            return Ok(None);
        }
    }
    // What was not listed: the checks of an open pull request whose CI still runs, which move
    // without it, and whether it is the user's, once the forge says who the user is.
    for old in stored {
        if listed_keys.contains(&(old.repository.clone(), old.key.clone())) {
            continue;
        }
        let mut item = old.clone();
        let mut changed = false;
        if let Some(me) = &me {
            item.mine = mine_of(old, me);
            changed = item.mine != old.mine;
        }
        if let Some(checks) = checks_now(daemon, &reader, old).await {
            item.checks = checks;
            item.seen_at = started;
            changed = true;
        }
        if !changed {
            continue;
        }
        let fetched = Fetched {
            item,
            comments: Vec::new(),
            reviews: Vec::new(),
            created_at: None,
            detailed: true,
        };
        events.extend(events_of(Some(old), &fetched));
        items.push(fetched.item);
    }
    if !flush(daemon, &project.name, &mut items, &mut events)? {
        return Ok(None);
    }
    if sweep || first {
        daemon
            .items
            .swept
            .lock()
            .unwrap()
            .insert(swept_key, started);
    }
    Ok(Some(listing.note))
}

/// The checks as one mark: `failing` when one fails, else `pending` when one runs, else `passing`.
fn mark(checks: &[Check]) -> &'static str {
    if checks.iter().any(|c| c.state == "failing") {
        "failing"
    } else if checks.iter().any(|c| c.state == "pending") {
        "pending"
    } else {
        "passing"
    }
}

/// What happened to an item between what was stored and what a round read: its events, each with
/// the item's identity.
fn events_of(old: Option<&Item>, new: &Fetched) -> Vec<(&'static str, Value)> {
    let item = &new.item;
    let base = || {
        json!({"project": item.project, "kind": item.kind, "source": item.source,
            "repository": item.repository, "key": item.key, "title": item.title,
            "state": item.state, "url": item.url})
    };
    let with = |mut data: Value, key: &str, value: Value| {
        data[key] = value;
        data
    };
    let Some(old) = old else {
        return vec![("item.opened", base())];
    };
    let mut events = Vec::new();
    let mut changes = serde_json::Map::new();
    let mut change = |key: &str, from: Value, to: Value| {
        if from != to {
            changes.insert(key.into(), json!({"from": from, "to": to}));
        }
    };
    if old.state != item.state {
        match item.state.as_str() {
            "merged" => events.push(("item.merged", base())),
            "closed" => events.push(("item.closed", base())),
            _ => change("state", json!(old.state), json!(item.state)),
        }
    }
    // A Jira work item's status, which its state says only as open or closed.
    if let (Some(from), Some(to)) = (old.details.get("status"), item.details.get("status")) {
        change("status", from.clone(), to.clone());
    }
    change("title", json!(old.title), json!(item.title));
    change("assignees", json!(old.assignees), json!(item.assignees));
    change("labels", json!(old.labels), json!(item.labels));
    if !changes.is_empty() {
        events.insert(
            0,
            (
                "item.changed",
                with(base(), "changes", Value::Object(changes)),
            ),
        );
    }
    let count = |item: &Item, key: &str| item.details[key].as_u64();
    let newest = |said: &[Said], new: u64, old: u64| -> Value {
        let more = (new.saturating_sub(old) as usize).min(SAID);
        said[said.len().saturating_sub(more)..]
            .iter()
            .map(Said::to_json)
            .collect()
    };
    if let (Some(was), Some(now)) = (count(old, "comments"), count(item, "comments"))
        && now > was
    {
        events.push((
            "item.commented",
            with(base(), "comments", newest(&new.comments, now, was)),
        ));
    }
    let reviews = (count(old, "reviews"), count(item, "reviews"));
    let reviewed = matches!(reviews, (Some(was), Some(now)) if now > was)
        || (old.details.get("review").is_some()
            && item.details.get("review").is_some()
            && old.details["review"] != item.details["review"])
        || old.details.get("reviewers").is_some_and(|reviewers| {
            item.details
                .get("reviewers")
                .is_some_and(|now| now != reviewers)
        });
    if reviewed {
        let mut data = with(base(), "review", item.details["review"].clone());
        if let (Some(was), Some(now)) = reviews {
            data["reviews"] = newest(&new.reviews, now, was);
        }
        if let Some(reviewers) = item.details.get("reviewers") {
            data["reviewers"] = reviewers.clone();
        }
        events.push(("item.reviewed", data));
    }
    // The checks ended: none runs now, and one was running, or the pull request's head moved.
    let running = |checks: &[Check]| checks.iter().any(|c| c.state == "pending");
    let moved = old.details["head_commit"] != item.details["head_commit"];
    if item.kind == "pull"
        && !item.checks.is_empty()
        && !running(&item.checks)
        && (running(&old.checks) || moved)
    {
        let ended: Vec<Value> = item
            .checks
            .iter()
            .filter(|check| {
                moved
                    || old
                        .checks
                        .iter()
                        .any(|c| c.name == check.name && c.state == "pending")
            })
            .map(|check| json!({"name": check.name, "state": check.state}))
            .collect();
        let mut data = with(base(), "checks", json!(mark(&item.checks)));
        data["ended"] = Value::from(ended);
        events.push(("item.checks", data));
    }
    events
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Listed {
    project: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    repository: Option<String>,
    #[serde(default)]
    mine: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Shown {
    project: String,
    kind: String,
    id: String,
    #[serde(default)]
    repository: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Refreshed {
    project: String,
}

fn sources_json(daemon: &Daemon, project: &str) -> Result<Value, Failure> {
    let cursors = daemon.read(|store| store.cursors(project))?;
    Ok(cursors
        .iter()
        .map(|c| {
            json!({"kind": c.kind, "target": c.target, "source": c.source, "cursor": c.cursor,
                "round_at": c.round_at, "failures": c.failures, "problem": c.problem,
                "note": c.note})
        })
        .collect())
}

/// `items.list`, `items.show` and `items.refresh`.
pub async fn call(daemon: Arc<Daemon>, method: &str, params: Value) -> Result<Value, Failure> {
    match method {
        "items.list" => {
            let asked: Listed = serde_json::from_value(params)?;
            projects::load_project(&daemon, &asked.project)?;
            if let Some(kind) = &asked.kind
                && !["pull", "ticket"].contains(&kind.as_str())
            {
                return Err(Failure::refused(format!(
                    "items have no kind {kind:?}: pull or ticket"
                )));
            }
            let filter = Filter {
                kind: asked.kind,
                state: asked.state,
                repository: asked.repository,
                mine: asked.mine,
            };
            let items = daemon.read(|store| store.items(&asked.project, &filter))?;
            Ok(json!({"project": asked.project, "items": items,
                "sources": sources_json(&daemon, &asked.project)?}))
        }
        "items.show" => {
            let asked: Shown = serde_json::from_value(params)?;
            projects::load_project(&daemon, &asked.project)?;
            let key = asked.id.trim_start_matches('#');
            let found: Vec<Item> = daemon
                .read(|store| {
                    store.items(
                        &asked.project,
                        &Filter {
                            kind: Some(asked.kind.clone()),
                            repository: asked.repository.clone(),
                            ..Filter::default()
                        },
                    )
                })?
                .into_iter()
                .filter(|item| item.key == key)
                .collect();
            match found.as_slice() {
                [item] => Ok(serde_json::to_value(item)?),
                [] => Err(Failure::new(
                    "not-found",
                    format!(
                        "{} has no {} {} in what was read of it",
                        asked.project, asked.kind, asked.id
                    ),
                )),
                several => Err(Failure::refused(format!(
                    "{} {} is in several repositories of {} ({}): name its repository",
                    asked.kind,
                    asked.id,
                    asked.project,
                    several
                        .iter()
                        .map(|i| i.repository.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))),
            }
        }
        "items.refresh" => {
            let asked: Refreshed = serde_json::from_value(params)?;
            let project = projects::load_project(&daemon, &asked.project)?;
            let lock = lock_of(&daemon, &project.name);
            let held = lock.lock().await;
            round(&daemon, &project, &sources_of(&project)).await;
            drop(held);
            Ok(json!({"project": project.name,
                "sources": sources_json(&daemon, &project.name)?}))
        }
        method => Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(state: &str) -> Item {
        Item {
            project: "app".into(),
            kind: "pull".into(),
            source: "github".into(),
            repository: "api".into(),
            key: "1".into(),
            title: "Add the login".into(),
            state: state.into(),
            author: Some("alice".into()),
            assignees: Vec::new(),
            labels: Vec::new(),
            url: None,
            updated_at: None,
            seen_at: 1,
            mine: false,
            details: json!({"head_commit": "abc1234", "comments": 1, "reviews": 0,
                "review": null}),
            checks: vec![Check {
                name: "test".into(),
                state: "pending".into(),
                started_at: None,
                ended_at: None,
                link: None,
            }],
        }
    }

    fn fetched(item: Item) -> Fetched {
        Fetched {
            item,
            comments: Vec::new(),
            reviews: Vec::new(),
            created_at: None,
            detailed: true,
        }
    }

    #[test]
    fn a_round_that_fails_to_store_waits_as_one_that_fails_to_read() {
        let mut cursor = Cursor {
            failures: 1,
            ..Cursor::default()
        };
        failed(
            &mut cursor,
            "the items could not be stored: disk full".into(),
        );
        assert_eq!(cursor.failures, 2);
        assert_eq!(
            cursor.problem.as_deref(),
            Some("the items could not be stored: disk full")
        );
    }

    #[test]
    fn times_are_kept_in_rfc_3339_utc_whatever_the_source_writes() {
        assert_eq!(
            normalized("2026-10-01T12:00:00+02:00"),
            "2026-10-01T10:00:00Z"
        );
        assert_eq!(
            normalized("2026-10-01T10:00:00.000+0000"),
            "2026-10-01T10:00:00Z"
        );
        assert_eq!(normalized("1600000000000"), "2020-09-13T12:26:40Z");
        assert_eq!(normalized("1600000000002"), "2020-09-13T12:26:40.002Z");
        assert_eq!(normalized("soon"), "soon");
    }

    #[test]
    fn details_are_read_for_open_items_and_for_those_changed_after_the_first_round() {
        let mut merged = stored("merged");
        merged.details = json!({});
        let listed = fetched(merged.clone());
        assert!(!wants_detail(None, &listed, true));
        assert!(wants_detail(None, &listed, false));
        let open = fetched(stored("open"));
        assert!(wants_detail(None, &open, true));
        // Read in full at the same update: not again.
        assert!(!wants_detail(Some(&stored("open")), &open, false));
        let mut changed = stored("open");
        changed.updated_at = Some("2026-10-02T00:00:00Z".into());
        assert!(wants_detail(
            Some(&stored("open")),
            &fetched(changed),
            false
        ));
    }

    fn names(events: &[(&str, Value)]) -> Vec<String> {
        events.iter().map(|(name, _)| (*name).to_owned()).collect()
    }

    #[test]
    fn an_item_not_stored_is_opened_and_one_unchanged_says_nothing() {
        let new = fetched(stored("open"));
        assert_eq!(names(&events_of(None, &new)), ["item.opened"]);
        assert!(events_of(Some(&stored("open")), &new).is_empty());
    }

    #[test]
    fn a_pull_merged_or_closed_is_said_so_and_a_reopened_one_changed() {
        let old = stored("open");
        assert_eq!(
            names(&events_of(Some(&old), &fetched(stored("merged")))),
            ["item.merged"]
        );
        assert_eq!(
            names(&events_of(Some(&old), &fetched(stored("closed")))),
            ["item.closed"]
        );
        let reopened = events_of(Some(&stored("closed")), &fetched(stored("open")));
        assert_eq!(names(&reopened), ["item.changed"]);
        assert_eq!(
            reopened[0].1["changes"]["state"],
            json!({"from": "closed", "to": "open"})
        );
    }

    #[test]
    fn new_comments_and_reviews_are_said_with_their_authors() {
        let old = stored("open");
        let mut new = stored("open");
        new.details["comments"] = json!(3);
        new.details["reviews"] = json!(1);
        new.details["review"] = json!("approved");
        let said = |author: &str| Said {
            author: author.into(),
            bot: false,
            by_agent: false,
            state: None,
            excerpt: "hello".into(),
        };
        let new = Fetched {
            item: new,
            comments: vec![said("old"), said("bob"), said("carol")],
            reviews: vec![said("dave")],
            created_at: None,
            detailed: true,
        };
        let events = events_of(Some(&old), &new);
        assert_eq!(names(&events), ["item.commented", "item.reviewed"]);
        let authors: Vec<&str> = events[0].1["comments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["author"].as_str().unwrap())
            .collect();
        assert_eq!(authors, ["bob", "carol"]);
        assert_eq!(events[1].1["review"], "approved");
        assert_eq!(events[1].1["reviews"][0]["author"], "dave");
    }

    #[test]
    fn checks_end_when_none_runs_any_more_or_the_head_moved() {
        let old = stored("open");
        let mut new = stored("open");
        new.checks[0].state = "passing".into();
        let events = events_of(Some(&old), &fetched(new.clone()));
        assert_eq!(names(&events), ["item.checks"]);
        assert_eq!(events[0].1["checks"], "passing");
        // Still passing on the next round: nothing.
        assert!(events_of(Some(&new), &fetched(new.clone())).is_empty());
        // A push whose checks ran and ended between two rounds.
        let mut pushed = new.clone();
        pushed.details["head_commit"] = json!("def5678");
        pushed.checks[0].state = "failing".into();
        let events = events_of(Some(&new), &fetched(pushed));
        assert_eq!(events[0].1["checks"], "failing");
        assert_eq!(
            events[0].1["ended"],
            json!([{"name": "test", "state": "failing"}])
        );
    }

    #[test]
    fn a_jira_status_is_a_change_of_status_and_done_closes_it() {
        let mut old = stored("open");
        old.kind = "ticket".into();
        old.source = "jira".into();
        old.checks.clear();
        old.details = json!({"status": "To Do"});
        let mut new = old.clone();
        new.details["status"] = json!("In Progress");
        let events = events_of(Some(&old), &fetched(new.clone()));
        assert_eq!(names(&events), ["item.changed"]);
        assert_eq!(
            events[0].1["changes"]["status"],
            json!({"from": "To Do", "to": "In Progress"})
        );
        let mut done = new.clone();
        done.state = "closed".into();
        done.details["status"] = json!("Done");
        assert_eq!(
            names(&events_of(Some(&new), &fetched(done))),
            ["item.changed", "item.closed"]
        );
    }

    #[test]
    fn a_source_that_fails_waits_twice_as_long_each_time_up_to_a_ceiling() {
        assert_eq!(wait_ms(60_000, 0), 60_000);
        assert_eq!(wait_ms(60_000, 1), 120_000);
        assert_eq!(wait_ms(60_000, 10), LONGEST_MS);
        // A rhythm longer than the ceiling is never shortened by it.
        assert_eq!(wait_ms(30 * 60_000, 1), 30 * 60_000);
    }
}
