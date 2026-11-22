//! The window does everything the command line does (#1179): the maintainer does nothing from a
//! terminal. Every command of `docs/cli.md` has an entry of `COMMANDS` below: the daemon methods
//! the window calls for it, each one a string literal of the window's sources, or a reason it is
//! for scripts only. A command added to the command line without its place in the window fails
//! here until it has one, or until its entry says why it has none. (The table is here, not in
//! the window's sources: the window never says "daemon stop" nor "extension list".)

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Where the window does a command.
pub enum Place {
    /// The daemon methods the window calls for it: each one is a string literal of the window's
    /// sources, which the test checks.
    Window(&'static [&'static str]),
    /// It has no place in the window, and why.
    Script(&'static str),
}

use Place::{Script, Window};

/// Every command of `docs/cli.md`, by its words (`spec link`), in the order of the file.
pub const COMMANDS: &[(&str, Place)] = &[
    // ── The daemon ───────────────────────────────────────────────────────────────────────────
    (
        "daemon",
        Script("starts the process the window talks to: the window is started by it"),
    ),
    (
        "daemon stop",
        Script("stops the process the window talks to, from a terminal"),
    ),
    ("status", Window(&["daemon.status"])),
    (
        "watch",
        Script("a terminal's live view of the sessions: the window is that view"),
    ),
    // The Check Flottille section of Settings, General.
    (
        "doctor",
        Window(&["provider.list", "project.programs", "daemon.status"]),
    ),
    ("provider list", Window(&["provider.list"])),
    (
        "provider new-session",
        Window(&["provider.set_new_session"]),
    ),
    ("provider enable", Window(&["provider.enable"])),
    ("provider disable", Window(&["provider.disable"])),
    ("developer show", Window(&["developer.show"])),
    ("developer on", Window(&["developer.set"])),
    ("developer off", Window(&["developer.set"])),
    (
        "developer reports",
        Window(&["developer.reports", "developer.reports_export"]),
    ),
    // Mark filed… in a finding's ⋯.
    (
        "developer reports filed",
        Window(&["developer.report_filed"]),
    ),
    (
        "update",
        Window(&[
            "update.check",
            "update.download",
            "update.cancel",
            "update.restart",
        ]),
    ),
    ("changes", Window(&["whatsnew.show"])),
    // ── Remote ───────────────────────────────────────────────────────────────────────────────
    ("remote on", Window(&["remote.configure"])),
    ("remote off", Window(&["remote.configure"])),
    ("remote pair", Window(&["remote.pair", "remote.declare"])),
    ("remote members", Window(&["remote.machines"])),
    ("remote rename", Window(&["remote.change"])),
    ("remote icon", Window(&["remote.change"])),
    ("remote colour", Window(&["remote.change"])),
    ("remote remove", Window(&["remote.change"])),
    // ── Tunnels ──────────────────────────────────────────────────────────────────────────────
    ("tunnel open", Window(&["tunnel.open"])),
    ("tunnel list", Window(&["tunnel.list"])),
    ("tunnel close", Window(&["tunnel.close"])),
    // ── Extensions and specs ─────────────────────────────────────────────────────────────────
    ("extension list", Window(&["extension.list"])),
    ("extension on", Window(&["extension.set"])),
    ("extension off", Window(&["extension.set"])),
    ("connection list", Window(&["connection.list"])),
    ("connection add", Window(&["connection.add"])),
    ("connection edit", Window(&["connection.edit"])),
    ("connection remove", Window(&["connection.remove"])),
    ("connection review", Window(&["connection.review"])),
    ("connection login", Window(&["connection.login"])),
    ("connection share", Window(&["connection.share"])),
    ("connection accept", Window(&["connection.accept"])),
    ("spec new", Window(&["spec.new"])),
    ("spec from-ticket", Window(&["spec.from_ticket"])),
    ("spec answer", Window(&["spec.answer"])),
    ("spec list", Window(&["spec.list"])),
    ("spec show", Window(&["spec.show"])),
    ("spec write", Window(&["spec.write"])),
    ("spec link", Window(&["spec.link"])),
    ("spec unlink", Window(&["spec.unlink"])),
    ("spec ready", Window(&["spec.ready"])),
    ("spec ticket", Window(&["spec.ticket"])),
    ("spec departures", Window(&["spec.departures"])),
    ("spec develop", Window(&["spec.develop"])),
    ("spec draft", Window(&["spec.draft"])),
    // The page of a spec draws its amendments from `spec.show`, which carries them.
    ("spec amendments", Window(&["spec.show"])),
    ("spec amendment accept|refuse", Window(&["spec.amendment"])),
    ("review", Window(&["review.start"])),
    // ── Projects ─────────────────────────────────────────────────────────────────────────────
    ("project add", Window(&["project.add"])),
    ("project list", Window(&["project.list"])),
    ("project show", Window(&["project.show"])),
    ("project check", Window(&["project.check"])),
    ("project commit-checks", Window(&["project.commit_checks"])),
    (
        "project set-commit-checks",
        Window(&["project.set_commit_checks"]),
    ),
    ("project edit", Window(&["project.set_settings"])),
    ("project remove", Window(&["project.remove"])),
    // ── Skills ───────────────────────────────────────────────────────────────────────────────
    ("skill list", Window(&["skill.list"])),
    ("skill show", Window(&["skill.show"])),
    ("skill add", Window(&["skill.add"])),
    ("skill edit", Window(&["skill.edit"])),
    ("skill import", Window(&["skill.import"])),
    ("skill remove", Window(&["skill.remove"])),
    // ── The vault ────────────────────────────────────────────────────────────────────────────
    ("vault list", Window(&["vault.list"])),
    ("vault set", Window(&["vault.set"])),
    ("vault edit", Window(&["vault.set"])),
    ("vault remove", Window(&["vault.remove"])),
    ("vault share", Window(&["vault.share"])),
    (
        "vault provide",
        Script("a socket the processes of a lent entry read: no one acts on it"),
    ),
    // ── Workspaces ───────────────────────────────────────────────────────────────────────────
    ("workspace create", Window(&["workspace.create"])),
    ("workspace list", Window(&["workspace.list"])),
    ("workspace show", Window(&["workspace.show"])),
    ("workspace ticket", Window(&["workspace.link_ticket"])),
    ("workspace prepare", Window(&["workspace.prepare"])),
    ("workspace remove", Window(&["workspace.remove"])),
    // ── Commands ─────────────────────────────────────────────────────────────────────────────
    ("command list", Window(&["command.list"])),
    ("command show", Window(&["command.show"])),
    ("command add", Window(&["command.add"])),
    ("command edit", Window(&["command.edit"])),
    ("command import", Window(&["command.import"])),
    ("command remove", Window(&["command.remove"])),
    // ── Agents ───────────────────────────────────────────────────────────────────────────────
    ("agent start", Window(&["agent.start"])),
    (
        "agent run",
        Script(
            "starts a session and prints its end, for a script: the window starts one and shows it",
        ),
    ),
    ("agent send", Window(&["agent.send"])),
    ("agent answer", Window(&["agent.answer"])),
    ("agent unqueue", Window(&["agent.unqueue"])),
    ("agent send-now", Window(&["agent.send_now"])),
    // The conversation of a session is its page.
    ("agent chat", Window(&["agent.send", "agent.cancel"])),
    ("agent read", Window(&["agent.show"])),
    ("agent show", Window(&["agent.show"])),
    ("agent stop", Window(&["agent.stop"])),
    ("agent reload-tools", Window(&["agent.reload_tools"])),
    ("agent compact", Window(&["agent.compact"])),
    ("agent resume", Window(&["agent.resume"])),
    ("agent rename", Window(&["agent.rename"])),
    ("agent detach", Window(&["agent.detach"])),
    ("agent options", Window(&["agent.options"])),
    ("agent mode", Window(&["agent.mode"])),
    ("agent list", Window(&["agent.list"])),
    ("agent tree", Window(&["agent.tree"])),
    // ── What reaches you ─────────────────────────────────────────────────────────────────────
    // The bell and the sessions that need an answer say what `inbox` lists.
    // ── Routines: the Routines tab of a project ──────────────────────────────────────────────
    ("routine add", Window(&["routine.add"])),
    ("routine list", Window(&["routine.list"])),
    ("routine show", Window(&["routine.show"])),
    ("routine edit", Window(&["routine.edit"])),
    ("routine run", Window(&["routine.run"])),
    ("routine pause", Window(&["routine.pause"])),
    ("routine resume", Window(&["routine.resume"])),
    ("routine remove", Window(&["routine.remove"])),
    ("inbox", Window(&["notifications.list"])),
    ("notifications", Window(&["notifications.list"])),
    ("notifications read", Window(&["notifications.read"])),
    (
        "notifications settings",
        Window(&["notifications.settings", "notifications.set_settings"]),
    ),
    ("runs list", Window(&["run.list"])),
    ("runs stop", Window(&["run.stop"])),
    ("runs restart", Window(&["run.restart"])),
    ("service list", Window(&["service.list"])),
    // A service's page shows what `service.show` would say, from `service.list`.
    ("service show", Window(&["service.list"])),
    ("service add", Window(&["service.add"])),
    ("service edit", Window(&["service.edit"])),
    ("service remove", Window(&["service.remove"])),
    ("service start", Window(&["service.start"])),
    ("service stop", Window(&["service.stop"])),
    ("service output", Window(&["run.output"])),
    ("approvals", Window(&["approval.list"])),
    ("approve", Window(&["approval.approve"])),
    ("deny", Window(&["approval.deny"])),
    // ── The window and the doors ─────────────────────────────────────────────────────────────
    ("window", Script("opens the window")),
    (
        "open",
        Script("brings the window to a session: what a desktop notification's click runs"),
    ),
    (
        "bridge claude",
        Script("the door a Claude Code agent is run through: not a user's act"),
    ),
    (
        "bridge codex",
        Script("the door a Codex agent is run through: not a user's act"),
    ),
    (
        "read-page",
        Script("the web tool's page reader, run by Flottille for an agent"),
    ),
    (
        "mcp",
        Script("the MCP server an outside agent starts on its own"),
    ),
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The commands of `docs/cli.md`: the words of each `### \`flottille …\`` heading before its first
/// argument or option.
fn documented() -> BTreeSet<String> {
    let text = std::fs::read_to_string(root().join("docs/cli.md")).unwrap();
    text.lines()
        .filter_map(|line| line.strip_prefix("### `flottille "))
        .filter_map(|line| line.split('`').next())
        .map(|heading| {
            heading
                .split_whitespace()
                .take_while(|word| {
                    !word.starts_with(['<', '[', '(', '-']) && !word.contains(['<', '['])
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

/// Everything the window's sources say, to find a method called by its string literal.
fn window_sources() -> String {
    fn walk(folder: &Path, into: &mut String) {
        for entry in std::fs::read_dir(folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, into);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                into.push_str(&std::fs::read_to_string(&path).unwrap());
            }
        }
    }
    let mut sources = String::new();
    walk(&root().join("crates/flottille-window/src"), &mut sources);
    sources
}

#[test]
fn every_cli_command_has_its_place_in_the_window() {
    let documented = documented();
    let listed: BTreeSet<String> = COMMANDS
        .iter()
        .map(|(name, _)| (*name).to_owned())
        .collect();
    assert_eq!(
        listed.len(),
        COMMANDS.len(),
        "a command is listed twice in flottille_window::parity::COMMANDS"
    );
    let missing: Vec<_> = documented.difference(&listed).collect();
    assert!(
        missing.is_empty(),
        "a command of docs/cli.md has no place in the window: give it the daemon methods the \
         window calls for it in crates/flottille/tests/window_parity.rs, or say why it is for \
         scripts only: {missing:?}"
    );
    let stale: Vec<_> = listed.difference(&documented).collect();
    assert!(
        stale.is_empty(),
        "the table of window_parity.rs lists commands docs/cli.md does not have: {stale:?}"
    );

    let sources = window_sources();
    for (name, place) in COMMANDS {
        match place {
            Place::Window(methods) => {
                assert!(
                    !methods.is_empty(),
                    "{name}: no method says where it is done"
                );
                for method in *methods {
                    assert!(
                        sources.contains(&format!("\"{method}\"")),
                        "{name}: the window never calls {method}"
                    );
                }
            }
            Place::Script(why) => assert!(
                why.len() > 12,
                "{name}: a script-only command says why, in a sentence"
            ),
        }
    }

    // The script-only list stays short: a command is for scripts when it is not a user's act.
    let scripts = COMMANDS
        .iter()
        .filter(|(_, place)| matches!(place, Place::Script(_)))
        .count();
    assert!(
        scripts <= 12,
        "{scripts} commands are for scripts only: is each one not a user's act?"
    );
}
