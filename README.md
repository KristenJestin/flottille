# Flottille

Flottille runs coding agents on your own machine. Each agent is a real process that Flottille
owns, works in git worktrees Flottille prepared (several repositories at once if your project has
them), and uses only the tools Flottille gives it. When an agent launches other agents, they stay
tied to it: Flottille tells the parent when a child reports, fails, gets stuck or waits for you,
and wakes it when nothing happens for too long.

A daemon does the work; a command line (`--json`, `watch`), a local MCP server and a window are
its clients. Agents are driven over [ACP](https://agentclientprotocol.com); Flottille runs the
agent CLIs you installed and signed in to yourself (Claude Code first).

Early work in progress: see [`docs/design.md`](docs/design.md).

## Installing

On Linux x86_64, into `~/.local/bin`, without root:

```
curl -fsSL https://raw.githubusercontent.com/KristenJestin/flottille/main/install.sh | sh
```

`sh -s -- --channel beta` takes the build of `dev`, `--channel alpha --branch <name>` a feature
branch's; `flottille update` takes the latest of its channel. See
[`docs/releases.md`](docs/releases.md).

## License

AGPL-3.0-only.
