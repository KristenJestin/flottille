//! Every method the daemon answers, with its class (`docs/remote.md`, "Roles"). The daemon
//! answers no other: a method left out of it is unknown.

use Class::{Drive, Local, Read};

/// What a method does: `Read` changes nothing (a list, a show, a read, `events.watch`), `Drive`
/// changes something. A device that may only view is allowed `Read` alone, so a method in doubt
/// is `Drive`. `Local` is answered only to a client of the machine itself, never over a link,
/// whatever the caller's role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Read,
    Drive,
    Local,
}

/// The catalogue, by family in the order of `docs/protocol.md`.
pub const METHODS: &[(&str, Class)] = &[
    ("daemon.status", Read),
    ("daemon.stop", Drive),
    ("events.watch", Read),
    ("project.list", Read),
    ("project.show", Read),
    // It looks through any folder the client names, and serves only `project.add`.
    ("project.detect", Drive),
    ("project.add", Drive),
    ("project.removal", Read),
    ("project.remove", Drive),
    ("project.check", Read),
    ("project.repositories", Read),
    ("project.add_repository", Drive),
    ("project.remove_repository", Drive),
    ("project.settings", Read),
    ("project.programs", Read),
    ("project.set_settings", Drive),
    ("project.set_conventions", Drive),
    ("project.set_message_checks", Drive),
    ("project.commit_checks", Read),
    ("project.set_commit_checks", Drive),
    ("project.ticket", Read),
    ("project.progress", Read),
    ("rules.show", Read),
    ("rules.set", Drive),
    ("rules.reset", Drive),
    ("judge.show", Read),
    ("judge.set", Drive),
    ("judge.reset", Drive),
    ("compaction.show", Read),
    ("compaction.set", Drive),
    ("compaction.set_guidance", Drive),
    ("compaction.reset_guidance", Drive),
    ("compaction.noticed", Drive),
    ("developer.show", Read),
    ("developer.set", Drive),
    ("update.show", Read),
    ("update.set", Drive),
    ("update.check", Drive),
    // They replace this machine's binary and restart its daemon: asked on the machine only.
    ("update.download", Local),
    ("update.restart", Local),
    ("extension.list", Read),
    ("extension.set", Drive),
    ("extension.act", Drive),
    ("connection.list", Read),
    ("connection.add", Drive),
    ("connection.edit", Drive),
    ("connection.remove", Drive),
    // It starts the connection's server.
    ("connection.review", Drive),
    ("connection.accept", Drive),
    ("connection.login", Drive),
    ("connection.proposed", Read),
    // The user's Share and Bind of a vault entry an agent's request names.
    ("connection.share_entry", Drive),
    ("connection.bind_entry", Drive),
    ("connection.share", Drive),
    ("spec.new", Drive),
    ("spec.from_ticket", Drive),
    ("spec.answer", Drive),
    // Settling a spec as developed is the daemon catching up on facts, not a decision of the
    // user: a viewer sees the specs.
    ("spec.list", Read),
    ("spec.show", Read),
    ("spec.write", Drive),
    ("spec.ready", Drive),
    ("spec.ticket", Drive),
    // With `post`, it posts them on the tickets.
    ("spec.departures", Drive),
    ("spec.develop", Drive),
    ("spec.link", Drive),
    ("spec.unlink", Drive),
    ("spec.amendments", Read),
    ("spec.amendment", Drive),
    ("spec.draft", Drive),
    ("review.start", Drive),
    ("review.list", Read),
    ("review.finding", Drive),
    ("review.show", Read),
    ("review.set", Drive),
    ("review.reset", Drive),
    ("autosubscriptions.events", Read),
    ("appearance.show", Read),
    ("appearance.set", Drive),
    ("tool_results.show", Read),
    ("tool_results.set", Drive),
    ("sessions.show", Read),
    ("sessions.set", Drive),
    ("workspaces_folder.show", Read),
    ("workspaces_folder.set", Drive),
    // What the user left in the window is of the machine: a device that follows over a link reads
    // none of it.
    ("draft.get", Local),
    ("draft.set", Local),
    ("draft.clear", Local),
    ("draft.list", Local),
    ("window.get", Local),
    ("window.set", Local),
    ("vault.list", Read),
    ("vault.set", Drive),
    ("vault.remove", Drive),
    ("vault.share", Drive),
    ("vault.asks", Read),
    ("vault.ask_dismiss", Drive),
    ("remote.machines", Read),
    ("remote.address.add", Drive),
    ("remote.address.remove", Drive),
    ("remote.members", Read),
    ("remote.launches", Read),
    ("remote.requests", Read),
    ("remote.invite", Drive),
    ("remote.refuse", Drive),
    ("remote.change", Drive),
    // It points this machine at a hub and a root of its own: the machine's user only.
    ("remote.setup", Local),
    // As `remote.setup`: a join, then the declaration of this machine by the device that joined.
    ("remote.join", Local),
    ("remote.declare", Local),
    // It opens a port on the client's machine for a run's port on this one: a user's act.
    ("tunnel.open", Drive),
    // What a machine's daemon carries is its own: the listeners are here, never over a link.
    ("tunnel.list", Local),
    ("tunnel.close", Local),
    ("skill.list", Read),
    ("skill.show", Read),
    ("skill.add", Drive),
    ("skill.edit", Drive),
    ("skill.remove", Drive),
    ("skill.detect", Read),
    ("skill.import", Drive),
    ("command.list", Read),
    ("command.show", Read),
    ("command.add", Drive),
    ("command.edit", Drive),
    ("command.remove", Drive),
    ("command.detect", Read),
    ("command.import", Drive),
    ("task.list", Read),
    ("task.show", Read),
    ("task.add", Drive),
    ("task.copy", Drive),
    ("task.edit", Drive),
    ("task.remove", Drive),
    ("task.run", Drive),
    ("setup.list", Read),
    ("setup.add", Drive),
    ("setup.edit", Drive),
    ("setup.remove", Drive),
    ("setup.move", Drive),
    ("workspace.create", Drive),
    ("workspace.list", Read),
    // Storing the branch a worktree moved to is the daemon catching up on facts, not a decision
    // of the user: a viewer sees the workspaces.
    ("workspace.show", Read),
    ("workspace.step_output", Read),
    // A read of the forge; storing its state is the daemon catching up on facts, not a decision of
    // the user, and a viewer sees the pull requests. With `watch`, the forge is read again while
    // the connection stays, a viewer's too: accepted (`docs/remote.md`, "Defaults taken, to
    // confirm").
    ("workspace.forge", Read),
    ("workspace.ticket", Read),
    ("workspace.link_ticket", Drive),
    ("workspace.rename_branch", Drive),
    ("workspace.prepare", Drive),
    ("workspace.remove", Drive),
    ("items.list", Read),
    ("items.show", Read),
    // It reads the forges and the tracker now and stores what changed.
    ("items.refresh", Drive),
    ("run.list", Read),
    ("run.output", Read),
    ("run.stop", Drive),
    ("run.restart", Drive),
    ("run.dismiss", Drive),
    ("service.list", Read),
    ("service.show", Read),
    ("service.add", Drive),
    ("service.edit", Drive),
    ("service.remove", Drive),
    ("service.start", Drive),
    ("service.stop", Drive),
    ("service.output", Read),
    ("agent.start", Drive),
    // `agent.start`'s name before it.
    ("agent.run", Drive),
    ("agent.send", Drive),
    ("agent.send_now", Drive),
    ("agent.unqueue", Drive),
    ("agent.answer", Drive),
    ("agent.image", Read),
    ("agent.cancel", Drive),
    ("agent.stop", Drive),
    ("agent.resume", Drive),
    ("agent.quota_resume", Drive),
    ("agent.reload_tools", Drive),
    ("agent.compact", Drive),
    ("agent.rename", Drive),
    ("agent.ending", Read),
    ("agent.detach", Drive),
    ("agent.mode", Drive),
    ("agent.options", Drive),
    ("agent.list", Read),
    ("agent.show", Read),
    ("agent.read", Read),
    ("agent.status", Read),
    ("agent.tree", Read),
    ("agent.trees", Read),
    ("approval.list", Read),
    ("approval.approve", Drive),
    ("approval.deny", Drive),
    ("subscription.list", Read),
    ("subscription.cancel", Drive),
    ("subscription.resume", Drive),
    ("leave.list", Read),
    ("leave.withdraw", Drive),
    ("inbox.list", Read),
    ("provider.list", Read),
    ("provider.enable", Drive),
    ("provider.disable", Drive),
    ("provider.set_judge", Drive),
    ("provider.set_new_session", Drive),
    ("provider.judge_noticed", Drive),
    // The tests' own, answered only when the daemon runs with their variable.
    ("test.hold", Drive),
    ("test.wait", Drive),
    ("test.advance", Drive),
    ("test.travel", Drive),
    ("test.travel_back", Drive),
];

/// The methods that start an agent's process (`docs/remote.md`, "Roles"): a machine whose member
/// is no runner runs no agent, so it refuses them. Each may start one: `spec.new` when it is
/// given an idea, `agent.quota_resume` when the reset comes.
pub const LAUNCHES: &[&str] = &[
    "agent.start",
    "agent.run",
    "agent.resume",
    "agent.quota_resume",
    "spec.new",
    "spec.from_ticket",
    "spec.develop",
    "review.start",
];

/// Whether `method` starts an agent's process.
pub fn launches(method: &str) -> bool {
    LAUNCHES.contains(&method)
}

/// The class of `method`; none for a method the daemon does not answer.
pub fn class(method: &str) -> Option<Class> {
    METHODS
        .iter()
        .find(|(name, _)| *name == method)
        .map(|(_, class)| *class)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_method_is_catalogued_once() {
        for (i, (name, _)) in METHODS.iter().enumerate() {
            assert!(
                METHODS[i + 1..].iter().all(|(other, _)| other != name),
                "{name} is catalogued twice"
            );
        }
    }

    #[test]
    fn a_method_that_changes_nothing_is_read_and_any_other_drive() {
        assert_eq!(class("daemon.status"), Some(Read));
        assert_eq!(class("events.watch"), Some(Read));
        assert_eq!(class("agent.list"), Some(Read));
        assert_eq!(class("agent.send"), Some(Drive));
        assert_eq!(class("approval.approve"), Some(Drive));
        assert_eq!(class("daemon.stop"), Some(Drive));
    }

    #[test]
    fn the_drafts_and_the_window_s_state_are_of_the_machine_alone() {
        // What the user left in the window is not read by a device that follows over a link.
        for method in [
            "draft.get",
            "draft.set",
            "draft.clear",
            "draft.list",
            "window.get",
            "window.set",
        ] {
            assert_eq!(class(method), Some(Local), "{method}");
        }
    }

    #[test]
    fn every_method_that_starts_an_agent_is_a_launch() {
        for method in [
            "agent.start",
            "agent.run",
            "agent.resume",
            "agent.quota_resume",
            "spec.new",
            "spec.from_ticket",
            "spec.develop",
            "review.start",
        ] {
            assert!(launches(method), "{method}");
        }
        // A command, a message to a session that runs, a read: no agent starts.
        for method in ["task.run", "agent.send", "agent.list", "spec.list"] {
            assert!(!launches(method), "{method}");
        }
        // Each launch is a method of the catalogue, and changes something.
        for method in LAUNCHES {
            assert_eq!(class(method), Some(Drive), "{method}");
        }
    }

    #[test]
    fn a_method_the_daemon_does_not_answer_has_no_class() {
        assert_eq!(class("agent.nope"), None);
        assert_eq!(class("agent"), None);
        assert_eq!(class(""), None);
    }
}
