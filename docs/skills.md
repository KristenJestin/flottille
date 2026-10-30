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

## Importing a `.claude` folder

A project that already has skills in the Anthropic layout need not write them again. A skill is
`.claude/skills/<name>/SKILL.md`, a Markdown file that starts with YAML front matter:

```markdown
---
name: react
description: How we write React components
---
Use hooks, never classes.
```

Flottille looks at the project folder's `.claude/skills` (its repositories' own `.claude` folders
are not looked at) and offers each folder holding a `SKILL.md`. Importing one writes
`[skills.react]` in the project's file with `description` from the front matter and
`path = ".claude/skills/react/SKILL.md"`: a skill that **points at the file**, not a copy of it,
so it is read as it is on disk at each use. The keys of the front matter other than `name` and
`description` (`allowed-tools`, `model`…) belong to the other tool and are left alone. A
description on several lines is made one. **Nothing is written in the project's folder.**

A file that cannot be imported whole is shown with why and never imported half: no front matter,
a front matter never closed or that is not YAML, no `name` or no `description`, a name that cannot
be one (`docs/projects.md`), a name another file already has, a file that cannot be read (not
UTF-8 text, larger than 1 MiB), a link that leads out of the project folder, a folder with no
`SKILL.md`. A name the project already uses is not overwritten: the file is said `taken`. A file
already imported is said `imported` and not offered again.

An agent is served the skills of `.claude/skills` as any other path of the project, although the
gate holds `.claude` wherever it sits, because an agent's own login folder is one
(`docs/tools.md`, "`skill_read`"): the exception covers only what is below the project's
`.claude/skills`, where a `.env`, a link out or a nested `.claude` are held as anywhere else.

The methods are `skill.detect` and `skill.import` (`docs/protocol.md`), the command
`flottille skill import` (`docs/cli.md`). The window's project page offers what is detected, each
with Import and an Import all (not built yet: it waits for the layout validated in the window's
review).

## Editing them

- **The window**: the project's Settings, section Skills: add (a path picked in the project
  folder, or a text written in the form), edit, remove (a second click confirms).
- **The command line**: `flottille skill list|show|add|edit|remove` (`docs/cli.md`).
- **An agent**: a root's `skills` tool (`docs/tools.md`, "The project"): `list` and `show` run at
  once, `add`, `edit` and `remove` are calls to judge, said in words.

Each writes the project's file where it stands, the rest of it left as written, comments
included. The methods are `skill.list`, `skill.show`, `skill.add`, `skill.edit` and `skill.remove`
(`docs/protocol.md`).
