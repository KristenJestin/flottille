# Reference: the daemon's children, how they are waited for and ended

The daemon is the child subreaper of everything it starts (`PR_SET_CHILD_SUBREAPER`): a process
orphaned anywhere below it is handed to the daemon, not to init. So the daemon has two kinds of
children, and waits for each in its own way.

## Waited for by knowing, not by guessing

- **The children it started.** Every process the daemon starts (agents, runs, tasks, setup steps,
  ready probes, git, the forges' and trackers' command lines, the judge, the model lists, the web
  reader, a provider's `--version`) is spawned through one place (`children.rs`), which records
  its pid with its start time (`/proc/<pid>/stat`, field 22) under a lock held across the spawn.
  Each is waited for by whoever started it, by its pid. A unit test refuses a spawn of the
  daemon's code that does not go through that place.
- **The ones it was handed.** On every `SIGCHLD`, and every 2 s, the sweep takes the same lock and
  waits for every zombie child of the daemon whose pid and start time are not recorded, whatever
  its session or group. A recorded child is never waited for by the sweep, so its owner always
  reads its exit status; a pid recorded once and reused since has another start time, and is
  waited for. Records of processes that are gone are dropped by the same sweep.

What a launcher leaves (pnpm starts its subcommands in a process group of its own and may end
without waiting for them) comes back to the daemon, and is waited for when it ends.

## Ended with what started it

A run, a task, a setup step and a ready probe each start in **a session of their own**
(`setsid`), which their first process leads; its process group is the session's first group.
A process can leave its group (`setpgid`) but cannot leave its session without starting one
(`setsid`), so the session holds everything the command started, even what a launcher put in a
group of its own and left behind.

When such a command ends (by itself, stopped, past its time limit, the daemon stopping, or found
left by a daemon that died), its group is killed, then every process of its session:
`/proc` is read for the processes in that session, and each is sent `SIGKILL` through a pidfd
opened before its session is read again, so a process that took the pid of one that ended
meanwhile is never signalled. This is read again, after a pause of a few milliseconds that grows
each time, until none is found, ten times at most: a process may start another as it is killed.
`/proc` is read off the daemon's async workers and never while the store is held.

This needs no cgroup, only `pidfd_open` (Linux 5.3 and later), so it works in a container or as a
systemd user service alike. Where `pidfd_open` is refused (an older kernel, a seccomp filter),
the daemon says so once in its log and ends only the command's process group: nothing is ever
signalled by its bare pid, so what a command left in a group of its own keeps running, and the
sweep waits for it once it ends. What escapes it everywhere: a process that starts a session of
its own (`setsid`, a daemon that detaches). It keeps running; once it ends, the sweep waits for
it.
