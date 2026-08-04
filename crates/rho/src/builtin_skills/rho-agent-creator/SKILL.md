---
name: rho-agent-creator
description: Create a new Rho agent through a guided questionnaire. Use when the user wants to define a custom agent, subagent, delegated role, reusable specialist, Claude Code runtime agent, runtime: claude-cli definition, or an agent that spends a Claude Pro/Max subscription through the official claude binary.
---

# Rho agent creator

Guide the user through creating one valid agent definition. Do not jump directly to a file. Collect decisions step by step with the `questionnaire` tool, draft the definition, confirm it, write it safely, and verify it.

The authoritative field and value contract is the agent definition schema in the Rho docs (`docs/subagents.md`, section "Agent definition schema"). Prefer that schema over inventing fields or values.

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
- No Rho `provider`, no Rho `@alias` models, no `tools: all`
- `reasoning:` is optional and maps to Claude `--effort` (`low`, `medium`, `high`, `xhigh`, `max`). Do not emit `off` or `minimal`.

If the user still wants `claude-cli`, ask whether to set `inherit_claude_config: true`. Default is `false` (closed). Explain that the opt-in loads the user's full Claude settings (`user,project,local`); closed keeps project-only settings. Rho still does not store Claude credentials.

Emit `runtime: rho` only when making the Rho choice explicit; omit it to keep the default. Always emit `runtime: claude-cli` when that runtime is chosen.

### Tools

Then ask whether the agent should receive all tools (Rho only) or a focused allowlist. Tool names depend on runtime and never mix:

- `runtime: rho`: multi-select from `agent`, `agents`, `bash`, `apply_patch`, `edit_file`, `hashline_edit`, `fetch_content`, `get_search_content`, `glob`, `grep`, `list_dir`, `powershell`, `process`, `questionnaire`, `read_file`, `rho`, `shell`, `skill`, `web_search`, `write_file`. `tools: all` is allowed. Prefer a focused list when the role is narrow.
- `runtime: claude-cli`: collect Claude Code tool names such as `Read`, `Edit`, `Glob`, `Grep`, and patterns like `Bash(git *)`. Specifier interiors may contain nested parentheses and quotes, but not commas (Claude's list grammar cannot round-trip commas). Do not use Rho capability names. Omitting `tools` means no Claude tools. `tools: all` is not valid.

For Claude-cli starters, offer concrete presets when the user is unsure:

- read-only planning/review: `Read`, `Glob`, `Grep`
- git-aware review: `Read`, `Glob`, `Grep`, `Bash(git *)`
- implementation: add `Edit` / `Write` only if the user explicitly wants workspace changes

### Reasoning

Ask for a reasoning level:

- `runtime: rho`: inherit/default, off, minimal, low, medium, high, xhigh, max. Omitting `reasoning` means the selected model's normal default.
- `runtime: claude-cli`: inherit/default, low, medium, high, xhigh, max. Omitting `reasoning` inherits Claude's default effort. Map the chosen value to Claude `--effort`. Never emit `off` or `minimal` for Claude.

## 4. Model policy

For `runtime: rho`, ask for one model policy: `inherit`, `prefer`, `require`, or `select`. Explain that `inherit` keeps the parent agent's provider and model, while every other policy names a model selection. Do not invent finer behavioral differences between the non-inherit policies.

If the answer is not `inherit`, ask for the model ID and optional provider. Both values must be non-empty and contain no whitespace when present. A model is required for `prefer`, `require`, and `select`. Do not emit `model` or `provider` for `inherit`. Rho may resolve `@alias` model values against `[model.aliases]`.

For `runtime: claude-cli`, do **not** invent or guess Claude model IDs from memory, marketing names, or Rho provider catalogs beyond the recommended list below. Claude `--model` is an opaque pass-through string. Ask with a choice questionnaire that names the default explicitly:

1. **Use default (`claude-opus-5`)** - emit `model: claude-opus-5`. Always say the default id in the label/help so the user knows exactly what will be written.
2. **`claude-sonnet-5`** - emit `model: claude-sonnet-5`
3. **`claude-fable-5`** - emit `model: claude-fable-5`
4. **Inherit Claude default** - omit `model` (and omit `model-policy`, or use `inherit` without a model). Explain this leaves model selection to the installed `claude` binary, not Rho.
5. **Other** - let the user type any non-empty model string with no whitespace (Claude model id or Claude alias). Emit that exact string as `model`. Do not rewrite, normalize, or "correct" it.

Recommended preset ids only (do not expand this list from memory):

- `claude-opus-5` (default when the user accepts the default)
- `claude-sonnet-5`
- `claude-fable-5`

Never emit `provider`. Prefer omitting `model-policy` when a model is set (parser treats that as select). If you emit `model-policy`, only `inherit` or `select` are valid. Reject empty model values. Do not combine `model-policy: inherit` with an explicit `model`, and do not use `model-policy: select` without `model`. Claude models are not Rho `@alias` values (`@name` is rejected).

If the user already named a specific Claude model earlier in the conversation, offer that exact string as a focused choice alongside the recommended presets and Other, still without inventing additional options.

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
model: claude-opus-5
reasoning: high
tools: [Read, Glob, Grep]
inherit_claude_config: false
---

You are a planning specialist running under Claude Code for a Rho parent.

- Prefer reading before proposing edits.
- Return a self-contained plan the parent can act on.
- Do not claim you can open nested Task agents; fan-out stays in Rho.
```

`prompt` must be `extend` or `replace`. `runtime` must be `rho` or `claude-cli`. For Rho, `model-policy` must be `inherit`, `prefer`, `require`, or `select`, and `tools` must be `all` or a YAML list of Rho capability names. For Claude, never set `provider`, `tools` must be a YAML list of Claude tool names or patterns, and `reasoning` when set must be one of `low`, `medium`, `high`, `xhigh`, `max`. Present the exact destination path and complete proposed file to the user, then ask for confirmation with a confirm questionnaire. Revise and reconfirm if requested.

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
