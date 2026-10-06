---
name: rho-agent-creator
description: >-
  Create a new Rho agent through a guided questionnaire.
disable-model-invocation: true
---

# Rho agent creator

Guide the user to one valid agent definition: collect decisions with the `questionnaire` tool, draft, confirm, `write` the file, then check it with `rho` action `agents`. Do not ask for anything the user already supplied.

The field contract is `docs/subagents/definition-schema.md` and the agent parser. Do not invent fields or values. Rho ships no built-in `claude-cli`, `cursor`, or `antigravity` agents; users who want one create it here.

## 1. Scope and identity

First call `rho` with `action: "agents"`. It returns absolute save directories (`dirs`) and the currently loaded agents. Write only to those paths; `write` does not expand `~`.

Then one questionnaire:

1. Location: `agents_home` (`~/.agents/agents`, shared), `rho_home` (`~/.rho/agents`), or `project` (`<project-root>/.agents/agents`, loaded only with `RHO_TRUST_PROJECT_AGENTS=1`; `dirs.project` is null when untrusted).
2. Agent ID (Other): 1-64 chars of lowercase ASCII letters, digits, and single hyphens; no leading, trailing, or double hyphen. File is `<id>.md`.
3. Description (Other): 1-1024 chars. It is the delegation metadata other agents use to decide when to call this one.

## 2. Role

Ask what the agent should do (Other). Follow up only where useful: may it modify files, what to avoid, what the final response contains, when to ask instead of proceeding.

## 3. Runtime

Runtime (harness) and model are separate axes. Do not pick an external runtime just because the user named a model ("Opus", "Gemini"); a normal Rho agent can pin that model through a configured provider.

| Runtime | Use when | Setup |
| --- | --- | --- |
| `rho` (default) | Rho's own loop and tools, any configured provider | none |
| `claude-cli` | child should run on Claude Code and spend a Claude.ai subscription | `claude` on `PATH`, `/login claude-code` |
| `cursor` | child should run on Cursor Agent with the Cursor sign-in | `cursor-agent` on `PATH`, `/login cursor` |
| `antigravity` | child should run on Google Antigravity (Gemini) with its Google sign-in | `agy_acp_server.par` + `localharness_external` on `PATH`, `/login antigravity` |

Explain before confirming any external runtime:

- Delegated only: interactive and `rho run` roots cannot bind it; a Rho parent launches it with the `agent` tool. Nested fan-out stays in Rho.
- Rho never sees or stores the external credential. For Claude this is the supported way to use a subscription, since Rho's Anthropic provider is API-key billing only.
- Permission modes: launch in Plan or Bypass. Supervised always refuses. Cursor and Antigravity also refuse Auto and Allow edits. Claude allows Auto and Allow edits only when every `tools:` entry is a proven no-prompt Claude built-in for that approval class (for example `Read`, `Glob`, `Grep`) and `inherit_claude_config` is false; specifiers like `Bash(git *)`, write/process tools, and plugin/MCP names refuse spawn.
- `provider`, `auth`, `fast`, and `@alias` models are Rho only.
- Cursor and Antigravity reject `prompt: replace` and `reasoning`; effort lives in the model id.

Emit `runtime` for external runtimes; omit it for `rho` unless the user wants it explicit.

For `claude-cli`, ask about `inherit_claude_config` (default `false`). `true` loads the user's full Claude settings (`user,project,local`) and blocks Auto and Allow edits; `false` keeps project-only settings.

## 4. Tools

Vocabularies never mix.

- `rho`: `tools: all` (default) or a focused multi-select from `agent`, `agents`, `bash`, `edit`, `fetch_content`, `get_search_content`, `glob`, `grep`, `list_dir`, `powershell`, `process`, `questionnaire`, `read_file`, `rho`, `shell`, `skill`, `todo`, `web_search`, `write`. Prefer focused lists for narrow roles.
- `claude-cli`: Claude Code entries like `Read`, `Edit`, `Glob`, `Grep`, `Bash(git *)`. No commas inside specifiers. Omitted means no tools; `all` is invalid. Presets: read-only `[Read, Glob, Grep]`; git-aware adds `Bash(git *)`; add `Edit`/`Write` only if the user wants workspace changes.
- `cursor`: required, nonempty, closed set: `read_tool_call`, `grep_tool_call`, `glob_tool_call`, `ls_tool_call`, `sem_search_tool_call`, `read_lints_tool_call`, `edit_tool_call`, `delete_tool_call`, `shell_tool_call`, `write_shell_stdin_tool_call`, `web_search_tool_call`, `web_fetch_tool_call`, `fetch_tool_call`, `mcp_tool_call`, `list_mcp_resources_tool_call`, `read_mcp_resource_tool_call`, `update_todos_tool_call`, `read_todos_tool_call`, `create_plan_tool_call`, `apply_agent_diff_tool_call`. This is the accepted vocabulary, not an exact fence. Before confirming, tell the user that Cursor fences by category: search tools (`grep`, `glob`, `ls`, `sem_search`, `read_lints`) always run, todo and plan tools are never fenced, and declaring one write, shell, fetch, or MCP name enables that whole category in Bypass (for example `edit_tool_call` also allows delete and apply-diff).
- `antigravity`: required, nonempty, closed set: `view_file` (read), `create_file`, `edit_file` (write), `run_command` (shell), `search_web`, `read_url_content` (network). Plan mode needs `view_file`. In Bypass, `run_command` runs anything the model asks.

## 5. Model and reasoning

**`rho`**: ask for `model-policy`: `inherit` (keep the parent's provider, model, and auth) or `prefer` / `require` / `select` (pin a model; do not invent finer differences). For a pin, ask for `model` and optional `provider` (non-empty, no whitespace; `@alias` allowed). If the provider has several logins, offer optional `auth` (for example `xai-oauth`, `xai-api-key`); unset keeps a compatible host login. When the pin supports fast mode (Codex GPT-5.5+ / GPT-6, or `xai/grok-4.7` with `auth: xai-oauth`), ask about `fast: true`; it bills higher and ignores the parent's `/fast`. Never emit `model`, `provider`, `auth`, or `fast` with `inherit`.

Reasoning: offer inherit (omit) plus the levels models.dev lists for the target model; if unknown, offer `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`.

**External runtimes**: `model` is an opaque pass-through string. Never guess ids from memory. Offer the presets below, an exact model the user already named, inherit (omit `model`; the harness picks), and Other (any non-empty string without whitespace, written verbatim). Name the default id in its label. Omit `model-policy` when `model` is set; only `inherit` (without `model`) or `select` (with `model`) are valid.

| Runtime | Presets (do not extend) | Reasoning |
| --- | --- | --- |
| `claude-cli` | `claude-opus-5` (default), `claude-sonnet-5`, `claude-fable-5-1` | optional `low`, `medium`, `high`, `xhigh`, `max` (`--effort`) |
| `cursor` | ids from `cursor-agent models`; bracket overrides allowed, e.g. `gpt-5.3-codex[effort=high,fast=false]` | none |
| `antigravity` | `gemini-3.8-flash-high` (recommended), `gemini-pro-agent` | none |

An Antigravity model the server does not offer fails the run before the prompt and lists the offered ids.

## 6. Prompt

Ask `extend` (append to the standard Rho prompt) or `replace` (body is the whole system prompt, must be self-contained) with `default: "extend"` and `default_selection: "focused"`. Skip the question for Cursor and Antigravity: only `extend` is valid, and the body is prepended to the first prompt.

Draft a concise body: role first, then operating rules, boundaries, and completion expectations. For external runtimes, the final child message returns verbatim to the parent, so demand a self-contained result.

## 7. Draft and confirm

Omit unselected optional fields. Examples:

```markdown
---
id: example-agent
description: Use for ... Not for ...
model-policy: inherit
reasoning: medium
tools: [read_file, grep, glob]
---

You are ...
```

```markdown
---
id: agy-reviewer
description: Reviews changes with Antigravity on Gemini. Requires /login antigravity. Not for root sessions.
runtime: antigravity
model: gemini-3.8-flash-high
tools: [view_file, run_command]
---

You are a code reviewer running under Antigravity for a Rho parent.

- Read before proposing edits.
- Return a self-contained review the parent can act on.
```

Show the destination path and the complete file, then confirm with a confirm questionnaire. Revise and reconfirm as needed.

## 8. Write and verify

Target `<dir>/<id>.md` with the chosen directory from `dirs`. Before writing, `read_file` that exact path, even if `agents` does not list it (a higher-precedence directory can shadow it). If it exists, show it and confirm overwrite first.

`write` the confirmed contents, then call `rho` with `action: "agents"` again:

- New path under `agents`: done.
- New path under `invalid`: fix the named `field`, rewrite, and recheck. Rho skips an invalid file, so the agent does not exist until the check is clean.
- Other files under `invalid`: they predate this change. Mention them, but do not edit them unless asked.

After a clean check:

1. Report the final path.
2. Ask the user to run `/agents`, which reloads definitions from disk. Do not claim an already initialized delegation tool schema changed; a new session is guaranteed to load the agent.
3. For external runtimes, remind them to install the binary, sign in (`/login claude-code`, `/login cursor`, or `/login antigravity`), check with `/doctor`, launch from a Rho parent in Plan or Bypass, and inspect runs with `rho attach <run-id>`.
4. For project agents, mention `RHO_TRUST_PROJECT_AGENTS=1`.
