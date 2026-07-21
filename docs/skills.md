# Skills

Skills give Rho reusable instructions for a task. Each skill lives in a
`SKILL.md` file. Rho shows the model each skill's name and description through
the [`skill` tool](/tools-workspace). The model loads the full instructions only
when it uses that skill, which leaves more context for your work.

## `SKILL.md` format

Create a directory for the skill and add a `SKILL.md` file. Start the file with
YAML front matter between `---` lines, then write the instructions in Markdown:

```text
---
name: inspect-logs
description: Find and summarize errors in application logs. Use when asked to inspect or triage log files.
---

Read the relevant log files with the file tools, group errors by message,
and report the most frequent failures first with file and line references.
```

The file has three parts:

- `name` (required): Use 1–64 lowercase letters, numbers, or single hyphens. The
  name must match the directory name. For example, `inspect-logs/SKILL.md` uses
  `name: inspect-logs`.
- `description` (required): Tell the model what the skill does and when to use
  it. A specific description helps the model choose the right skill.
- Body: Write the instructions after the front matter. Rho loads this text as
  written when the model uses the skill.

## Where Rho looks for skills

Rho checks these locations in order:

```text
built-in skills (shipped with Rho)
~/.rho/skills/<name>/SKILL.md
~/.agents/skills/<name>/SKILL.md
<project>/.agents/skills/<name>/SKILL.md   (nearest directory first, up to the repository root)
```

If Rho finds the same skill name more than once, it uses the first copy. You
can't replace a built-in skill with a user skill. Skills in your home directory
take priority over project skills. Within a project, Rho starts at the working
directory and searches up to the repository root.

## Add a skill

You don't need an install command. Choose where the skill should live:

- `~/.rho/skills` makes it available only to Rho.
- `~/.agents/skills` shares it with Rho and other agents that use this layout.
- `<project>/.agents/skills` shares it with everyone who works in that
  repository.

Create a directory whose name matches the skill name, then add `SKILL.md`:

```sh
mkdir -p ~/.agents/skills/inspect-logs
touch ~/.agents/skills/inspect-logs/SKILL.md
```

Open the new file and follow the format above. To add a third-party skill, copy
its directory into one of these locations.
Read its instructions before you use it.

## Built-in skills

Rho includes two built-in skills:

| Skill | Use |
| --- | --- |
| `rho-diagnostics` | Inspect harness diagnostics |
| `rho-agent-creator` | Define an agent through a guided questionnaire |

See [Tools and workspace](/tools-workspace) for details about the `skill` tool.
See [Agents and delegation](/subagents) to learn how to define agents.
