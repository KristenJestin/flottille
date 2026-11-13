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
on the runtime. The runtime beats every 500 ms; every request read counts as waiting until its
answer is queued. The daemon is **stuck** when its runtime has not beaten for 30 s, or when
requests wait and none was answered for 30 s (`FLOTTILLE_WATCHDOG_MS` sets the 30 s). Then the
watchdog writes, once, to the daemon's log (`daemon.log` in its state folder), each line starting
with `flottille daemon: watchdog:`:

- why it is stuck: how long the runtime has not run, how many requests wait;
- how many tasks the runtime has alive;
- every lock held, then every lock waited for, the longest first: its name, the line that took
  it, the thread (`main` is the runtime's), for how long;
- the commands that give the stacks of its threads.

When it answers again, it says so, and it would report the next time it is stuck.

A long request answered by a task of its own while no other request comes (a workspace prepared
from the command line, nothing else connected) passes for a stuck daemon: its report says that
the runtime ran a moment ago, which a stuck runtime never does.

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

`flottille daemon stop` asks the daemon to stop and waits for it to let go of its lock, 30 s at
most in all (`FLOTTILLE_STOP_WAIT_MS`). Past that, the daemon that still holds the lock, and every
process below it (its agents and their bridges, the runs, what they started), are killed
(`SIGKILL`, each through a pidfd opened first, so a pid taken since by another process is never
signalled), and the command says it had to: `The daemon did not stop (why): it was ended, with
the N process(es) it ran.` The next daemon settles what they left, as after any daemon that died
(`agents::kill_orphans`, `runs::settle_at_start`).

## The window

The window's link marks the request it waits for; the window says **Flottille is not answering**
when one waited more than 10 s, with since when and a button that restarts Flottille
(`flottille daemon stop`, then the link starts a new daemon), instead of anything it would say
of an empty store ("No project yet."). While the link has no request out, it asks the daemon's
status every 15 s, so that a daemon stuck while nothing happens is seen too.
