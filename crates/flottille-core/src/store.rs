//! The store: one SQLite file, its schema brought up to date by numbered migrations.

use std::cell::RefCell;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

use crate::agent::{Agent, AgentState, Choices, GateMode, TurnUsage, Used};
use crate::approval::{Decision, Delivery, Forward, Judgement, Kind, Leave, Request, RequestState};
use crate::item::{Check, Cursor, Filter, Item, Post};
use crate::protocol::Event;
use crate::run::{Readiness, Run, RunState};
use crate::subscription::Subscription;
use crate::workspace::{Action, Failure, Step, StepState, Workspace};
use crate::{Error, Result};

/// Applied in order; the database's `user_version` is the number of those already applied. A
/// migration is never edited once released: a change is a new one.
const MIGRATIONS: &[&str] = &[
    "
    -- AUTOINCREMENT: a number is never given twice, even after the last events are deleted, so a
    -- client resuming with `--after` never misses one.
    CREATE TABLE events (
        seq  INTEGER PRIMARY KEY AUTOINCREMENT,
        at   INTEGER NOT NULL,
        name TEXT NOT NULL,
        data TEXT NOT NULL
    ) STRICT;
",
    "
    -- A workspace and what it was made from: its repositories with their bases and the project's
    -- variables, as JSON, written once. Its steps change as it is prepared.
    CREATE TABLE workspaces (
        id             INTEGER PRIMARY KEY,
        project        TEXT NOT NULL,
        name           TEXT NOT NULL,
        branch         TEXT NOT NULL,
        folder         TEXT NOT NULL,
        project_folder TEXT NOT NULL,
        created_at     INTEGER NOT NULL,
        variables      TEXT NOT NULL,
        repositories   TEXT NOT NULL,
        UNIQUE (project, name)
    ) STRICT;
    CREATE TABLE workspace_steps (
        workspace     INTEGER NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
        position      INTEGER NOT NULL,
        repository    TEXT,
        action        TEXT NOT NULL,
        state         TEXT NOT NULL,
        failed_doing  TEXT,
        failed_output TEXT,
        PRIMARY KEY (workspace, position)
    ) STRICT;
",
    "
    -- An agent session (`docs/agents.md`): where it works, what it was started with (frozen:
    -- its instructions and its tool names), what the agent took, and the ACP session id a
    -- restarted process opens again. The tree (parent, lineage, depth) comes with the deliveries.
    CREATE TABLE agents (
        id            TEXT PRIMARY KEY,
        provider      TEXT NOT NULL,
        role          TEXT NOT NULL,
        project       TEXT NOT NULL,
        workspace     TEXT,
        folder        TEXT NOT NULL,
        state         TEXT NOT NULL,
        state_reason  TEXT,
        created_at    INTEGER NOT NULL,
        updated_at    INTEGER NOT NULL,
        active_at     INTEGER,
        ended_at      INTEGER,
        chosen_model  TEXT,
        chosen_effort TEXT,
        chosen_mode   TEXT,
        taken_model   TEXT,
        taken_effort  TEXT,
        taken_mode    TEXT,
        native_id     TEXT,
        agent_version TEXT,
        instructions  TEXT NOT NULL,
        tools         TEXT NOT NULL
    ) STRICT;
    -- Which events are an agent's conversation: an agent's events are read without scanning
    -- every event.
    CREATE TABLE agent_events (
        seq   INTEGER PRIMARY KEY REFERENCES events (seq),
        agent TEXT NOT NULL REFERENCES agents (id) ON DELETE CASCADE
    ) STRICT;
    CREATE INDEX agent_events_by_agent ON agent_events (agent, seq);
    -- The adapter processes running, each known by its pid and its start time, so that a daemon
    -- started after a crash kills its own leftovers and never a process that took the pid since.
    CREATE TABLE agent_processes (
        pid     INTEGER PRIMARY KEY,
        started INTEGER NOT NULL,
        agent   TEXT NOT NULL
    ) STRICT;
",
    "
    -- What each agent's session read (`docs/tools.md`): the fingerprint of every file it read or
    -- wrote, so that it never writes over a change it has not seen, across restarts too.
    CREATE TABLE session_files (
        agent  TEXT NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
        path   TEXT NOT NULL,
        sha256 TEXT NOT NULL,
        PRIMARY KEY (agent, path)
    ) STRICT;
    -- The commands agents run, and the end of their output once they ended.
    CREATE TABLE runs (
        id         TEXT PRIMARY KEY,
        agent      TEXT NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
        name       TEXT,
        line       TEXT NOT NULL,
        folder     TEXT NOT NULL,
        state      TEXT NOT NULL,
        exit_code  INTEGER,
        signal     INTEGER,
        started_at INTEGER NOT NULL,
        ended_at   INTEGER,
        output     TEXT NOT NULL
    ) STRICT;
    -- The process group of each run going on, as for the agents' processes: a daemon started
    -- after a crash kills its own leftovers and never a process that took the pid since.
    CREATE TABLE run_processes (
        pid     INTEGER PRIMARY KEY,
        started INTEGER NOT NULL,
        run     TEXT NOT NULL
    ) STRICT;
",
    "
    -- The calls the gate held (`docs/design.md` §7), numbered for the user across every agent.
    -- The same call key of the same agent is the same request. `held_by` is 'user' or the id of
    -- the agent that holds it.
    CREATE TABLE approval_requests (
        number     INTEGER PRIMARY KEY AUTOINCREMENT,
        agent      TEXT NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
        call_key   TEXT,
        tool       TEXT NOT NULL,
        arguments  TEXT NOT NULL,
        shown      TEXT NOT NULL,
        reasons    TEXT NOT NULL,
        sensitive  INTEGER NOT NULL,
        identity   TEXT NOT NULL,
        held_by    TEXT NOT NULL,
        forwards   TEXT NOT NULL,
        state      TEXT NOT NULL,
        decision   TEXT,
        created_at INTEGER NOT NULL,
        decided_at INTEGER,
        UNIQUE (agent, call_key)
    ) STRICT;
    CREATE INDEX approval_requests_held ON approval_requests (held_by, state);
    -- The calls the user allowed for a root's tree: an exact identity each.
    CREATE TABLE approval_grants (
        tree       TEXT NOT NULL,
        identity   TEXT NOT NULL,
        request    INTEGER NOT NULL REFERENCES approval_requests (number) ON DELETE CASCADE,
        created_at INTEGER NOT NULL,
        PRIMARY KEY (tree, identity)
    ) STRICT;
    -- What Flottille hands to an agent, stored before it is sent, once for its id. The gate
    -- stores the answers of the requests here; sending them comes with the tree.
    CREATE TABLE deliveries (
        id         TEXT PRIMARY KEY,
        agent      TEXT NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
        kind       TEXT NOT NULL,
        body       TEXT NOT NULL,
        state      TEXT NOT NULL,
        created_at INTEGER NOT NULL
    ) STRICT;
",
    "
    -- The tree (`docs/design.md` §5): an agent's id is its lineage, kept across replacements;
    -- its parent is another agent's id, `NULL` for a root. `epoch` grows with each replacement.
    ALTER TABLE agents ADD COLUMN parent TEXT REFERENCES agents (id);
    ALTER TABLE agents ADD COLUMN depth INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE agents ADD COLUMN epoch INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE agents ADD COLUMN title TEXT;
    ALTER TABLE agents ADD COLUMN brief TEXT;
    ALTER TABLE agents ADD COLUMN reported_at INTEGER;
    ALTER TABLE agents ADD COLUMN quiet_turns INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE agents ADD COLUMN waiting_on_parent INTEGER NOT NULL DEFAULT 0;
    CREATE INDEX agents_by_parent ON agents (parent);
    -- Deliveries are sent now: how soon, and when. `sending` while the prompt holding one plays,
    -- given back to `queued` when that turn fails.
    ALTER TABLE deliveries ADD COLUMN urgency TEXT NOT NULL DEFAULT 'between-turns';
    ALTER TABLE deliveries ADD COLUMN sent_at INTEGER;
    CREATE INDEX deliveries_by_agent ON deliveries (agent, state);
    -- Each replacement of an agent, for the replacement limit: those Flottille caused by
    -- restarting do not count.
    CREATE TABLE agent_replacements (
        agent   TEXT NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
        at      INTEGER NOT NULL,
        epoch   INTEGER NOT NULL,
        reason  TEXT NOT NULL,
        counted INTEGER NOT NULL
    ) STRICT;
",
    "
    -- A root's question waits for the user as a child's waits for its parent.
    ALTER TABLE agents RENAME COLUMN waiting_on_parent TO waiting_on_answer;
",
    "
    -- When a workspace's last preparation ended, kept with it rather than looked for among the
    -- events: a store that has them takes it from its newest `workspace.ready` event.
    ALTER TABLE workspaces ADD COLUMN ready_at INTEGER;
    UPDATE workspaces SET ready_at = (
        SELECT max(e.at) FROM events e WHERE e.name = 'workspace.ready'
        AND json_extract(e.data, '$.project') = workspaces.project
        AND json_extract(e.data, '$.workspace') = workspaces.name
        AND e.at >= workspaces.created_at
    );
",
    "
    -- What each provider's agents offered when their sessions opened (`docs/agents.md`,
    -- \"Providers\"): its models, each with its efforts once one was seen chosen, as JSON, and the
    -- version of the provider they were read from: another version starts over.
    CREATE TABLE provider_models (
        provider   TEXT PRIMARY KEY,
        version    TEXT,
        models     TEXT NOT NULL,
        updated_at INTEGER NOT NULL
    ) STRICT;
",
    "
    -- The ticket a workspace is linked to (`docs/tickets.md`): `#123` or a Jira key, or none.
    ALTER TABLE workspaces ADD COLUMN ticket TEXT;
",
    "
    -- The runs of procedures (`docs/procedures.md`): the grants each gives, frozen when the user
    -- confirmed them, held for the run's root agent until the run ends.
    CREATE TABLE procedure_runs (
        agent      TEXT PRIMARY KEY REFERENCES agents (id) ON DELETE CASCADE,
        project    TEXT NOT NULL,
        workspace  TEXT NOT NULL,
        procedure  TEXT NOT NULL,
        grants     TEXT NOT NULL,
        children   INTEGER NOT NULL,
        started_at INTEGER NOT NULL,
        ended_at   INTEGER
    ) STRICT;
",
    "
    -- The agents' tools as their providers name them: `commands_output` and `commands_stop` became
    -- `commands_status` and `agent_read` became `agent_status` (`docs/tools.md`), so an agent made
    -- before keeps what it could do. Each name takes the place of the first one it replaces, once,
    -- the others' order untouched.
    UPDATE agents SET tools = (
        SELECT json_group_array(name) FROM (
            SELECT name FROM (
                SELECT CASE value
                    WHEN 'mcp__flottille__commands_output' THEN 'mcp__flottille__commands_status'
                    WHEN 'mcp__flottille__commands_stop' THEN 'mcp__flottille__commands_status'
                    WHEN 'mcp__flottille__agent_read' THEN 'mcp__flottille__agent_status'
                    WHEN 'flottille_commands_output' THEN 'flottille_commands_status'
                    WHEN 'flottille_commands_stop' THEN 'flottille_commands_status'
                    WHEN 'flottille_agent_read' THEN 'flottille_agent_status'
                    ELSE value
                END AS name, min(key) AS first
                FROM json_each(agents.tools) GROUP BY name
            ) ORDER BY first
        )
    );
",
    "
    -- What a request is (`call`, `catalogue` or `action`), which says what the user may answer,
    -- and what it changes in words (`docs/tools.md`, \"The project\").
    ALTER TABLE approval_requests ADD COLUMN kind TEXT NOT NULL DEFAULT 'call';
    ALTER TABLE approval_requests ADD COLUMN says TEXT NOT NULL DEFAULT '[]';
",
    "
    -- Whether an agent's agent can resume or load its session, as it said when it last opened
    -- one: an ended root whose agent can is resumed on its own session (`agent.resume`).
    ALTER TABLE agents ADD COLUMN reopens INTEGER NOT NULL DEFAULT 0;
",
    "
    -- Whether an agent's agent takes images in a prompt, as it said when it last started (`NULL`
    -- before it did), and the images a delivery carries: the names of files in the agent's folder
    -- of the data folder.
    ALTER TABLE agents ADD COLUMN images INTEGER;
    ALTER TABLE deliveries ADD COLUMN images TEXT NOT NULL DEFAULT '[]';
",
    "
    -- Ask and Auto (`docs/design.md` §7): the gate classes each call instead of handing out
    -- rights. Each session has its mode, Auto unless the user chose Ask; a request keeps the
    -- judge's verdict; its `sensitive` is now whether its class is base. The grants go: those the
    -- user gave a tree, and those of procedure runs, which now run as plain instructions.
    ALTER TABLE agents ADD COLUMN gate_mode TEXT NOT NULL DEFAULT 'auto';
    ALTER TABLE approval_requests ADD COLUMN judgement TEXT;
    DROP TABLE approval_grants;
    ALTER TABLE procedure_runs DROP COLUMN grants;
    ALTER TABLE procedure_runs DROP COLUMN children;
",
    "
    -- A command that ended stays in the session's list until the user dismisses it.
    ALTER TABLE runs ADD COLUMN dismissed INTEGER NOT NULL DEFAULT 0;
    -- The lists of a session's commands, and the runs that go with their agents, look runs up by
    -- agent.
    CREATE INDEX runs_by_agent ON runs (agent);
",
    "
    -- Skills and Commands (`docs/skills.md`, `docs/commands.md`): references are skills and
    -- procedures are commands, sent to an agent as a message, so the runs of procedures go. An
    -- agent made before keeps its tools under their new names, in their order; a request keeps
    -- its tool's new name; a workspace's step names the task it runs. A procedure the user had
    -- not answered is refused, and its agent told what to do instead, as a refusal is.
    DROP TABLE procedure_runs;
    INSERT OR IGNORE INTO deliveries (id, agent, kind, body, urgency, state, created_at)
        SELECT 'approval-' || number, agent, 'approval',
            'Request #' || number || ' (procedure) was refused: procedures were replaced by '
                || 'commands; start it again with /'
                || coalesce(json_extract(arguments, '$.name'), '<name>') || '.',
            'urgent', 'queued', CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
        FROM approval_requests WHERE tool = 'procedure' AND state = 'pending';
    UPDATE approval_requests
        SET state = 'denied', decision = 'deny',
            decided_at = CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
        WHERE tool = 'procedure' AND state = 'pending';
    UPDATE agents SET tools = (
        SELECT json_group_array(name) FROM (
            SELECT CASE value
                WHEN 'mcp__flottille__reference_read' THEN 'mcp__flottille__skill_read'
                WHEN 'mcp__flottille__procedure' THEN 'mcp__flottille__commands'
                WHEN 'flottille_reference_read' THEN 'flottille_skill_read'
                WHEN 'flottille_procedure' THEN 'flottille_commands'
                ELSE value
            END AS name FROM json_each(agents.tools) ORDER BY key
        )
    );
    UPDATE approval_requests SET tool = CASE tool
        WHEN 'reference_read' THEN 'skill_read'
        WHEN 'procedure' THEN 'commands'
        ELSE tool
    END;
    UPDATE workspace_steps
        SET action = json_set(json_remove(action, '$.command'), '$.task',
            json_extract(action, '$.command'))
        WHERE json_type(action, '$.command') IS NOT NULL;
",
    "
    -- Tasks (`docs/tools.md`, \"Tasks\"): the user runs a task from the window too, a run of no
    -- agent's, kept with its project. The command tools take the names of what they run: an agent
    -- made before keeps its tools under their new names, in their order, and a request its tool's.
    CREATE TABLE runs_by_project (
        id         TEXT PRIMARY KEY,
        agent      TEXT REFERENCES agents (id) ON DELETE CASCADE,
        name       TEXT,
        line       TEXT NOT NULL,
        folder     TEXT NOT NULL,
        state      TEXT NOT NULL,
        exit_code  INTEGER,
        signal     INTEGER,
        started_at INTEGER NOT NULL,
        ended_at   INTEGER,
        output     TEXT NOT NULL,
        dismissed  INTEGER NOT NULL DEFAULT 0,
        project    TEXT NOT NULL DEFAULT ''
    ) STRICT;
    INSERT INTO runs_by_project (id, agent, name, line, folder, state, exit_code, signal,
            started_at, ended_at, output, dismissed, project)
        SELECT r.id, r.agent, r.name, r.line, r.folder, r.state, r.exit_code, r.signal,
            r.started_at, r.ended_at, r.output, r.dismissed, coalesce(a.project, '')
        FROM runs r LEFT JOIN agents a ON a.id = r.agent;
    DROP TABLE runs;
    ALTER TABLE runs_by_project RENAME TO runs;
    CREATE INDEX runs_by_agent ON runs (agent);
    CREATE INDEX runs_of_users ON runs (project) WHERE agent IS NULL;
    UPDATE agents SET tools = (
        SELECT json_group_array(name) FROM (
            SELECT CASE value
                WHEN 'mcp__flottille__commands_list' THEN 'mcp__flottille__tasks'
                WHEN 'mcp__flottille__commands_run' THEN 'mcp__flottille__run'
                WHEN 'mcp__flottille__commands_status' THEN 'mcp__flottille__run_status'
                WHEN 'flottille_commands_list' THEN 'flottille_tasks'
                WHEN 'flottille_commands_run' THEN 'flottille_run'
                WHEN 'flottille_commands_status' THEN 'flottille_run_status'
                ELSE value
            END AS name FROM json_each(agents.tools) ORDER BY key
        )
    );
    UPDATE approval_requests
        SET arguments = json_set(json_remove(arguments, '$.command'), '$.task',
            json_extract(arguments, '$.command'))
        WHERE tool = 'commands_run' AND json_type(arguments, '$.command') IS NOT NULL;
    UPDATE approval_requests SET tool = CASE tool
        WHEN 'commands_run' THEN 'run'
        WHEN 'commands_status' THEN 'run_status'
        ELSE tool
    END;
",
    "
    -- Complete families (#141): `forge_read` and `forge_write` became `forge`, `ticket_read`
    -- `tickets`, and `workspace_remove` the `remove` of `workspace`, which every agent now has. An
    -- agent made before keeps its tools under the new names, in their order, each once, and a
    -- request its tool's with the action it meant.
    UPDATE agents SET tools = (
        SELECT json_group_array(name) FROM (
            SELECT CASE value
                WHEN 'mcp__flottille__forge_read' THEN 'mcp__flottille__forge'
                WHEN 'mcp__flottille__forge_write' THEN 'mcp__flottille__forge'
                WHEN 'mcp__flottille__ticket_read' THEN 'mcp__flottille__tickets'
                WHEN 'mcp__flottille__workspace_remove' THEN 'mcp__flottille__workspace'
                WHEN 'flottille_forge_read' THEN 'flottille_forge'
                WHEN 'flottille_forge_write' THEN 'flottille_forge'
                WHEN 'flottille_ticket_read' THEN 'flottille_tickets'
                WHEN 'flottille_workspace_remove' THEN 'flottille_workspace'
                ELSE value
            END AS name FROM json_each(agents.tools) AS tool
            WHERE NOT (
                tool.value IN ('mcp__flottille__forge_write', 'flottille_forge_write')
                AND EXISTS (SELECT 1 FROM json_each(agents.tools) AS other WHERE other.value
                    IN ('mcp__flottille__forge_read', 'flottille_forge_read'))
            ) AND NOT (
                tool.value IN ('mcp__flottille__workspace_remove', 'flottille_workspace_remove')
                AND EXISTS (SELECT 1 FROM json_each(agents.tools) AS other WHERE other.value
                    IN ('mcp__flottille__workspace', 'flottille_workspace'))
            )
            ORDER BY tool.key
        )
    );
    UPDATE approval_requests SET arguments = json_set(arguments, '$.action', CASE
            json_extract(arguments, '$.action')
            WHEN 'pr-create' THEN 'create'
            WHEN 'pr-merge-squash' THEN 'merge'
            WHEN 'pipeline-rerun' THEN 'checks'
            ELSE json_extract(arguments, '$.action')
        END)
        WHERE tool = 'forge_write';
    UPDATE approval_requests SET arguments = json_set(arguments, '$.action', 'show')
        WHERE tool IN ('forge_read', 'ticket_read');
    UPDATE approval_requests
        SET arguments = json_set(json_remove(arguments, '$.workspace'), '$.action', 'remove')
        WHERE tool = 'workspace_remove' AND json_type(arguments, '$.workspace') IS NULL;
    UPDATE approval_requests
        SET arguments = json_set(json_remove(arguments, '$.workspace'), '$.action', 'remove',
            '$.name', json_extract(arguments, '$.workspace'))
        WHERE tool = 'workspace_remove' AND json_type(arguments, '$.workspace') IS NOT NULL;
    UPDATE approval_requests SET tool = CASE tool
        WHEN 'forge_read' THEN 'forge'
        WHEN 'forge_write' THEN 'forge'
        WHEN 'ticket_read' THEN 'tickets'
        WHEN 'workspace_remove' THEN 'workspace'
        ELSE tool
    END;
",
    "
    -- A root an agent started for the user keeps who started it: such a root cannot start one in
    -- turn without the user (`docs/tools.md`, \"The tree's tools\").
    ALTER TABLE agents ADD COLUMN launched_by TEXT REFERENCES agents (id);
",
    "
    -- When each step of a workspace's preparation last started and ended: the window says how
    -- long a step took, or how long it has been running.
    ALTER TABLE workspace_steps ADD COLUMN started_at INTEGER;
    ALTER TABLE workspace_steps ADD COLUMN ended_at INTEGER;
",
    "
    -- What a step wrote while it ran, masked and bounded as a run's output is: the window opens it
    -- from the project page, in the run drawer.
    ALTER TABLE workspace_steps ADD COLUMN output TEXT;
",
    "
    -- An agent waiting, once, for the next event of what a source watches (`docs/design.md`
    -- §5.6): kept across restarts, with what its source last saw, so that nothing that happened
    -- meanwhile is lost.
    CREATE TABLE subscriptions (
        id         TEXT PRIMARY KEY,
        agent      TEXT NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
        source     TEXT NOT NULL,
        target     TEXT NOT NULL,
        events     TEXT NOT NULL,
        from_whom  TEXT NOT NULL,
        seen       TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        expires_at INTEGER NOT NULL
    ) STRICT;
",
    "
    -- Every pull request and ticket of a project, as its forge or tracker last said it
    -- (`docs/items.md`), the checks of the pull requests, and where each source's reads stand.
    CREATE TABLE items (
        project    TEXT NOT NULL,
        kind       TEXT NOT NULL,
        source     TEXT NOT NULL,
        repository TEXT NOT NULL,
        key        TEXT NOT NULL,
        title      TEXT NOT NULL,
        state      TEXT NOT NULL,
        author     TEXT,
        assignees  TEXT NOT NULL,
        labels     TEXT NOT NULL,
        url        TEXT,
        updated_at TEXT,
        seen_at    INTEGER NOT NULL,
        mine       INTEGER NOT NULL,
        details    TEXT NOT NULL,
        PRIMARY KEY (project, kind, repository, key)
    ) STRICT;
    CREATE TABLE item_checks (
        project    TEXT NOT NULL,
        kind       TEXT NOT NULL,
        repository TEXT NOT NULL,
        key        TEXT NOT NULL,
        position   INTEGER NOT NULL,
        name       TEXT NOT NULL,
        state      TEXT NOT NULL,
        started_at TEXT,
        ended_at   TEXT,
        link       TEXT,
        PRIMARY KEY (project, kind, repository, key, position),
        FOREIGN KEY (project, kind, repository, key)
            REFERENCES items (project, kind, repository, key) ON DELETE CASCADE
    ) STRICT;
    CREATE TABLE fetch_cursors (
        project  TEXT NOT NULL,
        kind     TEXT NOT NULL,
        target   TEXT NOT NULL,
        source   TEXT NOT NULL,
        cursor   INTEGER,
        round_at INTEGER,
        failures INTEGER NOT NULL,
        problem  TEXT,
        first_at INTEGER,
        note     TEXT,
        PRIMARY KEY (project, kind, target)
    ) STRICT;
",
    "
    -- A model and an effort changed on a running session, set before its next prompt
    -- (`docs/agents.md`, \"The options\").
    ALTER TABLE agents ADD COLUMN pending_model TEXT;
    ALTER TABLE agents ADD COLUMN pending_effort TEXT;
",
    "
    -- The whole text of a tool result that was cut at the source (`docs/tools.md`, \"Long
    -- results\"), masked, for the agent whose session it is and nobody else: it goes with the
    -- session, as its runs do.
    CREATE TABLE tool_results (
        id         TEXT PRIMARY KEY,
        agent      TEXT NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
        tool       TEXT NOT NULL,
        text       TEXT NOT NULL,
        created_at INTEGER NOT NULL
    ) STRICT;
    CREATE INDEX tool_results_agent ON tool_results (agent);
",
    "
    -- Where the agent worked when it asked (`main`, or `workspace <name>`): a request is done
    -- only there (`docs/workspaces.md`, \"Its own workspace\"). NULL before it was kept.
    ALTER TABLE approval_requests ADD COLUMN place TEXT;
",
    "
    -- What Flottille posted for an agent on a forge or a tracker (`docs/forges.md`, \"What Flottille
    -- posts is recorded\"): the text carries no mark, so the daemon knows its posts by this.
    CREATE TABLE posts (
        id          INTEGER PRIMARY KEY,
        project     TEXT NOT NULL,
        source      TEXT NOT NULL,
        repository  TEXT NOT NULL,
        key         TEXT NOT NULL,
        kind        TEXT NOT NULL,
        comment_id  TEXT,
        fingerprint TEXT,
        posted_at   INTEGER NOT NULL
    ) STRICT;
    CREATE INDEX posts_item ON posts (project, source, repository, key);
",
    "
    -- The last exchange of a session (`docs/protocol.md`, `agent.tree`): the later of the last
    -- message the user gave the agent and the end of its last turn, which the sidebar sorts by.
    -- NULL before either.
    ALTER TABLE agents ADD COLUMN exchanged_at INTEGER;
    UPDATE agents SET exchanged_at = (
        SELECT max(at) FROM (
            SELECT max(sent_at) AS at FROM deliveries
                WHERE agent = agents.id AND kind = 'user' AND sent_at IS NOT NULL
            UNION ALL
            SELECT max(e.at) FROM agent_events a JOIN events e ON e.seq = a.seq
                WHERE a.agent = agents.id AND e.name = 'agent.turn_ended'
        )
    );
",
    "
    -- A task's run says when it is ready and where it answers (`docs/projects.md`, \"Tasks\"):
    -- `ready` is NULL for a task that does not say, else starting, ready or missed.
    ALTER TABLE runs ADD COLUMN ready TEXT;
    ALTER TABLE runs ADD COLUMN url TEXT;
    ALTER TABLE runs ADD COLUMN url_error TEXT;
",
    "
    -- What each turn used, one row per model the turn used (`docs/reference/acp.md`, \"What a
    -- turn used\"). History, not state: no reference to the agents or the workspaces, so that
    -- their removal neither takes the rows nor is held by them; the ids and names are those of
    -- the time. A count the agent did not give is NULL, never 0. Times in milliseconds.
    CREATE TABLE turn_usage (
        id           INTEGER PRIMARY KEY,
        agent        TEXT NOT NULL,
        title        TEXT,
        session      TEXT NOT NULL,
        project      TEXT NOT NULL,
        workspace    TEXT,
        provider     TEXT NOT NULL,
        role         TEXT NOT NULL,
        model        TEXT,
        started_at   INTEGER NOT NULL,
        ended_at     INTEGER NOT NULL,
        stop_reason  TEXT NOT NULL,
        input        INTEGER,
        output       INTEGER,
        cache_read   INTEGER,
        cache_write  INTEGER,
        reasoning    INTEGER,
        cost_usd     REAL
    ) STRICT;
    -- The statistics read a span of time, of everything or of one project, by day, model or
    -- kind of agent.
    CREATE INDEX turn_usage_by_time ON turn_usage (started_at);
    CREATE INDEX turn_usage_by_project ON turn_usage (project, started_at);
",
    "
    -- The tool list an agent's last process was given (`docs/tools.md`, \"When the tools
    -- change\"): its digest and the actions of each tool, as JSON; NULL before its first process.
    ALTER TABLE agents ADD COLUMN tools_given TEXT;
",
    "
    -- A message that used commands keeps what the user typed beside what the agent received
    -- (`body`), with the names of the commands, in the order of their first use: nothing reads
    -- them back from the text. A message stored before held one command, named by its header:
    -- it is read once here, and its typed text, never kept, stays NULL.
    ALTER TABLE deliveries ADD COLUMN typed TEXT;
    ALTER TABLE deliveries ADD COLUMN commands TEXT NOT NULL DEFAULT '[]';
    UPDATE deliveries
        SET commands = json_array(substr(body, 13, instr(body, ' of project ') - 13))
        WHERE kind IN ('user', 'parent')
            AND body GLOB 'The command * of project *: *'
            AND instr(body, ' of project ') > 13
            AND instr(substr(body, 13, instr(body, ' of project ') - 13), ' ') = 0;
",
    "
    -- The agent that wrote each service of `services.toml` the daemon last wrote for an agent
    -- (`docs/projects.md`, \"Services\"): its lines are judged again when another agent's call
    -- starts it. No row: the user's own (written by hand, from the command line or the window).
    -- Not in the file, which the user edits.
    CREATE TABLE service_writers (
        name   TEXT PRIMARY KEY,
        agent  TEXT NOT NULL
    ) STRICT;
",
    "
    -- The user's leave for a session to write to another it did not start, one way, given once
    -- for the pair (`docs/tools.md`, \"Sessions that talk to each other\"): it goes with the end
    -- of either session.
    CREATE TABLE session_leaves (
        writer     TEXT NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
        session    TEXT NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
        request    INTEGER NOT NULL,
        granted_at INTEGER NOT NULL,
        PRIMARY KEY (writer, session)
    ) STRICT;
    CREATE INDEX session_leaves_session ON session_leaves (session);
",
    "
    -- The vault's entries (`docs/vault.md`): their metadata only. Their secrets are the
    -- keyring's, never the store's.
    CREATE TABLE vault_entries (
        name        TEXT PRIMARY KEY,
        description TEXT,
        url         TEXT,
        username    TEXT,
        project     TEXT,
        updated_at  INTEGER NOT NULL
    ) STRICT;
",
    "
    -- A session an agent started is led by it or handed to the user (`docs/tools.md`, \"Sessions
    -- an agent starts\"): one its starter still followed is led, the others handed. A following,
    -- which woke its starter at each turn end, now wakes a pilot when its session asks, reports
    -- or ends.
    ALTER TABLE agents ADD COLUMN led INTEGER NOT NULL DEFAULT 0;
    UPDATE agents SET led = 1 WHERE launched_by IS NOT NULL AND EXISTS (
        SELECT 1 FROM subscriptions WHERE source = 'session' AND agent = agents.launched_by
            AND json_extract(target, '$.session') = agents.id);
    UPDATE subscriptions SET events = '[\"asked\",\"reported\",\"ended\"]'
        WHERE source = 'session';
",
    "
    -- The agent a session was detached from by the user (`docs/tools.md`, \"Detach\"): kept for
    -- the record once `launched_by` is cleared, and to keep its first message the agent's.
    ALTER TABLE agents ADD COLUMN detached_from TEXT REFERENCES agents (id);
",
    "
    -- The state of the workspace a removal's request names, read when it was asked: the answer to
    -- discard removes only that (`docs/workspaces.md`, \"Removal\"). NULL for any other request.
    ALTER TABLE approval_requests ADD COLUMN digest TEXT;
",
    "
    -- Who named a session (`docs/tools.md`, \"The session's title\"): its agent, with
    -- `session_title`, or the user, whose title always wins; `NULL` while it keeps the first line
    -- of its first message.
    ALTER TABLE agents ADD COLUMN titled_by TEXT;
",
    "
    -- The workspace a task the user ran from a session's rail runs in: that session lists it.
    ALTER TABLE runs ADD COLUMN workspace TEXT;
",
    "
    -- A session whose provider said its quota is spent waits for its reset (`docs/design.md`
    -- §5.9): the instant, NULL when it could not be read, and the user's choice for this session
    -- alone, NULL to follow the app's setting. Kept across restarts; gone with the session.
    CREATE TABLE quota_waits (
        agent      TEXT PRIMARY KEY REFERENCES agents (id) ON DELETE CASCADE,
        reset_at   INTEGER,
        resume     INTEGER,
        limited_at INTEGER NOT NULL
    ) STRICT;
",
    "
    -- The one use a vault entry is bound to (`docs/vault.md`, \"Bound to one use\"): only that
    -- feature reads it. The entries that exist are bound to none, and work as they did.
    ALTER TABLE vault_entries ADD COLUMN bound TEXT;
",
];

/// How many of each extension's own migrations were applied (`docs/extensions.md`, "Data"): its
/// tables, `ext_<name>_…`, are brought up to date as the store's are, and kept when it is turned
/// off. Made at every open after the core's migrations, never as one of them, so that its place
/// never depends on how many the core has.
const EXTENSION_MIGRATIONS: &str = "
    CREATE TABLE IF NOT EXISTS extension_migrations (
        name    TEXT PRIMARY KEY,
        applied INTEGER NOT NULL
    ) STRICT;
";

const REQUEST_COLUMNS: &str = "number, agent, call_key, tool, arguments, shown, reasons, sensitive, \
    identity, held_by, forwards, state, decision, created_at, decided_at, kind, says, judgement, place, digest";

const RUN_COLUMNS: &str = "id, agent, name, line, folder, state, exit_code, signal, started_at, \
    ended_at, output, dismissed, project, ready, url, url_error, workspace";

const AGENT_COLUMNS: &str = "id, provider, role, project, workspace, folder, state, state_reason, \
    created_at, updated_at, active_at, ended_at, chosen_model, chosen_effort, chosen_mode, \
    taken_model, taken_effort, taken_mode, native_id, agent_version, instructions, tools, parent, \
    depth, epoch, title, brief, reported_at, quiet_turns, waiting_on_answer, reopens, images, gate_mode, \
    launched_by, pending_model, pending_effort, exchanged_at, led, detached_from, titled_by";

const DELIVERY_COLUMNS: &str =
    "id, agent, kind, body, urgency, state, created_at, sent_at, images, typed, commands";

/// What changes on an agent; what is left `None` stays as it is.
#[derive(Clone, Debug, Default)]
pub struct AgentChange {
    pub state: Option<AgentState>,
    /// Set with the state: `None` there forgets the reason.
    pub state_reason: Option<String>,
    pub native_id: Option<String>,
    pub agent_version: Option<String>,
    pub reopens: Option<bool>,
    pub images: Option<bool>,
    /// Only the user changes it (`agent.mode`).
    pub gate_mode: Option<GateMode>,
    pub taken: Option<Choices>,
    /// The model and the effort a later process opens with (the mode is left as it is).
    pub chosen: Option<Choices>,
    /// The model and the effort to set before the next prompt; `Some` of none forgets them.
    pub pending: Option<Choices>,
    /// `Some(None)` forgets it.
    pub reported_at: Option<Option<u64>>,
    pub quiet_turns: Option<u32>,
    pub waiting_on_answer: Option<bool>,
    pub led: Option<bool>,
    /// The user detached the session from the agent that started it: `launched_by` is cleared,
    /// `detached_from` keeps that agent (`docs/tools.md`, "Detach").
    pub detached_from: Option<String>,
}

/// A session waiting for its quota's reset (`docs/design.md` §5.9).
#[derive(Clone, Debug, PartialEq)]
pub struct QuotaWait {
    pub agent: String,
    /// When the quota resets; `None` when the provider's words could not be read.
    pub reset_at: Option<u64>,
    /// The user's choice for this session; `None` follows the app's setting.
    pub resume: Option<bool>,
    /// When the provider said it.
    pub limited_at: u64,
}

impl QuotaWait {
    fn of_row(row: &rusqlite::Row) -> rusqlite::Result<QuotaWait> {
        Ok(QuotaWait {
            agent: row.get(0)?,
            reset_at: row.get(1)?,
            resume: row.get(2)?,
            limited_at: row.get(3)?,
        })
    }

    pub fn to_json(&self) -> Value {
        json!({"agent": self.agent, "limited": true, "reset_at": self.reset_at,
            "resume": self.resume, "limited_at": self.limited_at})
    }
}

/// How many cut tool results of one agent keep their whole text (`docs/tools.md`, "Long
/// results"): a result is at most 40,000 characters, so an agent holds at most a few MiB.
pub const KEPT_RESULTS_PER_AGENT: usize = 20;

/// What a cut result's id leads to.
#[derive(Debug, PartialEq, Eq)]
pub enum Kept {
    Text(String),
    /// Kept once, dropped since for the latest ones.
    Dropped,
}

pub struct Store {
    connection: Connection,
    /// The prefix of the extension whose space the connection is confined to, if one is.
    confined: Confined,
}

type Confined = Arc<Mutex<Option<String>>>;

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        Store::prepare(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Store> {
        Store::prepare(Connection::open_in_memory()?)
    }

    fn prepare(mut connection: Connection) -> Result<Store> {
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "foreign_keys", true)?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        migrate(&mut connection)?;
        // A `Transaction` lends only `&Connection`, and an authorizer is set with `&mut`: it is
        // set once, and lets everything through outside an extension's space.
        let confined = Confined::default();
        let held = Arc::clone(&confined);
        connection.authorizer(Some(move |context: AuthContext<'_>| {
            match held
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_deref()
            {
                None => Authorization::Allow,
                Some(prefix) => within(prefix, &context),
            }
        }))?;
        Ok(Store {
            connection,
            confined,
        })
    }

    pub fn schema_version(&self) -> Result<usize> {
        Ok(self
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))?)
    }

    pub fn append_event(&self, name: &str, data: Value) -> Result<Event> {
        append_event(&self.connection, name, data)
    }

    /// Writes a new workspace with its steps, all pending, and its event, in one transaction.
    /// The id is the store's. Refused when the project has a workspace of that name.
    pub fn create_workspace(
        &mut self,
        workspace: &Workspace,
        event: (&str, Value),
    ) -> Result<(i64, Event)> {
        let transaction = self.connection.transaction()?;
        let inserted = transaction.execute(
            "INSERT INTO workspaces (project, name, branch, folder, project_folder, created_at, \
             variables, repositories, ticket) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                workspace.project,
                workspace.name,
                workspace.branch,
                path_text(&workspace.folder)?,
                path_text(&workspace.project_folder)?,
                workspace.created_at,
                serde_json::to_string(&workspace.variables)?,
                serde_json::to_string(&workspace.repositories)?,
                workspace.ticket,
            ],
        );
        match inserted {
            Err(rusqlite::Error::SqliteFailure(error, _))
                if error.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                return Err(Error::Refused(format!(
                    "project {} already has a workspace named {}",
                    workspace.project, workspace.name
                )));
            }
            inserted => inserted?,
        };
        let id = transaction.last_insert_rowid();
        for step in &workspace.steps {
            transaction.execute(
                "INSERT INTO workspace_steps (workspace, position, repository, action, state) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    id,
                    step.position,
                    step.repository,
                    serde_json::to_string(&step.action)?,
                    StepState::Pending.as_str(),
                ],
            )?;
        }
        let event = append_event(&transaction, event.0, event.1)?;
        transaction.commit()?;
        Ok((id, event))
    }

    /// The project's workspace of that name.
    pub fn workspace(&self, project: &str, name: &str) -> Result<Option<Workspace>> {
        let id: Option<i64> = self
            .connection
            .query_row(
                "SELECT id FROM workspaces WHERE project = ?1 AND name = ?2",
                params![project, name],
                |row| row.get(0),
            )
            .optional()?;
        id.map(|id| self.workspace_by_id(id)).transpose()
    }

    /// Every workspace, or the project's, by project then name.
    pub fn workspaces(&self, project: Option<&str>) -> Result<Vec<Workspace>> {
        let mut statement = self.connection.prepare_cached(
            "SELECT id FROM workspaces WHERE ?1 IS NULL OR project = ?1 ORDER BY project, name",
        )?;
        let ids = statement
            .query_map(params![project], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        ids.into_iter().map(|id| self.workspace_by_id(id)).collect()
    }

    pub fn workspace_by_id(&self, id: i64) -> Result<Workspace> {
        let (
            project,
            name,
            branch,
            folder,
            project_folder,
            created_at,
            variables,
            repositories,
            ticket,
        ) = self.connection.query_row(
            "SELECT project, name, branch, folder, project_folder, created_at, variables, \
             repositories, ticket FROM workspaces WHERE id = ?1",
            params![id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, u64>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                ))
            },
        )?;
        let mut statement = self.connection.prepare_cached(
            "SELECT position, repository, action, state, failed_doing, failed_output, \
             started_at, ended_at FROM workspace_steps WHERE workspace = ?1 ORDER BY position",
        )?;
        let rows = statement.query_map(params![id], |row| {
            Ok((
                row.get::<_, u32>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<u64>>(6)?,
                row.get::<_, Option<u64>>(7)?,
            ))
        })?;
        let mut steps = Vec::new();
        for row in rows {
            let (position, repository, action, state, doing, output, started_at, ended_at) = row?;
            steps.push(Step {
                position,
                repository,
                action: serde_json::from_str::<Action>(&action)?,
                state: StepState::parse(&state).ok_or_else(|| {
                    Error::Refused(format!(
                        "the store holds a step in an unknown state {state}"
                    ))
                })?,
                failure: doing.map(|doing| Failure {
                    doing,
                    output: output.unwrap_or_default(),
                }),
                started_at,
                ended_at,
            });
        }
        Ok(Workspace {
            id,
            project,
            name,
            branch,
            folder: folder.into(),
            project_folder: project_folder.into(),
            created_at,
            variables: serde_json::from_str(&variables)?,
            repositories: serde_json::from_str(&repositories)?,
            steps,
            ticket,
        })
    }

    /// Sets one step's state, and its failure (forgotten unless given), with its event, in one
    /// transaction.
    pub fn set_step(
        &mut self,
        workspace: i64,
        position: u32,
        state: StepState,
        failure: Option<&Failure>,
        event: (&str, Value),
    ) -> Result<Event> {
        let transaction = self.connection.transaction()?;
        let event = append_event(&transaction, event.0, event.1)?;
        // A step that starts forgets when it last ended; one that ends keeps when it started.
        transaction.execute(
            "UPDATE workspace_steps SET state = ?3, failed_doing = ?4, failed_output = ?5, \
             started_at = CASE WHEN ?3 = 'running' THEN ?6 ELSE started_at END, \
             ended_at = CASE WHEN ?3 = 'running' THEN NULL ELSE ?6 END \
             WHERE workspace = ?1 AND position = ?2",
            params![
                workspace,
                position,
                state.as_str(),
                failure.map(|f| &f.doing),
                failure.map(|f| &f.output),
                event.at
            ],
        )?;
        transaction.commit()?;
        Ok(event)
    }

    /// Keeps what a step wrote, masked, once it ended.
    pub fn set_step_output(&mut self, workspace: i64, position: u32, output: &str) -> Result<()> {
        self.connection.execute(
            "UPDATE workspace_steps SET output = ?3 WHERE workspace = ?1 AND position = ?2",
            params![workspace, position, output],
        )?;
        Ok(())
    }

    /// What a step wrote and kept; none for a step that never ran.
    pub fn step_output(&self, workspace: i64, position: u32) -> Result<Option<String>> {
        Ok(self
            .connection
            .query_row(
                "SELECT output FROM workspace_steps WHERE workspace = ?1 AND position = ?2",
                params![workspace, position],
                |row| row.get(0),
            )
            .optional()?
            .flatten())
    }

    /// Puts these steps back to pending, forgetting their failure, with one event, in one
    /// transaction.
    pub fn reset_steps(
        &mut self,
        workspace: i64,
        positions: &[u32],
        event: (&str, Value),
    ) -> Result<Event> {
        let transaction = self.connection.transaction()?;
        for position in positions {
            transaction.execute(
                "UPDATE workspace_steps SET state = 'pending', failed_doing = NULL, \
                 failed_output = NULL, output = NULL, started_at = NULL, ended_at = NULL \
                 WHERE workspace = ?1 AND position = ?2",
                params![workspace, position],
            )?;
        }
        let event = append_event(&transaction, event.0, event.1)?;
        transaction.commit()?;
        Ok(event)
    }

    /// The workspace's preparation ended: when, kept with it, and its `workspace.ready` event, in
    /// one transaction.
    pub fn workspace_ready(&mut self, workspace: i64, data: Value) -> Result<Event> {
        let transaction = self.connection.transaction()?;
        let event = append_event(&transaction, "workspace.ready", data)?;
        transaction.execute(
            "UPDATE workspaces SET ready_at = ?2 WHERE id = ?1",
            params![workspace, event.at],
        )?;
        transaction.commit()?;
        Ok(event)
    }

    /// When the workspace's last preparation ended, if one did.
    pub fn workspace_ready_at(&self, workspace: i64) -> Result<Option<u64>> {
        Ok(self.connection.query_row(
            "SELECT ready_at FROM workspaces WHERE id = ?1",
            params![workspace],
            |row| row.get(0),
        )?)
    }

    /// Links the workspace to a ticket, or to none, with its event, in one transaction.
    pub fn set_ticket(
        &mut self,
        workspace: i64,
        ticket: Option<&str>,
        event: (&str, Value),
    ) -> Result<Event> {
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "UPDATE workspaces SET ticket = ?2 WHERE id = ?1",
            params![workspace, ticket],
        )?;
        let event = append_event(&transaction, event.0, event.1)?;
        transaction.commit()?;
        Ok(event)
    }

    /// Records the branch the workspace's worktrees are on now, with its event, in one
    /// transaction.
    pub fn set_workspace_branch(
        &mut self,
        workspace: i64,
        branch: &str,
        event: (&str, Value),
    ) -> Result<Event> {
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "UPDATE workspaces SET branch = ?2 WHERE id = ?1",
            params![workspace, branch],
        )?;
        let event = append_event(&transaction, event.0, event.1)?;
        transaction.commit()?;
        Ok(event)
    }

    /// Forgets a workspace and its steps, with its event, in one transaction.
    pub fn remove_workspace(&mut self, workspace: i64, event: (&str, Value)) -> Result<Event> {
        let transaction = self.connection.transaction()?;
        transaction.execute("DELETE FROM workspaces WHERE id = ?1", params![workspace])?;
        let event = append_event(&transaction, event.0, event.1)?;
        transaction.commit()?;
        Ok(event)
    }

    /// Forgets a workspace and moves the agent that worked in it to the project's `folder`
    /// (`main`), its instructions now saying so, with their events, in one transaction: the agent
    /// is never left in a workspace that is gone. The pending requests the agent made are
    /// cancelled in it (`why`): they were asked in the workspace, and would otherwise be performed
    /// in `main`. The events, then the requests cancelled.
    pub fn remove_workspace_moving(
        &mut self,
        workspace: i64,
        agent: &str,
        folder: &Path,
        instructions: &str,
        events: [(&str, Value); 2],
        why: &str,
    ) -> Result<(Vec<Event>, Vec<Request>)> {
        let transaction = self.connection.transaction()?;
        transaction.execute("DELETE FROM workspaces WHERE id = ?1", params![workspace])?;
        transaction.execute(
            "UPDATE agents SET workspace = NULL, folder = ?2, instructions = ?3, updated_at = ?4 \
             WHERE id = ?1",
            params![agent, path_text(folder)?, instructions, now_ms()],
        )?;
        let mut written = Vec::new();
        for (name, data) in events {
            written.push(append_event(&transaction, name, data)?);
        }
        let pending: Vec<Request> = {
            let mut statement = transaction.prepare_cached(&format!(
                "SELECT {REQUEST_COLUMNS} FROM approval_requests \
                 WHERE agent = ?1 AND state = 'pending' ORDER BY number"
            ))?;
            let rows = statement.query_map(params![agent], request_row)?;
            rows.map(|row| row?).collect::<Result<_>>()?
        };
        for request in &pending {
            transaction.execute(
                "UPDATE approval_requests SET state = 'cancelled', decided_at = ?2 \
                 WHERE number = ?1",
                params![request.number, now_ms()],
            )?;
            written.push(append_agent_event(
                &transaction,
                agent,
                "approval.cancelled",
                json!({"number": request.number, "agent": agent, "held_by": request.held_by,
                    "why": why}),
            )?);
        }
        transaction.commit()?;
        Ok((written, pending))
    }

    /// Writes a new agent with its event, in one transaction.
    pub fn create_agent(&mut self, agent: &Agent, event: (&str, Value)) -> Result<Event> {
        let transaction = self.connection.transaction()?;
        transaction.execute(
            &format!(
                "INSERT INTO agents ({AGENT_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, \
                 ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, \
                 ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32, ?33, ?34, ?35, ?36, ?37, ?38, ?39, ?40)"
            ),
            params![
                agent.id,
                agent.provider,
                agent.role,
                agent.project,
                agent.workspace,
                path_text(&agent.folder)?,
                agent.state.as_str(),
                agent.state_reason,
                agent.created_at,
                agent.updated_at,
                agent.active_at,
                agent.ended_at,
                agent.chosen.model,
                agent.chosen.effort,
                agent.chosen.mode,
                agent.taken.model,
                agent.taken.effort,
                agent.taken.mode,
                agent.native_id,
                agent.agent_version,
                agent.instructions,
                serde_json::to_string(&agent.tools)?,
                agent.parent,
                agent.depth,
                agent.epoch,
                agent.title,
                agent.brief,
                agent.reported_at,
                agent.quiet_turns,
                agent.waiting_on_answer,
                agent.reopens,
                agent.images,
                agent.gate_mode.as_str(),
                agent.launched_by,
                agent.pending.model,
                agent.pending.effort,
                agent.exchanged_at,
                agent.led,
                agent.detached_from,
                agent.titled_by,
            ],
        )?;
        let event = append_agent_event(&transaction, &agent.id, event.0, event.1)?;
        transaction.commit()?;
        Ok(event)
    }

    pub fn agent(&self, id: &str) -> Result<Option<Agent>> {
        let mut statement = self
            .connection
            .prepare_cached(&format!("SELECT {AGENT_COLUMNS} FROM agents WHERE id = ?1"))?;
        let agent = statement
            .query_row(params![id], agent_row)
            .optional()?
            .transpose()?;
        Ok(agent)
    }

    /// Every agent, the oldest first.
    pub fn agents(&self) -> Result<Vec<Agent>> {
        let mut statement = self.connection.prepare_cached(&format!(
            "SELECT {AGENT_COLUMNS} FROM agents ORDER BY created_at, id"
        ))?;
        let rows = statement.query_map([], agent_row)?;
        rows.map(|row| row?).collect()
    }

    /// The agents not in a final state: what the daemon's sweep looks after, every few seconds,
    /// without reading the whole history (each agent holds its instructions).
    pub fn live_agents(&self) -> Result<Vec<Agent>> {
        let mut statement = self.connection.prepare_cached(&format!(
            "SELECT {AGENT_COLUMNS} FROM agents WHERE state NOT IN ('ended', 'failed') \
             ORDER BY created_at, id"
        ))?;
        let rows = statement.query_map([], agent_row)?;
        rows.map(|row| row?).collect()
    }

    /// The ended or failed roots whose session is kept to be resumed: those the sweep forgets in
    /// time.
    pub fn kept_sessions(&self) -> Result<Vec<Agent>> {
        let mut statement = self.connection.prepare_cached(&format!(
            "SELECT {AGENT_COLUMNS} FROM agents WHERE state IN ('ended', 'failed') \
             AND parent IS NULL AND reopens = 1 AND native_id IS NOT NULL ORDER BY created_at, id"
        ))?;
        let rows = statement.query_map([], agent_row)?;
        rows.map(|row| row?).collect()
    }

    /// Changes an agent, with its event when one is given, in one transaction; the agent as it
    /// is now. An agent in a final state keeps it.
    pub fn change_agent(
        &mut self,
        id: &str,
        change: AgentChange,
        event: Option<(&str, Value)>,
    ) -> Result<(Agent, Option<Event>)> {
        let transaction = self.connection.transaction()?;
        let now = now_ms();
        if let Some(state) = change.state {
            transaction.execute(
                "UPDATE agents SET state = ?2, state_reason = ?3, \
                 ended_at = CASE WHEN ?4 THEN ?5 ELSE ended_at END \
                 WHERE id = ?1 AND state NOT IN ('ended', 'failed')",
                params![
                    id,
                    state.as_str(),
                    change.state_reason,
                    state.is_final(),
                    now
                ],
            )?;
        }
        // A leave lasts while both sessions do: one that ends, even to be resumed, takes it.
        // So does a wait for the quota's reset: an ended session is never resumed by it.
        if change.state.is_some_and(AgentState::is_final) {
            transaction.execute(
                "DELETE FROM session_leaves WHERE writer = ?1 OR session = ?1",
                params![id],
            )?;
            transaction.execute("DELETE FROM quota_waits WHERE agent = ?1", params![id])?;
        }
        if let Some(native_id) = change.native_id {
            transaction.execute(
                "UPDATE agents SET native_id = ?2 WHERE id = ?1",
                params![id, native_id],
            )?;
        }
        if let Some(version) = change.agent_version {
            transaction.execute(
                "UPDATE agents SET agent_version = ?2 WHERE id = ?1",
                params![id, version],
            )?;
        }
        if let Some(reopens) = change.reopens {
            transaction.execute(
                "UPDATE agents SET reopens = ?2 WHERE id = ?1",
                params![id, reopens],
            )?;
        }
        if let Some(images) = change.images {
            transaction.execute(
                "UPDATE agents SET images = ?2 WHERE id = ?1",
                params![id, images],
            )?;
        }
        if let Some(mode) = change.gate_mode {
            transaction.execute(
                "UPDATE agents SET gate_mode = ?2 WHERE id = ?1",
                params![id, mode.as_str()],
            )?;
        }
        if let Some(reported_at) = change.reported_at {
            transaction.execute(
                "UPDATE agents SET reported_at = ?2 WHERE id = ?1",
                params![id, reported_at],
            )?;
        }
        if let Some(quiet_turns) = change.quiet_turns {
            transaction.execute(
                "UPDATE agents SET quiet_turns = ?2 WHERE id = ?1",
                params![id, quiet_turns],
            )?;
        }
        if let Some(from) = change.detached_from {
            transaction.execute(
                "UPDATE agents SET detached_from = ?2, launched_by = NULL, led = 0 WHERE id = ?1",
                params![id, from],
            )?;
        }
        if let Some(led) = change.led {
            transaction.execute("UPDATE agents SET led = ?2 WHERE id = ?1", params![id, led])?;
        }
        if let Some(waiting) = change.waiting_on_answer {
            transaction.execute(
                "UPDATE agents SET waiting_on_answer = ?2 WHERE id = ?1",
                params![id, waiting],
            )?;
        }
        if let Some(taken) = change.taken {
            transaction.execute(
                "UPDATE agents SET taken_model = ?2, taken_effort = ?3, taken_mode = ?4 \
                 WHERE id = ?1",
                params![id, taken.model, taken.effort, taken.mode],
            )?;
        }
        if let Some(chosen) = change.chosen {
            transaction.execute(
                "UPDATE agents SET chosen_model = ?2, chosen_effort = ?3 WHERE id = ?1",
                params![id, chosen.model, chosen.effort],
            )?;
        }
        if let Some(pending) = change.pending {
            transaction.execute(
                "UPDATE agents SET pending_model = ?2, pending_effort = ?3 WHERE id = ?1",
                params![id, pending.model, pending.effort],
            )?;
        }
        transaction.execute(
            "UPDATE agents SET updated_at = ?2 WHERE id = ?1",
            params![id, now],
        )?;
        let event = event
            .map(|(name, data)| append_agent_event(&transaction, id, name, data))
            .transpose()?;
        transaction.commit()?;
        let agent = self
            .agent(id)?
            .ok_or_else(|| Error::Refused(format!("there is no agent {id}")))?;
        Ok((agent, event))
    }

    /// Names a session, with its `agent.titled` event, in one transaction: by `"user"` always, by
    /// `"agent"` only while the user has not named it. `None` when the user's title held.
    pub fn retitle_agent(&mut self, id: &str, title: &str, by: &str) -> Result<Option<Event>> {
        let transaction = self.connection.transaction()?;
        let changed = transaction.execute(
            "UPDATE agents SET title = ?2, titled_by = ?3, updated_at = ?4 \
             WHERE id = ?1 AND (?3 = 'user' OR titled_by IS NOT 'user')",
            params![id, title, by, now_ms()],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        let event = append_agent_event(
            &transaction,
            id,
            "agent.titled",
            json!({"agent": id, "title": title, "by": by}),
        )?;
        transaction.commit()?;
        Ok(Some(event))
    }

    /// Gives the agent the tools its role has now, with the agent's event, in one transaction.
    pub fn set_agent_tools(&mut self, agent: &str, tools: &[String], data: Value) -> Result<Event> {
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "UPDATE agents SET tools = ?2 WHERE id = ?1",
            params![agent, serde_json::to_string(tools)?],
        )?;
        let event = append_agent_event(&transaction, agent, "agent.tools_changed", data)?;
        transaction.commit()?;
        Ok(event)
    }

    /// The tool list the agent's last process was given, as `set_agent_tools_given` kept it.
    pub fn agent_tools_given(&self, agent: &str) -> Result<Option<String>> {
        Ok(self
            .connection
            .query_row(
                "SELECT tools_given FROM agents WHERE id = ?1",
                params![agent],
                |row| row.get(0),
            )
            .optional()?
            .flatten())
    }

    /// Keeps the tool list the agent's process is given.
    pub fn set_agent_tools_given(&mut self, agent: &str, given: &str) -> Result<()> {
        self.connection.execute(
            "UPDATE agents SET tools_given = ?2 WHERE id = ?1",
            params![agent, given],
        )?;
        Ok(())
    }

    /// An event of the agent's conversation; `activity` says it is a sign of activity, which
    /// moves the agent's `active_at`.
    pub fn append_agent_event(
        &mut self,
        agent: &str,
        name: &str,
        data: Value,
        activity: bool,
    ) -> Result<Event> {
        let transaction = self.connection.transaction()?;
        let event = append_agent_event(&transaction, agent, name, data)?;
        if activity {
            transaction.execute(
                "UPDATE agents SET active_at = ?2 WHERE id = ?1",
                params![agent, event.at],
            )?;
        }
        transaction.commit()?;
        Ok(event)
    }

    /// At most `limit` events of the agent numbered after `after`, oldest first.
    pub fn agent_events(&self, agent: &str, after: u64, limit: usize) -> Result<Vec<Event>> {
        let mut statement = self.connection.prepare_cached(
            "SELECT e.seq, e.at, e.name, e.data FROM agent_events a JOIN events e ON e.seq = a.seq \
             WHERE a.agent = ?1 AND a.seq > ?2 ORDER BY a.seq LIMIT ?3",
        )?;
        let rows = statement.query_map(params![agent, after, limit as u64], event_row)?;
        rows.map(|row| row?).collect()
    }

    /// At most `limit` events of the agent numbered before `before`, the last ones, oldest first.
    pub fn agent_events_before(
        &self,
        agent: &str,
        before: u64,
        limit: usize,
    ) -> Result<Vec<Event>> {
        let mut statement = self.connection.prepare_cached(
            "SELECT e.seq, e.at, e.name, e.data FROM agent_events a JOIN events e ON e.seq = a.seq \
             WHERE a.agent = ?1 AND a.seq < ?2 ORDER BY a.seq DESC LIMIT ?3",
        )?;
        // SQLite's integers are signed: a bound past them is every event.
        let before = i64::try_from(before).unwrap_or(i64::MAX);
        let rows = statement.query_map(params![agent, before, limit as u64], event_row)?;
        let mut events = rows.map(|row| row?).collect::<Result<Vec<_>>>()?;
        events.reverse();
        Ok(events)
    }

    /// The agent's last events of that name, the newest first.
    pub fn last_agent_events(&self, agent: &str, name: &str, limit: usize) -> Result<Vec<Event>> {
        let mut statement = self.connection.prepare_cached(
            "SELECT e.seq, e.at, e.name, e.data FROM agent_events a JOIN events e ON e.seq = a.seq \
             WHERE a.agent = ?1 AND e.name = ?2 ORDER BY a.seq DESC LIMIT ?3",
        )?;
        let rows = statement.query_map(params![agent, name, limit as u64], event_row)?;
        rows.map(|row| row?).collect()
    }

    /// When the last of the agent's runs ended, if one did.
    pub fn last_run_end(&self, agent: &str) -> Result<Option<u64>> {
        Ok(self.connection.query_row(
            "SELECT max(ended_at) FROM runs WHERE agent = ?1",
            params![agent],
            |row| row.get(0),
        )?)
    }

    /// The latest sign of activity of these agents, from what is recorded: their events (but
    /// the two the daemon says on its own, `agent.tools_outdated` and `agent.released`), their
    /// deliveries made and sent, their runs started and ended, their requests asked and decided,
    /// and the agents' own `created_at`, `active_at` and `exchanged_at`. `None` for no agent.
    pub fn last_activity_of(&self, agents: &[String]) -> Result<Option<u64>> {
        Ok(self.connection.query_row(
            "WITH tree(id) AS (SELECT value FROM json_each(?1)) \
             SELECT max(at) FROM ( \
               SELECT max(created_at, coalesce(active_at, 0), coalesce(exchanged_at, 0)) AS at \
                 FROM agents WHERE id IN tree \
               UNION ALL SELECT max(e.at) FROM agent_events a JOIN events e ON e.seq = a.seq \
                 WHERE a.agent IN tree \
                 AND e.name NOT IN ('agent.tools_outdated', 'agent.tools_reload', 'agent.released') \
               UNION ALL SELECT max(created_at, coalesce(sent_at, 0)) FROM deliveries \
                 WHERE agent IN tree \
               UNION ALL SELECT max(started_at, coalesce(ended_at, 0)) FROM runs \
                 WHERE agent IN tree \
               UNION ALL SELECT max(created_at, coalesce(decided_at, 0)) FROM approval_requests \
                 WHERE agent IN tree)",
            params![serde_json::to_string(agents)?],
            |row| row.get(0),
        )?)
    }

    /// An adapter process started for the agent: its pid and its start time.
    pub fn add_process(&self, pid: u32, started: u64, agent: &str) -> Result<()> {
        self.connection.execute(
            "INSERT OR REPLACE INTO agent_processes (pid, started, agent) VALUES (?1, ?2, ?3)",
            params![pid, started, agent],
        )?;
        Ok(())
    }

    pub fn remove_process(&self, pid: u32) -> Result<()> {
        self.connection
            .execute("DELETE FROM agent_processes WHERE pid = ?1", params![pid])?;
        Ok(())
    }

    /// Every process recorded, each `(pid, start time)`.
    pub fn processes(&self) -> Result<Vec<(u32, u64)>> {
        let mut statement = self
            .connection
            .prepare_cached("SELECT pid, started FROM agent_processes ORDER BY pid")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The fingerprint of the file as the agent last read or wrote it.
    pub fn file_read(&self, agent: &str, path: &str) -> Result<Option<String>> {
        Ok(self
            .connection
            .query_row(
                "SELECT sha256 FROM session_files WHERE agent = ?1 AND path = ?2",
                params![agent, path],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn set_file_read(&self, agent: &str, path: &str, sha256: &str) -> Result<()> {
        self.connection.execute(
            "INSERT OR REPLACE INTO session_files (agent, path, sha256) VALUES (?1, ?2, ?3)",
            params![agent, path, sha256],
        )?;
        Ok(())
    }

    /// Writes a new run with its event, in one transaction.
    pub fn create_run(&mut self, run: &Run, event: (&str, Value)) -> Result<Event> {
        let transaction = self.connection.transaction()?;
        transaction.execute(
            &format!(
                "INSERT INTO runs ({RUN_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, \
                 ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)"
            ),
            params![
                run.id,
                run.agent,
                run.name,
                run.line,
                path_text(&run.folder)?,
                run.state.as_str(),
                run.exit_code,
                run.signal,
                run.started_at,
                run.ended_at,
                run.output,
                run.dismissed,
                run.project,
                run.ready.map(Readiness::as_str),
                run.url,
                run.url_error,
                run.workspace,
            ],
        )?;
        let event = append_event(&transaction, event.0, event.1)?;
        transaction.commit()?;
        Ok(event)
    }

    /// Ends a running run, with its output and its event, in one transaction; `None` when it had
    /// already ended.
    pub fn end_run(
        &mut self,
        id: &str,
        state: RunState,
        exit_code: Option<i32>,
        signal: Option<i32>,
        output: &str,
        event: (&str, Value),
    ) -> Result<Option<Event>> {
        let transaction = self.connection.transaction()?;
        let changed = transaction.execute(
            "UPDATE runs SET state = ?2, exit_code = ?3, signal = ?4, output = ?5, ended_at = ?6, \
             ready = CASE ready WHEN 'starting' THEN 'missed' ELSE ready END \
             WHERE id = ?1 AND state = 'running'",
            params![id, state.as_str(), exit_code, signal, output, now_ms()],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        let event = append_event(&transaction, event.0, event.1)?;
        transaction.commit()?;
        Ok(Some(event))
    }

    /// Marks a starting run ready, with its event, in one transaction; `None` when it was not
    /// starting (ready already, ended, or with no ready state).
    pub fn ready_run(&mut self, id: &str, event: (&str, Value)) -> Result<Option<Event>> {
        let transaction = self.connection.transaction()?;
        let changed = transaction.execute(
            "UPDATE runs SET ready = 'ready' WHERE id = ?1 AND state = 'running' \
             AND ready = 'starting'",
            params![id],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        let event = append_event(&transaction, event.0, event.1)?;
        transaction.commit()?;
        Ok(Some(event))
    }

    pub fn run(&self, id: &str) -> Result<Option<Run>> {
        let mut statement = self
            .connection
            .prepare_cached(&format!("SELECT {RUN_COLUMNS} FROM runs WHERE id = ?1"))?;
        let run = statement
            .query_row(params![id], run_row)
            .optional()?
            .transpose()?;
        Ok(run)
    }

    /// The runs still marked running, the oldest first.
    pub fn running_runs(&self) -> Result<Vec<Run>> {
        let mut statement = self.connection.prepare_cached(&format!(
            "SELECT {RUN_COLUMNS} FROM runs WHERE state = 'running' ORDER BY started_at, id"
        ))?;
        let rows = statement.query_map([], run_row)?;
        rows.map(|row| row?).collect()
    }

    /// The runs of these agents that are still running, oldest first.
    pub fn running_runs_of(&self, agents: &[String]) -> Result<Vec<Run>> {
        let mut statement = self.connection.prepare_cached(&format!(
            "SELECT {RUN_COLUMNS} FROM runs WHERE agent = ?1 AND state = 'running'"
        ))?;
        let mut found = Vec::new();
        for agent in agents {
            for row in statement.query_map(params![agent], run_row)? {
                found.push(row??);
            }
        }
        found.sort_by(|a, b| (a.started_at, &a.id).cmp(&(b.started_at, &b.id)));
        Ok(found)
    }

    /// The `limit` runs of these agents that ended most recently and were not dismissed, the
    /// oldest started first, each with only the last `tail` characters of its output: a list
    /// reads the end of an output, not the 64 KiB a run keeps.
    pub fn ended_runs_of(&self, agents: &[String], limit: usize, tail: usize) -> Result<Vec<Run>> {
        let mut statement = self.connection.prepare_cached(&format!(
            "SELECT id, agent, name, line, folder, state, exit_code, signal, started_at, \
             ended_at, substr(output, -{tail}), dismissed, project, ready, url, url_error, workspace \
             FROM runs \
             WHERE agent = ?1 AND state != 'running' AND dismissed = 0 \
             ORDER BY ended_at DESC, id DESC LIMIT ?2"
        ))?;
        let mut found = Vec::new();
        for agent in agents {
            for row in statement.query_map(params![agent, limit as u64], run_row)? {
                found.push(row??);
            }
        }
        found.sort_by(|a, b| (b.ended_at, &b.id).cmp(&(a.ended_at, &a.id)));
        found.truncate(limit);
        found.sort_by(|a, b| (a.started_at, &a.id).cmp(&(b.started_at, &b.id)));
        Ok(found)
    }

    /// The runs the user started in the project, of no agent: those running, then the `limit`
    /// that ended most recently and were not dismissed, each with the last `tail` characters of
    /// its output; the oldest started first.
    pub fn user_runs_of(&self, project: &str, limit: usize, tail: usize) -> Result<Vec<Run>> {
        let mut statement = self.connection.prepare_cached(&format!(
            "SELECT id, agent, name, line, folder, state, exit_code, signal, started_at, \
             ended_at, substr(output, -{tail}), dismissed, project, ready, url, url_error, workspace \
             FROM runs \
             WHERE agent IS NULL AND project = ?1 AND (state = 'running' OR (dismissed = 0 \
             AND id IN (SELECT id FROM runs WHERE agent IS NULL AND project = ?1 \
             AND state != 'running' AND dismissed = 0 ORDER BY ended_at DESC, id DESC LIMIT ?2))) \
             ORDER BY started_at, id"
        ))?;
        let rows = statement.query_map(params![project, limit as u64], run_row)?;
        rows.map(|row| row?).collect()
    }

    /// The agents that have runs, ended or not, or a cut tool result kept.
    pub fn agents_with_runs(&self) -> Result<Vec<String>> {
        let mut statement = self.connection.prepare_cached(
            "SELECT agent FROM runs WHERE agent IS NOT NULL UNION SELECT agent FROM tool_results",
        )?;
        let rows = statement.query_map([], |row| row.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The runs of these agents that ended go, and the cut tool results kept for them, their
    /// session being over for good. A run still running is left to end.
    pub fn forget_runs_of(&self, agents: &[String]) -> Result<usize> {
        let mut statement = self
            .connection
            .prepare_cached("DELETE FROM runs WHERE agent = ?1 AND state != 'running'")?;
        let mut results = self
            .connection
            .prepare_cached("DELETE FROM tool_results WHERE agent = ?1")?;
        let mut forgotten = 0;
        for agent in agents {
            forgotten += statement.execute(params![agent])?;
            forgotten += results.execute(params![agent])?;
        }
        Ok(forgotten)
    }

    /// Keeps the whole text of a tool result of `agent` that was cut, under its id. Only the
    /// latest `KEPT_RESULTS_PER_AGENT` of an agent keep their text: the older ones are emptied,
    /// and say so when read.
    pub fn keep_result(&self, id: &str, agent: &str, tool: &str, text: &str) -> Result<()> {
        self.connection.execute(
            "INSERT INTO tool_results (id, agent, tool, text, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, agent, tool, text, now_ms()],
        )?;
        self.connection.execute(
            "UPDATE tool_results SET text = '' WHERE agent = ?1 AND text != '' AND rowid NOT IN \
             (SELECT rowid FROM tool_results WHERE agent = ?1 AND text != '' \
              ORDER BY rowid DESC LIMIT ?2)",
            params![agent, KEPT_RESULTS_PER_AGENT as i64],
        )?;
        Ok(())
    }

    /// What is kept under `id`, for the agent it was kept for alone.
    pub fn kept_result(&self, id: &str, agent: &str) -> Result<Option<Kept>> {
        let mut statement = self
            .connection
            .prepare_cached("SELECT text FROM tool_results WHERE id = ?1 AND agent = ?2")?;
        let text: Option<String> = statement
            .query_row(params![id, agent], |row| row.get(0))
            .optional()?;
        // A text kept is never empty: it was longer than the limit.
        Ok(text.map(|text| {
            if text.is_empty() {
                Kept::Dropped
            } else {
                Kept::Text(text)
            }
        }))
    }

    /// Dismisses a run that ended, with its event, in one transaction; `None` when it is still
    /// running, already dismissed or unknown.
    pub fn dismiss_run(&mut self, id: &str, event: (&str, Value)) -> Result<Option<Event>> {
        let transaction = self.connection.transaction()?;
        let changed = transaction.execute(
            "UPDATE runs SET dismissed = 1 WHERE id = ?1 AND state != 'running' AND dismissed = 0",
            params![id],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        let event = append_event(&transaction, event.0, event.1)?;
        transaction.commit()?;
        Ok(Some(event))
    }

    /// A run's process group started: its leader's pid and start time.
    pub fn add_run_process(&self, pid: u32, started: u64, run: &str) -> Result<()> {
        self.connection.execute(
            "INSERT OR REPLACE INTO run_processes (pid, started, run) VALUES (?1, ?2, ?3)",
            params![pid, started, run],
        )?;
        Ok(())
    }

    pub fn remove_run_process(&self, pid: u32) -> Result<()> {
        self.connection
            .execute("DELETE FROM run_processes WHERE pid = ?1", params![pid])?;
        Ok(())
    }

    /// Every run process recorded, each `(pid, start time)`.
    pub fn run_processes(&self) -> Result<Vec<(u32, u64)>> {
        let mut statement = self
            .connection
            .prepare_cached("SELECT pid, started FROM run_processes ORDER BY pid")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Writes a request the gate held, with its `approval.requested` event, in one transaction:
    /// its number, its state (pending) and when it was made are the store's. The same call key
    /// of the same agent gives back the request it already made, with no event.
    pub fn ask(&mut self, asked: &Request) -> Result<(Request, Option<Event>)> {
        let transaction = self.connection.transaction()?;
        if let Some(key) = &asked.call_key {
            let number: Option<u64> = transaction
                .query_row(
                    "SELECT number FROM approval_requests WHERE agent = ?1 AND call_key = ?2",
                    params![asked.agent, key],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(number) = number {
                drop(transaction);
                let request = self
                    .request(number)?
                    .ok_or_else(|| Error::Refused(format!("there is no request #{number}")))?;
                return Ok((request, None));
            }
        }
        let created_at = now_ms();
        let number: u64 = transaction.query_row(
            "INSERT INTO approval_requests (agent, call_key, tool, arguments, shown, reasons, \
             sensitive, identity, held_by, forwards, state, created_at, kind, says, judgement, \
             place, digest) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, '', ?8, ?9, 'pending', ?10, ?11, ?12, ?13, ?14, \
             ?15) \
             RETURNING number",
            params![
                asked.agent,
                asked.call_key,
                asked.tool,
                serde_json::to_string(&asked.arguments)?,
                serde_json::to_string(&asked.shown)?,
                serde_json::to_string(&asked.reasons)?,
                asked.base,
                asked.held_by,
                serde_json::to_string(&asked.forwards)?,
                created_at,
                asked.kind.as_str(),
                serde_json::to_string(&asked.says)?,
                asked
                    .judgement
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?,
                asked.place,
                asked.digest,
            ],
            |row| row.get(0),
        )?;
        let request = Request {
            number,
            state: RequestState::Pending,
            decision: None,
            created_at,
            decided_at: None,
            ..asked.clone()
        };
        let event = append_agent_event(
            &transaction,
            &request.agent,
            "approval.requested",
            request.to_json(),
        )?;
        transaction.commit()?;
        Ok((request, Some(event)))
    }

    pub fn request(&self, number: u64) -> Result<Option<Request>> {
        let mut statement = self.connection.prepare_cached(&format!(
            "SELECT {REQUEST_COLUMNS} FROM approval_requests WHERE number = ?1"
        ))?;
        let request = statement
            .query_row(params![number], request_row)
            .optional()?
            .transpose()?;
        Ok(request)
    }

    /// The pending requests `held_by` holds (`approval::USER` for the user's), oldest first.
    pub fn pending_requests(&self, held_by: &str) -> Result<Vec<Request>> {
        let mut statement = self.connection.prepare_cached(&format!(
            "SELECT {REQUEST_COLUMNS} FROM approval_requests \
             WHERE held_by = ?1 AND state = 'pending' ORDER BY number"
        ))?;
        let rows = statement.query_map(params![held_by], request_row)?;
        rows.map(|row| row?).collect()
    }

    /// The requests `held_by` holds or held, in any state, that one of `agents` asked, oldest
    /// first: a request that reached the user stays held by them.
    pub fn requests_reaching(&self, held_by: &str, agents: &[String]) -> Result<Vec<Request>> {
        let mut statement = self.connection.prepare_cached(&format!(
            "SELECT {REQUEST_COLUMNS} FROM approval_requests WHERE held_by = ?1 \
             AND agent IN (SELECT value FROM json_each(?2)) ORDER BY number"
        ))?;
        let rows = statement.query_map(
            params![held_by, serde_json::to_string(agents)?],
            request_row,
        )?;
        rows.map(|row| row?).collect()
    }

    /// The pending requests the agent's calls made, whoever holds them, oldest first.
    pub fn pending_requests_of(&self, agent: &str) -> Result<Vec<Request>> {
        let mut statement = self.connection.prepare_cached(&format!(
            "SELECT {REQUEST_COLUMNS} FROM approval_requests \
             WHERE agent = ?1 AND state = 'pending' ORDER BY number"
        ))?;
        let rows = statement.query_map(params![agent], request_row)?;
        rows.map(|row| row?).collect()
    }

    /// The requests the agent's calls made that were cancelled at `since` or after, oldest first.
    pub fn cancelled_requests_of(&self, agent: &str, since: u64) -> Result<Vec<Request>> {
        let mut statement = self.connection.prepare_cached(&format!(
            "SELECT {REQUEST_COLUMNS} FROM approval_requests \
             WHERE agent = ?1 AND state = 'cancelled' AND decided_at >= ?2 ORDER BY number"
        ))?;
        let rows = statement.query_map(params![agent, since], request_row)?;
        rows.map(|row| row?).collect()
    }

    /// The agent `by`, which holds the pending request, passes it to `to` (its parent, or
    /// `approval::USER`) with one sentence of why, and its `approval.forwarded` event; `None`
    /// when the request is not pending or not held by `by`.
    pub fn forward(
        &mut self,
        number: u64,
        by: &str,
        why: &str,
        to: &str,
    ) -> Result<Option<(Request, Event)>> {
        let transaction = self.connection.transaction()?;
        let forwards: Option<String> = transaction
            .query_row(
                "SELECT forwards FROM approval_requests \
                 WHERE number = ?1 AND state = 'pending' AND held_by = ?2",
                params![number, by],
                |row| row.get(0),
            )
            .optional()?;
        let Some(forwards) = forwards else {
            return Ok(None);
        };
        let mut forwards: Vec<Forward> = serde_json::from_str(&forwards)?;
        forwards.push(Forward {
            by: by.to_owned(),
            why: why.to_owned(),
        });
        transaction.execute(
            "UPDATE approval_requests SET held_by = ?2, forwards = ?3 WHERE number = ?1",
            params![number, to, serde_json::to_string(&forwards)?],
        )?;
        let agent: String = transaction.query_row(
            "SELECT agent FROM approval_requests WHERE number = ?1",
            params![number],
            |row| row.get(0),
        )?;
        let event = append_agent_event(
            &transaction,
            &agent,
            "approval.forwarded",
            json!({"number": number, "agent": agent, "by": by, "why": why, "held_by": to}),
        )?;
        transaction.commit()?;
        let request = self
            .request(number)?
            .ok_or_else(|| Error::Refused(format!("there is no request #{number}")))?;
        Ok(Some((request, event)))
    }

    /// The decision on a pending request `by` holds (`approval::USER`, or an agent, which only
    /// denies), with its `approval.decided` event; `None` when the request is not pending or not
    /// held by `by`.
    pub fn decide(
        &mut self,
        number: u64,
        decision: Decision,
        by: &str,
    ) -> Result<Option<(Request, Event)>> {
        let transaction = self.connection.transaction()?;
        let now = now_ms();
        let state = match decision {
            Decision::Deny => RequestState::Denied,
            Decision::Once => RequestState::Allowed,
        };
        let changed = transaction.execute(
            "UPDATE approval_requests SET state = ?2, decision = ?3, decided_at = ?4 \
             WHERE number = ?1 AND state = 'pending' AND held_by = ?5",
            params![number, state.as_str(), decision.as_str(), now, by],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        let agent: String = transaction.query_row(
            "SELECT agent FROM approval_requests WHERE number = ?1",
            params![number],
            |row| row.get(0),
        )?;
        let event = append_agent_event(
            &transaction,
            &agent,
            "approval.decided",
            json!({"number": number, "agent": agent, "decision": decision.as_str(), "by": by}),
        )?;
        transaction.commit()?;
        let request = self
            .request(number)?
            .ok_or_else(|| Error::Refused(format!("there is no request #{number}")))?;
        Ok(Some((request, event)))
    }

    /// The pending requests the agent made or holds are cancelled: it ended, and nobody is left
    /// to answer them or to be answered. Their `approval.cancelled` events, one per request.
    pub fn cancel_requests(&mut self, agent: &str, why: &str) -> Result<Vec<Event>> {
        let transaction = self.connection.transaction()?;
        let cancelled: Vec<(u64, String, String)> = {
            let mut statement = transaction.prepare_cached(
                "UPDATE approval_requests SET state = 'cancelled', decided_at = ?2 \
                 WHERE (agent = ?1 OR held_by = ?1) AND state = 'pending' \
                 RETURNING number, agent, held_by",
            )?;
            statement
                .query_map(params![agent, now_ms()], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })?
                .collect::<rusqlite::Result<_>>()?
        };
        let mut events = Vec::new();
        for (number, asker, held_by) in cancelled {
            events.push(append_agent_event(
                &transaction,
                &asker,
                "approval.cancelled",
                json!({"number": number, "agent": asker, "held_by": held_by, "why": why}),
            )?);
        }
        transaction.commit()?;
        events.sort_by_key(|e| e.data["number"].as_u64());
        Ok(events)
    }

    /// Stores the answer of a decided request as the `approval` delivery of the agent that asked,
    /// with its `approval.settled` event; `None` when it was stored already.
    pub fn settle(
        &mut self,
        request: &Request,
        outcome: &str,
        body: &str,
    ) -> Result<Option<Event>> {
        let transaction = self.connection.transaction()?;
        let id = format!("approval-{}", request.number);
        let stored = transaction.execute(
            "INSERT OR IGNORE INTO deliveries (id, agent, kind, body, urgency, state, created_at) \
             VALUES (?1, ?2, 'approval', ?3, 'urgent', 'queued', ?4)",
            params![id, request.agent, body, now_ms()],
        )?;
        if stored == 0 {
            return Ok(None);
        }
        let event = append_agent_event(
            &transaction,
            &request.agent,
            "approval.settled",
            json!({"number": request.number, "agent": request.agent, "outcome": outcome,
                "delivery": id}),
        )?;
        transaction.commit()?;
        Ok(Some(event))
    }

    /// The requests allowed whose answer was never stored: a daemon stopped while it performed
    /// them.
    pub fn unsettled_requests(&self) -> Result<Vec<Request>> {
        let mut statement = self.connection.prepare_cached(&format!(
            "SELECT {REQUEST_COLUMNS} FROM approval_requests WHERE state = 'allowed' \
             AND NOT EXISTS (SELECT 1 FROM deliveries WHERE id = 'approval-' || number) \
             ORDER BY number"
        ))?;
        let rows = statement.query_map([], request_row)?;
        rows.map(|row| row?).collect()
    }

    pub fn delivery(&self, id: &str) -> Result<Option<Delivery>> {
        let mut statement = self.connection.prepare_cached(&format!(
            "SELECT {DELIVERY_COLUMNS} FROM deliveries WHERE id = ?1"
        ))?;
        Ok(statement.query_row(params![id], delivery_row).optional()?)
    }

    /// Stores a delivery, queued, with its `delivery.queued` event; `None` when one of its id
    /// was stored already, or its agent ended (nothing would ever send it).
    pub fn deliver(&mut self, delivery: &Delivery) -> Result<Option<Event>> {
        let transaction = self.connection.transaction()?;
        let stored = transaction.execute(
            "INSERT OR IGNORE INTO deliveries (id, agent, kind, body, urgency, state, created_at, \
             images, typed, commands) SELECT ?1, ?2, ?3, ?4, ?5, 'queued', ?6, ?7, ?8, ?9 WHERE \
             EXISTS (SELECT 1 FROM agents WHERE id = ?2 AND state NOT IN ('ended', 'failed'))",
            params![
                delivery.id,
                delivery.agent,
                delivery.kind,
                delivery.body,
                delivery.urgency,
                now_ms(),
                serde_json::to_string(&delivery.images)?,
                delivery.typed,
                serde_json::to_string(&delivery.commands)?
            ],
        )?;
        if stored == 0 {
            return Ok(None);
        }
        let event = append_agent_event(
            &transaction,
            &delivery.agent,
            "delivery.queued",
            json!({"agent": delivery.agent, "delivery": delivery.id, "kind": delivery.kind,
                "urgency": delivery.urgency}),
        )?;
        transaction.commit()?;
        Ok(Some(event))
    }

    /// The agent's deliveries in one state, oldest first.
    pub fn deliveries(&self, agent: &str, state: &str) -> Result<Vec<Delivery>> {
        let mut statement = self.connection.prepare_cached(&format!(
            "SELECT {DELIVERY_COLUMNS} FROM deliveries WHERE agent = ?1 AND state = ?2 \
             ORDER BY created_at, rowid"
        ))?;
        let rows = statement.query_map(params![agent, state], delivery_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The agent's last deliveries, the newest first, whatever their state.
    pub fn last_deliveries(&self, agent: &str, limit: usize) -> Result<Vec<Delivery>> {
        let mut statement = self.connection.prepare_cached(&format!(
            "SELECT {DELIVERY_COLUMNS} FROM deliveries WHERE agent = ?1 \
             ORDER BY created_at DESC, rowid DESC LIMIT ?2"
        ))?;
        let rows = statement.query_map(params![agent, limit as u64], delivery_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Whether a message of the user whose id starts with `prefix` waits for the agent or was
    /// taken by it: one withdrawn is neither.
    pub fn user_message_taken_or_waiting(&self, agent: &str, prefix: &str) -> Result<bool> {
        Ok(self.connection.query_row(
            "SELECT EXISTS (SELECT 1 FROM deliveries WHERE agent = ?1 AND kind = 'user' \
             AND substr(id, 1, length(?2)) = ?2 AND state IN ('queued', 'sending', 'sent'))",
            params![agent, prefix],
            |row| row.get(0),
        )?)
    }

    /// The agent's last deliveries of `kind`, newest first.
    pub fn last_deliveries_of(
        &self,
        agent: &str,
        kind: &str,
        limit: usize,
    ) -> Result<Vec<Delivery>> {
        let mut statement = self.connection.prepare_cached(&format!(
            "SELECT {DELIVERY_COLUMNS} FROM deliveries WHERE agent = ?1 AND kind = ?2 \
             ORDER BY created_at DESC, rowid DESC LIMIT ?3"
        ))?;
        let rows = statement.query_map(params![agent, kind, limit as u64], delivery_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Moves the agent's deliveries of these ids from `from` to `to` (`sending` or `sent`), with
    /// a `delivery.sent` event saying `how` when they are sent; the ids moved. A delivery that
    /// left `from` meanwhile is not moved.
    pub fn move_deliveries(
        &mut self,
        agent: &str,
        ids: &[String],
        from: &str,
        to: &str,
        how: &str,
    ) -> Result<(Vec<String>, Option<Event>)> {
        let transaction = self.connection.transaction()?;
        let now = now_ms();
        let mut moved = Vec::new();
        for id in ids {
            let changed = transaction.execute(
                "UPDATE deliveries SET state = ?3, sent_at = ?4 \
                 WHERE id = ?1 AND agent = ?2 AND state = ?5",
                params![id, agent, to, now, from],
            )?;
            if changed > 0 {
                moved.push(id.clone());
                // The user's message reaches the agent: the session's last exchange.
                transaction.execute(
                    "UPDATE agents SET exchanged_at = ?2 WHERE id = ?1 \
                     AND EXISTS (SELECT 1 FROM deliveries WHERE id = ?3 AND kind = 'user')",
                    params![agent, now, id],
                )?;
            }
        }
        let event = if moved.is_empty() {
            None
        } else {
            Some(append_agent_event(
                &transaction,
                agent,
                "delivery.sent",
                json!({"agent": agent, "deliveries": moved, "how": how}),
            )?)
        };
        transaction.commit()?;
        Ok((moved, event))
    }

    /// Takes back a message the user wrote to the agent, if it still waits: its state becomes
    /// `withdrawn`, with a `delivery.withdrawn` event. Checked and written in one transaction, so
    /// a delivery a prompt took first is left as it is and `None` comes back.
    pub fn withdraw(&mut self, agent: &str, id: &str) -> Result<(Option<Delivery>, Option<Event>)> {
        let transaction = self.connection.transaction()?;
        let changed = transaction.execute(
            "UPDATE deliveries SET state = 'withdrawn' \
             WHERE id = ?1 AND agent = ?2 AND kind = 'user' AND state = 'queued'",
            params![id, agent],
        )?;
        if changed == 0 {
            return Ok((None, None));
        }
        let delivery = transaction
            .prepare_cached(&format!(
                "SELECT {DELIVERY_COLUMNS} FROM deliveries WHERE id = ?1"
            ))?
            .query_row(params![id], delivery_row)?;
        let event = append_agent_event(
            &transaction,
            agent,
            "delivery.withdrawn",
            json!({"agent": agent, "deliveries": [id]}),
        )?;
        transaction.commit()?;
        Ok((Some(delivery), Some(event)))
    }

    /// Makes a message the user wrote a redirect, if it still waits: the agent's turn is
    /// cancelled and this message is its next prompt, alone. Whether it still waited.
    pub fn redirect(&self, agent: &str, id: &str) -> Result<bool> {
        Ok(self.connection.execute(
            "UPDATE deliveries SET urgency = ?3 \
             WHERE id = ?1 AND agent = ?2 AND kind = 'user' AND state = 'queued'",
            params![id, agent, crate::approval::REDIRECT],
        )? > 0)
    }

    /// Stores a subscription, in place of the agent's own on the same target.
    pub fn subscribe(&mut self, subscription: &Subscription) -> Result<()> {
        let transaction = self.connection.transaction()?;
        let target = serde_json::to_string(&subscription.target)?;
        transaction.execute(
            "DELETE FROM subscriptions WHERE agent = ?1 AND source = ?2 AND target = ?3",
            params![subscription.agent, subscription.source, target],
        )?;
        transaction.execute(
            "INSERT INTO subscriptions (id, agent, source, target, events, from_whom, seen, \
             created_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                subscription.id,
                subscription.agent,
                subscription.source,
                target,
                serde_json::to_string(&subscription.events)?,
                subscription.from,
                serde_json::to_string(&subscription.seen)?,
                subscription.created_at,
                subscription.expires_at
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Every subscription, the oldest first.
    pub fn subscriptions(&self) -> Result<Vec<Subscription>> {
        let mut statement = self.connection.prepare_cached(
            "SELECT id, agent, source, target, events, from_whom, seen, created_at, expires_at \
             FROM subscriptions ORDER BY created_at, rowid",
        )?;
        let rows = statement.query_map([], subscription_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Writes what the source of a subscription last saw of its target; whether it was still
    /// there: one ended meanwhile is not written back.
    pub fn set_seen(&self, id: &str, seen: &serde_json::Value) -> Result<bool> {
        Ok(self.connection.execute(
            "UPDATE subscriptions SET seen = ?2 WHERE id = ?1",
            params![id, serde_json::to_string(seen)?],
        )? > 0)
    }

    /// Removes a subscription; whether it was there.
    pub fn unsubscribe(&self, id: &str) -> Result<bool> {
        Ok(self
            .connection
            .execute("DELETE FROM subscriptions WHERE id = ?1", params![id])?
            > 0)
    }

    /// The user lets `writer` write to `session`, with the request it was asked in, and its
    /// `leave.granted` event; `None` when it was given already or either session has ended.
    pub fn grant_leave(
        &mut self,
        writer: &str,
        session: &str,
        request: u64,
    ) -> Result<(Option<Leave>, Option<Event>)> {
        let transaction = self.connection.transaction()?;
        let leave = Leave {
            writer: writer.to_owned(),
            session: session.to_owned(),
            request,
            granted_at: now_ms(),
        };
        // Checked in the same write as `change_agent`'s DELETE, so a session ended between the
        // approval and here never keeps a leave a resume would find.
        let added = transaction.execute(
            "INSERT OR IGNORE INTO session_leaves (writer, session, request, granted_at) \
             SELECT ?1, ?2, ?3, ?4 WHERE (SELECT count(*) FROM agents \
             WHERE id IN (?1, ?2) AND state NOT IN ('ended', 'failed')) = 2",
            params![writer, session, request, leave.granted_at],
        )?;
        if added == 0 {
            return Ok((None, None));
        }
        let event = append_event(&transaction, "leave.granted", leave.to_json())?;
        transaction.commit()?;
        Ok((Some(leave), Some(event)))
    }

    /// Whether the user let `writer` write to `session`.
    pub fn has_leave(&self, writer: &str, session: &str) -> Result<bool> {
        Ok(self
            .connection
            .query_row(
                "SELECT 1 FROM session_leaves WHERE writer = ?1 AND session = ?2",
                params![writer, session],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// The leaves that name `agent`, as the writer or as the session written to, oldest first.
    pub fn leaves_of(&self, agent: &str) -> Result<Vec<Leave>> {
        let mut statement = self.connection.prepare_cached(
            "SELECT writer, session, request, granted_at FROM session_leaves \
             WHERE writer = ?1 OR session = ?1 ORDER BY granted_at, rowid",
        )?;
        let rows = statement.query_map(params![agent], |row| {
            Ok(Leave {
                writer: row.get(0)?,
                session: row.get(1)?,
                request: row.get(2)?,
                granted_at: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The session `agent` waits for its quota's reset at `reset_at` (`None`: it could not be
    /// read), and its `agent.quota` event. A wait already there takes the new instant and keeps
    /// the user's choice for this session.
    pub fn limit_quota(
        &mut self,
        agent: &str,
        reset_at: Option<u64>,
    ) -> Result<(QuotaWait, Option<Event>)> {
        let transaction = self.connection.transaction()?;
        let wait = transaction.query_row(
            "INSERT INTO quota_waits (agent, reset_at, resume, limited_at) VALUES (?1, ?2, NULL, ?3) \
             ON CONFLICT (agent) DO UPDATE SET reset_at = excluded.reset_at, \
             limited_at = excluded.limited_at \
             RETURNING agent, reset_at, resume, limited_at",
            params![agent, reset_at, now_ms()],
            QuotaWait::of_row,
        )?;
        let event = append_event(&transaction, "agent.quota", wait.to_json())?;
        transaction.commit()?;
        Ok((wait, Some(event)))
    }

    /// The session's wait for its quota's reset, if it waits.
    pub fn quota_wait(&self, agent: &str) -> Result<Option<QuotaWait>> {
        Ok(self
            .connection
            .query_row(
                "SELECT agent, reset_at, resume, limited_at FROM quota_waits WHERE agent = ?1",
                params![agent],
                QuotaWait::of_row,
            )
            .optional()?)
    }

    /// Every session waiting for its quota's reset.
    pub fn quota_waits(&self) -> Result<Vec<QuotaWait>> {
        let mut statement = self.connection.prepare_cached(
            "SELECT agent, reset_at, resume, limited_at FROM quota_waits ORDER BY limited_at",
        )?;
        let rows = statement.query_map([], QuotaWait::of_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The user's choice to resume this session at its reset or not; `None` when it does not
    /// wait.
    pub fn choose_quota_resume(
        &mut self,
        agent: &str,
        resume: bool,
    ) -> Result<(Option<QuotaWait>, Option<Event>)> {
        let transaction = self.connection.transaction()?;
        let wait = transaction
            .query_row(
                "UPDATE quota_waits SET resume = ?2 WHERE agent = ?1 \
                 RETURNING agent, reset_at, resume, limited_at",
                params![agent, resume],
                QuotaWait::of_row,
            )
            .optional()?;
        let Some(wait) = wait else {
            return Ok((None, None));
        };
        let event = append_event(&transaction, "agent.quota", wait.to_json())?;
        transaction.commit()?;
        Ok((Some(wait), Some(event)))
    }

    /// The session no longer waits for its quota (`why`); with `reset_at`, only the wait for that
    /// instant goes, so that two takers never both take it. Whether a wait went.
    pub fn end_quota_wait(
        &mut self,
        agent: &str,
        reset_at: Option<u64>,
        why: &str,
    ) -> Result<(bool, Option<Event>)> {
        let transaction = self.connection.transaction()?;
        let gone = match reset_at {
            Some(at) => transaction.execute(
                "DELETE FROM quota_waits WHERE agent = ?1 AND reset_at = ?2",
                params![agent, at],
            )?,
            None => {
                transaction.execute("DELETE FROM quota_waits WHERE agent = ?1", params![agent])?
            }
        };
        if gone == 0 {
            return Ok((false, None));
        }
        let event = append_event(
            &transaction,
            "agent.quota",
            json!({"agent": agent, "limited": false, "why": why}),
        )?;
        transaction.commit()?;
        Ok((true, Some(event)))
    }

    /// The vault's entries, by name.
    pub fn vault_entries(&self) -> Result<Vec<crate::vault::Entry>> {
        let mut statement = self.connection.prepare_cached(
            "SELECT name, description, url, username, project, updated_at, bound \
             FROM vault_entries ORDER BY name",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(crate::vault::Entry {
                name: row.get(0)?,
                description: row.get(1)?,
                url: row.get(2)?,
                username: row.get(3)?,
                project: row.get(4)?,
                updated_at: row.get(5)?,
                bound: row.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Keeps `entry`, in place of the one of its name, with its `vault.changed` event: its
    /// metadata, never a secret.
    pub fn put_vault_entry(&mut self, entry: &crate::vault::Entry) -> Result<((), Event)> {
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "INSERT INTO vault_entries \
             (name, description, url, username, project, updated_at, bound) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) ON CONFLICT (name) DO UPDATE SET \
             description = ?2, url = ?3, username = ?4, project = ?5, updated_at = ?6, bound = ?7",
            params![
                entry.name,
                entry.description,
                entry.url,
                entry.username,
                entry.project,
                entry.updated_at,
                entry.bound
            ],
        )?;
        let event = append_event(
            &transaction,
            "vault.changed",
            json!({"name": entry.name, "project": entry.project, "for": entry.bound}),
        )?;
        transaction.commit()?;
        Ok(((), event))
    }

    /// Forgets the entry `name`, with its `vault.changed` event; `None` when there was none.
    pub fn remove_vault_entry(
        &mut self,
        name: &str,
    ) -> Result<(Option<crate::vault::Entry>, Option<Event>)> {
        let transaction = self.connection.transaction()?;
        let removed = transaction
            .query_row(
                "DELETE FROM vault_entries WHERE name = ?1 \
                 RETURNING description, url, username, project, updated_at, bound",
                params![name],
                |row| {
                    Ok(crate::vault::Entry {
                        name: name.to_owned(),
                        description: row.get(0)?,
                        url: row.get(1)?,
                        username: row.get(2)?,
                        project: row.get(3)?,
                        updated_at: row.get(4)?,
                        bound: row.get(5)?,
                    })
                },
            )
            .optional()?;
        let Some(removed) = removed else {
            return Ok((None, None));
        };
        let event = append_event(
            &transaction,
            "vault.changed",
            json!({"name": name, "project": removed.project, "removed": true}),
        )?;
        transaction.commit()?;
        Ok((Some(removed), Some(event)))
    }

    /// The user takes the leave back, with its `leave.withdrawn` event; `None` when there was
    /// none.
    pub fn withdraw_leave(
        &mut self,
        writer: &str,
        session: &str,
    ) -> Result<(Option<Leave>, Option<Event>)> {
        let transaction = self.connection.transaction()?;
        let leave = transaction
            .query_row(
                "DELETE FROM session_leaves WHERE writer = ?1 AND session = ?2 \
                 RETURNING request, granted_at",
                params![writer, session],
                |row| {
                    Ok(Leave {
                        writer: writer.to_owned(),
                        session: session.to_owned(),
                        request: row.get(0)?,
                        granted_at: row.get(1)?,
                    })
                },
            )
            .optional()?;
        let Some(leave) = leave else {
            return Ok((None, None));
        };
        let event = append_event(&transaction, "leave.withdrawn", leave.to_json())?;
        transaction.commit()?;
        Ok((Some(leave), Some(event)))
    }

    /// The project's items that `filter` keeps, with their checks, the most recently changed
    /// first.
    pub fn items(&self, project: &str, filter: &Filter) -> Result<Vec<Item>> {
        let mut statement = self.connection.prepare_cached(
            "SELECT project, kind, source, repository, key, title, state, author, assignees, \
             labels, url, updated_at, seen_at, mine, details FROM items WHERE project = ?1 \
             AND (?2 IS NULL OR kind = ?2) AND (?3 IS NULL OR state = ?3) \
             AND (?4 IS NULL OR repository = ?4) AND (?5 = 0 OR mine = 1) \
             ORDER BY updated_at DESC, kind, repository, key",
        )?;
        let rows = statement.query_map(
            params![
                project,
                filter.kind,
                filter.state,
                filter.repository,
                filter.mine
            ],
            item_row,
        )?;
        let mut items: Vec<Item> = rows.collect::<rusqlite::Result<_>>()?;
        let mut checks = self.connection.prepare_cached(
            "SELECT name, state, started_at, ended_at, link FROM item_checks \
             WHERE project = ?1 AND kind = ?2 AND repository = ?3 AND key = ?4 ORDER BY position",
        )?;
        for item in &mut items {
            item.checks = checks
                .query_map(
                    params![item.project, item.kind, item.repository, item.key],
                    |row| {
                        Ok(Check {
                            name: row.get(0)?,
                            state: row.get(1)?,
                            started_at: row.get(2)?,
                            ended_at: row.get(3)?,
                            link: row.get(4)?,
                        })
                    },
                )?
                .collect::<rusqlite::Result<_>>()?;
        }
        Ok(items)
    }

    /// Stores what a round read, each item in place of the one it was, with its checks, and the
    /// events it gave, in one transaction.
    pub fn put_items(&mut self, items: &[Item], events: Vec<(&str, Value)>) -> Result<Vec<Event>> {
        let transaction = self.connection.transaction()?;
        for item in items {
            let key = params![item.project, item.kind, item.repository, item.key];
            transaction.execute(
                "DELETE FROM item_checks WHERE project = ?1 AND kind = ?2 AND repository = ?3 \
                 AND key = ?4",
                key,
            )?;
            transaction.execute(
                "INSERT OR REPLACE INTO items (project, kind, source, repository, key, title, \
                 state, author, assignees, labels, url, updated_at, seen_at, mine, details) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                params![
                    item.project,
                    item.kind,
                    item.source,
                    item.repository,
                    item.key,
                    item.title,
                    item.state,
                    item.author,
                    serde_json::to_string(&item.assignees)?,
                    serde_json::to_string(&item.labels)?,
                    item.url,
                    item.updated_at,
                    item.seen_at,
                    item.mine,
                    serde_json::to_string(&item.details)?
                ],
            )?;
            for (position, check) in item.checks.iter().enumerate() {
                transaction.execute(
                    "INSERT INTO item_checks (project, kind, repository, key, position, name, \
                     state, started_at, ended_at, link) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![
                        item.project,
                        item.kind,
                        item.repository,
                        item.key,
                        position,
                        check.name,
                        check.state,
                        check.started_at,
                        check.ended_at,
                        check.link
                    ],
                )?;
            }
        }
        let mut written = Vec::new();
        for (name, data) in events {
            written.push(append_event(&transaction, name, data)?);
        }
        transaction.commit()?;
        Ok(written)
    }

    /// Forgets the items and the cursors of every project but those of `kept`, and of each kept
    /// project those of a source it no longer has. A source is `(kind, target, source,
    /// repository)`: `target` is a repository's path or `tracker`, `source` its forge or tracker,
    /// `repository` the path its items are of (empty for Jira). One `items.forgotten` for each
    /// source whose items went, in the same transaction.
    pub fn retain_items(&mut self, kept: &[(String, Vec<[String; 4]>)]) -> Result<Vec<Event>> {
        let transaction = self.connection.transaction()?;
        let has = |project: &str, kept_one: &dyn Fn(&[String; 4]) -> bool| {
            kept.iter()
                .any(|(name, sources)| name == project && sources.iter().any(kept_one))
        };
        let mut cursors: Vec<[String; 4]> = Vec::new();
        {
            let mut statement =
                transaction.prepare("SELECT project, kind, target, source FROM fetch_cursors")?;
            let rows = statement.query_map([], |row| {
                Ok([row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?])
            })?;
            for row in rows {
                cursors.push(row?);
            }
        }
        for [project, kind, target, source] in cursors {
            if !has(&project, &|[k, t, s, _]| {
                *k == kind && *t == target && *s == source
            }) {
                transaction.execute(
                    "DELETE FROM fetch_cursors WHERE project = ?1 AND kind = ?2 AND target = ?3",
                    params![project, kind, target],
                )?;
            }
        }
        let mut items: Vec<[String; 4]> = Vec::new();
        {
            let mut statement = transaction.prepare(
                "SELECT DISTINCT project, kind, source, repository FROM items \
                 ORDER BY project, kind, source, repository",
            )?;
            let rows = statement.query_map([], |row| {
                Ok([row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?])
            })?;
            for row in rows {
                items.push(row?);
            }
        }
        let mut written = Vec::new();
        for [project, kind, source, repository] in items {
            if !has(&project, &|[k, _, s, r]| {
                *k == kind && *s == source && *r == repository
            }) {
                let gone = transaction.execute(
                    "DELETE FROM items WHERE project = ?1 AND kind = ?2 AND source = ?3 \
                     AND repository = ?4",
                    params![project, kind, source, repository],
                )?;
                let data = json!({"project": project, "kind": kind, "source": source,
                    "repository": repository, "items": gone});
                written.push(append_event(&transaction, "items.forgotten", data)?);
            }
        }
        transaction.commit()?;
        Ok(written)
    }

    /// Where each source of the project stands.
    pub fn cursors(&self, project: &str) -> Result<Vec<Cursor>> {
        let mut statement = self.connection.prepare_cached(
            "SELECT project, kind, target, source, cursor, round_at, failures, problem, \
             first_at, note FROM fetch_cursors WHERE project = ?1 ORDER BY kind, target",
        )?;
        let rows = statement.query_map(params![project], |row| {
            Ok(Cursor {
                project: row.get(0)?,
                kind: row.get(1)?,
                target: row.get(2)?,
                source: row.get(3)?,
                cursor: row.get(4)?,
                round_at: row.get(5)?,
                failures: row.get(6)?,
                problem: row.get(7)?,
                first_at: row.get(8)?,
                note: row.get(9)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn set_cursor(&self, cursor: &Cursor) -> Result<()> {
        self.connection.execute(
            "INSERT OR REPLACE INTO fetch_cursors (project, kind, target, source, cursor, \
             round_at, failures, problem, first_at, note) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                cursor.project,
                cursor.kind,
                cursor.target,
                cursor.source,
                cursor.cursor,
                cursor.round_at,
                cursor.failures,
                cursor.problem,
                cursor.first_at,
                cursor.note
            ],
        )?;
        Ok(())
    }

    /// Records a post that succeeded. An item's own text (`body`) is recorded once: a later post
    /// of it replaces the earlier.
    pub fn add_post(&self, post: &Post) -> Result<()> {
        if post.kind == "body" {
            self.connection.execute(
                "DELETE FROM posts WHERE project = ?1 AND source = ?2 AND repository = ?3 \
                 AND key = ?4 AND kind = 'body'",
                params![post.project, post.source, post.repository, post.key],
            )?;
        }
        self.connection.execute(
            "INSERT INTO posts (project, source, repository, key, kind, comment_id, \
             fingerprint, posted_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                post.project,
                post.source,
                post.repository,
                post.key,
                post.kind,
                post.comment_id,
                post.fingerprint,
                post.posted_at
            ],
        )?;
        Ok(())
    }

    /// Forgets the comment `comment_id` of an item: it was deleted, or its record is replaced.
    pub fn drop_comment_posts(
        &self,
        (project, source, repository, key): (&str, &str, &str, &str),
        comment_id: &str,
    ) -> Result<()> {
        self.connection.execute(
            "DELETE FROM posts WHERE project = ?1 AND source = ?2 AND repository = ?3 \
             AND key = ?4 AND kind = 'comment' AND comment_id = ?5",
            params![project, source, repository, key, comment_id],
        )?;
        Ok(())
    }

    /// What was posted on one item, oldest first.
    pub fn posts(
        &self,
        project: &str,
        source: &str,
        repository: &str,
        key: &str,
    ) -> Result<Vec<Post>> {
        let mut statement = self.connection.prepare_cached(
            "SELECT kind, comment_id, fingerprint, posted_at FROM posts WHERE project = ?1 \
             AND source = ?2 AND repository = ?3 AND key = ?4 ORDER BY id",
        )?;
        let rows = statement.query_map(params![project, source, repository, key], |row| {
            Ok(Post {
                project: project.to_owned(),
                source: source.to_owned(),
                repository: repository.to_owned(),
                key: key.to_owned(),
                kind: row.get(0)?,
                comment_id: row.get(1)?,
                fingerprint: row.get(2)?,
                posted_at: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The agent that wrote the service; `None` writes it as the user's.
    pub fn set_service_writer(&mut self, name: &str, by: Option<&str>) -> Result<()> {
        match by {
            Some(agent) => self.connection.execute(
                "INSERT INTO service_writers (name, agent) VALUES (?1, ?2) \
                 ON CONFLICT (name) DO UPDATE SET agent = ?2",
                params![name, agent],
            )?,
            None => self
                .connection
                .execute("DELETE FROM service_writers WHERE name = ?1", params![name])?,
        };
        Ok(())
    }

    /// The agent that wrote the service; `None` when it is the user's.
    pub fn service_writer(&self, name: &str) -> Result<Option<String>> {
        Ok(self
            .connection
            .query_row(
                "SELECT agent FROM service_writers WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// Records what a turn used, one row per model.
    pub fn record_turn_usage(&mut self, rows: &[TurnUsage]) -> Result<()> {
        let transaction = self.connection.transaction()?;
        {
            let mut statement = transaction.prepare_cached(
                "INSERT INTO turn_usage (agent, title, session, project, workspace, provider, \
                 role, model, started_at, ended_at, stop_reason, input, output, cache_read, \
                 cache_write, reasoning, cost_usd) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            )?;
            for row in rows {
                let used = &row.used;
                statement.execute(params![
                    row.agent,
                    row.title,
                    row.session,
                    row.project,
                    row.workspace,
                    row.provider,
                    row.role,
                    used.model,
                    row.started_at,
                    row.ended_at,
                    row.stop_reason,
                    used.input,
                    used.output,
                    used.cache_read,
                    used.cache_write,
                    used.reasoning,
                    used.cost_usd
                ])?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    /// Every row of the turns' usage, in the order they were recorded.
    pub fn turn_usage(&self) -> Result<Vec<TurnUsage>> {
        let mut statement = self.connection.prepare_cached(
            "SELECT agent, title, session, project, workspace, provider, role, model, started_at, \
             ended_at, stop_reason, input, output, cache_read, cache_write, reasoning, cost_usd \
             FROM turn_usage ORDER BY id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(TurnUsage {
                agent: row.get(0)?,
                title: row.get(1)?,
                session: row.get(2)?,
                project: row.get(3)?,
                workspace: row.get(4)?,
                provider: row.get(5)?,
                role: row.get(6)?,
                started_at: row.get(8)?,
                ended_at: row.get(9)?,
                stop_reason: row.get(10)?,
                used: Used {
                    model: row.get(7)?,
                    input: row.get(11)?,
                    output: row.get(12)?,
                    cache_read: row.get(13)?,
                    cache_write: row.get(14)?,
                    reasoning: row.get(15)?,
                    cost_usd: row.get(16)?,
                },
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The turns of a session, each once whatever the models it used: its agent and its start and
    /// end, in milliseconds.
    pub fn turns_of_session(&self, session: &str) -> Result<Vec<(String, u64, u64)>> {
        let mut statement = self.connection.prepare_cached(
            "SELECT agent, started_at, ended_at FROM turn_usage WHERE session = ?1 \
             GROUP BY agent, started_at, ended_at ORDER BY started_at",
        )?;
        let rows = statement.query_map(params![session], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The agent's deliveries `sending` become `sent`: the turn that held them ended.
    pub fn deliveries_taken(&self, agent: &str) -> Result<()> {
        self.connection.execute(
            "UPDATE deliveries SET state = 'sent' WHERE agent = ?1 AND state = 'sending'",
            params![agent],
        )?;
        Ok(())
    }

    /// The agent's deliveries `sending` are queued again: the turn that held them failed. Their
    /// `delivery.given_back` event, when there were some.
    pub fn give_back(&mut self, agent: &str) -> Result<Option<Event>> {
        let transaction = self.connection.transaction()?;
        let ids: Vec<String> = {
            let mut statement = transaction.prepare_cached(
                "UPDATE deliveries SET state = 'queued', sent_at = NULL \
                 WHERE agent = ?1 AND state = 'sending' RETURNING id",
            )?;
            statement
                .query_map(params![agent], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let event = if ids.is_empty() {
            None
        } else {
            Some(append_agent_event(
                &transaction,
                agent,
                "delivery.given_back",
                json!({"agent": agent, "deliveries": ids}),
            )?)
        };
        transaction.commit()?;
        Ok(event)
    }

    /// Every delivery of the agent not sent yet is superseded: it stopped. Their
    /// `delivery.superseded` event, when there were some.
    pub fn supersede(&mut self, agent: &str) -> Result<Option<Event>> {
        let transaction = self.connection.transaction()?;
        let ids: Vec<String> = {
            let mut statement = transaction.prepare_cached(
                "UPDATE deliveries SET state = 'superseded' \
                 WHERE agent = ?1 AND state IN ('queued', 'sending') RETURNING id",
            )?;
            statement
                .query_map(params![agent], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let event = if ids.is_empty() {
            None
        } else {
            Some(append_agent_event(
                &transaction,
                agent,
                "delivery.superseded",
                json!({"agent": agent, "deliveries": ids}),
            )?)
        };
        transaction.commit()?;
        Ok(event)
    }

    /// The agent's deliveries not sent yet whose ids start with one of `prefixes` are
    /// superseded: a fresh one takes their place. Their `delivery.superseded` event, when there
    /// were some.
    pub fn supersede_like(&mut self, agent: &str, prefixes: &[&str]) -> Result<Option<Event>> {
        let transaction = self.connection.transaction()?;
        let mut ids: Vec<String> = Vec::new();
        for prefix in prefixes {
            let mut statement = transaction.prepare_cached(
                "UPDATE deliveries SET state = 'superseded' \
                 WHERE agent = ?1 AND state IN ('queued', 'sending') \
                 AND substr(id, 1, length(?2)) = ?2 RETURNING id",
            )?;
            let found = statement
                .query_map(params![agent, prefix], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?;
            ids.extend(found);
        }
        let event = if ids.is_empty() {
            None
        } else {
            Some(append_agent_event(
                &transaction,
                agent,
                "delivery.superseded",
                json!({"agent": agent, "deliveries": ids}),
            )?)
        };
        transaction.commit()?;
        Ok(event)
    }

    /// The agents a parent launched, the oldest first.
    pub fn children(&self, parent: &str) -> Result<Vec<Agent>> {
        let mut statement = self.connection.prepare_cached(&format!(
            "SELECT {AGENT_COLUMNS} FROM agents WHERE parent = ?1 ORDER BY created_at, id"
        ))?;
        let rows = statement.query_map(params![parent], agent_row)?;
        rows.map(|row| row?).collect()
    }

    /// The children of the project that have not ended: what its children cap counts.
    pub fn live_children(&self, project: &str) -> Result<usize> {
        let count: u64 = self.connection.query_row(
            "SELECT count(*) FROM agents WHERE project = ?1 AND parent IS NOT NULL \
             AND state NOT IN ('ended', 'failed')",
            params![project],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    /// The replacements of the agent since `since` that count against its limit.
    /// The models a provider's agents offered last, with the version they were read from.
    pub fn provider_models(&self, provider: &str) -> Result<Option<(Option<String>, Value)>> {
        let row: Option<(Option<String>, String)> = self
            .connection
            .query_row(
                "SELECT version, models FROM provider_models WHERE provider = ?1",
                params![provider],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        row.map(|(version, models)| Ok((version, serde_json::from_str(&models)?)))
            .transpose()
    }

    /// Keeps what a provider's agent offered, in place of what was kept.
    pub fn set_provider_models(
        &self,
        provider: &str,
        version: Option<&str>,
        models: &Value,
    ) -> Result<()> {
        self.connection.execute(
            "INSERT INTO provider_models (provider, version, models, updated_at) \
             VALUES (?1, ?2, ?3, ?4) ON CONFLICT (provider) DO UPDATE SET version = ?2, \
             models = ?3, updated_at = ?4",
            params![provider, version, models.to_string(), now_ms()],
        )?;
        Ok(())
    }

    pub fn replacements_since(&self, agent: &str, since: u64) -> Result<usize> {
        let count: u64 = self.connection.query_row(
            "SELECT count(*) FROM agent_replacements WHERE agent = ?1 AND at >= ?2 AND counted",
            params![agent, since],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    /// A new session of the agent replaces the one it had, with its `agent.replaced` event: its
    /// epoch grows, its ACP session is forgotten (the next process opens a new one), and it
    /// starts again. `counted` says the replacement counts against its limit.
    pub fn replace_agent(
        &mut self,
        id: &str,
        reason: &str,
        counted: bool,
    ) -> Result<(Agent, Option<Event>)> {
        let transaction = self.connection.transaction()?;
        let now = now_ms();
        let changed = transaction.execute(
            "UPDATE agents SET epoch = epoch + 1, native_id = NULL, state = 'starting', \
             state_reason = ?2, reported_at = NULL, quiet_turns = 0, waiting_on_answer = 0, \
             updated_at = ?3 WHERE id = ?1 AND state NOT IN ('ended', 'failed')",
            params![id, reason, now],
        )?;
        let event = if changed == 0 {
            None
        } else {
            let epoch: u64 = transaction.query_row(
                "SELECT epoch FROM agents WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )?;
            transaction.execute(
                "INSERT INTO agent_replacements (agent, at, epoch, reason, counted) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, now, epoch, reason, counted],
            )?;
            Some(append_agent_event(
                &transaction,
                id,
                "agent.replaced",
                json!({"agent": id, "epoch": epoch, "reason": reason, "counted": counted}),
            )?)
        };
        transaction.commit()?;
        let agent = self
            .agent(id)?
            .ok_or_else(|| Error::Refused(format!("there is no agent {id}")))?;
        Ok((agent, event))
    }

    /// An ended or failed root starts again on its own session, with its `agent.resumed` event:
    /// idle, its end and its reason forgotten, its epoch and its session kept. `None` when it is
    /// neither.
    pub fn resume_agent(&mut self, id: &str) -> Result<Option<(Agent, Event)>> {
        let transaction = self.connection.transaction()?;
        let changed = transaction.execute(
            "UPDATE agents SET state = 'idle', state_reason = NULL, ended_at = NULL, \
             reported_at = NULL, quiet_turns = 0, waiting_on_answer = 0, updated_at = ?2 \
             WHERE id = ?1 AND state IN ('ended', 'failed')",
            params![id, now_ms()],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        let event = append_agent_event(&transaction, id, "agent.resumed", json!({"agent": id}))?;
        transaction.commit()?;
        let agent = self
            .agent(id)?
            .ok_or_else(|| Error::Refused(format!("there is no agent {id}")))?;
        Ok(Some((agent, event)))
    }

    /// At most `limit` events numbered after `after`, oldest first.
    pub fn events_after(&self, after: u64, limit: usize) -> Result<Vec<Event>> {
        let mut statement = self.connection.prepare_cached(
            "SELECT seq, at, name, data FROM events WHERE seq > ?1 ORDER BY seq LIMIT ?2",
        )?;
        let rows = statement.query_map(params![after, limit as u64], event_row)?;
        rows.map(|row| row?).collect()
    }

    /// Every event numbered after `after` whose name is one of `names`, oldest first.
    pub fn events_named(&self, names: &[&str], after: u64) -> Result<Vec<Event>> {
        let marks = vec!["?"; names.len()].join(", ");
        let mut statement = self.connection.prepare(&format!(
            "SELECT seq, at, name, data FROM events WHERE seq > ?1 AND name IN ({marks}) \
             ORDER BY seq"
        ))?;
        let values = std::iter::once(rusqlite::types::Value::from(after as i64)).chain(
            names
                .iter()
                .map(|name| rusqlite::types::Value::from((*name).to_owned())),
        );
        let rows = statement.query_map(rusqlite::params_from_iter(values), event_row)?;
        rows.map(|row| row?).collect()
    }

    /// The last event of that name, read from the end.
    pub fn last_event_named(&self, name: &str) -> Result<Option<Event>> {
        self.connection
            .query_row(
                "SELECT seq, at, name, data FROM events WHERE name = ?1 ORDER BY seq DESC LIMIT 1",
                [name],
                event_row,
            )
            .optional()?
            .transpose()
    }

    /// The number of the last event, 0 when there is none.
    pub fn last_seq(&self) -> Result<u64> {
        let seq: Option<u64> = self
            .connection
            .query_row("SELECT max(seq) FROM events", [], |row| row.get(0))
            .optional()?
            .flatten();
        Ok(seq.unwrap_or(0))
    }

    /// Brings the tables of the extension `name` up to date: `migrations` applied in order, those
    /// already applied never twice, as the store's own are. A migration is never edited once
    /// released.
    pub fn migrate_extension(&mut self, name: &str, migrations: &[&str]) -> Result<()> {
        let prefix = prefix_of(name)?;
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let applied: usize = transaction
            .query_row(
                "SELECT applied FROM extension_migrations WHERE name = ?1",
                [name],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(0);
        if applied > migrations.len() {
            return Err(Error::Refused(format!(
                "the store's tables of {name} are at schema {applied}, made by a newer Flottille \
                 than this one (schema {})",
                migrations.len()
            )));
        }
        {
            let confinement = Confinement::enter(&transaction, &self.confined, prefix)?;
            for migration in &migrations[applied..] {
                transaction.execute_batch(migration)?;
            }
            confinement.check(&transaction)?;
        }
        transaction.execute(
            "INSERT INTO extension_migrations (name, applied) VALUES (?1, ?2) \
             ON CONFLICT (name) DO UPDATE SET applied = excluded.applied",
            params![name, migrations.len()],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// What `read` reads of the tables of the extension `name`, confined as in its space, in a
    /// transaction that keeps nothing: never a write lock taken to read.
    pub fn read_space<T>(
        &self,
        name: &str,
        read: impl FnOnce(&Connection) -> Result<T>,
    ) -> Result<T> {
        let prefix = prefix_of(name)?;
        let transaction = self.connection.unchecked_transaction()?;
        let confinement = Confinement::enter(&transaction, &self.confined, prefix)?;
        let read = read(&transaction);
        // Before the transaction, rolled back outside the confinement.
        drop(confinement);
        drop(transaction);
        read
    }

    /// The space of the extension `name`: one transaction over its own tables, in which its events
    /// are stored with the change they say. Nothing is kept unless it is committed.
    pub fn space(&mut self, name: &str) -> Result<Space<'_>> {
        let prefix = prefix_of(name)?;
        let transaction = self.connection.transaction()?;
        Ok(Space {
            confinement: Confinement::enter(&transaction, &self.confined, prefix)?,
            transaction,
        })
    }
}

/// The prefix of the tables of the extension `name`. A name is letters and digits only: were `a`
/// and `a_b` both names, `ext_a_b_notes` would be in the space of each.
fn prefix_of(name: &str) -> Result<String> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    {
        return Err(Error::Refused(format!(
            "{name:?}: an extension's name is lower-case letters and digits only"
        )));
    }
    Ok(format!("ext_{name}_"))
}

/// What an extension may do while it is confined (`docs/extensions.md`, "Data"): make, change,
/// read and write its own tables, `ext_<name>_…`, and nothing else. The core's tables and another
/// extension's are refused, as are views (SQLite does not check what a view reads when it is made),
/// pragmas, attachments and the end of the transaction.
fn within(prefix: &str, context: &AuthContext<'_>) -> Authorization {
    let own = |name: &str| name.to_ascii_lowercase().starts_with(prefix);
    // Making or dropping a table reads and writes the schema's table, which holds no data and
    // which SQLite never lets a statement write by itself while `writable_schema` is off; pragmas
    // are refused.
    // Renaming one rewrites the temporary schema's table as well.
    let own_or_schema =
        |name: &str| own(name) || name == "sqlite_master" || name == "sqlite_temp_master";
    // Renaming or dropping a table with `AUTOINCREMENT` finds and writes its row of
    // `sqlite_sequence`, which also holds the core's counters: those are checked unchanged before anything is kept
    // (`Confinement::check`).
    let own_or_internal = |name: &str| own_or_schema(name) || name == "sqlite_sequence";
    let allowed = match context.action {
        AuthAction::CreateTable { table_name }
        | AuthAction::DropTable { table_name }
        | AuthAction::AlterTable { table_name, .. } => own(table_name),
        // `ALTER TABLE` checks the table it changed with `pragma_quick_check`.
        AuthAction::Read { table_name, .. } => {
            own_or_internal(table_name) || table_name == "pragma_quick_check"
        }
        AuthAction::Pragma {
            pragma_name: "quick_check",
            pragma_value: Some(table_name),
        } => own(table_name),
        AuthAction::Insert { table_name }
        | AuthAction::Update { table_name, .. }
        | AuthAction::Delete { table_name } => own_or_internal(table_name),
        // The index SQLite makes for a table's own `PRIMARY KEY` or `UNIQUE` is named by it.
        AuthAction::CreateIndex {
            index_name,
            table_name,
        }
        | AuthAction::DropIndex {
            index_name,
            table_name,
        } => {
            own(table_name)
                && (own(index_name)
                    || index_name.starts_with(&format!("sqlite_autoindex_{table_name}_")))
        }
        AuthAction::CreateTrigger {
            trigger_name: name,
            table_name,
        }
        | AuthAction::DropTrigger {
            trigger_name: name,
            table_name,
        } => own(name) && own(table_name),
        // Renaming a table rebuilds its indexes, all of them the space's.
        AuthAction::Reindex { index_name } => own(index_name),
        AuthAction::Select | AuthAction::Function { .. } | AuthAction::Recursive => true,
        _ => false,
    };
    if allowed {
        Authorization::Allow
    } else {
        Authorization::Deny
    }
}

/// Held while the connection is confined to an extension's space; lifted when dropped.
struct Confinement {
    confined: Confined,
    prefix: String,
    /// What is not the extension's, as it was when the core last let go: the schema of every
    /// other table and the counters of `sqlite_sequence`.
    outside: RefCell<Vec<String>>,
}

impl Confinement {
    fn enter(connection: &Connection, confined: &Confined, prefix: String) -> Result<Confinement> {
        // A statement is authorized when it is prepared: none prepared outside the space is kept
        // for it to run.
        connection.flush_prepared_statement_cache();
        let outside = RefCell::new(outside(connection, &prefix)?);
        let confinement = Confinement {
            confined: Arc::clone(confined),
            prefix,
            outside,
        };
        confinement.set(Some(confinement.prefix.clone()));
        Ok(confinement)
    }

    fn set(&self, prefix: Option<String>) {
        *self.confined.lock().unwrap_or_else(PoisonError::into_inner) = prefix;
    }

    /// Refuses what the authorizer cannot see: a table renamed out of the space (it is told the old
    /// name alone), a core counter written, and a table of the space that references one outside
    /// it, which a deletion of the core would cascade into, running its triggers unconfined.
    fn check(&self, connection: &Connection) -> Result<()> {
        self.set(None);
        let found = self.found(connection);
        self.set(Some(self.prefix.clone()));
        found
    }

    fn found(&self, connection: &Connection) -> Result<()> {
        if outside(connection, &self.prefix)? != *self.outside.borrow() {
            return Err(Error::Refused(format!(
                "an extension changed what is not its own: only tables {}… are its",
                self.prefix
            )));
        }
        let references: Vec<(String, String)> = connection
            .prepare(
                "SELECT t.name, f.\"table\" FROM sqlite_master t, pragma_foreign_key_list(t.name) f \
                 WHERE t.type = 'table'",
            )?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let own = |name: &str| name.to_ascii_lowercase().starts_with(&self.prefix);
        if let Some((table, target)) = references
            .iter()
            .find(|(table, target)| own(table) && !own(target))
        {
            return Err(Error::Refused(format!(
                "{table} references {target}: a table of an extension references its own alone"
            )));
        }
        Ok(())
    }

    /// Runs `f` as the core, outside the confinement, once what the extension did is checked; what
    /// the core changes there is the new state to check against, even when `f` fails after a
    /// write, so that it is never taken for the extension's.
    fn lifted<T>(&self, connection: &Connection, f: impl FnOnce() -> Result<T>) -> Result<T> {
        self.check(connection)?;
        self.set(None);
        let result = f();
        let taken = outside(connection, &self.prefix);
        self.set(Some(self.prefix.clone()));
        *self.outside.borrow_mut() = taken?;
        result
    }
}

/// The schema of every table that is not the space's, its indexes and triggers, and the counters
/// of `sqlite_sequence` that are not the space's, read as the core.
fn outside(connection: &Connection, prefix: &str) -> Result<Vec<String>> {
    let rows: Vec<(String, String)> = connection
        .prepare(
            "SELECT tbl_name, type || ' ' || name || ' ' || ifnull(sql, '') FROM sqlite_master \
             UNION ALL SELECT name, 'sequence ' || name || ' ' || seq FROM sqlite_sequence \
             ORDER BY 2",
        )?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows
        .into_iter()
        .filter(|(table, _)| !table.to_ascii_lowercase().starts_with(prefix))
        .map(|(_, row)| row)
        .collect())
}

impl Drop for Confinement {
    fn drop(&mut self) {
        self.set(None);
    }
}

/// An extension's change to its own tables and its events, written together or not at all.
pub struct Space<'a> {
    // Declared first, so dropped first: the rollback runs outside the confinement.
    confinement: Confinement,
    transaction: rusqlite::Transaction<'a>,
}

impl Space<'_> {
    /// Its tables, read and written by the extension alone.
    pub fn tables(&self) -> &Connection {
        &self.transaction
    }

    pub fn append_event(&self, name: &str, data: Value) -> Result<Event> {
        self.confinement.lifted(&self.transaction, || {
            append_event(&self.transaction, name, data)
        })
    }

    pub fn commit(self) -> Result<()> {
        let Space {
            confinement,
            transaction,
        } = self;
        let checked = confinement.check(&transaction);
        // Before the transaction, kept or rolled back outside the confinement.
        drop(confinement);
        checked?;
        Ok(transaction.commit()?)
    }
}

fn append_event(connection: &Connection, name: &str, data: Value) -> Result<Event> {
    let at = now_ms();
    let text = serde_json::to_string(&data)?;
    let seq: u64 = connection.query_row(
        "INSERT INTO events (at, name, data) VALUES (?1, ?2, ?3) RETURNING seq",
        params![at, name, text],
        |row| row.get(0),
    )?;
    Ok(Event {
        seq,
        at,
        name: name.to_owned(),
        data,
    })
}

fn append_agent_event(
    connection: &Connection,
    agent: &str,
    name: &str,
    data: Value,
) -> Result<Event> {
    let event = append_event(connection, name, data)?;
    connection.execute(
        "INSERT INTO agent_events (seq, agent) VALUES (?1, ?2)",
        params![event.seq, agent],
    )?;
    if name == "agent.turn_ended" {
        connection.execute(
            "UPDATE agents SET exchanged_at = ?2 WHERE id = ?1",
            params![agent, event.at],
        )?;
    }
    Ok(event)
}

fn event_row(row: &rusqlite::Row) -> rusqlite::Result<Result<Event>> {
    let (seq, at, name, data): (u64, u64, String, String) =
        (row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?);
    Ok(serde_json::from_str(&data)
        .map(|data| Event {
            seq,
            at,
            name,
            data,
        })
        .map_err(Error::from))
}

fn run_row(row: &rusqlite::Row) -> rusqlite::Result<Result<Run>> {
    let state: String = row.get(5)?;
    let folder: String = row.get(4)?;
    let Some(state) = RunState::parse(&state) else {
        return Ok(Err(Error::Refused(format!(
            "the store holds a run in an unknown state {state}"
        ))));
    };
    Ok(Ok(Run {
        id: row.get(0)?,
        agent: row.get(1)?,
        name: row.get(2)?,
        line: row.get(3)?,
        folder: folder.into(),
        state,
        exit_code: row.get(6)?,
        signal: row.get(7)?,
        started_at: row.get(8)?,
        ended_at: row.get(9)?,
        output: row.get(10)?,
        dismissed: row.get(11)?,
        project: row.get(12)?,
        ready: row
            .get::<_, Option<String>>(13)?
            .and_then(|ready| Readiness::parse(&ready)),
        url: row.get(14)?,
        url_error: row.get(15)?,
        workspace: row.get(16)?,
    }))
}

/// A stored request's JSON columns: arguments, shown, reasons, forwards and says.
type RequestParts = (
    Value,
    Value,
    Vec<String>,
    Vec<Forward>,
    Vec<String>,
    Option<Judgement>,
);

fn request_row(row: &rusqlite::Row) -> rusqlite::Result<Result<Request>> {
    let json = |i| -> rusqlite::Result<Result<Value>> {
        let text: String = row.get(i)?;
        Ok(serde_json::from_str(&text).map_err(Error::from))
    };
    let (state, decision): (String, Option<String>) = (row.get(11)?, row.get(12)?);
    let Some(state) = RequestState::parse(&state) else {
        return Ok(Err(Error::Refused(format!(
            "the store holds a request in an unknown state {state}"
        ))));
    };
    let decision = decision.as_deref().map(Decision::parse);
    if decision == Some(None) {
        return Ok(Err(Error::Refused(
            "the store holds a request with an unknown decision".into(),
        )));
    }
    let kind: String = row.get(15)?;
    let Some(kind) = Kind::parse(&kind) else {
        return Ok(Err(Error::Refused(format!(
            "the store holds a request of an unknown kind {kind}"
        ))));
    };
    let judgement: Option<String> = row.get(17)?;
    let parsed = (|| -> Result<RequestParts> {
        Ok((
            json(4)??,
            json(5)??,
            serde_json::from_value(json(6)??)?,
            serde_json::from_value(json(10)??)?,
            serde_json::from_value(json(16)??)?,
            judgement.as_deref().map(serde_json::from_str).transpose()?,
        ))
    })();
    let (arguments, shown, reasons, forwards, says, judgement) = match parsed {
        Ok(parsed) => parsed,
        Err(error) => return Ok(Err(error)),
    };
    Ok(Ok(Request {
        number: row.get(0)?,
        agent: row.get(1)?,
        call_key: row.get(2)?,
        tool: row.get(3)?,
        arguments,
        shown,
        reasons,
        base: row.get(7)?,
        kind,
        says,
        judgement,
        held_by: row.get(9)?,
        forwards,
        state,
        decision: decision.flatten(),
        created_at: row.get(13)?,
        decided_at: row.get(14)?,
        place: row.get(18)?,
        digest: row.get(19)?,
    }))
}

fn subscription_row(row: &rusqlite::Row) -> rusqlite::Result<Subscription> {
    let json = |i| {
        row.get::<_, String>(i)
            .map(|text| serde_json::from_str(&text).unwrap_or_default())
    };
    Ok(Subscription {
        id: row.get(0)?,
        agent: row.get(1)?,
        source: row.get(2)?,
        target: json(3)?,
        events: serde_json::from_value(json(4)?).unwrap_or_default(),
        from: row.get(5)?,
        seen: json(6)?,
        created_at: row.get(7)?,
        expires_at: row.get(8)?,
    })
}

fn item_row(row: &rusqlite::Row) -> rusqlite::Result<Item> {
    let json = |i| {
        row.get::<_, String>(i)
            .map(|text| serde_json::from_str::<Value>(&text).unwrap_or_default())
    };
    let names = |i| json(i).map(|value| serde_json::from_value(value).unwrap_or_default());
    Ok(Item {
        project: row.get(0)?,
        kind: row.get(1)?,
        source: row.get(2)?,
        repository: row.get(3)?,
        key: row.get(4)?,
        title: row.get(5)?,
        state: row.get(6)?,
        author: row.get(7)?,
        assignees: names(8)?,
        labels: names(9)?,
        url: row.get(10)?,
        updated_at: row.get(11)?,
        seen_at: row.get(12)?,
        mine: row.get(13)?,
        details: json(14)?,
        checks: Vec::new(),
    })
}

fn delivery_row(row: &rusqlite::Row) -> rusqlite::Result<Delivery> {
    Ok(Delivery {
        id: row.get(0)?,
        agent: row.get(1)?,
        kind: row.get(2)?,
        body: row.get(3)?,
        urgency: row.get(4)?,
        state: row.get(5)?,
        created_at: row.get(6)?,
        sent_at: row.get(7)?,
        images: serde_json::from_str(&row.get::<_, String>(8)?).unwrap_or_default(),
        typed: row.get(9)?,
        commands: serde_json::from_str(&row.get::<_, String>(10)?).unwrap_or_default(),
    })
}

fn agent_row(row: &rusqlite::Row) -> rusqlite::Result<Result<Agent>> {
    let text = |i| row.get::<_, Option<String>>(i);
    let state: String = row.get(6)?;
    let folder: String = row.get(5)?;
    let tools: String = row.get(21)?;
    let Some(state) = AgentState::parse(&state) else {
        return Ok(Err(Error::Refused(format!(
            "the store holds an agent in an unknown state {state}"
        ))));
    };
    let tools = match serde_json::from_str(&tools) {
        Ok(tools) => tools,
        Err(error) => return Ok(Err(error.into())),
    };
    Ok(Ok(Agent {
        id: row.get(0)?,
        provider: row.get(1)?,
        role: row.get(2)?,
        project: row.get(3)?,
        workspace: text(4)?,
        folder: folder.into(),
        state,
        state_reason: text(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
        active_at: row.get(10)?,
        ended_at: row.get(11)?,
        chosen: Choices {
            model: text(12)?,
            effort: text(13)?,
            mode: text(14)?,
        },
        taken: Choices {
            model: text(15)?,
            effort: text(16)?,
            mode: text(17)?,
        },
        native_id: text(18)?,
        agent_version: text(19)?,
        instructions: row.get(20)?,
        tools,
        parent: text(22)?,
        depth: row.get(23)?,
        epoch: row.get(24)?,
        title: text(25)?,
        brief: text(26)?,
        reported_at: row.get(27)?,
        quiet_turns: row.get(28)?,
        waiting_on_answer: row.get(29)?,
        reopens: row.get(30)?,
        images: row.get(31)?,
        // A mode the store does not know is the safer one.
        gate_mode: GateMode::parse(&row.get::<_, String>(32)?).unwrap_or(GateMode::Ask),
        launched_by: text(33)?,
        pending: Choices {
            model: text(34)?,
            effort: text(35)?,
            mode: None,
        },
        exchanged_at: row.get(36)?,
        led: row.get(37)?,
        detached_from: text(38)?,
        titled_by: text(39)?,
    }))
}

/// How far the clock was moved ahead, for tests only (`advance_clock`).
static CLOCK_AHEAD_MS: AtomicU64 = AtomicU64::new(0);

/// Milliseconds since the Unix epoch, on the clock every time of Flottille is read from.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
        + CLOCK_AHEAD_MS.load(Ordering::SeqCst)
}

/// Moves the clock of this process ahead, so that a test sees minutes pass without waiting for
/// them: the daemon calls it only when `FLOTTILLE_TEST_CLOCK` is set (`docs/agents.md`).
pub fn advance_clock(ms: u64) {
    CLOCK_AHEAD_MS.fetch_add(ms, Ordering::SeqCst);
}

fn path_text(path: &Path) -> Result<&str> {
    path.to_str().ok_or_else(|| {
        Error::Refused(format!(
            "{} is not valid UTF-8: Flottille cannot keep it",
            path.display()
        ))
    })
}

fn migrate(connection: &mut Connection) -> Result<()> {
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let mut applied: usize =
        transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
    // The alpha builds of the extensions made their bookkeeping, `extension_schemas`, as the last
    // numbered migration of their day: that number was the core's next, whose migration such a
    // store never ran. The table's name says such a store, and its count is moved over.
    let alpha = transaction
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'extension_schemas'",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if alpha {
        applied -= 1;
    }
    if applied > MIGRATIONS.len() {
        return Err(Error::Refused(format!(
            "the store is at schema {applied}, made by a newer Flottille than this one (schema {})",
            MIGRATIONS.len()
        )));
    }
    for migration in &MIGRATIONS[applied..] {
        transaction.execute_batch(migration)?;
    }
    transaction.execute_batch(EXTENSION_MIGRATIONS)?;
    if alpha {
        transaction.execute_batch(
            "INSERT INTO extension_migrations (name, applied) \
                 SELECT name, applied FROM extension_schemas;
             DROP TABLE extension_schemas;",
        )?;
    }
    transaction.pragma_update(None, "user_version", MIGRATIONS.len())?;
    transaction.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval;
    use crate::workspace::{Base, Worktree};
    use serde_json::json;

    #[test]
    fn a_new_store_is_brought_to_the_last_schema() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.schema_version().unwrap(), MIGRATIONS.len());
    }

    #[test]
    fn reopening_a_store_applies_nothing_twice() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flottille.db");
        Store::open(&path)
            .unwrap()
            .append_event("test.one", json!({}))
            .unwrap();

        let store = Store::open(&path).unwrap();

        assert_eq!(store.schema_version().unwrap(), MIGRATIONS.len());
        assert_eq!(store.last_seq().unwrap(), 1);
    }

    #[test]
    fn a_store_of_the_first_schema_keeps_its_events_and_gains_the_workspaces() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flottille.db");
        let old = Connection::open(&path).unwrap();
        old.execute_batch(MIGRATIONS[0]).unwrap();
        old.pragma_update(None, "user_version", 1).unwrap();
        append_event(&old, "test.one", json!({})).unwrap();
        drop(old);

        let store = Store::open(&path).unwrap();

        assert_eq!(store.schema_version().unwrap(), MIGRATIONS.len());
        assert_eq!(store.last_seq().unwrap(), 1);
        assert!(store.workspaces(None).unwrap().is_empty());
    }

    #[test]
    fn a_store_from_a_newer_flottille_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flottille.db");
        Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", MIGRATIONS.len() + 1)
            .unwrap();

        let error = Store::open(&path).err().unwrap();

        assert!(error.to_string().contains("newer Flottille"), "{error}");
    }

    fn a_post(kind: &str, comment_id: Option<&str>, fingerprint: Option<&str>) -> Post {
        Post {
            project: "app".into(),
            source: "github".into(),
            repository: "acme/app".into(),
            key: "12".into(),
            kind: kind.into(),
            comment_id: comment_id.map(str::to_owned),
            fingerprint: fingerprint.map(str::to_owned),
            posted_at: 7,
        }
    }

    #[test]
    fn the_posts_of_an_item_are_kept_across_a_restart_and_a_body_is_kept_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flottille.db");
        let store = Store::open(&path).unwrap();
        let comment = a_post("comment", Some("issuecomment-501"), None);
        let by_text = a_post("comment", None, Some("abc"));
        store.add_post(&comment).unwrap();
        store.add_post(&by_text).unwrap();
        store.add_post(&a_post("body", None, Some("one"))).unwrap();
        store.add_post(&a_post("body", None, Some("two"))).unwrap();
        drop(store);

        let store = Store::open(&path).unwrap();

        let posts = store.posts("app", "github", "acme/app", "12").unwrap();
        assert_eq!(posts.len(), 3, "{posts:?}");
        assert_eq!(posts[0], comment);
        assert_eq!(posts[1], by_text);
        assert_eq!(posts[2].fingerprint.as_deref(), Some("two"));
        assert!(
            store
                .posts("app", "github", "acme/app", "13")
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .posts("other", "github", "acme/app", "12")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_store_before_the_posts_gains_their_table_and_keeps_its_events() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flottille.db");
        let old = Connection::open(&path).unwrap();
        let previous = MIGRATIONS
            .iter()
            .position(|m| m.contains("CREATE TABLE posts"))
            .unwrap();
        for migration in &MIGRATIONS[..previous] {
            old.execute_batch(migration).unwrap();
        }
        old.pragma_update(None, "user_version", previous).unwrap();
        append_event(&old, "test.one", json!({})).unwrap();
        drop(old);

        let store = Store::open(&path).unwrap();

        assert_eq!(store.schema_version().unwrap(), MIGRATIONS.len());
        assert_eq!(store.last_seq().unwrap(), 1);
        store.add_post(&a_post("comment", Some("1"), None)).unwrap();
    }

    fn a_workspace() -> Workspace {
        Workspace {
            id: 0,
            project: "app".into(),
            name: "login".into(),
            branch: "app/login".into(),
            folder: "/data/workspaces/app/login".into(),
            project_folder: "/work/app".into(),
            created_at: 1_792_843_730_808,
            variables: [("STORE".to_owned(), "{workspace.path}/.store".to_owned())].into(),
            repositories: vec![Worktree {
                path: ".".into(),
                base: Base {
                    branch: "main".into(),
                    remote: "origin".into(),
                    urls: vec!["https://git.example.com/acme/app.git".into()],
                    reference: "refs/remotes/origin/main".into(),
                    commit: "0123abcd".into(),
                    fetched_now: true,
                    fetched_at: Some(1_792_843_730_000),
                    note: None,
                },
                resumed: false,
                resumed_branch: None,
            }],
            steps: vec![
                Step {
                    position: 0,
                    repository: Some(".".into()),
                    action: Action::Worktree,
                    state: StepState::Pending,
                    failure: None,
                    started_at: None,
                    ended_at: None,
                },
                Step {
                    position: 1,
                    repository: None,
                    action: Action::Run {
                        line: "make".into(),
                        task: None,
                        time_limit: 1800,
                    },
                    state: StepState::Pending,
                    failure: None,
                    started_at: None,
                    ended_at: None,
                },
            ],
            ticket: None,
        }
    }

    #[test]
    fn a_workspace_is_written_with_its_event_and_read_back() {
        let mut store = Store::open_in_memory().unwrap();
        let workspace = a_workspace();

        let (id, event) = store
            .create_workspace(&workspace, ("workspace.created", json!({"name": "login"})))
            .unwrap();

        assert_eq!(event.name, "workspace.created");
        let read = store.workspace("app", "login").unwrap().unwrap();
        assert_eq!(read, Workspace { id, ..workspace });
        assert_eq!(
            store.workspaces(Some("app")).unwrap(),
            std::slice::from_ref(&read)
        );
        assert_eq!(store.workspaces(None).unwrap(), [read]);
        assert!(store.workspaces(Some("other")).unwrap().is_empty());
        assert_eq!(store.workspace("app", "other").unwrap(), None);
    }

    #[test]
    fn a_workspace_s_branch_follows_with_its_event() {
        let mut store = Store::open_in_memory().unwrap();
        let (id, _) = store
            .create_workspace(&a_workspace(), ("workspace.created", json!({})))
            .unwrap();

        let event = store
            .set_workspace_branch(
                id,
                "feature/x",
                ("workspace.changed", json!({"branch": "feature/x"})),
            )
            .unwrap();

        assert_eq!(event.name, "workspace.changed");
        assert_eq!(store.workspace_by_id(id).unwrap().branch, "feature/x");
    }

    #[test]
    fn a_workspace_keeps_its_ticket_and_forgets_it_when_unlinked() {
        let mut store = Store::open_in_memory().unwrap();
        let (id, _) = store
            .create_workspace(
                &Workspace {
                    ticket: Some("#12".into()),
                    ..a_workspace()
                },
                ("workspace.created", json!({})),
            )
            .unwrap();
        assert_eq!(
            store.workspace_by_id(id).unwrap().ticket.as_deref(),
            Some("#12")
        );

        let event = store
            .set_ticket(
                id,
                Some("ABC-1"),
                ("workspace.ticket", json!({"ticket": "ABC-1"})),
            )
            .unwrap();
        assert_eq!(event.name, "workspace.ticket");
        assert_eq!(
            store.workspace_by_id(id).unwrap().ticket.as_deref(),
            Some("ABC-1")
        );
        store
            .set_ticket(id, None, ("workspace.ticket", json!({"ticket": null})))
            .unwrap();
        assert_eq!(store.workspace_by_id(id).unwrap().ticket, None);
    }

    #[test]
    fn a_workspace_keeps_when_its_preparation_ended() {
        let mut store = Store::open_in_memory().unwrap();
        let (id, _) = store
            .create_workspace(&a_workspace(), ("workspace.created", json!({})))
            .unwrap();
        assert_eq!(store.workspace_ready_at(id).unwrap(), None);

        let event = store
            .workspace_ready(id, json!({"project": "app", "workspace": "login"}))
            .unwrap();

        assert_eq!(event.name, "workspace.ready");
        assert_eq!(store.workspace_ready_at(id).unwrap(), Some(event.at));
    }

    #[test]
    fn a_store_before_the_merged_tools_gives_its_agents_commands_status_and_agent_status() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flottille.db");
        let old = Connection::open(&path).unwrap();
        let previous = MIGRATIONS
            .iter()
            .position(|m| m.contains("commands_status"))
            .unwrap();
        for migration in &MIGRATIONS[..previous] {
            old.execute_batch(migration).unwrap();
        }
        old.pragma_update(None, "user_version", previous).unwrap();
        for (id, tools) in [
            // A worker (Claude's names), a lead (opencode's), a stop with no output, an agent
            // already on the new names.
            (
                "000000000001",
                r#"["mcp__flottille__fs_read","mcp__flottille__commands_run","mcp__flottille__commands_output","mcp__flottille__commands_stop","mcp__flottille__report"]"#,
            ),
            (
                "000000000002",
                r#"["flottille_agent_launch","flottille_agent_read","flottille_agent_status","flottille_commands_output","flottille_report"]"#,
            ),
            (
                "000000000003",
                r#"["mcp__flottille__agent_read","mcp__flottille__commands_stop"]"#,
            ),
            (
                "000000000004",
                r#"["mcp__flottille__agent_status","mcp__flottille__commands_status"]"#,
            ),
        ] {
            old.execute(
                "INSERT INTO agents (id, provider, role, project, workspace, folder, state, \
                 created_at, updated_at, instructions, tools) VALUES (?1, 'claude', 'lead', \
                 'app', NULL, '/work/app', 'idle', 1, 1, 'text', ?2)",
                params![id, tools],
            )
            .unwrap();
        }
        drop(old);

        let store = Store::open(&path).unwrap();

        // Renamed again since by tasks (#131): `run` and `run_status`.
        let tools = |id: &str| store.agent(id).unwrap().unwrap().tools;
        assert_eq!(
            tools("000000000001"),
            [
                "mcp__flottille__fs_read",
                "mcp__flottille__run",
                "mcp__flottille__run_status",
                "mcp__flottille__report"
            ]
        );
        assert_eq!(
            tools("000000000002"),
            [
                "flottille_agent_launch",
                "flottille_agent_status",
                "flottille_run_status",
                "flottille_report"
            ]
        );
        assert_eq!(
            tools("000000000003"),
            ["mcp__flottille__agent_status", "mcp__flottille__run_status"]
        );
        assert_eq!(
            tools("000000000004"),
            ["mcp__flottille__agent_status", "mcp__flottille__run_status"]
        );
    }

    #[test]
    fn the_migration_removes_grants_and_families_and_starts_every_session_in_auto() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flottille.db");
        let old = Connection::open(&path).unwrap();
        let previous = MIGRATIONS
            .iter()
            .position(|m| m.contains("ADD COLUMN gate_mode"))
            .unwrap();
        for migration in &MIGRATIONS[..previous] {
            old.execute_batch(migration).unwrap();
        }
        old.pragma_update(None, "user_version", previous).unwrap();
        drop(old);
        let agent = an_agent();
        // The old schema, with an agent, a grant for its tree and a run with grants.
        let old = Connection::open(&path).unwrap();
        old.execute_batch(&format!(
            "INSERT INTO agents (id, provider, role, project, folder, state, created_at, \
             updated_at, instructions, tools) VALUES ('{}', 'claude', 'lead', 'app', '/w', \
             'idle', 1, 1, 'i', '[]'); \
             INSERT INTO approval_requests (number, agent, tool, arguments, shown, reasons, \
             sensitive, identity, held_by, forwards, state, created_at, decision, kind) VALUES \
             (1, '{0}', 'commands_run', '{{}}', '{{}}', '[]', 0, 'x', 'user', '[]', 'allowed', 1, \
             'tree', 'catalogue'); \
             INSERT INTO approval_grants (tree, identity, request, created_at) VALUES \
             ('{0}', 'x', 1, 1); \
             INSERT INTO procedure_runs (agent, project, workspace, procedure, grants, children, \
             started_at) VALUES ('{0}', 'app', 'login', 'ship', '{{\"push\":\"allow\"}}', 1, 1);",
            agent.id
        ))
        .unwrap();
        drop(old);

        let store = Store::open(&path).unwrap();

        assert_eq!(store.schema_version().unwrap(), MIGRATIONS.len());
        let tables: u64 = store
            .connection
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = 'approval_grants'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tables, 0);
        assert_eq!(
            store.agent(&agent.id).unwrap().unwrap().gate_mode,
            crate::agent::GateMode::Auto
        );
        let request = store.request(1).unwrap().unwrap();
        assert_eq!(
            (request.kind, request.decision),
            (approval::Kind::Call, Some(Decision::Once))
        );
    }

    #[test]
    fn a_store_before_the_tickets_keeps_its_workspaces_with_no_ticket() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flottille.db");
        let old = Connection::open(&path).unwrap();
        let previous = MIGRATIONS
            .iter()
            .position(|m| m.contains("ADD COLUMN ticket"))
            .unwrap();
        for migration in &MIGRATIONS[..previous] {
            old.execute_batch(migration).unwrap();
        }
        old.pragma_update(None, "user_version", previous).unwrap();
        old.execute_batch(
            "INSERT INTO workspaces (project, name, branch, folder, project_folder, created_at, \
             variables, repositories) VALUES ('app', 'login', 'app/login', '/w', '/p', 100, '{}', '[]');",
        )
        .unwrap();
        drop(old);

        let store = Store::open(&path).unwrap();

        assert_eq!(store.schema_version().unwrap(), MIGRATIONS.len());
        assert_eq!(
            store.workspace("app", "login").unwrap().unwrap().ticket,
            None
        );
    }

    #[test]
    fn a_store_of_the_previous_schema_takes_when_each_workspace_was_ready_from_its_events() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flottille.db");
        let old = Connection::open(&path).unwrap();
        // The schema just before the migration that keeps `ready_at`, wherever it sits.
        let previous = MIGRATIONS
            .iter()
            .position(|m| m.contains("ADD COLUMN ready_at"))
            .unwrap();
        for migration in &MIGRATIONS[..previous] {
            old.execute_batch(migration).unwrap();
        }
        old.pragma_update(None, "user_version", previous).unwrap();
        old.execute_batch(
            "INSERT INTO workspaces (project, name, branch, folder, project_folder, created_at, \
             variables, repositories) VALUES ('app', 'login', 'app/login', '/w', '/p', 100, '{}', '[]'), \
             ('app', 'docs', 'app/docs', '/d', '/p', 100, '{}', '[]');
             INSERT INTO events (at, name, data) VALUES \
             (50, 'workspace.ready', '{\"project\":\"app\",\"workspace\":\"login\"}'), \
             (150, 'workspace.ready', '{\"project\":\"app\",\"workspace\":\"login\"}'), \
             (160, 'workspace.ready', '{\"project\":\"other\",\"workspace\":\"login\"}');",
        )
        .unwrap();
        drop(old);

        let store = Store::open(&path).unwrap();

        assert_eq!(store.schema_version().unwrap(), MIGRATIONS.len());
        let id = |name: &str| store.workspace("app", name).unwrap().unwrap().id;
        assert_eq!(store.workspace_ready_at(id("login")).unwrap(), Some(150));
        assert_eq!(
            store.workspace_ready_at(id("docs")).unwrap(),
            None,
            "never ready"
        );
    }

    #[test]
    fn a_second_workspace_of_the_same_name_is_refused() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .create_workspace(&a_workspace(), ("workspace.created", json!({})))
            .unwrap();

        let error = store
            .create_workspace(&a_workspace(), ("workspace.created", json!({})))
            .unwrap_err();

        assert_eq!(
            error.to_string(),
            "project app already has a workspace named login"
        );
        assert_eq!(store.last_seq().unwrap(), 1);
    }

    #[test]
    fn a_step_keeps_when_it_started_and_ended_and_forgets_it_when_it_goes_back_to_pending() {
        let mut store = Store::open_in_memory().unwrap();
        let (id, _) = store
            .create_workspace(&a_workspace(), ("workspace.created", json!({})))
            .unwrap();
        let started = store
            .set_step(
                id,
                0,
                StepState::Running,
                None,
                ("workspace.step_started", json!({})),
            )
            .unwrap();
        let running = store.workspace_by_id(id).unwrap();
        let done = store
            .set_step(
                id,
                0,
                StepState::Done,
                None,
                ("workspace.step_done", json!({})),
            )
            .unwrap();
        let ended = store.workspace_by_id(id).unwrap();
        store
            .reset_steps(id, &[0], ("workspace.resumed", json!({})))
            .unwrap();
        let reset = store.workspace_by_id(id).unwrap();

        assert_eq!(running.steps[0].started_at, Some(started.at));
        assert_eq!(running.steps[0].ended_at, None);
        assert_eq!(ended.steps[0].started_at, Some(started.at));
        assert_eq!(ended.steps[0].ended_at, Some(done.at));
        assert_eq!(
            ended.steps[1].started_at, None,
            "a step not started has no time"
        );
        assert_eq!(
            (reset.steps[0].started_at, reset.steps[0].ended_at),
            (None, None)
        );
    }

    #[test]
    fn a_step_changes_state_with_its_event_and_goes_back_to_pending() {
        let mut store = Store::open_in_memory().unwrap();
        let (id, _) = store
            .create_workspace(&a_workspace(), ("workspace.created", json!({})))
            .unwrap();
        let failure = Failure {
            doing: "make (in .)".into(),
            output: "make: *** no rule".into(),
        };

        store
            .set_step(
                id,
                0,
                StepState::Done,
                None,
                ("workspace.step_done", json!({})),
            )
            .unwrap();
        store
            .set_step(
                id,
                1,
                StepState::Failed,
                Some(&failure),
                ("workspace.step_failed", json!({})),
            )
            .unwrap();
        let failed = store.workspace_by_id(id).unwrap();
        store
            .reset_steps(id, &[1], ("workspace.resumed", json!({})))
            .unwrap();
        let resumed = store.workspace_by_id(id).unwrap();

        assert_eq!(failed.state(), crate::workspace::State::Failed);
        assert_eq!(failed.steps[0].state, StepState::Done);
        assert_eq!(failed.steps[1].failure, Some(failure));
        assert_eq!(resumed.steps[1].state, StepState::Pending);
        assert_eq!(resumed.steps[1].failure, None);
        assert_eq!(resumed.state(), crate::workspace::State::Preparing);
        let names: Vec<_> = store
            .events_after(1, 10)
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(
            names,
            [
                "workspace.step_done",
                "workspace.step_failed",
                "workspace.resumed"
            ]
        );
    }

    #[test]
    fn a_removed_workspace_takes_its_steps_with_it() {
        let mut store = Store::open_in_memory().unwrap();
        let (id, _) = store
            .create_workspace(&a_workspace(), ("workspace.created", json!({})))
            .unwrap();

        store
            .remove_workspace(id, ("workspace.removed", json!({})))
            .unwrap();

        assert_eq!(store.workspace("app", "login").unwrap(), None);
        let steps: i64 = store
            .connection
            .query_row("SELECT count(*) FROM workspace_steps", [], |row| row.get(0))
            .unwrap();
        assert_eq!(steps, 0);
    }

    /// An agent written as a store of the agents' first schema (3) did.
    fn insert_old_agent(path: &Path) {
        let old = Connection::open(path).unwrap();
        old.execute(
            "INSERT INTO agents (id, provider, role, project, workspace, folder, state, \
             created_at, updated_at, instructions, tools) VALUES ('0123456789ab', 'claude', \
             'lead', 'app', NULL, '/work/app', 'idle', 1, 1, 'text', '[]')",
            [],
        )
        .unwrap();
    }

    fn an_agent() -> Agent {
        Agent {
            id: "0123456789ab".into(),
            provider: "claude".into(),
            role: "lead".into(),
            project: "app".into(),
            workspace: Some("login".into()),
            folder: "/data/workspaces/app/login".into(),
            state: AgentState::Starting,
            state_reason: None,
            created_at: 1_792_843_730_808,
            updated_at: 1_792_843_730_808,
            active_at: None,
            exchanged_at: None,
            ended_at: None,
            chosen: Choices {
                model: Some("haiku".into()),
                ..Choices::default()
            },
            taken: Choices::default(),
            pending: Choices::default(),
            native_id: None,
            agent_version: None,
            instructions: "shared\n\n__SYSTEM_PROMPT_DYNAMIC_BOUNDARY__\n\nsession".into(),
            tools: vec!["mcp__flottille__fs_read".into()],
            parent: None,
            depth: 0,
            epoch: 0,
            title: None,
            brief: Some("hello".into()),
            reported_at: None,
            quiet_turns: 0,
            waiting_on_answer: false,
            reopens: false,
            images: None,
            gate_mode: GateMode::Auto,
            launched_by: None,
            led: false,
            detached_from: None,
            titled_by: None,
        }
    }

    #[test]
    fn an_agent_is_written_with_its_event_and_read_back() {
        let mut store = Store::open_in_memory().unwrap();
        let agent = an_agent();

        let event = store
            .create_agent(&agent, ("agent.created", json!({"agent": agent.id})))
            .unwrap();

        assert_eq!(store.agent(&agent.id).unwrap(), Some(agent.clone()));
        assert_eq!(store.agents().unwrap(), std::slice::from_ref(&agent));
        assert_eq!(store.agent("other").unwrap(), None);
        assert_eq!(store.agent_events(&agent.id, 0, 10).unwrap(), [event]);
    }

    #[test]
    fn the_sweep_reads_the_agents_still_running_and_the_sessions_kept_not_the_whole_history() {
        let mut store = Store::open_in_memory().unwrap();
        let made = |id: &str, parent: Option<&str>, reopens: bool, native: Option<&str>| Agent {
            id: id.into(),
            parent: parent.map(str::to_owned),
            reopens,
            native_id: native.map(str::to_owned),
            ..an_agent()
        };
        let agents = [
            made("running00000", None, true, Some("s1")),
            made("kept00000000", None, true, Some("s2")),
            made("forgotten000", None, false, Some("s3")),
            made("nosession000", None, true, None),
            made("worker000000", Some("kept00000000"), true, Some("s4")),
            made("failed000000", None, true, Some("s5")),
        ];
        for agent in &agents {
            store
                .create_agent(agent, ("agent.created", json!({})))
                .unwrap();
        }
        for (id, state) in [
            ("kept00000000", AgentState::Ended),
            ("forgotten000", AgentState::Ended),
            ("nosession000", AgentState::Ended),
            ("worker000000", AgentState::Ended),
            ("failed000000", AgentState::Failed),
        ] {
            let change = AgentChange {
                state: Some(state),
                ..AgentChange::default()
            };
            store.change_agent(id, change, None).unwrap();
        }
        let ids = |agents: Vec<Agent>| agents.into_iter().map(|a| a.id).collect::<Vec<_>>();

        assert_eq!(ids(store.live_agents().unwrap()), ["running00000"]);
        // A failed root keeps its session too: it can be resumed (#631).
        assert_eq!(
            ids(store.kept_sessions().unwrap()),
            ["failed000000", "kept00000000"]
        );
    }

    #[test]
    fn a_leave_is_granted_only_while_both_sessions_are_live() {
        let mut store = Store::open_in_memory().unwrap();
        let writer = an_agent();
        let mut session = an_agent();
        session.id = "session00000".into();
        for agent in [&writer, &session] {
            store
                .create_agent(agent, ("agent.created", json!({})))
                .unwrap();
        }
        let ended = AgentChange {
            state: Some(AgentState::Ended),
            ..AgentChange::default()
        };
        store.change_agent(&session.id, ended, None).unwrap();

        let granted = store.grant_leave(&writer.id, &session.id, 7).unwrap();

        assert_eq!(granted, (None, None));
        store.resume_agent(&session.id).unwrap().unwrap();
        assert!(!store.has_leave(&writer.id, &session.id).unwrap());
        let (leave, event) = store.grant_leave(&writer.id, &session.id, 8).unwrap();
        assert_eq!(leave.map(|l| l.request), Some(8));
        assert_eq!(event.unwrap().name, "leave.granted");
    }

    #[test]
    fn an_ended_or_failed_agent_is_resumed_on_its_session() {
        let mut store = Store::open_in_memory().unwrap();
        let mut agent = an_agent();
        agent.native_id = Some("s1".into());
        let mut failed = an_agent();
        failed.id = "failed000000".into();
        for agent in [&agent, &failed] {
            store
                .create_agent(agent, ("agent.created", json!({})))
                .unwrap();
        }
        assert_eq!(store.resume_agent(&agent.id).unwrap(), None, "not ended");
        for (id, state) in [
            (&agent.id, AgentState::Ended),
            (&failed.id, AgentState::Failed),
        ] {
            let change = AgentChange {
                state: Some(state),
                state_reason: Some("stopped by the user".into()),
                ..AgentChange::default()
            };
            store.change_agent(id, change, None).unwrap();
        }

        let (resumed, event) = store.resume_agent(&agent.id).unwrap().unwrap();

        assert_eq!(resumed.state, AgentState::Idle);
        assert_eq!((resumed.state_reason, resumed.ended_at), (None, None));
        assert_eq!(resumed.native_id.as_deref(), Some("s1"));
        assert_eq!(resumed.epoch, agent.epoch);
        assert_eq!(event.name, "agent.resumed");
        // A failed root too (#631): its session is rarely what failed.
        let (resumed, _) = store.resume_agent(&failed.id).unwrap().unwrap();
        assert_eq!(resumed.state, AgentState::Idle);
        assert_eq!(store.resume_agent(&failed.id).unwrap(), None, "once");
    }

    #[test]
    fn a_pending_change_of_options_is_kept_then_chosen_and_forgotten() {
        let mut store = Store::open_in_memory().unwrap();
        let agent = an_agent();
        store
            .create_agent(&agent, ("agent.created", json!({})))
            .unwrap();
        let pending = Choices {
            model: Some("opus".into()),
            effort: Some("high".into()),
            mode: None,
        };

        let (waiting, event) = store
            .change_agent(
                &agent.id,
                AgentChange {
                    pending: Some(pending.clone()),
                    ..AgentChange::default()
                },
                Some(("agent.options_pending", json!({}))),
            )
            .unwrap();

        assert_eq!(waiting.pending, pending);
        assert_eq!(waiting.chosen.model.as_deref(), Some("haiku"));
        assert_eq!(event.unwrap().name, "agent.options_pending");
        let (taken, _) = store
            .change_agent(
                &agent.id,
                AgentChange {
                    chosen: Some(pending.clone()),
                    pending: Some(Choices::default()),
                    ..AgentChange::default()
                },
                None,
            )
            .unwrap();
        assert_eq!(taken.pending, Choices::default());
        assert_eq!(taken.chosen.model.as_deref(), Some("opus"));
        assert_eq!(taken.chosen.effort.as_deref(), Some("high"));
    }

    #[test]
    fn an_agent_changes_and_a_final_state_is_never_left() {
        let mut store = Store::open_in_memory().unwrap();
        let agent = an_agent();
        store
            .create_agent(&agent, ("agent.created", json!({})))
            .unwrap();
        let taken = Choices {
            model: Some("haiku-4".into()),
            ..Choices::default()
        };

        let (opened, _) = store
            .change_agent(
                &agent.id,
                AgentChange {
                    state: Some(AgentState::Idle),
                    native_id: Some("session-1".into()),
                    agent_version: Some("fake 1.0".into()),
                    taken: Some(taken.clone()),
                    ..AgentChange::default()
                },
                None,
            )
            .unwrap();
        let (failed, event) = store
            .change_agent(
                &agent.id,
                AgentChange {
                    state: Some(AgentState::Failed),
                    state_reason: Some("its process died".into()),
                    ..AgentChange::default()
                },
                Some(("agent.died", json!({}))),
            )
            .unwrap();
        let (still, _) = store
            .change_agent(
                &agent.id,
                AgentChange {
                    state: Some(AgentState::Idle),
                    ..AgentChange::default()
                },
                None,
            )
            .unwrap();

        assert_eq!(opened.state, AgentState::Idle);
        assert_eq!(opened.native_id.as_deref(), Some("session-1"));
        assert_eq!(opened.taken, taken);
        assert_eq!(failed.state, AgentState::Failed);
        assert!(failed.ended_at.is_some());
        assert_eq!(event.unwrap().name, "agent.died");
        assert_eq!(still.state, AgentState::Failed);
        assert_eq!(still.state_reason.as_deref(), Some("its process died"));
    }

    #[test]
    fn only_an_event_of_activity_moves_the_agent_active_time() {
        let mut store = Store::open_in_memory().unwrap();
        let agent = an_agent();
        store
            .create_agent(&agent, ("agent.created", json!({})))
            .unwrap();

        let replayed = store
            .append_agent_event(&agent.id, "agent.message", json!({"replay": true}), false)
            .unwrap();
        let before = store.agent(&agent.id).unwrap().unwrap().active_at;
        let live = store
            .append_agent_event(&agent.id, "agent.message", json!({"replay": false}), true)
            .unwrap();
        store.append_event("daemon.started", json!({})).unwrap();

        assert_eq!(before, None);
        assert_eq!(
            store.agent(&agent.id).unwrap().unwrap().active_at,
            Some(live.at)
        );
        let names: Vec<u64> = store
            .agent_events(&agent.id, replayed.seq - 1, 10)
            .unwrap()
            .into_iter()
            .map(|e| e.seq)
            .collect();
        assert_eq!(names, [replayed.seq, live.seq]);
    }

    #[test]
    fn the_last_exchange_moves_with_the_user_s_message_and_the_end_of_a_turn_only() {
        let mut store = Store::open_in_memory().unwrap();
        let agent = an_agent();
        store
            .create_agent(&agent, ("agent.created", json!({})))
            .unwrap();
        let exchanged = |store: &Store| store.agent(&agent.id).unwrap().unwrap().exchanged_at;
        assert_eq!(exchanged(&store), None, "nothing exchanged yet");

        store
            .append_agent_event(&agent.id, "agent.message", json!({"text": "hi"}), true)
            .unwrap();
        store
            .append_agent_event(&agent.id, "agent.tool_call", json!({}), true)
            .unwrap();
        assert_eq!(
            exchanged(&store),
            None,
            "work done during a turn is no exchange"
        );

        let mut brief = a_delivery("brief-1", "brief");
        brief.agent = agent.id.clone();
        store.deliver(&brief).unwrap();
        store
            .move_deliveries(
                &agent.id,
                &[brief.id.clone()],
                "queued",
                "sending",
                "prompt",
            )
            .unwrap();
        assert_eq!(exchanged(&store), None, "a brief is not the user's message");

        let mut note = a_delivery("user-1", "user");
        note.agent = agent.id.clone();
        store.deliver(&note).unwrap();
        store
            .move_deliveries(&agent.id, &[note.id.clone()], "queued", "sending", "prompt")
            .unwrap();
        let written = exchanged(&store).expect("the user's message is an exchange");

        let ended = store
            .append_agent_event(&agent.id, "agent.turn_ended", json!({}), false)
            .unwrap();
        assert_eq!(exchanged(&store), Some(ended.at.max(written)));
    }

    #[test]
    fn processes_are_recorded_with_their_start_time_and_forgotten() {
        let store = Store::open_in_memory().unwrap();
        store.add_process(4242, 77, "a").unwrap();
        store.add_process(4343, 78, "b").unwrap();
        store.remove_process(4242).unwrap();
        assert_eq!(store.processes().unwrap(), [(4343, 78)]);
    }

    #[test]
    fn a_store_of_the_workspaces_schema_keeps_them_and_gains_the_agents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flottille.db");
        let old = Connection::open(&path).unwrap();
        old.execute_batch(MIGRATIONS[0]).unwrap();
        old.execute_batch(MIGRATIONS[1]).unwrap();
        old.pragma_update(None, "user_version", 2).unwrap();
        old.execute_batch(
            "INSERT INTO workspaces (project, name, branch, folder, project_folder, created_at, \
             variables, repositories) VALUES ('app', 'login', 'app/login', '/w', '/p', 1, '{}', '[]')",
        )
        .unwrap();
        drop(old);

        let store = Store::open(&path).unwrap();

        assert_eq!(store.schema_version().unwrap(), MIGRATIONS.len());
        assert_eq!(store.workspaces(None).unwrap().len(), 1);
        assert!(store.agents().unwrap().is_empty());
    }

    #[test]
    fn the_migration_of_skills_and_commands_renames_the_tools_requests_and_steps() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flottille.db");
        let old = Connection::open(&path).unwrap();
        let previous = MIGRATIONS
            .iter()
            .position(|m| m.contains("DROP TABLE procedure_runs"))
            .unwrap();
        for migration in &MIGRATIONS[..previous] {
            old.execute_batch(migration).unwrap();
        }
        old.pragma_update(None, "user_version", previous).unwrap();
        let agent = an_agent();
        old.execute_batch(&format!(
            "INSERT INTO agents (id, provider, role, project, folder, state, created_at, \
             updated_at, instructions, tools) VALUES ('{}', 'claude', 'lead', 'app', '/w', \
             'idle', 1, 1, 'i', '[\"mcp__flottille__fs_read\",\"mcp__flottille__reference_read\",\
             \"mcp__flottille__procedure\",\"mcp__flottille__project\"]'); \
             INSERT INTO approval_requests (number, agent, tool, arguments, shown, reasons, \
             sensitive, identity, held_by, forwards, state, created_at, kind) VALUES \
             (1, '{0}', 'procedure', '{{\"action\":\"start\"}}', '{{}}', '[]', 0, 'x', 'user', \
             '[]', 'pending', 1, 'action'); \
             INSERT INTO workspaces (id, project, name, branch, folder, project_folder, \
             created_at, variables, repositories) VALUES (1, 'app', 'login', 'app/login', '/w', \
             '/p', 1, '{{}}', '[]'); \
             INSERT INTO workspace_steps (workspace, position, repository, action, state) VALUES \
             (1, 0, NULL, '{{\"kind\":\"run\",\"line\":\"make\",\"command\":\"build\",\
             \"time_limit\":60}}', 'done');",
            agent.id
        ))
        .unwrap();
        drop(old);

        let store = Store::open(&path).unwrap();

        assert_eq!(
            store.agent(&agent.id).unwrap().unwrap().tools,
            [
                "mcp__flottille__fs_read",
                "mcp__flottille__skill_read",
                "mcp__flottille__commands",
                "mcp__flottille__project"
            ]
        );
        assert_eq!(store.request(1).unwrap().unwrap().tool, "commands");
        let steps = store.workspace("app", "login").unwrap().unwrap().steps;
        assert_eq!(
            steps[0].action,
            Action::Run {
                line: "make".into(),
                task: Some("build".into()),
                time_limit: 60
            }
        );
        let tables: u64 = store
            .connection
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = 'procedure_runs'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tables, 0);
    }

    #[test]
    fn the_migration_of_tasks_renames_the_command_tools_and_keeps_the_runs_with_their_project() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flottille.db");
        let old = Connection::open(&path).unwrap();
        let previous = MIGRATIONS
            .iter()
            .position(|m| m.contains("CREATE TABLE runs_by_project"))
            .unwrap();
        for migration in &MIGRATIONS[..previous] {
            old.execute_batch(migration).unwrap();
        }
        old.pragma_update(None, "user_version", previous).unwrap();
        let agent = an_agent();
        old.execute_batch(&format!(
            "INSERT INTO agents (id, provider, role, project, folder, state, created_at, \
             updated_at, instructions, tools) VALUES ('{}', 'claude', 'lead', 'app', '/w', \
             'idle', 1, 1, 'i', '[\"mcp__flottille__fs_read\",\"mcp__flottille__commands_list\",\
             \"mcp__flottille__commands_run\",\"mcp__flottille__commands_status\",\
             \"mcp__flottille__skill_read\"]'); \
             INSERT INTO approval_requests (number, agent, tool, arguments, shown, reasons, \
             sensitive, identity, held_by, forwards, state, created_at, kind) VALUES \
             (1, '{0}', 'commands_run', '{{\"command\":\"test\"}}', '{{}}', '[]', 0, 'x', \
             'user', '[]', 'pending', 1, 'call'); \
             INSERT INTO runs (id, agent, name, line, folder, state, started_at, output) VALUES \
             ('r1', '{0}', 'test', 'make test', '/w', 'exited', 5, 'ok');",
            agent.id
        ))
        .unwrap();
        drop(old);

        let store = Store::open(&path).unwrap();

        assert_eq!(
            store.agent(&agent.id).unwrap().unwrap().tools,
            [
                "mcp__flottille__fs_read",
                "mcp__flottille__tasks",
                "mcp__flottille__run",
                "mcp__flottille__run_status",
                "mcp__flottille__skill_read",
            ]
        );
        let request = store.request(1).unwrap().unwrap();
        assert_eq!(request.tool, "run");
        assert_eq!(request.arguments, json!({"task": "test"}));
        let run = store.run("r1").unwrap().unwrap();
        assert_eq!(run.agent.as_deref(), Some(agent.id.as_str()));
        assert_eq!(run.project, "app");
    }

    #[test]
    fn sessions_launched_before_keep_their_relation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flottille.db");
        let old = Connection::open(&path).unwrap();
        let previous = MIGRATIONS
            .iter()
            .position(|m| m.contains("ADD COLUMN led INTEGER"))
            .unwrap();
        for migration in &MIGRATIONS[..previous] {
            old.execute_batch(migration).unwrap();
        }
        old.pragma_update(None, "user_version", previous).unwrap();
        // A starter, a session it still follows, one it does not, and a root of the user's.
        for (id, launched_by) in [
            ("000000000001", None),
            ("000000000002", Some("000000000001")),
            ("000000000003", Some("000000000001")),
            ("000000000004", None),
        ] {
            old.execute(
                "INSERT INTO agents (id, provider, role, project, workspace, folder, state, \
                 created_at, updated_at, instructions, tools, launched_by) VALUES (?1, 'claude', \
                 'lead', 'app', NULL, '/work/app', 'idle', 1, 1, 'text', '[]', ?2)",
                params![id, launched_by],
            )
            .unwrap();
        }
        old.execute(
            "INSERT INTO subscriptions (id, agent, source, target, events, from_whom, seen, \
             created_at, expires_at) VALUES ('s1', '000000000001', 'session', \
             '{\"session\":\"000000000002\",\"title\":\"Login\"}', '[\"turn-ended\"]', \
             'anyone', '{}', 1, 9)",
            [],
        )
        .unwrap();
        drop(old);

        let store = Store::open(&path).unwrap();

        let led = |id: &str| store.agent(id).unwrap().unwrap().led;
        assert!(led("000000000002"), "a followed session is led");
        assert!(!led("000000000003"), "an unfollowed one is handed");
        assert!(!led("000000000004"));
        for id in ["000000000002", "000000000003"] {
            assert_eq!(
                store.agent(id).unwrap().unwrap().launched_by.as_deref(),
                Some("000000000001")
            );
        }
        let events: String = store
            .connection
            .query_row(
                "SELECT events FROM subscriptions WHERE id = 's1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(events, r#"["asked","reported","ended"]"#);
    }

    #[test]
    fn the_migration_of_the_tool_families_renames_the_forge_ticket_and_removal_tools() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flottille.db");
        let old = Connection::open(&path).unwrap();
        let previous = MIGRATIONS
            .iter()
            .position(|m| m.contains("WHEN 'workspace_remove' THEN 'workspace'"))
            .unwrap();
        for migration in &MIGRATIONS[..previous] {
            old.execute_batch(migration).unwrap();
        }
        old.pragma_update(None, "user_version", previous).unwrap();
        for (id, tools) in [
            // A child (Claude's names), a root that also had the workspace family (opencode's).
            (
                "000000000001",
                r#"["mcp__flottille__fs_read","mcp__flottille__forge_read","mcp__flottille__skill_read","mcp__flottille__ticket_read","mcp__flottille__forge_write","mcp__flottille__workspace_remove","mcp__flottille__report"]"#,
            ),
            (
                "000000000002",
                r#"["flottille_forge_read","flottille_skill_read","flottille_forge_write","flottille_workspace_remove","flottille_agent_launch","flottille_project","flottille_workspace","flottille_report"]"#,
            ),
        ] {
            old.execute(
                "INSERT INTO agents (id, provider, role, project, workspace, folder, state, \
                 created_at, updated_at, instructions, tools) VALUES (?1, 'claude', 'lead', \
                 'app', NULL, '/work/app', 'idle', 1, 1, 'text', ?2)",
                params![id, tools],
            )
            .unwrap();
        }
        for (number, tool, arguments) in [
            (
                1,
                "forge_write",
                r#"{"action":"pr-merge-squash","repository":"api","waive":"ship it"}"#,
            ),
            (2, "forge_read", r#"{"workspace":"login"}"#),
            (3, "ticket_read", r##"{"ticket":"#12"}"##),
            (4, "workspace_remove", r#"{"workspace":"login"}"#),
            (5, "workspace_remove", r#"{}"#),
            (
                6,
                "forge_write",
                r#"{"action":"pr-create","repository":"api","title":"t"}"#,
            ),
            (
                7,
                "forge_write",
                r#"{"action":"pipeline-rerun","repository":"api"}"#,
            ),
        ] {
            old.execute(
                "INSERT INTO approval_requests (number, agent, tool, arguments, shown, reasons, \
                 sensitive, identity, held_by, forwards, state, created_at, kind) VALUES \
                 (?1, '000000000001', ?2, ?3, '{}', '[]', 0, 'x', 'user', '[]', 'pending', 1, \
                 'call')",
                params![number, tool, arguments],
            )
            .unwrap();
        }
        drop(old);

        let store = Store::open(&path).unwrap();

        let tools = |id: &str| store.agent(id).unwrap().unwrap().tools;
        assert_eq!(
            tools("000000000001"),
            [
                "mcp__flottille__fs_read",
                "mcp__flottille__forge",
                "mcp__flottille__skill_read",
                "mcp__flottille__tickets",
                "mcp__flottille__workspace",
                "mcp__flottille__report"
            ]
        );
        assert_eq!(
            tools("000000000002"),
            [
                "flottille_forge",
                "flottille_skill_read",
                "flottille_agent_launch",
                "flottille_project",
                "flottille_workspace",
                "flottille_report"
            ]
        );
        let request = |number: u64| {
            let request = store.request(number).unwrap().unwrap();
            (request.tool, request.arguments)
        };
        assert_eq!(
            request(1),
            (
                "forge".into(),
                json!({"action": "merge", "repository": "api", "waive": "ship it"})
            )
        );
        assert_eq!(
            request(2),
            (
                "forge".into(),
                json!({"action": "show", "workspace": "login"})
            )
        );
        assert_eq!(
            request(3),
            ("tickets".into(), json!({"action": "show", "ticket": "#12"}))
        );
        assert_eq!(
            request(4),
            (
                "workspace".into(),
                json!({"action": "remove", "name": "login"})
            )
        );
        assert_eq!(
            request(5),
            ("workspace".into(), json!({"action": "remove"}))
        );
        assert_eq!(request(6).1["action"], "create");
        assert_eq!(request(7).1["action"], "checks");
    }

    #[test]
    fn a_task_the_user_ran_is_a_run_of_no_agent_listed_with_its_project() {
        let mut store = Store::open_in_memory().unwrap();
        let run = |id: &str, project: &str, at: u64| Run {
            id: id.into(),
            agent: None,
            name: Some("test".into()),
            line: "make test".into(),
            folder: "/p".into(),
            state: RunState::Running,
            exit_code: None,
            signal: None,
            started_at: at,
            ended_at: None,
            output: String::new(),
            dismissed: false,
            project: project.into(),
            ready: None,
            url: None,
            url_error: None,
            workspace: None,
        };
        for (id, project, at) in [("r1", "app", 5), ("r2", "app", 6), ("r3", "other", 7)] {
            store
                .create_run(&run(id, project, at), ("run.started", json!({})))
                .unwrap();
        }
        store
            .end_run(
                "r1",
                RunState::Exited,
                Some(0),
                None,
                "done\n",
                ("run.ended", json!({})),
            )
            .unwrap();
        let ids = |runs: Vec<Run>| runs.into_iter().map(|r| r.id).collect::<Vec<_>>();

        assert_eq!(
            ids(store.user_runs_of("app", 30, 100).unwrap()),
            ["r1", "r2"]
        );
        assert_eq!(ids(store.user_runs_of("other", 30, 100).unwrap()), ["r3"]);
        store
            .dismiss_run("r1", ("run.dismissed", json!({"run": "r1"})))
            .unwrap();
        assert_eq!(ids(store.user_runs_of("app", 30, 100).unwrap()), ["r2"]);
        assert!(store.agents_with_runs().unwrap().is_empty());
    }

    #[test]
    fn a_pending_procedure_request_is_refused_by_the_migration_and_its_agent_told() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flottille.db");
        let old = Connection::open(&path).unwrap();
        let previous = MIGRATIONS
            .iter()
            .position(|m| m.contains("DROP TABLE procedure_runs"))
            .unwrap();
        for migration in &MIGRATIONS[..previous] {
            old.execute_batch(migration).unwrap();
        }
        old.pragma_update(None, "user_version", previous).unwrap();
        let agent = an_agent();
        old.execute_batch(&format!(
            "INSERT INTO agents (id, provider, role, project, folder, state, created_at, \
             updated_at, instructions, tools) VALUES ('{}', 'claude', 'lead', 'app', '/w', \
             'idle', 1, 1, 'i', '[]'); \
             INSERT INTO approval_requests (number, agent, tool, arguments, shown, reasons, \
             sensitive, identity, held_by, forwards, state, created_at, kind) VALUES \
             (1, '{0}', 'procedure', '{{\"action\":\"start\",\"name\":\"ship\"}}', '{{}}', \
             '[]', 0, 'x', 'user', '[]', 'pending', 1, 'action'), \
             (2, '{0}', 'procedure', '{{\"action\":\"start\",\"name\":\"old\"}}', '{{}}', \
             '[]', 0, 'y', 'user', '[]', 'denied', 1, 'action'), \
             (3, '{0}', 'fs_write', '{{}}', '{{}}', '[]', 0, 'z', 'user', '[]', 'pending', 1, \
             'action');",
            agent.id
        ))
        .unwrap();
        drop(old);

        let store = Store::open(&path).unwrap();

        let refused = store.request(1).unwrap().unwrap();
        assert_eq!(refused.state, RequestState::Denied);
        assert_eq!(
            store.request(2).unwrap().unwrap().state,
            RequestState::Denied
        );
        assert_eq!(
            store.request(3).unwrap().unwrap().state,
            RequestState::Pending
        );
        assert_eq!(
            store
                .pending_requests(approval::USER)
                .unwrap()
                .iter()
                .map(|request| request.number)
                .collect::<Vec<_>>(),
            [3]
        );
        let told = store.deliveries(&agent.id, "queued").unwrap();
        assert_eq!(told.len(), 1, "{told:?}");
        assert_eq!(told[0].id, "approval-1");
        assert_eq!(told[0].kind, "approval");
        assert_eq!(
            told[0].body,
            "Request #1 (procedure) was refused: procedures were replaced by commands; start it \
             again with /ship."
        );
    }

    #[test]
    fn a_provider_s_models_are_kept_with_its_version_and_replaced_whole() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.provider_models("claude").unwrap(), None);

        let models = json!([{"id": "small", "efforts": ["low"]}]);
        store
            .set_provider_models("claude", Some("2.1.295"), &models)
            .unwrap();
        store
            .set_provider_models("opencode", None, &json!([]))
            .unwrap();

        assert_eq!(
            store.provider_models("claude").unwrap(),
            Some((Some("2.1.295".into()), models))
        );
        store
            .set_provider_models("claude", Some("2.1.300"), &json!([]))
            .unwrap();
        assert_eq!(
            store.provider_models("claude").unwrap(),
            Some((Some("2.1.300".into()), json!([])))
        );
        assert_eq!(
            store.provider_models("opencode").unwrap(),
            Some((None, json!([])))
        );
    }

    #[test]
    fn a_store_of_the_agents_schema_keeps_them_and_gains_the_runs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flottille.db");
        let old = Connection::open(&path).unwrap();
        for migration in &MIGRATIONS[..3] {
            old.execute_batch(migration).unwrap();
        }
        old.pragma_update(None, "user_version", 3).unwrap();
        drop(old);
        insert_old_agent(&path);

        let store = Store::open(&path).unwrap();

        assert_eq!(store.schema_version().unwrap(), MIGRATIONS.len());
        assert_eq!(store.agents().unwrap().len(), 1);
        assert!(store.running_runs().unwrap().is_empty());
    }

    #[test]
    fn a_store_of_the_runs_schema_keeps_them_and_gains_the_requests() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flottille.db");
        let old = Connection::open(&path).unwrap();
        for migration in &MIGRATIONS[..4] {
            old.execute_batch(migration).unwrap();
        }
        old.pragma_update(None, "user_version", 4).unwrap();
        drop(old);
        insert_old_agent(&path);

        let store = Store::open(&path).unwrap();

        assert_eq!(store.schema_version().unwrap(), MIGRATIONS.len());
        assert_eq!(store.agents().unwrap().len(), 1);
        assert!(store.pending_requests(approval::USER).unwrap().is_empty());
    }

    #[test]
    fn a_store_of_the_requests_schema_keeps_its_agents_and_deliveries_and_gains_the_tree() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flottille.db");
        let old = Connection::open(&path).unwrap();
        for migration in &MIGRATIONS[..5] {
            old.execute_batch(migration).unwrap();
        }
        old.pragma_update(None, "user_version", 5).unwrap();
        drop(old);
        insert_old_agent(&path);
        Connection::open(&path)
            .unwrap()
            .execute(
                "INSERT INTO deliveries (id, agent, kind, body, state, created_at) \
                 VALUES ('approval-1', '0123456789ab', 'approval', 'done', 'queued', 1)",
                [],
            )
            .unwrap();

        let store = Store::open(&path).unwrap();

        assert_eq!(store.schema_version().unwrap(), MIGRATIONS.len());
        let agent = store.agent("0123456789ab").unwrap().unwrap();
        assert_eq!((agent.parent, agent.depth, agent.epoch), (None, 0, 0));
        let delivery = store.delivery("approval-1").unwrap().unwrap();
        assert_eq!(delivery.urgency, approval::BETWEEN_TURNS);
        assert_eq!(delivery.state, "queued");
    }

    fn a_delivery(id: &str, kind: &str) -> Delivery {
        Delivery {
            id: id.into(),
            agent: "0123456789ab".into(),
            kind: kind.into(),
            body: "body".into(),
            urgency: approval::BETWEEN_TURNS.into(),
            state: "queued".into(),
            created_at: 0,
            sent_at: None,
            images: Vec::new(),
            typed: None,
            commands: Vec::new(),
        }
    }

    #[test]
    fn a_message_keeps_what_the_user_typed_and_the_commands_it_used() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .create_agent(&an_agent(), ("agent.created", json!({})))
            .unwrap();
        let mut said = a_delivery("d1", "user");
        said.typed = Some("test /ship then /deploy".into());
        said.commands = vec!["ship".into(), "deploy".into()];
        store.deliver(&said).unwrap().unwrap();
        store.deliver(&a_delivery("d2", "user")).unwrap().unwrap();

        let read = store.delivery("d1").unwrap().unwrap();
        assert_eq!(read.typed.as_deref(), Some("test /ship then /deploy"));
        assert_eq!(read.commands, ["ship", "deploy"]);
        let plain = store.delivery("d2").unwrap().unwrap();
        assert_eq!((plain.typed, plain.commands), (None, Vec::<String>::new()));
    }

    #[test]
    fn an_old_command_message_reads_with_its_text_and_the_command_its_header_names() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flottille.db");
        let old = Connection::open(&path).unwrap();
        let previous = MIGRATIONS
            .iter()
            .position(|m| m.contains("ADD COLUMN typed"))
            .unwrap();
        for migration in &MIGRATIONS[..previous] {
            old.execute_batch(migration).unwrap();
        }
        old.pragma_update(None, "user_version", previous).unwrap();
        old.execute_batch(
            "INSERT INTO agents (id, provider, role, project, folder, state, created_at, \
             updated_at, instructions, tools) VALUES ('a', 'claude', 'lead', 'app', '/w', \
             'idle', 1, 1, 'i', '[]'); \
             INSERT INTO deliveries (id, agent, kind, body, state, created_at) VALUES \
             ('d1', 'a', 'user', 'The command open-prs of project app: Open them\n\n---\n\nDo it.', \
              'sent', 1), \
             ('d2', 'a', 'user', 'The command is broken, look', 'sent', 2), \
             ('d3', 'a', 'user', 'plain words', 'sent', 3), \
             ('d4', 'a', 'child-report', 'The command deploy of project app: hi', 'sent', 4), \
             ('d5', 'a', 'user', 'THE COMMAND deploy OF PROJECT app: hi', 'sent', 5), \
             ('d6', 'a', 'parent', 'The command deploy of project app: hi', 'sent', 6);",
        )
        .unwrap();
        drop(old);

        let store = Store::open(&path).unwrap();

        let d1 = store.delivery("d1").unwrap().unwrap();
        assert!(d1.body.ends_with("Do it."), "its text as the agent got it");
        assert_eq!((d1.typed, d1.commands), (None, vec!["open-prs".to_owned()]));
        let d6 = store.delivery("d6").unwrap().unwrap();
        assert_eq!(d6.commands, ["deploy"], "a parent's message is one too");
        for id in ["d2", "d3", "d4", "d5"] {
            let plain = store.delivery(id).unwrap().unwrap();
            assert_eq!(
                (plain.typed, plain.commands),
                (None, Vec::<String>::new()),
                "{id}"
            );
        }
    }

    #[test]
    fn a_subscription_is_kept_across_a_reopening_and_replaces_the_agent_s_own_on_its_target() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flottille.db");
        let mut store = Store::open(&path).unwrap();
        store
            .create_agent(&an_agent(), ("agent.created", json!({})))
            .unwrap();
        let subscription = |id: &str, events: &[&str]| Subscription {
            id: id.into(),
            agent: "0123456789ab".into(),
            source: "forge".into(),
            target: json!({"project": "app", "workspace": "login", "repository": "api"}),
            events: events.iter().map(|e| e.to_string()).collect(),
            from: "anyone".into(),
            seen: json!({"state": "open"}),
            created_at: 1,
            expires_at: 2,
        };
        store.subscribe(&subscription("s1", &["checks"])).unwrap();
        store
            .subscribe(&subscription("s2", &["checks", "review"]))
            .unwrap();
        drop(store);

        let store = Store::open(&path).unwrap();
        let kept = store.subscriptions().unwrap();
        assert_eq!(kept, [subscription("s2", &["checks", "review"])]);
        assert!(store.unsubscribe("s2").unwrap());
        assert!(!store.unsubscribe("s2").unwrap());
        assert!(store.subscriptions().unwrap().is_empty());
    }

    #[test]
    fn a_waiting_message_is_withdrawn_once_and_one_taken_first_cannot_be() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .create_agent(&an_agent(), ("agent.created", json!({})))
            .unwrap();
        let agent = "0123456789ab";
        store.deliver(&a_delivery("d1", "user")).unwrap();
        store.deliver(&a_delivery("d2", "user")).unwrap();
        store.deliver(&a_delivery("d3", "child-report")).unwrap();

        let (taken, event) = store.withdraw(agent, "d1").unwrap();
        assert_eq!(taken.unwrap().id, "d1");
        let event = event.unwrap();
        assert_eq!(event.name, "delivery.withdrawn");
        assert_eq!(event.data["deliveries"], json!(["d1"]));
        assert_eq!(store.delivery("d1").unwrap().unwrap().state, "withdrawn");
        assert!(store.withdraw(agent, "d1").unwrap().0.is_none(), "twice");
        // What the user did not write is not theirs to take back.
        assert!(store.withdraw(agent, "d3").unwrap().0.is_none());
        assert_eq!(store.delivery("d3").unwrap().unwrap().state, "queued");

        // The agent took d2 first: nothing is withdrawn, and the delivery is as it was.
        store
            .move_deliveries(agent, &["d2".into()], "queued", "sending", "prompt")
            .unwrap();
        let (taken, event) = store.withdraw(agent, "d2").unwrap();
        assert!(taken.is_none() && event.is_none());
        assert_eq!(store.delivery("d2").unwrap().unwrap().state, "sending");
    }

    #[test]
    fn a_waiting_message_is_redirected_only_while_it_waits() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .create_agent(&an_agent(), ("agent.created", json!({})))
            .unwrap();
        let agent = "0123456789ab";
        store.deliver(&a_delivery("d1", "user")).unwrap();
        store.deliver(&a_delivery("d2", "child-report")).unwrap();

        assert!(store.redirect(agent, "d1").unwrap());
        assert_eq!(
            store.delivery("d1").unwrap().unwrap().urgency,
            approval::REDIRECT
        );
        assert!(!store.redirect(agent, "d2").unwrap(), "not the user's");
        store
            .move_deliveries(agent, &["d1".into()], "queued", "sending", "prompt")
            .unwrap();
        assert!(!store.redirect(agent, "d1").unwrap(), "taken already");
    }

    #[test]
    fn a_delivery_is_stored_once_sent_given_back_and_superseded() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .create_agent(&an_agent(), ("agent.created", json!({})))
            .unwrap();
        let agent = "0123456789ab";

        let event = store.deliver(&a_delivery("d1", "user")).unwrap().unwrap();
        assert_eq!(event.name, "delivery.queued");
        assert!(store.deliver(&a_delivery("d1", "user")).unwrap().is_none());
        store.deliver(&a_delivery("d2", "brief")).unwrap();
        let queued: Vec<String> = store
            .deliveries(agent, "queued")
            .unwrap()
            .into_iter()
            .map(|d| d.id)
            .collect();
        assert_eq!(queued, ["d1", "d2"]);

        let (moved, event) = store
            .move_deliveries(agent, &queued, "queued", "sending", "prompt")
            .unwrap();
        assert_eq!(moved, ["d1", "d2"]);
        assert_eq!(event.unwrap().name, "delivery.sent");
        // Taken already: moving them again does nothing.
        let (moved, event) = store
            .move_deliveries(agent, &queued, "queued", "sending", "prompt")
            .unwrap();
        assert!(moved.is_empty() && event.is_none());

        assert_eq!(
            store.give_back(agent).unwrap().unwrap().name,
            "delivery.given_back"
        );
        assert_eq!(store.deliveries(agent, "queued").unwrap().len(), 2);
        store
            .move_deliveries(agent, &["d1".into()], "queued", "sending", "prompt")
            .unwrap();
        store.deliveries_taken(agent).unwrap();
        assert_eq!(store.delivery("d1").unwrap().unwrap().state, "sent");

        assert_eq!(
            store.supersede(agent).unwrap().unwrap().data["deliveries"],
            json!(["d2"])
        );
        assert!(store.supersede(agent).unwrap().is_none());
        assert_eq!(store.last_deliveries(agent, 1).unwrap()[0].id, "d2");
    }

    #[test]
    fn the_user_s_last_messages_are_found_past_any_number_of_other_deliveries() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .create_agent(&an_agent(), ("agent.created", json!({})))
            .unwrap();
        let agent = "0123456789ab";
        for id in ["u1", "u2", "u3"] {
            store.deliver(&a_delivery(id, "user")).unwrap();
        }
        for n in 0..40 {
            store
                .deliver(&a_delivery(&format!("r{n}"), "child-report"))
                .unwrap();
        }

        let ids: Vec<String> = store
            .last_deliveries_of(agent, "user", 2)
            .unwrap()
            .into_iter()
            .map(|delivery| delivery.id)
            .collect();
        assert_eq!(ids, ["u3", "u2"]);
    }

    #[test]
    fn a_fresh_brief_supersedes_the_ones_not_sent_and_an_ended_agent_is_delivered_nothing() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .create_agent(&an_agent(), ("agent.created", json!({})))
            .unwrap();
        let agent = "0123456789ab";
        for id in ["brief-a-0", "resume-a-0", "child-1", "brief-a-1"] {
            store.deliver(&a_delivery(id, "brief")).unwrap();
        }
        store
            .move_deliveries(agent, &["brief-a-0".into()], "queued", "sending", "prompt")
            .unwrap();
        store
            .move_deliveries(agent, &["brief-a-1".into()], "queued", "sent", "prompt")
            .unwrap();

        let event = store
            .supersede_like(agent, &["brief-a-", "resume-a-"])
            .unwrap();

        assert_eq!(
            event.unwrap().data["deliveries"],
            json!(["brief-a-0", "resume-a-0"])
        );
        let queued: Vec<String> = store
            .deliveries(agent, "queued")
            .unwrap()
            .into_iter()
            .map(|d| d.id)
            .collect();
        assert_eq!(queued, ["child-1"]);
        assert_eq!(store.delivery("brief-a-1").unwrap().unwrap().state, "sent");

        store
            .change_agent(
                agent,
                AgentChange {
                    state: Some(AgentState::Ended),
                    ..AgentChange::default()
                },
                None,
            )
            .unwrap();
        assert!(
            store
                .deliver(&a_delivery("late", "child-report"))
                .unwrap()
                .is_none()
        );
        assert!(store.delivery("late").unwrap().is_none());
    }

    #[test]
    fn a_replacement_bumps_the_epoch_forgets_the_session_and_is_counted() {
        let mut store = Store::open_in_memory().unwrap();
        let mut agent = an_agent();
        agent.native_id = Some("session-1".into());
        store
            .create_agent(&agent, ("agent.created", json!({})))
            .unwrap();

        let (replaced, event) = store
            .replace_agent(&agent.id, "its agent stopped", true)
            .unwrap();
        store
            .replace_agent(&agent.id, "Flottille restarted", false)
            .unwrap();

        assert_eq!(replaced.epoch, 1);
        assert_eq!(replaced.native_id, None);
        assert_eq!(replaced.state, AgentState::Starting);
        assert_eq!(event.unwrap().data["epoch"], 1);
        assert_eq!(store.replacements_since(&agent.id, 0).unwrap(), 1);
        assert_eq!(
            store.replacements_since(&agent.id, now_ms() + 1).unwrap(),
            0
        );
    }

    #[test]
    fn children_are_read_by_parent_and_the_live_ones_counted_per_project() {
        let mut store = Store::open_in_memory().unwrap();
        let lead = an_agent();
        store
            .create_agent(&lead, ("agent.created", json!({})))
            .unwrap();
        for (id, state) in [
            ("child000001", AgentState::Working),
            ("child000002", AgentState::Ended),
        ] {
            let child = Agent {
                id: id.into(),
                parent: Some(lead.id.clone()),
                depth: 1,
                state,
                ..an_agent()
            };
            store
                .create_agent(&child, ("agent.created", json!({})))
                .unwrap();
        }

        let children = store.children(&lead.id).unwrap();

        assert_eq!(children.len(), 2);
        assert_eq!(children[0].parent.as_deref(), Some(lead.id.as_str()));
        assert_eq!(store.live_children("app").unwrap(), 1);
        assert_eq!(store.live_children("other").unwrap(), 0);
    }

    fn a_request(key: Option<&str>) -> Request {
        Request {
            number: 0,
            agent: an_agent().id,
            call_key: key.map(str::to_owned),
            tool: "fs_read".into(),
            arguments: json!({"path": ".env"}),
            shown: json!({"path": ".env"}),
            reasons: vec![".env is a sensitive place (.env)".into()],
            base: true,
            kind: approval::Kind::Call,
            says: Vec::new(),
            judgement: None,
            held_by: approval::USER.into(),
            forwards: Vec::new(),
            state: RequestState::Denied,
            decision: Some(Decision::Once),
            created_at: 0,
            decided_at: None,
            place: None,
            digest: None,
        }
    }

    #[test]
    fn a_request_is_numbered_and_the_same_call_key_gives_the_same_one() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .create_agent(&an_agent(), ("agent.created", json!({})))
            .unwrap();

        let (first, event) = store.ask(&a_request(Some("toolu_01"))).unwrap();
        let (again, none) = store.ask(&a_request(Some("toolu_01"))).unwrap();
        let (other, _) = store.ask(&a_request(Some("toolu_02"))).unwrap();
        let (keyless, _) = store.ask(&a_request(None)).unwrap();
        let (keyless_again, _) = store.ask(&a_request(None)).unwrap();

        assert_eq!(first.state, RequestState::Pending);
        assert_eq!(first.decision, None);
        assert!(first.created_at > 0);
        let event = event.unwrap();
        assert_eq!(event.name, "approval.requested");
        assert_eq!(event.data["number"], first.number);
        assert!(event.data.get("arguments").is_none());
        assert_eq!(again, first);
        assert!(none.is_none());
        assert_ne!(other.number, first.number);
        assert_ne!(keyless.number, keyless_again.number);
        assert_eq!(store.request(first.number).unwrap(), Some(first));
        assert_eq!(store.pending_requests(approval::USER).unwrap().len(), 4);
    }

    #[test]
    fn a_request_keeps_its_kind_what_it_says_its_class_and_the_judge_s_verdict() {
        let mut store = Store::open_in_memory().unwrap();
        let agent = an_agent();
        store
            .create_agent(&agent, ("agent.created", json!({})))
            .unwrap();
        let judgement = Judgement {
            by: "claude haiku".into(),
            verdict: "ask".into(),
            reason: "it pushes to a remote the user did not mention".into(),
        };
        let (made, event) = store
            .ask(&Request {
                kind: approval::Kind::Action,
                says: vec!["Start the procedure ship in workspace login".into()],
                base: false,
                judgement: Some(judgement.clone()),
                ..a_request(None)
            })
            .unwrap();
        assert_eq!(made.kind, approval::Kind::Action);
        let event = event.unwrap();
        assert_eq!(
            event.data["says"][0],
            "Start the procedure ship in workspace login"
        );
        assert_eq!(event.data["class"], "judge");
        assert_eq!(event.data["judgement"]["verdict"], "ask");
        let read = store.request(made.number).unwrap().unwrap();
        assert_eq!(read.says, made.says);
        assert!(!read.base);
        assert_eq!(read.judgement, Some(judgement));
    }

    #[test]
    fn a_request_held_by_an_agent_is_not_the_user_s_until_forwarded() {
        let mut store = Store::open_in_memory().unwrap();
        let agent = an_agent();
        store
            .create_agent(&agent, ("agent.created", json!({})))
            .unwrap();
        let mut asked = a_request(None);
        asked.held_by = "parent0000ab".into();
        let (request, _) = store.ask(&asked).unwrap();

        assert!(store.pending_requests(approval::USER).unwrap().is_empty());
        assert!(
            store
                .decide(request.number, Decision::Once, approval::USER)
                .unwrap()
                .is_none()
        );
        // Only its holder forwards it.
        assert!(
            store
                .forward(request.number, "someone0000", "why", approval::USER)
                .unwrap()
                .is_none()
        );

        let (forwarded, event) = store
            .forward(
                request.number,
                "parent0000ab",
                "it needs the key",
                approval::USER,
            )
            .unwrap()
            .unwrap();

        assert_eq!(event.name, "approval.forwarded");
        assert_eq!(event.data["held_by"], approval::USER);
        assert_eq!(forwarded.held_by, approval::USER);
        assert_eq!(
            forwarded.forwards,
            [Forward {
                by: "parent0000ab".into(),
                why: "it needs the key".into()
            }]
        );
        assert_eq!(store.pending_requests(approval::USER).unwrap().len(), 1);
        assert!(
            store
                .decide(request.number, Decision::Once, approval::USER)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn a_decision_is_taken_once() {
        let mut store = Store::open_in_memory().unwrap();
        let agent = an_agent();
        store
            .create_agent(&agent, ("agent.created", json!({})))
            .unwrap();
        let (once, _) = store.ask(&a_request(None)).unwrap();
        let (denied, _) = store.ask(&a_request(None)).unwrap();

        let (decided, event) = store
            .decide(once.number, Decision::Once, approval::USER)
            .unwrap()
            .unwrap();
        assert_eq!(decided.state, RequestState::Allowed);
        assert_eq!(decided.decision, Some(Decision::Once));
        assert!(decided.decided_at.is_some());
        assert_eq!(event.name, "approval.decided");
        assert_eq!(event.data["decision"], "once");
        // Decided once: a second decision changes nothing.
        assert!(
            store
                .decide(once.number, Decision::Deny, approval::USER)
                .unwrap()
                .is_none()
        );

        let (denied, _) = store
            .decide(denied.number, Decision::Deny, approval::USER)
            .unwrap()
            .unwrap();
        assert_eq!(denied.state, RequestState::Denied);
        assert!(store.pending_requests(approval::USER).unwrap().is_empty());
    }

    #[test]
    fn the_last_activity_of_a_tree_is_its_latest_sign_but_for_what_the_daemon_says_alone() {
        let mut store = Store::open_in_memory().unwrap();
        // Created long ago, so that the clock decides what is later.
        let agent = Agent {
            created_at: 1_000,
            updated_at: 1_000,
            ..an_agent()
        };
        assert_eq!(
            store
                .last_activity_of(std::slice::from_ref(&agent.id))
                .unwrap(),
            None
        );
        store
            .create_agent(&agent, ("agent.created", json!({})))
            .unwrap();
        let ids = [agent.id.clone()];
        let created = store.last_activity_of(&ids).unwrap().unwrap();
        assert!(created > agent.created_at);

        // The daemon's own words are no activity.
        std::thread::sleep(std::time::Duration::from_millis(5));
        for name in [
            "agent.tools_outdated",
            "agent.tools_reload",
            "agent.released",
        ] {
            store
                .append_agent_event(&agent.id, name, json!({}), false)
                .unwrap();
        }
        assert_eq!(store.last_activity_of(&ids).unwrap(), Some(created));

        // A message of the conversation, then a request, is.
        let message = store
            .append_agent_event(&agent.id, "agent.message", json!({}), true)
            .unwrap();
        assert_eq!(store.last_activity_of(&ids).unwrap(), Some(message.at));
        std::thread::sleep(std::time::Duration::from_millis(5));
        let (asked, event) = store.ask(&a_request(Some("toolu_01"))).unwrap();
        assert!(asked.created_at > message.at);
        // The request's event reads the clock again, a millisecond later at times.
        assert_eq!(
            store.last_activity_of(&ids).unwrap(),
            Some(event.unwrap().at)
        );
        assert_eq!(
            store.last_activity_of(&["somebody-else".into()]).unwrap(),
            None
        );
    }

    #[test]
    fn the_pending_requests_an_ended_agent_made_or_holds_are_cancelled_with_their_event() {
        let mut store = Store::open_in_memory().unwrap();
        let agent = an_agent();
        store
            .create_agent(&agent, ("agent.created", json!({})))
            .unwrap();
        let (made, _) = store.ask(&a_request(Some("toolu_01"))).unwrap();
        let (held, _) = store
            .ask(&Request {
                held_by: agent.id.clone(),
                ..a_request(Some("toolu_02"))
            })
            .unwrap();
        let (decided, _) = store.ask(&a_request(Some("toolu_03"))).unwrap();
        store
            .decide(decided.number, Decision::Deny, approval::USER)
            .unwrap();

        let events = store.cancel_requests(&agent.id, "its agent ended").unwrap();

        let numbers: Vec<_> = events.iter().map(|e| e.data["number"].clone()).collect();
        assert_eq!(numbers, [json!(made.number), json!(held.number)]);
        assert_eq!(events[0].name, "approval.cancelled");
        assert_eq!(events[0].data["why"], "its agent ended");
        let state = |n| store.request(n).unwrap().unwrap().state;
        assert_eq!(state(made.number), RequestState::Cancelled);
        assert_eq!(state(held.number), RequestState::Cancelled);
        assert_eq!(state(decided.number), RequestState::Denied);
        assert!(store.pending_requests(approval::USER).unwrap().is_empty());
        assert!(
            store
                .cancel_requests(&agent.id, "again")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_settled_request_stores_its_delivery_once() {
        let mut store = Store::open_in_memory().unwrap();
        let agent = an_agent();
        store
            .create_agent(&agent, ("agent.created", json!({})))
            .unwrap();
        let (request, _) = store.ask(&a_request(None)).unwrap();
        let (request, _) = store
            .decide(request.number, Decision::Once, approval::USER)
            .unwrap()
            .unwrap();
        assert_eq!(
            store.unsettled_requests().unwrap(),
            std::slice::from_ref(&request)
        );

        let event = store
            .settle(&request, "done", "the result")
            .unwrap()
            .unwrap();
        let again = store.settle(&request, "done", "another").unwrap();

        assert_eq!(event.name, "approval.settled");
        assert_eq!(event.data["outcome"], "done");
        assert!(again.is_none());
        let id = format!("approval-{}", request.number);
        let delivery = store.delivery(&id).unwrap().unwrap();
        assert_eq!(delivery.agent, agent.id);
        assert_eq!(delivery.kind, "approval");
        assert_eq!(delivery.body, "the result");
        assert_eq!(delivery.state, "queued");
        assert!(store.unsettled_requests().unwrap().is_empty());
    }

    #[test]
    fn a_file_read_keeps_its_fingerprint_per_agent() {
        let mut store = Store::open_in_memory().unwrap();
        for id in ["a", "b"] {
            let mut agent = an_agent();
            agent.id = id.into();
            store
                .create_agent(&agent, ("agent.created", json!({})))
                .unwrap();
        }
        store.set_file_read("a", "/w/f", "one").unwrap();
        store.set_file_read("a", "/w/f", "two").unwrap();

        assert_eq!(
            store.file_read("a", "/w/f").unwrap().as_deref(),
            Some("two")
        );
        assert_eq!(store.file_read("b", "/w/f").unwrap(), None);
    }

    #[test]
    fn a_run_is_written_ended_once_and_read_back() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .create_agent(&an_agent(), ("agent.created", json!({})))
            .unwrap();
        let run = Run {
            id: "r1".into(),
            agent: Some(an_agent().id),
            name: Some("test".into()),
            line: "cargo test".into(),
            folder: "/w".into(),
            state: RunState::Running,
            exit_code: None,
            signal: None,
            started_at: 5,
            ended_at: None,
            output: String::new(),
            dismissed: false,
            project: "app".into(),
            ready: None,
            url: None,
            url_error: None,
            workspace: None,
        };
        store.create_run(&run, ("run.started", json!({}))).unwrap();
        assert_eq!(store.run("r1").unwrap().unwrap(), run);
        assert_eq!(store.running_runs().unwrap(), std::slice::from_ref(&run));

        let ended = store
            .end_run(
                "r1",
                RunState::Exited,
                Some(0),
                None,
                "ok\n",
                ("run.ended", json!({})),
            )
            .unwrap();
        let again = store
            .end_run(
                "r1",
                RunState::Stopped,
                None,
                None,
                "",
                ("run.ended", json!({})),
            )
            .unwrap();

        assert!(ended.is_some() && again.is_none());
        let read = store.run("r1").unwrap().unwrap();
        assert_eq!((read.state, read.exit_code), (RunState::Exited, Some(0)));
        assert_eq!(read.output, "ok\n");
        assert!(read.ended_at.is_some());
        assert!(store.running_runs().unwrap().is_empty());
        store.add_run_process(4242, 7, "r1").unwrap();
        assert_eq!(store.run_processes().unwrap(), [(4242, 7)]);
        store.remove_run_process(4242).unwrap();
        assert!(store.run_processes().unwrap().is_empty());
    }

    #[test]
    fn a_run_keeps_whether_it_is_ready_and_its_address() {
        let mut store = Store::open_in_memory().unwrap();
        let run = |id: &str| Run {
            id: id.into(),
            agent: None,
            name: Some("web".into()),
            line: "npm run dev".into(),
            folder: "/w".into(),
            state: RunState::Running,
            exit_code: None,
            signal: None,
            started_at: 5,
            ended_at: None,
            output: String::new(),
            dismissed: false,
            project: "app".into(),
            ready: Some(Readiness::Starting),
            url: Some("https://w.app.localhost".into()),
            url_error: None,
            workspace: None,
        };
        for id in ["r1", "r2"] {
            store
                .create_run(&run(id), ("run.started", json!({})))
                .unwrap();
        }
        assert_eq!(store.run("r1").unwrap().unwrap(), run("r1"));

        let ready = store.ready_run("r1", ("run.ready", json!({}))).unwrap();
        let again = store.ready_run("r1", ("run.ready", json!({}))).unwrap();
        let end = |store: &mut Store, id| {
            store
                .end_run(
                    id,
                    RunState::Exited,
                    Some(1),
                    None,
                    "",
                    ("run.ended", json!({})),
                )
                .unwrap()
        };
        end(&mut store, "r1");
        end(&mut store, "r2");

        assert!(ready.is_some() && again.is_none());
        let r1 = store.run("r1").unwrap().unwrap();
        assert_eq!(r1.ready, Some(Readiness::Ready), "ready stays once ended");
        assert_eq!(r1.url.as_deref(), Some("https://w.app.localhost"));
        let r2 = store.run("r2").unwrap().unwrap();
        assert_eq!(
            r2.ready,
            Some(Readiness::Missed),
            "it ended before it was ready"
        );
        assert!(
            store
                .ready_run("r2", ("run.ready", json!({})))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_run_that_ended_is_dismissed_once_and_a_running_one_is_not() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .create_agent(&an_agent(), ("agent.created", json!({})))
            .unwrap();
        let agent = an_agent().id;
        for (id, at) in [("r1", 5), ("r2", 6)] {
            let run = Run {
                id: id.into(),
                agent: Some(agent.clone()),
                name: None,
                line: "sleep 1".into(),
                folder: "/w".into(),
                state: RunState::Running,
                exit_code: None,
                signal: None,
                started_at: at,
                ended_at: None,
                output: String::new(),
                dismissed: false,
                project: "app".into(),
                ready: None,
                url: None,
                url_error: None,
                workspace: None,
            };
            store.create_run(&run, ("run.started", json!({}))).unwrap();
        }
        store
            .end_run(
                "r1",
                RunState::Exited,
                Some(0),
                None,
                "",
                ("run.ended", json!({})),
            )
            .unwrap();
        let dismiss = |store: &mut Store, id: &str| {
            store
                .dismiss_run(id, ("run.dismissed", json!({"run": id})))
                .unwrap()
        };

        assert!(dismiss(&mut store, "r2").is_none(), "still running");
        assert!(dismiss(&mut store, "r1").is_some());
        assert!(dismiss(&mut store, "r1").is_none(), "once");

        let agents = std::slice::from_ref(&agent);
        let running = store.running_runs_of(agents).unwrap();
        assert_eq!(
            running.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["r2"]
        );
        assert!(store.ended_runs_of(agents, 30, 100).unwrap().is_empty());
        assert!(store.run("r1").unwrap().unwrap().dismissed);
    }

    /// Runs of one agent, each started at its number and ended at `ended` when it did.
    fn runs_of_an_agent(store: &mut Store, runs: &[(&str, u64, Option<u64>, &str)]) -> String {
        let agent = an_agent().id;
        store
            .create_agent(&an_agent(), ("agent.created", json!({})))
            .unwrap();
        for (id, started_at, ended_at, output) in runs {
            let run = Run {
                id: (*id).into(),
                agent: Some(agent.clone()),
                name: None,
                line: "make".into(),
                folder: "/w".into(),
                state: if ended_at.is_some() {
                    RunState::Exited
                } else {
                    RunState::Running
                },
                exit_code: ended_at.map(|_| 0),
                signal: None,
                started_at: *started_at,
                ended_at: *ended_at,
                output: (*output).into(),
                dismissed: false,
                project: "app".into(),
                ready: None,
                url: None,
                url_error: None,
                workspace: None,
            };
            store.create_run(&run, ("run.started", json!({}))).unwrap();
        }
        agent
    }

    #[test]
    fn the_ended_runs_listed_are_the_latest_to_end_with_only_the_tail_of_their_output() {
        let mut store = Store::open_in_memory().unwrap();
        let agent = runs_of_an_agent(
            &mut store,
            &[
                // Started first, ended last.
                ("slow", 1, Some(90), "start\nabcdefghij"),
                ("a", 2, Some(10), "a"),
                ("b", 3, Some(20), "b"),
                ("c", 4, Some(30), "c"),
                ("live", 5, None, ""),
            ],
        );
        let agents = std::slice::from_ref(&agent);

        let listed = store.ended_runs_of(agents, 3, 4).unwrap();

        assert_eq!(
            listed.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["slow", "b", "c"],
            "the three that ended last, in the order they started"
        );
        assert_eq!(listed[0].output, "ghij", "the last 4 characters of 16");
        assert_eq!(listed[1].output, "b", "a shorter output is whole");
        let running = store.running_runs_of(agents).unwrap();
        assert_eq!(
            running.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["live"]
        );
    }

    #[test]
    fn the_runs_that_ended_are_forgotten_and_the_one_that_runs_is_not() {
        let mut store = Store::open_in_memory().unwrap();
        let agent = runs_of_an_agent(
            &mut store,
            &[("done", 1, Some(2), "x"), ("live", 3, None, "")],
        );
        let agents = std::slice::from_ref(&agent);

        assert_eq!(store.forget_runs_of(agents).unwrap(), 1);

        assert!(store.run("done").unwrap().is_none());
        assert!(store.run("live").unwrap().is_some());
    }

    #[test]
    fn a_kept_tool_result_is_read_by_its_agent_alone_and_goes_with_its_session() {
        let mut store = Store::open_in_memory().unwrap();
        let agent = runs_of_an_agent(&mut store, &[]);
        store
            .keep_result("res_1", &agent, "fs_read", "the whole text")
            .unwrap();

        assert_eq!(
            store.kept_result("res_1", &agent).unwrap(),
            Some(Kept::Text("the whole text".to_owned()))
        );
        assert_eq!(store.kept_result("res_1", "another").unwrap(), None);
        assert_eq!(store.kept_result("res_2", &agent).unwrap(), None);
        assert_eq!(
            store.agents_with_runs().unwrap(),
            std::slice::from_ref(&agent)
        );

        store.forget_runs_of(std::slice::from_ref(&agent)).unwrap();

        assert_eq!(store.kept_result("res_1", &agent).unwrap(), None);
        assert!(store.agents_with_runs().unwrap().is_empty());
    }

    #[test]
    fn only_the_latest_results_of_an_agent_are_kept_and_a_dropped_one_says_so() {
        let mut store = Store::open_in_memory().unwrap();
        let agent = runs_of_an_agent(&mut store, &[]);
        let kept = KEPT_RESULTS_PER_AGENT;
        for n in 0..kept + 2 {
            store
                .keep_result(&format!("res_{n}"), &agent, "run_status", "text")
                .unwrap();
        }

        for n in 0..2 {
            assert_eq!(
                store.kept_result(&format!("res_{n}"), &agent).unwrap(),
                Some(Kept::Dropped)
            );
        }
        for n in 2..kept + 2 {
            assert_eq!(
                store.kept_result(&format!("res_{n}"), &agent).unwrap(),
                Some(Kept::Text("text".to_owned()))
            );
        }
        // Another agent's results are its own count.
        assert_eq!(store.kept_result("res_0", "another").unwrap(), None);
    }

    #[test]
    fn the_agents_that_have_runs_are_listed_once() {
        let mut store = Store::open_in_memory().unwrap();
        assert!(store.agents_with_runs().unwrap().is_empty());
        let agent = runs_of_an_agent(&mut store, &[("a", 1, Some(2), ""), ("b", 3, None, "")]);

        assert_eq!(store.agents_with_runs().unwrap(), [agent]);
    }

    #[test]
    fn runs_are_looked_up_by_agent_through_an_index() {
        let store = Store::open_in_memory().unwrap();
        let plan: String = store
            .connection
            .query_row(
                "EXPLAIN QUERY PLAN SELECT id FROM runs WHERE agent = 'a' AND state = 'running'",
                [],
                |row| row.get(3),
            )
            .unwrap();
        assert!(plan.contains("runs_by_agent"), "{plan}");
    }

    #[test]
    fn events_are_numbered_in_order_and_read_back_after_a_number() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.last_seq().unwrap(), 0);
        let first = store.append_event("test.one", json!({"n": 1})).unwrap();
        let second = store.append_event("test.two", json!({"n": 2})).unwrap();
        let third = store.append_event("test.three", json!({"n": 3})).unwrap();

        assert!(first.seq < second.seq && second.seq < third.seq);
        assert_eq!(
            store.events_after(first.seq, 10).unwrap(),
            vec![second.clone(), third]
        );
        assert_eq!(store.events_after(0, 1).unwrap(), vec![first]);
        assert_eq!(store.last_seq().unwrap(), 3);
    }

    fn an_item(project: &str, kind: &str, repository: &str, key: &str, state: &str) -> Item {
        Item {
            project: project.into(),
            kind: kind.into(),
            source: "github".into(),
            repository: repository.into(),
            key: key.into(),
            title: format!("#{key}"),
            state: state.into(),
            author: Some("alice".into()),
            assignees: vec!["bob".into()],
            labels: vec!["web".into()],
            url: None,
            updated_at: Some(format!("2026-10-0{key}T00:00:00Z")),
            seen_at: 7,
            mine: key == "1",
            details: json!({"head": "login"}),
            checks: Vec::new(),
        }
    }

    #[test]
    fn items_are_kept_with_their_checks_filtered_and_forgotten_with_their_source() {
        let mut store = Store::open_in_memory().unwrap();
        let mut pull = an_item("app", "pull", "api", "1", "open");
        pull.checks = vec![Check {
            name: "test".into(),
            state: "pending".into(),
            started_at: Some("2026-10-01T00:00:00Z".into()),
            ended_at: None,
            link: Some("https://example.com/1".into()),
        }];
        let written = store
            .put_items(
                &[
                    pull.clone(),
                    an_item("app", "pull", "web", "2", "merged"),
                    an_item("app", "ticket", "api", "3", "open"),
                    an_item("other", "pull", "api", "4", "open"),
                ],
                vec![("item.opened", json!({"key": "1"}))],
            )
            .unwrap();
        assert_eq!(written[0].name, "item.opened");
        let all = store.items("app", &Filter::default()).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all.iter().find(|i| i.key == "1").unwrap(), &pull);
        let keys = |store: &Store, filter: Filter| -> Vec<String> {
            let mut keys: Vec<String> = store
                .items("app", &filter)
                .unwrap()
                .into_iter()
                .map(|i| i.key)
                .collect();
            keys.sort();
            keys
        };
        let pulls = Filter {
            kind: Some("pull".into()),
            ..Filter::default()
        };
        assert_eq!(keys(&store, pulls), ["1", "2"]);
        let open = Filter {
            state: Some("open".into()),
            ..Filter::default()
        };
        assert_eq!(keys(&store, open), ["1", "3"]);
        let web = Filter {
            repository: Some("web".into()),
            ..Filter::default()
        };
        assert_eq!(keys(&store, web), ["2"]);
        let mine = Filter {
            mine: true,
            ..Filter::default()
        };
        assert_eq!(keys(&store, mine), ["1"]);

        // The same item again replaces it, its checks with it.
        let mut again = pull.clone();
        again.checks.clear();
        again.title = "Renamed".into();
        store.put_items(&[again], Vec::new()).unwrap();
        let read = store.items("app", &Filter::default()).unwrap();
        let read = read.iter().find(|i| i.key == "1").unwrap();
        assert_eq!(read.title, "Renamed");
        assert!(read.checks.is_empty());

        let cursor = Cursor {
            project: "app".into(),
            kind: "pull".into(),
            target: "web".into(),
            source: "github".into(),
            cursor: Some(5),
            round_at: Some(6),
            failures: 1,
            problem: Some("gh is not signed in".into()),
            first_at: Some(4),
            note: Some("only the newest 1000 are read".into()),
        };
        store.set_cursor(&cursor).unwrap();
        assert_eq!(store.cursors("app").unwrap(), vec![cursor]);
        // `web` left the project, and `other` is gone: one event for each source forgotten.
        let forgotten = store
            .retain_items(&[(
                "app".into(),
                vec![
                    ["pull", "api", "github", "api"].map(String::from),
                    ["ticket", "tracker", "github", "api"].map(String::from),
                ],
            )])
            .unwrap();
        let said: Vec<(&str, &Value)> = forgotten
            .iter()
            .map(|e| (e.name.as_str(), &e.data))
            .collect();
        assert_eq!(
            said,
            [
                (
                    "items.forgotten",
                    &json!({"project": "app", "kind": "pull", "source": "github",
                        "repository": "web", "items": 1})
                ),
                (
                    "items.forgotten",
                    &json!({"project": "other", "kind": "pull", "source": "github",
                        "repository": "api", "items": 1})
                ),
            ]
        );
        // Nothing more to forget: no event.
        let again = store
            .retain_items(&[(
                "app".into(),
                vec![
                    ["pull", "api", "github", "api"].map(String::from),
                    ["ticket", "tracker", "github", "api"].map(String::from),
                ],
            )])
            .unwrap();
        assert!(again.is_empty());
        assert_eq!(keys(&store, Filter::default()), ["1", "3"]);
        assert!(store.items("other", &Filter::default()).unwrap().is_empty());
        assert!(store.cursors("app").unwrap().is_empty());
    }

    const TABLE: &str =
        "CREATE TABLE ext_test_notes (id INTEGER PRIMARY KEY, text TEXT NOT NULL) STRICT;";
    const COLUMN: &str = "ALTER TABLE ext_test_notes ADD COLUMN at INTEGER;";

    fn notes(store: &mut Store) -> Vec<String> {
        let space = store.space("test").unwrap();
        let mut statement = space
            .tables()
            .prepare("SELECT text FROM ext_test_notes ORDER BY id")
            .unwrap();
        statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    #[test]
    fn an_extension_s_migrations_run_in_order_once_and_a_later_one_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flottille.db");
        let mut store = Store::open(&path).unwrap();
        store.migrate_extension("test", &[TABLE]).unwrap();
        let space = store.space("test").unwrap();
        space
            .tables()
            .execute("INSERT INTO ext_test_notes (text) VALUES ('kept')", [])
            .unwrap();
        space.commit().unwrap();
        drop(store);

        let mut store = Store::open(&path).unwrap();
        store.migrate_extension("test", &[TABLE]).unwrap();
        store.migrate_extension("test", &[TABLE, COLUMN]).unwrap();

        assert_eq!(notes(&mut store), ["kept"]);
        let space = store.space("test").unwrap();
        space
            .tables()
            .execute("UPDATE ext_test_notes SET at = 1", [])
            .unwrap();
    }

    #[test]
    fn an_extension_reads_its_own_tables_without_writing_and_nothing_else() {
        let mut store = Store::open_in_memory().unwrap();
        store.migrate_extension("test", &[TABLE]).unwrap();
        let space = store.space("test").unwrap();
        space
            .tables()
            .execute("INSERT INTO ext_test_notes (text) VALUES ('kept')", [])
            .unwrap();
        space.commit().unwrap();

        let read = store.read_space("test", |tables| {
            tables
                .execute("INSERT INTO ext_test_notes (text) VALUES ('not kept')", [])
                .unwrap();
            Ok(
                tables.query_row("SELECT count(*) FROM ext_test_notes", [], |row| {
                    row.get::<_, u64>(0)
                })?,
            )
        });
        let outside = store.read_space("test", |tables| {
            Ok(tables.query_row("SELECT count(*) FROM agents", [], |row| {
                row.get::<_, u64>(0)
            })?)
        });

        assert_eq!(read.unwrap(), 2);
        assert_eq!(notes(&mut store), ["kept"]);
        assert!(outside.is_err());
    }

    fn schema(store: &Store) -> Vec<String> {
        let mut statement = store
            .connection
            .prepare("SELECT ifnull(sql, name) FROM sqlite_master ORDER BY name")
            .unwrap();
        statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    #[test]
    fn an_extension_migration_cannot_touch_a_core_table() {
        let mut store = Store::open_in_memory().unwrap();
        let before = schema(&store);

        for migration in [
            "CREATE TABLE notes (id INTEGER PRIMARY KEY) STRICT;",
            "ALTER TABLE agents ADD COLUMN note TEXT;",
            "DROP TABLE events;",
            "CREATE INDEX ext_test_index ON agents (id);",
            "CREATE TRIGGER ext_test_trigger AFTER INSERT ON agents BEGIN SELECT 1; END;",
            "CREATE VIEW ext_test_view AS SELECT * FROM agents;",
            "INSERT INTO events (name, data, at) VALUES ('forged', '{}', 0);",
            "DELETE FROM projects;",
            "PRAGMA foreign_keys = OFF;",
        ] {
            let result = store.migrate_extension("test", &[TABLE, migration]);
            assert!(result.is_err(), "{migration}");
        }

        assert_eq!(schema(&store), before);
        assert_eq!(store.last_seq().unwrap(), 0);
    }

    #[test]
    fn an_extension_cannot_write_another_extension_s_tables() {
        let mut store = Store::open_in_memory().unwrap();
        store.migrate_extension("test", &[TABLE]).unwrap();
        store
            .migrate_extension(
                "other",
                &["CREATE TABLE ext_other_notes (id INTEGER PRIMARY KEY, text TEXT) STRICT;"],
            )
            .unwrap();
        let space = store.space("test").unwrap();

        for statement in [
            "INSERT INTO ext_other_notes (text) VALUES ('forged')",
            "UPDATE ext_other_notes SET text = 'forged'",
            "DELETE FROM ext_other_notes",
            "DROP TABLE ext_other_notes",
            "SELECT count(*) FROM ext_other_notes",
            "SELECT count(*) FROM agents",
            "DELETE FROM agents",
            "COMMIT",
        ] {
            assert!(
                space.tables().execute_batch(statement).is_err(),
                "{statement}"
            );
        }
        space
            .tables()
            .execute("INSERT INTO ext_test_notes (text) VALUES ('kept')", [])
            .unwrap();
        space.append_event("test.noted", json!({})).unwrap();
        space.commit().unwrap();

        assert_eq!(notes(&mut store), ["kept"]);
        assert_eq!(store.last_seq().unwrap(), 1);
    }

    #[test]
    fn an_extension_can_drop_and_rename_its_own_tables() {
        let mut store = Store::open_in_memory().unwrap();
        let log =
            "CREATE TABLE ext_test_log (id INTEGER PRIMARY KEY AUTOINCREMENT, text TEXT) STRICT;
                   CREATE INDEX ext_test_log_text ON ext_test_log (text);
                   INSERT INTO ext_test_log (text) VALUES ('kept');";
        // The rebuild SQLite's documentation gives for a change `ALTER TABLE` cannot make.
        let rebuild = "CREATE TABLE ext_test_log_new (
                           id INTEGER PRIMARY KEY AUTOINCREMENT, text TEXT NOT NULL, at INTEGER
                       ) STRICT;
                       INSERT INTO ext_test_log_new (id, text) SELECT id, text FROM ext_test_log;
                       DROP TABLE ext_test_log;
                       ALTER TABLE ext_test_log_new RENAME TO ext_test_log;
                       CREATE INDEX ext_test_log_text ON ext_test_log (text);";
        let gone = "CREATE TABLE ext_test_gone (id INTEGER PRIMARY KEY AUTOINCREMENT) STRICT;
                    INSERT INTO ext_test_gone DEFAULT VALUES;
                    DROP TABLE ext_test_gone;";

        store
            .migrate_extension("test", &[log, rebuild, gone])
            .unwrap();

        let space = store.space("test").unwrap();
        space
            .tables()
            .execute("INSERT INTO ext_test_log (text, at) VALUES ('new', 1)", [])
            .unwrap();
        let texts: Vec<String> = space
            .tables()
            .prepare("SELECT text FROM ext_test_log ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(texts, ["kept", "new"]);
    }

    #[test]
    fn an_extension_cannot_rename_a_table_out_of_its_space() {
        let mut store = Store::open_in_memory().unwrap();
        store.migrate_extension("test", &[TABLE]).unwrap();
        let before = schema(&store);

        for name in ["notes", "ext_other_notes"] {
            let rename = format!("ALTER TABLE ext_test_notes RENAME TO {name};");
            let error = store
                .migrate_extension("test", &[TABLE, &rename])
                .unwrap_err();
            assert!(
                error.to_string().contains("not its own"),
                "{rename}: {error}"
            );

            let space = store.space("test").unwrap();
            space.tables().execute_batch(&rename).unwrap();
            let error = space.commit().unwrap_err();
            assert!(
                error.to_string().contains("not its own"),
                "{rename}: {error}"
            );
        }

        assert_eq!(schema(&store), before);
    }

    #[test]
    fn an_extension_cannot_reset_a_core_counter() {
        let mut store = Store::open_in_memory().unwrap();
        store.append_event("test.one", json!({})).unwrap();
        store.migrate_extension("test", &[TABLE]).unwrap();
        let reset = "UPDATE sqlite_sequence SET seq = 0 WHERE name = 'events'";

        let space = store.space("test").unwrap();
        space.tables().execute_batch(reset).unwrap();
        assert!(space.append_event("test.noted", json!({})).is_err());
        drop(space);
        let space = store.space("test").unwrap();
        space.tables().execute_batch(reset).unwrap();
        assert!(space.commit().is_err());

        assert_eq!(store.append_event("test.two", json!({})).unwrap().seq, 2);
    }

    #[test]
    fn a_core_write_that_fails_partway_in_a_space_is_not_taken_for_the_extension_s() {
        let mut store = Store::open_in_memory().unwrap();
        store.migrate_extension("test", &[TABLE]).unwrap();

        let space = store.space("test").unwrap();
        let failed = space.confinement.lifted(space.tables(), || {
            append_event(space.tables(), "test.half", json!({}))?;
            Err::<(), _>(Error::Refused("failed partway".into()))
        });

        assert!(failed.unwrap_err().to_string().contains("failed partway"));
        space.commit().unwrap();
    }

    #[test]
    fn an_extension_s_table_cannot_reference_a_core_table() {
        let mut store = Store::open_in_memory().unwrap();

        let result = store.migrate_extension(
            "test",
            &["CREATE TABLE ext_test_owned (
                   id INTEGER PRIMARY KEY, agent TEXT REFERENCES agents (id) ON DELETE CASCADE
               ) STRICT;"],
        );

        assert!(result.is_err());
        store
            .migrate_extension(
                "test",
                &[
                    TABLE,
                    "CREATE TABLE ext_test_tags (note INTEGER REFERENCES ext_test_notes (id)) STRICT;",
                ],
            )
            .unwrap();
    }

    #[test]
    fn an_extension_s_column_cannot_reference_a_core_table_from_its_space() {
        let mut store = Store::open_in_memory().unwrap();
        store.migrate_extension("test", &[TABLE]).unwrap();
        let before = schema(&store);

        let space = store.space("test").unwrap();
        space
            .tables()
            .execute_batch(
                "ALTER TABLE ext_test_notes ADD COLUMN agent TEXT REFERENCES agents (id);",
            )
            .unwrap();
        let error = space.commit().unwrap_err();

        assert!(error.to_string().contains("references agents"), "{error}");
        assert_eq!(schema(&store), before);
    }

    #[test]
    fn renaming_an_extension_s_table_carries_its_trigger_and_stays_in_its_space() {
        let mut store = Store::open_in_memory().unwrap();
        let counted = "CREATE TABLE ext_test_count (n INTEGER NOT NULL) STRICT;
                       INSERT INTO ext_test_count VALUES (0);
                       CREATE TRIGGER ext_test_counted AFTER INSERT ON ext_test_notes
                           BEGIN UPDATE ext_test_count SET n = n + 1; END;";
        let rename = "ALTER TABLE ext_test_notes RENAME TO ext_test_kept;";

        store
            .migrate_extension("test", &[TABLE, counted, rename])
            .unwrap();

        let space = store.space("test").unwrap();
        space
            .tables()
            .execute("INSERT INTO ext_test_kept (text) VALUES ('one')", [])
            .unwrap();
        let n: i64 = space
            .tables()
            .query_row("SELECT n FROM ext_test_count", [], |row| row.get(0))
            .unwrap();
        assert_eq!(n, 1);
        space.commit().unwrap();
        let on: String = store
            .connection
            .query_row(
                "SELECT tbl_name FROM sqlite_master WHERE name = 'ext_test_counted'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(on, "ext_test_kept");
    }

    #[test]
    fn an_extension_whose_name_holds_an_underscore_is_refused() {
        let mut store = Store::open_in_memory().unwrap();

        let error = store.migrate_extension("a_b", &[]).unwrap_err();

        assert!(error.to_string().contains("a_b"), "{error}");
        assert!(store.space("a_b").is_err());
    }

    #[test]
    fn an_extension_whose_name_is_empty_upper_case_or_dashed_is_refused() {
        let mut store = Store::open_in_memory().unwrap();

        for name in ["", "Test", "a-b"] {
            let error = store.migrate_extension(name, &[]).unwrap_err();
            assert!(
                error.to_string().contains("letters and digits"),
                "{name}: {error}"
            );
            assert!(store.space(name).is_err(), "{name}");
        }
        store.migrate_extension("test2", &[]).unwrap();
    }

    #[test]
    fn an_extension_s_tables_from_a_newer_flottille_are_refused() {
        let mut store = Store::open_in_memory().unwrap();
        store.migrate_extension("test", &[TABLE, COLUMN]).unwrap();

        let error = store.migrate_extension("test", &[TABLE]).unwrap_err();

        assert!(error.to_string().contains("newer Flottille"), "{error}");
    }

    #[test]
    fn a_change_of_a_space_and_its_event_are_kept_together_or_not_at_all() {
        let mut store = Store::open_in_memory().unwrap();
        store.migrate_extension("test", &[TABLE]).unwrap();
        let space = store.space("test").unwrap();
        space
            .tables()
            .execute("INSERT INTO ext_test_notes (text) VALUES ('lost')", [])
            .unwrap();
        space.append_event("test.noted", json!({})).unwrap();
        drop(space);
        assert!(notes(&mut store).is_empty());
        assert_eq!(store.last_seq().unwrap(), 0);

        let space = store.space("test").unwrap();
        space
            .tables()
            .execute("INSERT INTO ext_test_notes (text) VALUES ('kept')", [])
            .unwrap();
        space.append_event("test.noted", json!({})).unwrap();
        space.commit().unwrap();
        assert_eq!(notes(&mut store), ["kept"]);
        assert_eq!(store.last_seq().unwrap(), 1);
    }

    /// A store as a build of the core alone leaves it at `version`, with an event.
    fn a_core_store(path: &Path, version: usize) {
        let old = Connection::open(path).unwrap();
        for migration in &MIGRATIONS[..version] {
            old.execute_batch(migration).unwrap();
        }
        old.pragma_update(None, "user_version", version).unwrap();
        append_event(&old, "test.one", json!({})).unwrap();
    }

    #[test]
    fn a_store_of_the_previous_schema_keeps_its_data_and_gains_the_extensions_tables() {
        // At every schema, not only one: where the extensions' bookkeeping is made must never
        // depend on how many migrations the core has.
        for version in 1..=MIGRATIONS.len() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("flottille.db");
            a_core_store(&path, version);

            let mut store = Store::open(&path).unwrap();
            store.migrate_extension("test", &[TABLE]).unwrap();

            assert_eq!(
                store.schema_version().unwrap(),
                MIGRATIONS.len(),
                "{version}"
            );
            assert_eq!(store.last_seq().unwrap(), 1, "{version}");
            assert!(notes(&mut store).is_empty(), "{version}");
        }
    }

    #[test]
    fn a_store_of_dev_s_latest_schema_gains_the_extensions_table_when_opened() {
        // The numbered migrations are the core's alone, so a store of the core's latest is this.
        assert!(!MIGRATIONS.iter().any(|m| m.contains("extension")));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flottille.db");
        a_core_store(&path, MIGRATIONS.len());
        Connection::open(&path)
            .unwrap()
            .execute_batch(
                "INSERT INTO workspaces (project, name, branch, folder, project_folder, \
                 created_at, variables, repositories) \
                 VALUES ('app', 'login', 'app/login', '/w', '/p', 100, '{}', '[]');",
            )
            .unwrap();

        let mut store = Store::open(&path).unwrap();
        store.migrate_extension("test", &[TABLE]).unwrap();

        assert_eq!(store.schema_version().unwrap(), MIGRATIONS.len());
        assert!(store.workspace("app", "login").unwrap().is_some());
        assert!(notes(&mut store).is_empty());
        // Opened again, nothing is made twice and the extension's count is kept.
        drop(store);
        let mut store = Store::open(&path).unwrap();
        store.migrate_extension("test", &[TABLE, COLUMN]).unwrap();
        assert!(notes(&mut store).is_empty());
    }

    #[test]
    fn a_store_of_this_branch_opens_under_the_latest_dev_schema() {
        // The alpha builds of the extensions made their bookkeeping as the last numbered
        // migration: at index 27 first, then at 30, each taking the number the core gave its own
        // next migration.
        for core in [27, 30] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("flottille.db");
            a_core_store(&path, core);
            let old = Connection::open(&path).unwrap();
            old.execute_batch(
                "CREATE TABLE extension_schemas (
                    name    TEXT PRIMARY KEY,
                    applied INTEGER NOT NULL
                ) STRICT;",
            )
            .unwrap();
            old.execute_batch(TABLE).unwrap();
            old.execute(
                "INSERT INTO extension_schemas (name, applied) VALUES ('test', 1)",
                [],
            )
            .unwrap();
            old.execute("INSERT INTO ext_test_notes (text) VALUES ('kept')", [])
                .unwrap();
            old.pragma_update(None, "user_version", core + 1).unwrap();
            drop(old);

            let mut store = Store::open(&path).unwrap();

            assert_eq!(store.schema_version().unwrap(), MIGRATIONS.len(), "{core}");
            // The core's migrations that took the same numbers are applied.
            for column in [
                "SELECT place FROM approval_requests",
                "SELECT ready FROM runs",
            ] {
                store.connection.prepare(column).unwrap();
            }
            // The extension's count is kept: its first migration is not run again.
            store.migrate_extension("test", &[TABLE, COLUMN]).unwrap();
            assert_eq!(notes(&mut store), ["kept"], "{core}");
            assert_eq!(store.last_seq().unwrap(), 1, "{core}");
            // Opened again, it is a store like any other.
            drop(store);
            assert_eq!(
                Store::open(&path).unwrap().schema_version().unwrap(),
                MIGRATIONS.len()
            );
        }
    }

    #[test]
    fn the_usage_of_a_removed_session_is_kept() {
        let mut store = Store::open_in_memory().unwrap();
        let agent = an_agent();
        store
            .create_agent(&agent, ("agent.created", json!({})))
            .unwrap();
        let row = TurnUsage {
            agent: agent.id.clone(),
            title: None,
            session: agent.id.clone(),
            project: "app".into(),
            workspace: Some("login".into()),
            provider: "claude".into(),
            role: "lead".into(),
            started_at: 1_792_843_730_808,
            ended_at: 1_792_843_731_808,
            stop_reason: "end_turn".into(),
            used: Used {
                model: Some("claude-fake-1".into()),
                input: Some(100),
                output: Some(20),
                cache_read: Some(1000),
                cache_write: Some(50),
                reasoning: None,
                cost_usd: Some(0.0125),
            },
        };
        store.record_turn_usage(std::slice::from_ref(&row)).unwrap();

        // Nothing removes an agent yet: the rows must neither hold its removal nor go with it.
        store
            .connection
            .execute("DELETE FROM agents WHERE id = ?1", params![agent.id])
            .unwrap();

        assert_eq!(store.agent(&agent.id).unwrap(), None);
        assert_eq!(store.turn_usage().unwrap(), [row]);
    }

    #[test]
    fn the_agent_that_wrote_a_service_is_kept_until_the_user_writes_it() {
        let mut store = Store::open_in_memory().unwrap();
        assert_eq!(store.service_writer("mongo").unwrap(), None);

        store.set_service_writer("mongo", Some("a1b2c3")).unwrap();
        assert_eq!(
            store.service_writer("mongo").unwrap(),
            Some("a1b2c3".to_owned())
        );
        store.set_service_writer("mongo", Some("d4e5f6")).unwrap();
        assert_eq!(
            store.service_writer("mongo").unwrap(),
            Some("d4e5f6".to_owned())
        );

        store.set_service_writer("mongo", None).unwrap();
        assert_eq!(store.service_writer("mongo").unwrap(), None);
    }
}
