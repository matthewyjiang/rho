# Skills

Skills are reusable instruction sets that Rho loads on demand. Each skill is a
`SKILL.md` file describing how to perform a task. Rho exposes available skills to
the model by name and description through the [`skill` tool](/tools-workspace),
and loads a skill's full content only when it is used, so many skills can be
available without filling the context.

## SKILL.md format

A skill is a directory containing a `SKILL.md` file. The file begins with a
frontmatter block delimited by `---`, followed by the Markdown instructions:

```text
---
name: inspect-logs
description: Find and summarize errors in application logs. Use when asked to inspect or triage log files.
---

Read the relevant log files with the file tools, group errors by message,
and report the most frequent failures first with file and line references.
```

- `name` (required) must be 1–64 characters, lowercase alphanumeric with single
  hyphen separators, and must match the skill's directory name
  (`inspect-logs/SKILL.md` uses `name: inspect-logs`).
- `description` (required) is what Rho shows the model to decide when the skill
  applies, so make it specific about when to use it.
- Everything after the frontmatter is the skill body, loaded verbatim when the
  skill is used.

## Discovery and precedence

Rho discovers skills from these locations, and the first skill found for a given
name wins:

```text
built-in skills (shipped with Rho)
~/.rho/skills/<name>/SKILL.md
~/.agents/skills/<name>/SKILL.md
<project>/.agents/skills/<name>/SKILL.md   (nearest directory first, up to the repository root)
```

Because built-in skills are resolved first, a user skill cannot override a
built-in of the same name. Home skills take precedence over project skills, and
within a project the directory nearest the working directory wins.

## Adding a custom or third-party skill

There is no install command — a skill is a plain file. Create the directory and
`SKILL.md`:

```text
mkdir -p ~/.agents/skills/inspect-logs
# then write ~/.agents/skills/inspect-logs/SKILL.md
```

`.agents/skills` is a shared convention across agents, so a `SKILL.md` placed
there is available to Rho and to other agents that follow the same layout. Use
`~/.rho/skills` for skills scoped to Rho only, or a project's `.agents/skills` to
share a skill with everyone who works in that repository.

## Built-in skills

Rho ships these skills:

```text
rho-diagnostics    inspect harness diagnostics
rho-agent-creator  define a new agent through a guided questionnaire
```

See [Tools and workspace](/tools-workspace) for the `skill` tool and
[Agents and delegation](/subagents) for agent definitions.
