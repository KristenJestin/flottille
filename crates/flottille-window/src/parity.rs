//! What the command line does, and where the window does it (#1179). The maintainer does nothing
//! from the command line: every command of `docs/cli.md` has its place in the window, said here
//! by the daemon methods the window calls for it, or is for scripts only, with the reason.
//! `tests/window_parity.rs` walks the headings of `docs/cli.md` against this list: a command
//! added to the command line comes with its place in the window in the same pull request, or an
//! entry of the second kind that says why it has none.

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
        Window(&["update.check", "update.download", "update.restart"]),
    ),
    ("changes", Window(&["whatsnew.show"])),
    // ── Remote ───────────────────────────────────────────────────────────────────────────────
    ("remote on", Window(&["remote.configure"])),
    ("remote off", Window(&["remote.configure"])),
    ("remote listen", Window(&["remote.configure"])),
    ("remote pair", Window(&["remote.pair", "remote.declare"])),
    ("remote members", Window(&["remote.machines"])),
    ("remote rename", Window(&["remote.change"])),
    ("remote icon", Window(&["remote.change"])),
    ("remote colour", Window(&["remote.change"])),
    ("remote remove", Window(&["remote.change"])),
    ("remote address list", Window(&["remote.machines"])),
    ("remote address add", Window(&["remote.address.add"])),
    ("remote address remove", Window(&["remote.address.remove"])),
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
    ("inbox", Window(&["notifications.list"])),
    ("notifications", Window(&["notifications.list"])),
    ("notifications read", Window(&["notifications.read"])),
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
