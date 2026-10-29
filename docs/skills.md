# Skills

A skill is a text a project's agents read with a tool when their task needs it, in place of the
knowledge skills (framework guides, a test library, a design system) that other tools load every
time. It is Flottille's data about the project: kept in the project's file in Flottille's
configuration, its text, when it has one, in Flottille's data folder, never in the project's
repositories.

## The file

`[skills.<name>]` in the project's file (`docs/projects.md`) holds:

| Key | |
|---|---|
| `description` | One line, not empty: when the skill is worth reading. |
| `path` | A file or a folder of files, relative to the project folder (a file of a repository is `<repository>/<file>`). It must stay inside the project folder, once the links are followed, and not go through a `.git`. |

A skill with no `path` is a **text skill**: its text is kept by Flottille in
`$XDG_DATA_HOME/flottille/skills/<project>/<name>.md`, written there when it is added or edited
(the file replaced at once) and removed with the skill.

```toml
[skills.react]
description = "How we write React components"
path = "docs/react"

[skills.review]                 # a text Flottille keeps
description = "How we review a pull request"
```

The name is a project name's kind of word (letters, digits, `.`, `-`, `_`).

**Former name.** Until #127 a skill was a *reference*, under `[references.<name>]`, always a path.
The former key is still read for one release, and the daemon renames it in the file at its start,
comments kept (`docs/projects.md`, "Former keys").

## Reading one

An agent sees the names and descriptions in its instructions, after the cache boundary, never the
content (`docs/design.md` §8); it reads one with `skill_read` (`docs/tools.md`): a file or a text
skill by its name, a page at a time, a folder skill listed, then one of its files by `path`. Only
the skill's own files are served, never one outside it, never a file of secrets. The list is
frozen in an agent's instructions when the agent is created, so a skill added later is seen by the
agents started after it; `skill_read` itself follows the skill as it is now. A path skill is read
from the project folder, whatever the agent's place: it is the project's, not the branch's.

## Editing them

- **The window**: the project's Settings, section Skills: add (a path picked in the project
  folder, or a text written in the form), edit, remove (a second click confirms).
- **The command line**: `flottille skill list|show|add|edit|remove` (`docs/cli.md`).
- **An agent**: a root's `skills` tool (`docs/tools.md`, "The project"): `list` and `show` run at
  once, `add`, `edit` and `remove` are calls to judge, said in words.

Each writes the project's file where it stands, the rest of it left as written, comments
included. The methods are `skill.list`, `skill.show`, `skill.add`, `skill.edit` and `skill.remove`
(`docs/protocol.md`).
