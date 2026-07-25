---
name: rho-agent-creator
description: Create a new Rho agent through a guided questionnaire. Use when the user wants to define a custom agent, subagent, delegated role, reusable specialist, Claude Code runtime agent, runtime: claude-cli definition, or an agent that spends a Claude Pro/Max subscription through the official claude binary.
---

# Rho agent creator

Guide the user through creating one valid agent definition. Do not jump directly to a file. Collect decisions step by step with the `questionnaire` tool, draft the definition, confirm it, write it safely, and verify it.

Rho ships no built-in `runtime: claude-cli` agent. If the user wants Claude Code or a Claude subscription-backed specialist under Rho, create a user-defined agent with this skill.

Agent definitions are Markdown files with YAML frontmatter and a prompt body. Valid discovery locations are:

- `~/.agents/agents/<id>.md` for the shared agents home
- `~/.rho/agents/<id>.md` for Rho-specific global agents
- `<project-root>/.agents/agents/<id>.md` for a project agent

Project agents are loaded only when the project is trusted, currently by starting Rho with `RHO_TRUST_PROJECT_AGENTS=1`.

## 1. Scope and identity

Use one questionnaire with these choices:

1. Save location: shared global (`~/.agents/agents`), Rho global (`~/.rho/agents`), or current project (`<project-root>/.agents/agents`).
2. Agent ID: offer an Other response so the user can enter it.
3. Description: offer an Other response. Explain that this is the short delegation metadata other agents use to decide when this agent is appropriate.

Validate the ID before continuing. It must contain 1-64 lowercase ASCII letters, digits, or single hyphens, with no leading hyphen, trailing hyphen, or double hyphen. The destination filename must be `<id>.md`. The description must contain 1-1024 characters.

## 2. Role and behavior

Ask what the agent should accomplish and how it should behave. Use an Other response for the role/instructions. Ask follow-up choices when useful, such as whether it may modify files, what it should avoid, what its final response should contain, and when it should ask the user rather than proceed. Keep this conversational and do not ask for information the user already supplied.

## 3. Runtime and capabilities

First choose the harness. Runtime and model are separate axes.

### When to pick each runtime

- `rho` (default): Rho's own loop and Rho tool capabilities. Use this for normal subagents, including ones that call Anthropic or other providers with API keys / OAuth already configured in Rho.
- `claude-cli`: the external `claude` binary. Use this only when the user wants a **delegated** child that runs on Claude Code and can spend a Claude.ai Free/Pro/Max subscription.

Anthropic does not allow third-party clients to put Claude.ai subscription credentials on their own API stacks. Rho's Anthropic provider path is API-key billing only. `runtime: claude-cli` is the supported **indirect** workaround: Rho stays the parent orchestrator, and the official `claude` binary owns sign-in, the child loop, and plan usage. Rho never sees or stores the subscription token.

Do **not** choose `claude-cli` merely because the user said "Opus" or "Claude". If they only want a model through Rho's normal provider path, keep `runtime: rho` and set model/provider later.

### Claude-cli constraints to explain before confirming that runtime

- Delegated only. Interactive and `rho run` roots cannot bind `runtime: claude-cli`. A Rho parent must launch the agent through the `agent` tool.
- Requires the `claude` binary on `PATH` and a Claude Code login (`/login claude-code`). Offer to remind the user after write if they have not signed in yet.
- Launch under Plan or Auto. Supervised mode refuses Claude-cli spawn because `claude -p` cannot prompt through Rho.
- No nested Claude `Task` agents. Fan-out stays under Rho.
- No Rho `provider`, no Rho `@alias` models, no `reasoning` field, no `tools: all`.

If the user still wants `claude-cli`, ask whether to set `inherit_claude_config: true`. Default is `false` (closed). Explain that the opt-in loads the user's full Claude settings (`user,project,local`); closed keeps project-only settings. Rho still does not store Claude credentials.

Emit `runtime: rho` only when making the Rho choice explicit; omit it to keep the default. Always emit `runtime: claude-cli` when that runtime is chosen.

### Tools

Then ask whether the agent should receive all tools (Rho only) or a focused allowlist. Tool names depend on runtime and never mix:

- `runtime: rho`: multi-select from `agent`, `agents`, `bash`, `edit_file`, `fetch_content`, `get_search_content`, `list_dir`, `powershell`, `process`, `questionnaire`, `read_file`, `rho`, `shell`, `skill`, `web_search`, `write_file`. `tools: all` is allowed. Prefer a focused list when the role is narrow.
- `runtime: claude-cli`: collect Claude Code tool names such as `Read`, `Edit`, `Glob`, `Grep`, and patterns like `Bash(git *)`. Specifier interiors may contain nested parentheses and quotes, but not commas (Claude's list grammar cannot round-trip commas). Do not use Rho capability names. Omitting `tools` means no Claude tools. `tools: all` is not valid.

For Claude-cli starters, offer concrete presets when the user is unsure:

- read-only planning/review: `Read`, `Glob`, `Grep`
- git-aware review: `Read`, `Glob`, `Grep`, `Bash(git *)`
- implementation: add `Edit` / `Write` only if the user explicitly wants workspace changes

### Reasoning

Ask for a reasoning level only on `runtime: rho`: inherit/default, off, minimal, low, medium, high, xhigh, max. Omitting `reasoning` means the selected model's normal default.

Never emit `reasoning` for `runtime: claude-cli`. Claude keeps its own defaults; Rho does not map reasoning to Claude `--effort`.

## 4. Model policy

For `runtime: rho`, ask for one model policy: `inherit`, `prefer`, `require`, or `select`. Explain that `inherit` keeps the parent agent's provider and model, while every other policy names a model selection. Do not invent finer behavioral differences between the non-inherit policies.

If the answer is not `inherit`, ask for the model ID and optional provider. Both values must be non-empty and contain no whitespace when present. A model is required for `prefer`, `require`, and `select`. Do not emit `model` or `provider` for `inherit`. Rho may resolve `@alias` model values against `[model.aliases]`.

For `runtime: claude-cli`, allow an optional `model` passed through byte-for-byte as Claude `--model` (Claude model id or Claude alias such as `opus`). Never emit `provider`. Prefer omitting `model-policy`, or use `inherit` / `select` only. Reject empty model values. Do not combine `model-policy: inherit` with an explicit `model`, and do not use `model-policy: select` without `model`. Claude models are not Rho `@alias` values. Omitting `model` lets Claude use its own default.

## 5. Prompt policy

Ask whether the body should:

- `extend` the standard Rho system prompt
- `replace` the system prompt completely

Use a choice questionnaire with `default: "extend"` and `default_selection: "focused"` so extend is recommended without being pre-selected. Explain that `replace` needs a non-empty, self-contained body. Draft a concise body from the user's answers. It should state the role first, then give concrete operating rules, boundaries, and completion expectations. Do not repeat metadata merely to make the body longer.

For Claude-cli agents, still write the body for the child role. Remind the user that the final child message returns verbatim to the Rho parent, so the body should demand a self-contained result.

## 6. Draft and confirm

Construct valid content in this shape, omitting optional fields that were not selected. Rho example:

```markdown
---
id: example-agent
description: Use for ... Not for ...
prompt: extend
runtime: rho
model-policy: inherit
reasoning: medium
tools: [read_file, list_dir]
---

You are ...

- ...
```

Claude Code example (subscription-backed delegated specialist):

```markdown
---
id: claude-planner
description: Plans with Claude Code on the user subscription. Requires /login claude-code. Not for Rho-native tools or root sessions.
runtime: claude-cli
model: claude-opus-4-6
tools: [Read, Glob, Grep]
inherit_claude_config: false
---

You are a planning specialist running under Claude Code for a Rho parent.

- Prefer reading before proposing edits.
- Return a self-contained plan the parent can act on.
- Do not claim you can open nested Task agents; fan-out stays in Rho.
```

`prompt` must be `extend` or `replace`. `runtime` must be `rho` or `claude-cli`. For Rho, `model-policy` must be `inherit`, `prefer`, `require`, or `select`, and `tools` must be `all` or a YAML list of Rho capability names. For Claude, never set `provider` or `reasoning`, and `tools` must be a YAML list of Claude tool names or patterns. Present the exact destination path and complete proposed file to the user, then ask for confirmation with a confirm questionnaire. Revise and reconfirm if requested.

## 7. Write safely and verify

Before writing, inspect the destination. If `<id>.md` already exists, read it and ask for explicit overwrite confirmation. Never overwrite based only on the earlier draft confirmation. Create the destination directory if needed using the available platform shell, then use `write_file` for the definition.

After writing:

1. Read the file back.
2. Check that the frontmatter delimiters, ID, description, runtime, policies, tools, and non-empty body are present and match the confirmed draft.
3. Correct only clear serialization mistakes. For any semantic change, ask first.
4. Tell the user the final path.
5. Ask the user to run `/agents`. Opening `/agents` reloads definitions from disk and shows the new agent, including `runtime` and tool vocabulary.
6. For `runtime: claude-cli`, also tell the user to:
   - install `claude` if needed
   - run `/login claude-code` if not already signed in
   - use Plan or Auto before delegating
   - launch it from a Rho parent via the `agent` tool (not as the interactive root)
   - optionally confirm binary/auth with `/doctor` and inspect later runs with `rho attach <run-id>`

Mention that project-scoped agents need project trust. Do not claim that an already initialized delegation tool schema has changed merely because the file was written. The new agent is guaranteed to be available after starting a new Rho session that loads it.
