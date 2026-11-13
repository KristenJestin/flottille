# Reference: a daemon that stops answering

The daemon's runtime has **one thread** (`daemon::run`, a current-thread tokio runtime). Every
request, every agent's task, every subscriber runs on it, the methods "answered by a task of their
own" included: a task of its own lets the connection read the next request, it does not give the
work another thread. So anything that blocks that thread (a `std::sync::Mutex` taken while it is
held, by the same task or by another one on the same thread; a synchronous wait on a child, a
pipe or a file) stops the whole daemon at once: nothing more is written to its log nor its store,
the requests are accepted by the kernel on the socket and never read, and the processes that
write to it (the agents' bridges) block in turn. Only the threads off the runtime still run: the
blocking pool (`spawn_blocking`: the file tools, the snapshots of agent folders, the providers'
checks) and the watchdog.

## The named locks

The daemon's locks are `locks::Lock`, a `std::sync::Mutex` that keeps, while it is held, which
thread holds it, the line of the code that took it (`#[track_caller]`: the store's lock names the
caller of `Daemon::read`, `write` or `emit`, not `daemon.rs`) and since when, and the same of
every thread that waits for it. It answers as the standard one does (`lock()` gives a
`LockResult`), and costs a few short updates of one table per lock taken. The store is `store`,
the other fields of `Daemon` their own names; those of the sub-systems (`compactions`, the agents'
tasks, the forges, the items, the MCP server, the runs, the subscriptions) are known by the line
that took them.

A lock taken by the thread that holds it waits for ever: the daemon says it in its log as it
happens, with both lines.

The guard is in `clippy.toml`'s `await-holding-invalid-types`: held across an `.await`, it is
refused as the standard guard is.

## The watchdog

A plain OS thread (`watchdog`, named `flottille-watchdog`), started with the daemon, that is not
on the runtime. Each quarter of its limit it hands the runtime a probe, a task that does nothing
but say it ran, unless the last one has not run yet: an idle daemon is woken once every 7.5 s for
it, never more (`daemon_at_rest`). Every request answered on its connection counts as waiting
until its answer is queued; one answered by a task of its own (`workspace.prepare`, `agent.start`,
`task.run`…) does not, as it may wait on work outside the daemon (git, a setup step, an agent) for
as long as that takes. The daemon is **stuck** when a probe has waited 30 s, or when requests wait
and none was answered for 30 s (`FLOTTILLE_WATCHDOG_MS` sets the 30 s, 100 ms at least). Then the
watchdog writes, once, to the daemon's log (`daemon.log` in its state folder), each line starting
with `flottille daemon: watchdog:`:

- why it is stuck: how long the runtime has not run, how many requests wait;
- how many tasks the runtime has alive;
- every lock held, then every lock waited for, the longest first: its name, the line that took
  it, the thread (`main` is the runtime's), for how long;
- the commands that give the stacks of its threads.

When it answers again, it says so, and it would report the next time it is stuck.

The gate's verdict on an agent's call (`mcp::judged_off`) is made on the blocking pool, off the
runtime's thread: it reads the file system (a folder walked for a secret, paths resolved), and
asks git whether a branch is pushed, 5 s at most (`PUSHED_WAIT`), a git that does not answer
then taken for "pushed", the safer class.

## Before ending a stuck daemon

Its stacks say what each thread waits for, which the log alone may not. With the daemon's pid
(`flottille status --json` if it still answers, else the first line of `daemon.lock` in the
runtime folder, `$XDG_RUNTIME_DIR/flottille`):

```
sudo eu-stack -p <pid> > flottille-stacks.txt
sudo gdb -p <pid> -batch -ex "thread apply all bt" > flottille-stacks.txt
```

Either one is enough (`eu-stack` comes with elfutils, `gdb` with gdb). Where the kernel lets a
process trace only its own descendants (`/proc/sys/kernel/yama/ptrace_scope` at 1, the default of
most distributions), run it with `sudo`: the daemon does not open itself to every process of the
user, its agents' commands included, which could then read its memory and the vault's secrets in
it. A release build keeps its symbols and the line tables of Flottille's own crates (`[profile.release]`,
compressed by `tools/package.sh`), so each frame says its function, and Flottille's its file and
line.

Then `flottille daemon stop`.

## `daemon stop` always ends

`flottille daemon stop` asks the daemon to stop. A daemon that refuses (the command was started
by an agent) is not stopped, and the command fails with its refusal. A daemon that does not answer
within 30 s
(`FLOTTILLE_STOP_WAIT_MS`) is ended at once, unless the command descends from it: a stuck daemon
cannot refuse a client an agent started, so the command refuses itself (`refused: this client was
started by an agent; it does not end the daemon`), and one whose ancestry cannot be read counts as
such a client. One that answered is stopping its agents, one after
another and up to 4 s each, so it is given 30 s more and 5 s for each process it runs before it is
ended in turn.

Ending it: the daemon and the processes below it that are its own, those of its session (its
agents and their process groups) and of the sessions it started (runs, setup steps, probes), are
sent `SIGTERM`, then, 2 s later, `SIGKILL` for those left (a git killed outright leaves
its `index.lock`). As their subreaper the daemon holds more: a helper that something it ran left
detached in a session of its own (an ssh control master, an agent of gpg, git's credential cache)
may serve the user's terminals too, and is spared, even one handed to it as its child once its
parent exited, as git's `daemonize()` and glibc's `daemon()` do. The command runs outside the
daemon, which cannot be asked when it is stuck: the daemon writes the sessions it starts in
`$XDG_RUNTIME_DIR/flottille/daemon.sessions` as it starts them (each leader's pid and start time,
so that one whose pid was taken since is not one of them). A session counts while its leader runs
with that start time, or, once the leader exited, while a process still runs in it: what a run
left behind it is ended too, and Linux gives the leader's pid to no other process meanwhile. Each process is signalled through a pidfd,
opened, then checked to be still in one of those sessions, so that a pid taken since by another
process is never signalled; the daemon's own is opened before it is asked to stop. The command
says it had to: `The daemon did not stop (why): it was ended, with the N process(es) it ran.` The
next daemon settles what they left, as after any daemon that died (`agents::kill_orphans`,
`runs::settle_at_start`).

## The window

The window's link marks the request it waits for; when one waited more than 30 s (the watchdog's
limit), a banner above the page says **Flottille is not answering**, since when, with a button
that restarts Flottille (`flottille daemon stop`, then the link starts a new daemon; the button
says "Restarting…" and cannot be clicked again until it ended). The banner unfolds and fades in,
and folds away and fades out once Flottille answers again or its connection broke; the page under
it stays. Before Flottille was read once, an empty page says nothing, never "No project yet.". While the link has no request out, it asks the daemon's
status every 15 s, so that a daemon stuck while nothing happens is seen too.
