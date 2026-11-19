//! A place's code on its way to another machine, and its new commits on their way back
//! (`docs/remote.md`, "What travels"), over Flottille's own link: the daemons' work, never an
//! agent's. Git's side is `git::bundle`'s; this is what the two daemons say to each other (the
//! `git` machine message), and what each keeps of it (`travels` in the store).
//!
//! A bundle never goes in one frame: the side that sends it offers it, and the side it is offered
//! to reads it on a stream of its own, checks its length and its SHA-256, then has git verify and
//! fetch it. Only that side may read it, and only while it is offered. Nothing is pushed, no
//! remote is contacted, nothing is asked.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use flottille_core::store::{Travel, now_ms};
use flottille_link::{
    Git, GitFetch, GitOffer, GitPlace, GitRepository, Link, MachineMessage, Message, PublicKey,
    Received, Role,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};

use crate::daemon::Daemon;
use crate::git::{self, Class, bundle};
use crate::link::Remote;
use crate::projects;
use crate::workspaces::{self, Travelled};

/// The bytes of a bundle in one frame of its stream: well under the link's frame once in base64.
const CHUNK: usize = 192 * 1024;

/// The longest bundle a machine sends or takes: past it, it is refused before a byte is read.
pub(crate) const MOST_BYTES: u64 = 2 << 30;

/// How long a bundle being read may go without a frame before it is given up.
const FRAME_WAIT: Duration = Duration::from_secs(60);

/// Whether code travels between this machine and the others: only in a test build that asks for
/// it, until a travel is tied to a launch this machine's gate accepted (#816).
pub fn on() -> bool {
    cfg!(debug_assertions) && std::env::var_os("FLOTTILLE_TEST_TRAVEL").is_some()
}

/// What a test asks of a travel: its bundle broken, each offer held open a while.
#[derive(Default)]
pub struct Testing {
    pub corrupt: bool,
    pub hold: Duration,
}

/// The machines this one is taking something from: one thing at a time each, so that a machine
/// fills no disk with bundles read side by side and makes no workspace twice.
#[derive(Default)]
pub(crate) struct Receiving(Mutex<HashSet<[u8; 32]>>);

impl Receiving {
    /// `peer` held until what is returned is dropped; refused while it is already.
    fn hold<'a>(&'a self, peer: &PublicKey, name: &str) -> Result<Held<'a>, String> {
        if !self.0.lock().unwrap().insert(*peer.as_bytes()) {
            return Err(format!(
                "{name} is sending something already: one thing at a time"
            ));
        }
        Ok(Held {
            receiving: self,
            peer: *peer.as_bytes(),
        })
    }
}

struct Held<'a> {
    receiving: &'a Receiving,
    peer: [u8; 32],
}

impl Drop for Held<'_> {
    fn drop(&mut self) {
        self.receiving.0.lock().unwrap().remove(&self.peer);
    }
}

/// Refuses a bundle longer than [`MOST_BYTES`].
fn fits(size: u64) -> Result<(), String> {
    if size > MOST_BYTES {
        return Err("the bundle is refused: it is more than 2 GiB".to_owned());
    }
    Ok(())
}

/// Workspaces are made of what came one at a time: a branch found free stays free until its
/// worktree is made.
static PLACING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The bundles this machine offers, by transfer: the machine each is for, and its file.
#[derive(Default)]
pub(crate) struct Transfers(Mutex<HashMap<String, ([u8; 32], PathBuf)>>);

impl Transfers {
    /// The file of `transfer`, when it is offered to `peer`.
    fn file_for(&self, transfer: &str, peer: &PublicKey) -> Option<PathBuf> {
        let offered = self.0.lock().unwrap();
        let (to, file) = offered.get(transfer)?;
        (to == peer.as_bytes()).then(|| file.clone())
    }
}

/// A bundle offered, until it is dropped.
struct Offered<'a> {
    transfers: &'a Transfers,
    transfer: String,
}

impl<'a> Offered<'a> {
    fn new(transfers: &'a Transfers, transfer: String, peer: &PublicKey, file: &Path) -> Self {
        transfers
            .0
            .lock()
            .unwrap()
            .insert(transfer.clone(), (*peer.as_bytes(), file.to_owned()));
        Offered {
            transfers,
            transfer,
        }
    }
}

impl Drop for Offered<'_> {
    fn drop(&mut self) {
        self.transfers.0.lock().unwrap().remove(&self.transfer);
    }
}

fn remote_of(daemon: &Daemon) -> Result<Arc<Remote>, String> {
    crate::link::remote_of(daemon).map_err(|failure| failure.message)
}

fn stored<T>(result: flottille_core::Result<T>) -> Result<T, String> {
    result.map_err(|error| error.to_string())
}

/// Sends the code of a place to the machine named `machine` for `launch`: for each repository of
/// `place` (its path in `project`, and its folder here), its uncommitted work made a snapshot and
/// a bundle of what that machine lacks; then has it make a workspace of them. What that machine
/// answered, with what was sent of each repository. The launch is kept as sent only once its
/// workspace is made there: one that failed part way is sent again. The snapshots' refs are
/// removed once it ends, either way.
pub async fn send(
    daemon: &Arc<Daemon>,
    machine: &str,
    launch: &str,
    project: &str,
    place: &[(String, PathBuf)],
    testing: Testing,
) -> Result<Value, String> {
    let remote = remote_of(daemon)?;
    let (link, key) = remote.link_to(machine)?;
    if !stored(daemon.read(|store| store.travels("out", None, launch)))?.is_empty() {
        return Err(format!("launch {launch} was sent already"));
    }
    let mut snapshotted = Vec::new();
    let sent = send_place(
        daemon,
        &remote,
        (&link, &key, machine),
        launch,
        project,
        place,
        &testing,
        &mut snapshotted,
    )
    .await;
    for folder in snapshotted {
        if let Err(error) = bundle::remove_ref(folder, &bundle::outgoing_ref(launch)).await {
            eprintln!(
                "flottille daemon: the snapshot of launch {launch} in {} is kept: {error}",
                folder.display()
            );
        }
    }
    sent
}

#[allow(clippy::too_many_arguments)]
async fn send_place<'a>(
    daemon: &Arc<Daemon>,
    remote: &Remote,
    (link, key, machine): (&Link, &PublicKey, &str),
    launch: &str,
    project: &str,
    place: &'a [(String, PathBuf)],
    testing: &Testing,
    snapshotted: &mut Vec<&'a Path>,
) -> Result<Value, String> {
    let mut sent = Vec::new();
    let mut records = Vec::new();
    let mut branch = None;
    for (index, (repository, folder)) in place.iter().enumerate() {
        let snapshot = bundle::snapshot(folder, launch).await?;
        snapshotted.push(folder);
        branch.get_or_insert_with(|| snapshot.branch.clone());
        let asked = GitRepository {
            project: project.to_owned(),
            repository: repository.clone(),
            launch: None,
        };
        let has = ask(link, Git::Has(asked)).await?;
        let has: Vec<String> = serde_json::from_value(has["commits"].clone()).unwrap_or_default();
        let scratch = scratch()?;
        let file = scratch.path().join("bundle");
        if bundle::outgoing(folder, launch, &has, &file).await? == bundle::Bundled::NothingNew {
            return Err(format!(
                "{machine} has the snapshot of {repository} already"
            ));
        }
        if testing.corrupt {
            corrupted(&file)?;
        }
        let prerequisites = prerequisites(&file).await?;
        let commits = counted(folder, &bundle::outgoing_ref(launch), &prerequisites).await?;
        let (size, sha256) = measured(&file).await?;
        fits(size).map_err(|_| format!("the bundle of {repository} is more than 2 GiB"))?;
        let transfer = format!("{launch}/{index}");
        let _offered = Offered::new(&remote.transfers, transfer.clone(), key, &file);
        eprintln!("flottille daemon: offers {repository} of launch {launch} to {machine}");
        tokio::time::sleep(testing.hold).await;
        let offer = GitOffer {
            transfer,
            launch: launch.to_owned(),
            project: project.to_owned(),
            repository: repository.clone(),
            branch: snapshot.branch.clone(),
            size,
            sha256,
            back: false,
        };
        ask(link, Git::Offer(offer)).await?;
        records.push(Travel {
            way: "out".into(),
            machine: hex(key.as_bytes()),
            launch: launch.to_owned(),
            repository: repository.clone(),
            project: project.to_owned(),
            folder: folder.to_string_lossy().into_owned(),
            branch: snapshot.branch.clone(),
            tip: snapshot.head.clone(),
            snapshot: snapshot.commit.clone(),
            workspace: None,
            there: None,
            returned: None,
            at: now_ms(),
        });
        sent.push(json!({
            "path": repository,
            "commits": commits,
            "prerequisites": prerequisites,
        }));
    }
    let Some(branch) = branch else {
        return Err("a place has at least one repository".to_owned());
    };
    let placed = ask(
        link,
        Git::Place(GitPlace {
            launch: launch.to_owned(),
            project: project.to_owned(),
            branch,
            repositories: place.iter().map(|(path, _)| path.clone()).collect(),
        }),
    )
    .await?;
    stored(daemon.write(|store| {
        for record in &records {
            store.record_travel(record)?;
        }
        Ok(((), None))
    }))?;
    let mut answer = placed;
    answer["repositories"] = sent.into();
    Ok(answer)
}

/// Sends back the new commits of the workspace this machine made for `launch`: for each of its
/// repositories, a bundle of the commits of its branch past what was sent and what came back
/// already, fetched there into `refs/flottille/machines/<this machine>/<launch>/<branch>`.
pub async fn send_back(daemon: &Arc<Daemon>, launch: &str) -> Result<Value, String> {
    let remote = remote_of(daemon)?;
    let travels = stored(daemon.read(|store| store.travels("in", None, launch)))?;
    let Some(first) = travels.first() else {
        return Err(format!("nothing came for launch {launch}"));
    };
    if travels.iter().any(|travel| travel.machine != first.machine) {
        return Err(format!("launch {launch} came from more than one machine"));
    }
    let (Some(name), Some(there)) = (&first.workspace, &first.there) else {
        return Err(format!("launch {launch} has no workspace here"));
    };
    let workspace = stored(daemon.read(|store| store.workspace(&first.project, name)))?
        .ok_or_else(|| format!("the workspace {name} of launch {launch} is gone"))?;
    let key: PublicKey = serde_json::from_value(json!(first.machine))
        .map_err(|_| "the machine of the launch is not a key".to_owned())?;
    let machine = remote
        .name_of(&key)
        .ok_or_else(|| format!("the machine launch {launch} came from is not the user's now"))?;
    let (link, key) = remote.link_to(&machine)?;
    let mut back = Vec::new();
    for (index, travel) in travels.iter().enumerate() {
        let folder = workspace.folder_of(&travel.repository);
        let mut has = vec![travel.tip.clone(), travel.snapshot.clone()];
        has.extend(travel.returned.clone());
        let scratch = scratch()?;
        let file = scratch.path().join("bundle");
        let reference = format!("refs/heads/{there}");
        if bundle::returning(&folder, there, &has, &file).await? == bundle::Bundled::NothingNew {
            back.push(json!({"path": travel.repository, "commits": 0}));
            continue;
        }
        let commits = counted(&folder, &reference, &prerequisites(&file).await?).await?;
        let tip = git::output(&folder, &["rev-parse", "--verify", &reference], Class::Read)
            .await?
            .trim()
            .to_owned();
        let (size, sha256) = measured(&file).await?;
        fits(size)
            .map_err(|_| format!("the bundle of {} is more than 2 GiB", travel.repository))?;
        let transfer = format!("{launch}/back/{index}");
        let _offered = Offered::new(&remote.transfers, transfer.clone(), &key, &file);
        let answered = ask(
            &link,
            Git::Offer(GitOffer {
                transfer,
                launch: launch.to_owned(),
                project: travel.project.clone(),
                repository: travel.repository.clone(),
                branch: there.clone(),
                size,
                sha256,
                back: true,
            }),
        )
        .await?;
        stored(daemon.write(|store| {
            store
                .set_travel_returned("in", &travel.machine, launch, &travel.repository, &tip)
                .map(|()| ((), None))
        }))?;
        back.push(json!({
            "path": travel.repository,
            "commits": commits,
            "ref": answered["ref"],
        }));
    }
    Ok(json!({"machine": machine, "repositories": back}))
}

/// Sends a `git` message on `link`, and gives its answer; the other side's refusal as it said it.
async fn ask(link: &Link, git: Git) -> Result<Value, String> {
    link.send(&Message::Machine(MachineMessage::Git(git)))
        .await
        .map_err(|error| error.to_string())
}

/// A `git` message another machine sent on `link`, served once code travels in this build and
/// that machine is still one of the user's: what sends code to run here (`Has`, `Offer`,
/// `Place`) only from a runner's or a controller's machine, one thing at a time.
pub(crate) async fn serve(remote: Arc<Remote>, link: Link, received: Received, git: Git) {
    if !on() {
        received.answer(Err(
            "this machine takes no code from another yet: code travels only with a launch its \
             gate accepted"
                .to_owned(),
        ));
        return;
    }
    let from = link.peer();
    let Some(name) = remote.name_of(&from) else {
        received.answer(Err("it was removed from the member list".to_owned()));
        return;
    };
    let Some(daemon) = remote.daemon() else {
        received.answer(Err("the daemon is stopping".to_owned()));
        return;
    };
    if let Git::Fetch(fetch) = git {
        stream(&remote, &from, fetch, received).await;
        return;
    }
    if let Err(refused) = may_send_code(&remote, &from, &name) {
        received.answer(Err(refused));
        return;
    }
    let answer = match git {
        Git::Fetch(_) => unreachable!("served above"),
        Git::Has(asked) => has(&daemon, &asked).await,
        Git::Offer(offer) => match remote.receiving.hold(&from, &name) {
            Err(refused) => Err(refused),
            Ok(_held) if offer.back => came_back(&daemon, &link, &from, &name, offer).await,
            Ok(_held) => offered(&daemon, &link, &from, &name, offer).await,
        },
        Git::Place(place) => match remote.receiving.hold(&from, &name) {
            Err(refused) => Err(refused),
            Ok(_held) => placed(&daemon, &from, &name, place).await,
        },
    };
    received.answer(answer);
}

/// Refuses a machine that is no runner's or controller's: a viewer's, or one of no member.
fn may_send_code(remote: &Remote, from: &PublicKey, name: &str) -> Result<(), String> {
    let refused = |who: &str| {
        Err(format!(
            "{name} is {who}: only a runner's or a controller's machine sends code to run here"
        ))
    };
    match remote.role_of(from) {
        Some(Role::Runner | Role::Controller) => Ok(()),
        Some(Role::Viewer) => refused("a viewer's machine"),
        None => refused("no member's machine"),
    }
}

/// The bytes of a bundle offered to `from`, frame by frame.
async fn stream(remote: &Remote, from: &PublicKey, fetch: GitFetch, received: Received) {
    let mut answer = received.stream();
    let Some(file) = remote.transfers.file_for(&fetch.transfer, from) else {
        let refused = json!({"refused": "no bundle is offered to this machine under that name"});
        answer.send(refused.to_string().as_bytes()).await;
        answer.end().await;
        return;
    };
    let Ok(mut file) = tokio::fs::File::open(&file).await else {
        let refused = json!({"refused": "the bundle cannot be read"});
        answer.send(refused.to_string().as_bytes()).await;
        answer.end().await;
        return;
    };
    let mut buffer = vec![0; CHUNK];
    loop {
        let read = match file.read(&mut buffer).await {
            Ok(0) => break,
            Ok(read) => read,
            Err(_) => {
                let refused = json!({"refused": "the bundle cannot be read"});
                answer.send(refused.to_string().as_bytes()).await;
                break;
            }
        };
        let chunk = base64::engine::general_purpose::STANDARD.encode(&buffer[..read]);
        if !answer
            .send(json!({"chunk": chunk}).to_string().as_bytes())
            .await
        {
            return;
        }
    }
    answer.end().await;
}

/// Reads the bundle offered as `offer` from the machine on `link` into `file`: refused unless it
/// is the length and the SHA-256 offered.
async fn read_offered(link: &Link, offer: &GitOffer, file: &Path) -> Result<(), String> {
    fits(offer.size)?;
    let fetch = Git::Fetch(GitFetch {
        transfer: offer.transfer.clone(),
    });
    let mut answers = link
        .ask(&Message::Machine(MachineMessage::Git(fetch)))
        .await
        .map_err(|error| error.to_string())?;
    let mut written = tokio::fs::File::create(file)
        .await
        .map_err(|error| format!("cannot write the bundle: {error}"))?;
    let mut hash = Sha256::new();
    let mut size = 0u64;
    loop {
        let Ok(frame) = tokio::time::timeout(FRAME_WAIT, answers.next()).await else {
            return Err("the bundle is refused: nothing of it came for a minute".to_owned());
        };
        let Some(frame) = frame else {
            break;
        };
        let frame = frame.map_err(|error| error.to_string())?;
        let bytes = frame["chunk"]
            .as_str()
            .and_then(|chunk| base64::engine::general_purpose::STANDARD.decode(chunk).ok())
            .ok_or("the bundle is refused: a frame of it is not a chunk")?;
        size += bytes.len() as u64;
        if size > offer.size {
            return Err("the bundle is refused: it is longer than offered".to_owned());
        }
        hash.update(&bytes);
        written
            .write_all(&bytes)
            .await
            .map_err(|error| format!("cannot write the bundle: {error}"))?;
    }
    written
        .flush()
        .await
        .map_err(|error| format!("cannot write the bundle: {error}"))?;
    if size != offer.size || hex(&hash.finalize()) != offer.sha256 {
        return Err("the bundle is refused: it is not what was offered".to_owned());
    }
    Ok(())
}

/// The project named `project` here, and the folder of its repository `repository`.
fn repository_of(daemon: &Daemon, project: &str, repository: &str) -> Result<PathBuf, String> {
    let project = projects::load_project(daemon, project).map_err(|_| {
        format!(
            "the project {project} is not on this machine: register it there with flottille \
             --machine <this machine> project add <folder>"
        )
    })?;
    if !project.repositories.iter().any(|r| r.path == repository) {
        return Err(format!(
            "the project {} has no repository {repository} on this machine",
            project.name
        ));
    }
    Ok(match repository {
        "." => project.folder.clone(),
        path => project.folder.join(path),
    })
}

/// Whether there is a repository at `folder`, which may be a folder of nothing yet.
fn received_before(folder: &Path) -> Result<bool, String> {
    if folder.join(".git").exists() {
        return Ok(true);
    }
    match std::fs::read_dir(folder).map(|mut entries| entries.next().is_none()) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("cannot read {}: {error}", folder.display())),
        Ok(true) => Ok(false),
        Ok(false) => Err(format!(
            "{} is there and is not a repository: nothing is received into it",
            folder.display()
        )),
    }
}

/// `Has`: the commits of the repository's branches and of `refs/flottille/machines/*`.
async fn has(daemon: &Daemon, asked: &GitRepository) -> Result<Value, String> {
    let folder = repository_of(daemon, &asked.project, &asked.repository)?;
    if !received_before(&folder)? {
        return Ok(json!({"commits": null}));
    }
    let listed = git::output(
        &folder,
        &[
            "for-each-ref",
            "--format=%(objectname)",
            "refs/heads",
            "refs/flottille/machines",
        ],
        Class::Read,
    )
    .await?;
    let mut commits: Vec<&str> = listed.lines().collect();
    commits.sort_unstable();
    commits.dedup();
    Ok(json!({"commits": commits}))
}

/// `Offer` of a launch's code: read, checked, and fetched under
/// `refs/flottille/incoming/<machine>/<launch>` of the project's repository, which is cloned from
/// it when this machine never received it. A launch whose workspace is not made yet takes a
/// repository again: its sending failed part way and is tried again.
async fn offered(
    daemon: &Daemon,
    link: &Link,
    from: &PublicKey,
    name: &str,
    offer: GitOffer,
) -> Result<Value, String> {
    let folder = repository_of(daemon, &offer.project, &offer.repository)?;
    let machine = hex(from.as_bytes());
    let before = stored(daemon.read(|store| store.travels("in", Some(&machine), &offer.launch)))?;
    if before.iter().any(|travel| travel.workspace.is_some()) {
        return Err(format!("launch {} has a workspace already", offer.launch));
    }
    let scratch = scratch()?;
    let file = scratch.path().join("bundle");
    read_offered(link, &offer, &file).await?;
    let received = if received_before(&folder)? {
        bundle::receive(&folder, &file, &machine, &offer.launch).await?
    } else {
        if folder.exists() {
            std::fs::remove_dir(&folder)
        } else {
            folder.parent().map_or(Ok(()), std::fs::create_dir_all)
        }
        .map_err(|error| format!("cannot make {}: {error}", folder.display()))?;
        bundle::clone(&folder, &file, &machine, &offer.launch).await?
    };
    eprintln!(
        "flottille daemon: received {} of project {} from {name} for launch {}",
        offer.repository, offer.project, offer.launch
    );
    stored(daemon.write(|store| {
        store
            .record_travel(&Travel {
                way: "in".into(),
                machine: machine.clone(),
                launch: offer.launch.clone(),
                repository: offer.repository.clone(),
                project: offer.project.clone(),
                folder: folder.to_string_lossy().into_owned(),
                branch: offer.branch.clone(),
                tip: received.tip.clone(),
                snapshot: received.snapshot.clone(),
                workspace: None,
                there: None,
                returned: None,
                at: now_ms(),
            })
            .map(|_| ((), None))
    }))?;
    Ok(json!({}))
}

/// `Place`: the workspace made of what came for a launch, on the sender's branch name or the
/// first of `-2`, `-3`… that no repository of it has.
async fn placed(
    daemon: &Arc<Daemon>,
    from: &PublicKey,
    name: &str,
    place: GitPlace,
) -> Result<Value, String> {
    let machine = hex(from.as_bytes());
    let _placing = PLACING.lock().await;
    let travels = stored(daemon.read(|store| store.travels("in", Some(&machine), &place.launch)))?;
    let made = place_travels(daemon, &machine, name, &place, &travels).await;
    // What came is in the workspace now, or is of no use: its refs go, either way.
    for travel in &travels {
        let folder = Path::new(&travel.folder);
        if let Err(error) =
            bundle::remove_ref(folder, &bundle::incoming_ref(&machine, &place.launch)).await
        {
            eprintln!(
                "flottille daemon: what came for launch {} in {} is kept: {error}",
                place.launch,
                folder.display()
            );
        }
    }
    made
}

async fn place_travels(
    daemon: &Arc<Daemon>,
    machine: &str,
    name: &str,
    place: &GitPlace,
    travels: &[Travel],
) -> Result<Value, String> {
    let mut repositories = BTreeMap::new();
    let mut folders = Vec::new();
    for path in &place.repositories {
        let travel = travels
            .iter()
            .find(|travel| &travel.repository == path && travel.project == place.project)
            .ok_or_else(|| format!("nothing of {path} came for launch {}", place.launch))?;
        if travel.workspace.is_some() {
            return Err(format!("launch {} has a workspace already", place.launch));
        }
        repositories.insert(path.clone(), (travel.tip.clone(), travel.snapshot.clone()));
        folders.push(PathBuf::from(&travel.folder));
    }
    let branch = free_everywhere(&folders, &place.branch).await?;
    let workspace = free_name(daemon, &place.project, &branch)?;
    let asked = flottille_core::protocol::WorkspaceCreate {
        project: place.project.clone(),
        name: workspace.clone(),
        repositories: Some(place.repositories.clone()),
        branch: Some(branch.clone()),
        bases: BTreeMap::new(),
        offline: true,
        ticket: None,
    };
    let travelled = Travelled {
        from: name.to_owned(),
        repositories,
    };
    let made = workspaces::create_from(daemon.clone(), asked, Some(travelled))
        .await
        .map_err(|failure| failure.message)?;
    stored(daemon.write(|store| {
        store
            .set_travel_place(machine, &place.launch, &workspace, &branch)
            .map(|()| ((), None))
    }))?;
    eprintln!(
        "flottille daemon: workspace {workspace} of project {} made of launch {} from {name}",
        place.project, place.launch
    );
    Ok(json!({"workspace": workspace, "branch": branch, "folder": made["folder"]}))
}

/// `Offer` of new commits coming back for a launch this machine sent to `from`: fetched into
/// `refs/flottille/machines/<that machine>/<launch>/<branch>` of the repository they were sent from,
/// never onto a branch. A launch this machine did not send to that machine is refused.
async fn came_back(
    daemon: &Daemon,
    link: &Link,
    from: &PublicKey,
    name: &str,
    offer: GitOffer,
) -> Result<Value, String> {
    let machine = hex(from.as_bytes());
    let travels = stored(daemon.read(|store| store.travels("out", Some(&machine), &offer.launch)))?;
    let travel = travels
        .iter()
        .find(|travel| travel.repository == offer.repository && travel.project == offer.project)
        .ok_or_else(|| {
            format!(
                "{} of launch {} was not sent to {name}",
                offer.repository, offer.launch
            )
        })?;
    let scratch = scratch()?;
    let file = scratch.path().join("bundle");
    read_offered(link, &offer, &file).await?;
    let folder = PathBuf::from(&travel.folder);
    let reference =
        bundle::fetch_returned(&folder, &file, name, &offer.launch, &travel.branch).await?;
    let tip = git::output(&folder, &["rev-parse", "--verify", &reference], Class::Read)
        .await?
        .trim()
        .to_owned();
    stored(daemon.write(|store| {
        store
            .set_travel_returned("out", &machine, &offer.launch, &offer.repository, &tip)
            .map(|()| ((), None))
    }))?;
    eprintln!(
        "flottille daemon: the new commits of launch {} came back from {name} into {reference}",
        offer.launch
    );
    Ok(json!({"ref": reference}))
}

/// The first of `wanted`, `wanted-2`, `wanted-3`… that is no branch of any of `folders`.
async fn free_everywhere(folders: &[PathBuf], wanted: &str) -> Result<String, String> {
    let mut name = wanted.to_owned();
    'names: for n in 2.. {
        for folder in folders {
            if bundle::free_branch(folder, &name).await? != name {
                name = format!("{wanted}-{n}");
                continue 'names;
            }
        }
        break;
    }
    Ok(name)
}

/// A workspace name of the project made of `branch`, free: its letters, digits, dots, dashes and
/// underscores, the others dashes, `-2`, `-3`… when it is taken.
fn free_name(daemon: &Daemon, project: &str, branch: &str) -> Result<String, String> {
    let mut base: String = branch
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '-' | '_' => c,
            _ => '-',
        })
        .collect();
    base = base.trim_start_matches(['.', '-', '_']).to_owned();
    base.truncate(56);
    if base.is_empty() {
        base = "travelled".to_owned();
    }
    let root = projects::load_project(daemon, project)
        .map_err(|failure| failure.message)?
        .workspaces_root;
    for n in 1.. {
        let name = match n {
            1 => base.clone(),
            n => format!("{base}-{n}"),
        };
        let taken = stored(daemon.read(|store| store.workspace(project, &name)))?.is_some()
            || std::fs::symlink_metadata(root.join(&name)).is_ok();
        if !taken && flottille_core::project::valid_name(&name) {
            return Ok(name);
        }
    }
    unreachable!("a workspace name is free past every one taken")
}

fn scratch() -> Result<tempfile::TempDir, String> {
    tempfile::tempdir().map_err(|error| format!("cannot make a folder: {error}"))
}

/// The commits a bundle of `reference` holds past its prerequisites.
async fn counted(folder: &Path, reference: &str, past: &[String]) -> Result<u64, String> {
    let mut args = vec!["rev-list", "--count", reference];
    if !past.is_empty() {
        args.push("--not");
        args.extend(past.iter().map(String::as_str));
    }
    let count = git::output(folder, &args, Class::Read).await?;
    count
        .trim()
        .parse()
        .map_err(|_| format!("git counted {count:?}"))
}

/// The prerequisites written in a bundle's header: its lines up to the first blank one, never
/// the pack after it.
async fn prerequisites(file: &Path) -> Result<Vec<String>, String> {
    let failed = |error: std::io::Error| format!("cannot read the bundle: {error}");
    let mut lines =
        tokio::io::BufReader::new(tokio::fs::File::open(file).await.map_err(failed)?).lines();
    let mut found = Vec::new();
    while let Some(line) = lines.next_line().await.map_err(failed)? {
        if line.is_empty() {
            break;
        }
        if let Some(id) = line
            .strip_prefix('-')
            .and_then(|line| line.split(' ').next())
        {
            found.push(id.to_owned());
        }
    }
    Ok(found)
}

/// The length of a bundle, and its SHA-256 in hexadecimal, read a chunk at a time.
async fn measured(file: &Path) -> Result<(u64, String), String> {
    let failed = |error: std::io::Error| format!("cannot read the bundle: {error}");
    let mut read = tokio::fs::File::open(file).await.map_err(failed)?;
    let mut hash = Sha256::new();
    let mut size = 0u64;
    let mut buffer = vec![0; CHUNK];
    loop {
        let n = read.read(&mut buffer).await.map_err(failed)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
        size += n as u64;
    }
    Ok((size, hex(&hash.finalize())))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// A test's bundle broken in its pack, for the receiver to refuse.
fn corrupted(file: &Path) -> Result<(), String> {
    let mut bytes =
        std::fs::read(file).map_err(|error| format!("cannot read the bundle: {error}"))?;
    let at = bytes.len().saturating_sub(100);
    if let Some(byte) = bytes.get_mut(at) {
        *byte ^= 0xff;
    }
    std::fs::write(file, bytes).map_err(|error| format!("cannot write the bundle: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bundle_is_read_only_by_the_machine_it_is_offered_to() {
        let transfers = Transfers::default();
        let keys = tempfile::tempdir().unwrap();
        let key = |name: &str| {
            flottille_link::MachineKey::load_or_make(&keys.path().join(name))
                .unwrap()
                .public()
        };
        let (ours, other) = (key("ours"), key("other"));
        let file = Path::new("/nowhere/bundle");
        {
            let _offered = Offered::new(&transfers, "l1/0".into(), &ours, file);
            assert_eq!(transfers.file_for("l1/0", &ours), Some(file.to_owned()));
            assert_eq!(transfers.file_for("l1/0", &other), None);
            assert_eq!(transfers.file_for("l1/1", &ours), None);
        }
        // Once its offer is answered, no one reads it.
        assert_eq!(transfers.file_for("l1/0", &ours), None);
    }

    #[test]
    fn a_bundle_past_the_ceiling_is_refused_before_it_is_read() {
        assert!(fits(MOST_BYTES).is_ok());
        let refused = fits(MOST_BYTES + 1).unwrap_err();
        assert!(refused.contains("more than 2 GiB"), "{refused}");
        assert_eq!(MOST_BYTES, 2 << 30);
    }

    #[test]
    fn a_machine_sends_one_bundle_at_a_time() {
        let receiving = Receiving::default();
        let keys = tempfile::tempdir().unwrap();
        let key = |name: &str| {
            flottille_link::MachineKey::load_or_make(&keys.path().join(name))
                .unwrap()
                .public()
        };
        let (pc, laptop) = (key("pc"), key("laptop"));
        {
            let _held = receiving.hold(&pc, "pc").unwrap();
            let refused = receiving.hold(&pc, "pc").err().unwrap();
            assert!(
                refused.contains("pc is sending something already"),
                "{refused}"
            );
            // Another machine is not kept waiting by it.
            assert!(receiving.hold(&laptop, "laptop").is_ok());
        }
        assert!(receiving.hold(&pc, "pc").is_ok());
    }

    #[tokio::test]
    async fn a_bundle_s_prerequisites_are_read_from_its_header_alone() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("bundle");
        let header = format!(
            "# v2 git bundle\n-{} first\n{} refs/x\n\n",
            "a".repeat(40),
            "b".repeat(40)
        );
        let mut bytes = header.into_bytes();
        // A pack that happens to hold a line like one of the header's.
        bytes.extend(format!("PACK\n-{} not one\n", "c".repeat(40)).as_bytes());
        std::fs::write(&file, bytes).unwrap();

        assert_eq!(prerequisites(&file).await.unwrap(), vec!["a".repeat(40)]);
    }

    #[tokio::test]
    async fn a_bundle_is_measured_in_chunks() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("bundle");
        let bytes: Vec<u8> = (0..CHUNK * 2 + 7).map(|n| n as u8).collect();
        std::fs::write(&file, &bytes).unwrap();

        let (size, sha256) = measured(&file).await.unwrap();

        assert_eq!(size, bytes.len() as u64);
        assert_eq!(sha256, hex(&Sha256::digest(&bytes)));
    }
}
