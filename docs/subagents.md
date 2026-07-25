# Agents and delegation

Rho uses one agent definition model for interactive sessions, `rho run`, and delegated work. The built-in catalog contains:

- `default` - standard root coding-agent behavior
- `explorer` - fast read-only investigation
- `reviewer` - read-only code review
- `worker` - independent implementation

Select an agent at startup:

```bash
rho --agent reviewer
rho run --agent worker "address the issue"
```

Agent switching within an active session is intentionally unsupported.

## Definition files

Agent definitions are Markdown with strict frontmatter. The Markdown body extends the base coding prompt by default:

```markdown
---
id: security-review
description: Reviews changes for security defects
runtime: rho
model-policy: inherit
reasoning: high
tools: [read_file, list_dir, bash]
---
Review the requested changes. Do not modify files.
```

Definitions are discovered deterministically from built-ins, `~/.agents/agents`, `~/.rho/agents`, and trusted project `.agents/agents` directories, with later sources taking precedence. Project definitions are ignored unless `RHO_TRUST_PROJECT_AGENTS=1`, so an untrusted checkout cannot affect prompts, models, or tools. Duplicate IDs within one precedence level are errors. The file name supplies `id` when the field is omitted.

For the full value set, constraints, and defaults, see [Agent definition schema](#agent-definition-schema).

### Quick field summary

| Field | Required | Meaning |
| --- | --- | --- |
| `id` | no | Stable lowercase identifier; defaults to the file name |
| `description` | yes | Description shown by the `agent` tool |
| `runtime` | no | Execution harness: `rho` (default) or `claude-cli` |
| `prompt` | no | `extend` (default) or `replace` |
| `model-policy` | no | For `runtime: rho`: `inherit`, `prefer`, `require`, or `select`. For `runtime: claude-cli`: omit, `inherit`, or `select` |
| `model` | policy-dependent | Model selected by non-inherit policies. On `runtime: rho`, use `@name` to reference a [model alias](/configuration#model-aliases). On `runtime: claude-cli`, the value is passed through as Claude's `--model` and must be a Claude model name or Claude alias such as `opus` (Rho `@alias` references are rejected) |
| `provider` | no | Provider selected with the model. Valid only for `runtime: rho`; rejected on `runtime: claude-cli` |
| `reasoning` | no | For `runtime: rho`: `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, or `max`. For `runtime: claude-cli`: maps to Claude `--effort` as `low`, `medium`, `high`, `xhigh`, or `max`. Omit to inherit Claude's default. `off` and `minimal` are rejected on `claude-cli` |
| `tools` | no | Tool allowlist. Vocabulary depends on `runtime` (see below) |
| `inherit_claude_config` | no | `true` or `false` (default). Opt in only with `runtime: claude-cli` to load the user's full Claude settings (`user,project,local`). Default stays closed |

## Agent definition schema

This is the parse contract for agent Markdown files. Unknown frontmatter keys fail. Invalid values fail before execution. Field order does not matter; `runtime` is resolved before `tools`.

### File shape

```text
---
<yaml-like frontmatter keys>
---
<markdown body>
```

| Part | Rule |
| --- | --- |
| Frontmatter | Starts and ends with a line that is exactly `---` |
| Body | Markdown after the closing `---`. Trimmed. Used by `prompt` |
| Encoding | Text file, one agent |

### Frontmatter fields

| Field | Type | Required | Default | Allowed values / constraints |
| --- | --- | --- | --- | --- |
| `id` | string | no | file stem (`name` in `name.md`) | 1-64 chars; lowercase ASCII letters, digits, single hyphens only; no leading/trailing/double hyphen |
| `description` | string | yes | - | 1-1024 Unicode characters after trim; empty rejected |
| `runtime` | enum | no | `rho` | `rho` \| `claude-cli` |
| `prompt` | enum | no | `extend` | `extend` \| `replace`. `replace` requires a non-empty Markdown body |
| `model-policy` | enum | no | see model rules | Depends on `runtime` (below) |
| `model` | string | policy-dependent | unset | Non-empty; no whitespace. Rho may use `@alias`. Claude rejects `@alias` and passes the value to `--model` |
| `provider` | string | no | unset | Non-empty; no whitespace. **Rho only**. Rejected on `claude-cli` |
| `reasoning` | enum | no | unset (inherit) | Rho: `off` \| `minimal` \| `low` \| `medium` \| `high` \| `xhigh` \| `max`. Claude: `low` \| `medium` \| `high` \| `xhigh` \| `max` only (maps to `--effort`). `off` / `minimal` rejected on Claude |
| `tools` | `all` or string list | no | runtime-specific | See tool vocabulary. Mixing Rho and Claude names is a parse error |
| `inherit_claude_config` | bool | no | `false` | `true` \| `false`. `true` only with `runtime: claude-cli` |

Scalars are plain or single/double quoted. Booleans are only `true` / `false`. Lists use `[a, b]` form (comma-separated). Nested YAML maps/objects are not accepted.

### Model rules by runtime

**`runtime: rho` (default)**

| `model-policy` | `model` | `provider` | Result |
| --- | --- | --- | --- |
| omitted, no `model` | omitted | omitted | `inherit` |
| omitted, with `model` | required | optional | treated as `select` |
| `inherit` | must omit | must omit | keep parent provider/model |
| `prefer` \| `require` \| `select` | required | optional | pin that selection; `@alias` allowed |

**`runtime: claude-cli`**

| `model-policy` | `model` | `provider` | Result |
| --- | --- | --- | --- |
| omitted / `inherit`, no `model` | omitted | must omit | Claude default model (no `--model`) |
| omitted / `select`, with `model` | required | must omit | pass-through `--model` |
| `prefer` \| `require` | - | - | rejected |
| any | `@...` | - | rejected (no Rho alias resolution) |
| any | any | set | rejected |

### Tool vocabulary by runtime

**`runtime: rho`**

| Form | Meaning |
| --- | --- |
| omitted or `tools: all` | all host-supplied Rho capabilities (default) |
| `tools: [name, ...]` | allowlist of Rho capabilities |

Built-in Rho capability names:

```text
agent
agents
bash
edit_file
fetch_content
get_search_content
list_dir
powershell
process
questionnaire
read_file
rho
shell
skill
web_search
write_file
```

Notes:

- `shell` resolves at bind time to the platform shell (`bash` or `powershell`) when that capability is available
- unknown names become extension capabilities and still fail bind unless the host supplies them
- delegated agents never receive `agent` / `agents` even if listed

**`runtime: claude-cli`**

| Form | Meaning |
| --- | --- |
| omitted | empty allowlist (no Claude tools) |
| `tools: all` | rejected |
| `tools: [entry, ...]` | Claude Code tool entries |

Each entry must match:

```text
ToolName
ToolName(specifier)
```

| Rule | Detail |
| --- | --- |
| Base name | non-empty; letters, digits, `_`, `-` only |
| Specifier | optional `(...)` with balanced parentheses; may contain spaces and quotes |
| Commas | not allowed inside a specifier (Claude list grammar cannot round-trip them) |
| Membership | open-ended (plugins/MCP may add tools); Rho checks shape, not a fixed catalog |
| Examples | `Read`, `Edit`, `Glob`, `Grep`, `Bash(git *)`, `mcp__server__tool` |

Base names feed Claude `--tools`. Full entries (except nested `Task`) feed `--allowedTools`. Nested Claude `Task` stays disallowed at spawn.

### Body / prompt semantics

| `prompt` | Body empty | Body non-empty |
| --- | --- | --- |
| `extend` (default) | keep base coding prompt only | append body to base coding prompt |
| `replace` | parse error | body becomes the full system prompt |

### JSON Schema (frontmatter)

Machine-readable shape for the frontmatter object after parse. Runtime-specific exclusions (`provider` on Claude, `tools: all` on Claude, `reasoning: off|minimal` on Claude, and model-policy combinations) are enforced in prose and by the Rho parser beyond plain JSON Schema.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://rho.dev/schemas/agent-definition-frontmatter.json",
  "title": "Rho agent definition frontmatter",
  "type": "object",
  "additionalProperties": false,
  "required": ["description"],
  "properties": {
    "id": {
      "type": "string",
      "minLength": 1,
      "maxLength": 64,
      "pattern": "^[a-z0-9]+(?:-[a-z0-9]+)*$"
    },
    "description": {
      "type": "string",
      "minLength": 1,
      "maxLength": 1024
    },
    "runtime": {
      "type": "string",
      "enum": ["rho", "claude-cli"],
      "default": "rho"
    },
    "prompt": {
      "type": "string",
      "enum": ["extend", "replace"],
      "default": "extend"
    },
    "model-policy": {
      "type": "string",
      "enum": ["inherit", "prefer", "require", "select"]
    },
    "model": {
      "type": "string",
      "minLength": 1,
      "pattern": "^\\S+$"
    },
    "provider": {
      "type": "string",
      "minLength": 1,
      "pattern": "^\\S+$"
    },
    "reasoning": {
      "type": "string",
      "enum": ["off", "minimal", "low", "medium", "high", "xhigh", "max"]
    },
    "tools": {
      "oneOf": [
        { "const": "all" },
        {
          "type": "array",
          "items": { "type": "string", "minLength": 1 },
          "uniqueItems": true
        }
      ]
    },
    "inherit_claude_config": {
      "type": "boolean",
      "default": false
    }
  },
  "allOf": [
    {
      "if": {
        "properties": { "runtime": { "const": "claude-cli" } },
        "required": ["runtime"]
      },
      "then": {
        "properties": {
          "provider": false,
          "model-policy": { "enum": ["inherit", "select"] },
          "reasoning": { "enum": ["low", "medium", "high", "xhigh", "max"] },
          "tools": {
            "type": "array",
            "items": { "type": "string", "minLength": 1 }
          },
          "inherit_claude_config": { "type": "boolean" }
        },
        "not": {
          "required": ["model-policy", "model"],
          "properties": {
            "model-policy": { "const": "inherit" },
            "model": true
          }
        }
      }
    },
    {
      "if": {
        "properties": { "model-policy": { "const": "inherit" } },
        "required": ["model-policy"]
      },
      "then": {
        "not": {
          "anyOf": [{ "required": ["model"] }, { "required": ["provider"] }]
        }
      }
    },
    {
      "if": {
        "properties": {
          "model-policy": { "enum": ["prefer", "require", "select"] }
        },
        "required": ["model-policy"]
      },
      "then": { "required": ["model"] }
    }
  ]
}
```

### Examples

Rho agent:

```markdown
---
id: security-review
description: Reviews changes for security defects
runtime: rho
model-policy: inherit
reasoning: high
tools: [read_file, list_dir, bash]
---
Review the requested changes. Do not modify files.
```

Claude Code delegated agent:

```markdown
---
id: claude-planner
description: Plans with Claude Code on the user subscription
runtime: claude-cli
model: claude-opus-4-6
reasoning: high
tools: [Read, Edit, "Bash(git *)"]
inherit_claude_config: false
---
Produce a short plan. Prefer reading before editing.
```

Unknown fields, values, and tool references fail before execution. Definitions contain no credentials or mutable runtime state. New sessions store a v2 semantic fingerprint over behaviorally relevant fields, including `runtime`, tools, and `inherit_claude_config`, not file paths or formatting. Resume also accepts the exact pre-runtime-axis v1 fingerprint for unchanged default Rho definitions (`runtime: rho`, `inherit_claude_config: false`, Rho tools encoding). Real definition changes still fail resume.

## Binding and security

Every invocation goes through the same binder. Binding is runtime-specific:

- `runtime: rho`: resolve model aliases and reasoning against the host config, render prompt policy, and intersect requested Rho tools with host-supplied capabilities. Host policy remains the upper authority boundary.
- `runtime: claude-cli`: copy `model` byte-for-byte (or omit it when inherited), keep the Claude tool list, map optional `reasoning:` to Claude `--effort` (`low`/`medium`/`high`/`xhigh`/`max`), and record `inherit_claude_config`. No Rho model-alias resolution and no mutation of the parent provider/model config. Rho-style `@alias` model values and `reasoning: off` / `reasoning: minimal` are rejected. `runtime: claude-cli` is delegated-only: interactive and automation roots cannot bind it.

Delegated Rho invocations do not receive `agent` or `agents`, so they cannot recursively delegate. Background delegated Rho agents under an interactive parent may use the questionnaire tool. The child pauses on that request, the parent TUI presents the structured form without blocking its active turn or goal loop, and the answer is routed back to the same child run. TUI approvals and questionnaires still use one shared interaction slot, so concurrent requests wait in order. Foreground delegated agents and headless automation omit questionnaire support. Each delegated run owns a fresh run status file, cancellation token, and attachment stream. Rho-runtime delegated runs also own a fresh SDK runtime, session, tool registry, and usage accounting. Claude-cli delegated runs spawn an external `claude` process instead of an in-process SDK loop. Immutable configuration and provider infrastructure may be shared for Rho runs.

## Delegating work

The `agent` tool accepts an `agent_id`, prompt, and optional `background` flag:

- Foreground delegation waits on the run handle and returns its final result.
- Background delegation returns a six-character run ID immediately and sends a completion notification later.

Both modes use the same `AgentExecutor`. Rho-runtime agents stay in-process. `runtime: claude-cli` agents spawn the external `claude` binary and still report through the same status and attachment files. The `agents` tool lists, inspects, or cancels handles tracked by `SubagentManager`. Parent shutdown cancels active handles and waits for bounded cleanup. Delegated agents run without their own TUI. Questionnaires raised by background Rho agents surface in the parent session; approvals still cannot. In Supervised mode, Rho-runtime delegated Write and Process operations fail closed. Claude-cli agents refuse to spawn under Supervised mode entirely because `claude -p` cannot prompt for approval; use Plan or Auto instead. Interactive permission-mode changes apply to delegated agents launched after the change. An already-running delegated agent keeps the launch-time mode because it cannot be retroactively sandboxed; future launches receive the changed mode.

Pass `--no-subagents` to remove delegation capabilities from a root invocation.

## Claude Code as a delegated runtime

Rho can hand a delegated agent to the installed `claude` binary instead of running Rho's own loop. The parent stays in Rho. The child uses Claude Code's harness and the user's Claude subscription. Model choice and runtime choice stay separate: picking an Anthropic model on the Rho runtime is not the same as `runtime: claude-cli`.

### Subscription workaround

Anthropic does not allow third-party clients to sign in with Claude.ai Free/Pro/Max credentials or to route those plans through their own API stacks. Rho's Anthropic provider path is API-key billing only.

`runtime: claude-cli` is the supported **indirect** way to spend a Claude subscription from a Rho session: Rho stays the parent orchestrator, and the official `claude` binary owns sign-in, the child loop, and plan usage. Rho never sees or stores the subscription token. This is not a substitute for Anthropic API access inside Rho's own runtime, and it is not a root-session Claude Code mode.

### When this is useful

Use `runtime: claude-cli` when you want that subscription-backed child while the main session stays on Rho:

- Use a Claude Pro/Max plan on planning, review, or research without making Claude Code the root harness
- Keep Rho as the orchestrator (fan-out, attach, cancel, session tree) while Claude owns the child loop and credential
- Reuse Claude Code tool names and permission behaviour for a bounded child task
- Open the full Claude transcript later with `claude --resume <session-id>` after Rho finishes the run

Skip this feature when you only need "a subagent on Opus" through Rho's normal provider path (API key or another provider). Set `model:` / `provider:` on a `runtime: rho` agent instead. You do not need the `claude` binary for that.

Claude-cli agents are **delegated only**. The interactive root and `rho run` root cannot bind `runtime: claude-cli`. A Rho parent must launch them through the `agent` tool.

### How to use it

1. **Install the binary** (Rho does not ship it):

   ```bash
   curl -fsSL https://claude.ai/install.sh | bash
   claude --version
   ```

   See [Claude Code binary](/installation#claude-code-binary-optional).

2. **Sign in from Rho** so Claude Code stores the subscription credential:

   ```text
   /login claude-code
   ```

   Or open bare `/login`, pick **Anthropic**, then **Claude Code (delegation only)**. Rho hands the terminal to `claude auth login --claudeai` and never sees or stores the token. Details: [Claude Code runtime sign-in](/authentication-and-models#claude-code-runtime-sign-in).

3. **Write a delegated agent definition** (there is no built-in Claude agent). Use the `rho-agent-creator` skill for a guided questionnaire, or write a file such as `~/.rho/agents/claude-planner.md`:

   ```markdown
   ---
   id: claude-planner
   description: Use Claude Code to plan with an Anthropic model
   runtime: claude-cli
   model: claude-opus-5
   reasoning: high
   tools: [Read, Glob, Grep]
   inherit_claude_config: false
   ---
   Produce a short plan. Prefer reading before editing.
   ```

   Notes:

   - `tools:` uses Claude Code names (`Read`, `Edit`, `Bash(git *)`), not Rho capabilities
   - Omitting `tools` means no tools. There is no `tools: all`
   - `model:` is a Claude model name or Claude alias such as `opus`, not a Rho `@alias`
   - optional `reasoning:` maps to Claude `--effort` (`low`/`medium`/`high`/`xhigh`/`max`); omit to inherit Claude's default; `off` and `minimal` are rejected
   - Keep permission mode at Plan or Auto before launch. Supervised refuses Claude-cli spawn because `claude -p` cannot prompt through Rho

4. **Confirm setup** in the TUI:

   ```text
   /doctor
   /agents
   /info
   ```

   `/doctor` checks binary and auth health. `/agents` shows runtime and Claude tool lists. `/info` shows Claude Code ownership wording when signed in.

5. **Delegate from a Rho root session** (interactive or automation parent on `runtime: rho`):

   Ask the parent to call the `agent` tool with `agent_id: claude-planner` and a clear prompt. Use foreground for a blocking result, or background for a run ID plus later completion notification.

6. **Watch, cancel, and resume**:

   ```bash
   rho attach <run-id>
   ```

   Cancel through the `agents` tool or parent shutdown. When the run finishes, attach and the completion entry may show `claude_session_id`. Reopen the Claude-side transcript with:

   ```bash
   claude --resume <session-id>
   ```

   After at least one Claude-cli run has reported limits, `/limits` shows the last observed Claude rate-limit windows (no live probe, no invented percentage).

### Quick checklist

| Step | Command or field |
| --- | --- |
| Install | `claude` on `PATH` |
| Sign in | `/login claude-code` |
| Define | `runtime: claude-cli` + Claude `tools:` / `model:` |
| Permission mode | Plan or Auto (not Supervised) |
| Launch | Rho parent `agent` tool, delegated only |
| Inspect | `rho attach <id>`, `/agents`, `/limits` |
| Full Claude transcript | `claude --resume <session-id>` |

### Claude CLI execution details

A `runtime: claude-cli` agent runs as `claude -p` with stream-json output. Rho owns the parent tree node; Claude owns the child loop and credential.

Before spawn, Rho checks `claude auth status`. If the binary is missing or the user is signed out, the run fails immediately with a message pointing at `/login claude-code`. Rho never stores Claude tokens.

Spawn flags are fixed and deliberate:

| Flag | Behaviour |
| --- | --- |
| `--output-format stream-json --verbose --include-partial-messages` | NDJSON event stream with partial text |
| `--permission-mode` | Always set. Plan maps to `plan`, Auto maps to `dontAsk`. Supervised refuses before spawn. Never `bypassPermissions` |
| `--disallowedTools Task` | Blocks Claude nested subagents so fan-out stays under Rho |
| `--tools` | Restricts built-in tool availability to the base Claude tool names from `tools:`. Empty allowlist still sets `--tools ""` so ambient tools are not inherited |
| `--allowedTools` | Every declared non-`Task` tool entry from `tools:` as separate argv values (bare names such as `Read` and patterns such as `Bash(git *)`). `Task` is never listed here |
| `--setting-sources` | `project` by default. `user,project,local` only when `inherit_claude_config: true` |
| `--strict-mcp-config` | MCP servers only from what the spawn passes |
| `--system-prompt-file` / `--append-system-prompt-file` | From the agent definition body. `prompt: replace` writes a private run-dir file and passes `--system-prompt-file`; nonempty `prompt: extend` uses `--append-system-prompt-file`. Empty extend omits both flags. Prompt body bytes never appear on argv |
| `--model` | From the agent `model:` field when set, passed through unchanged. Omitted when the definition inherits Claude's model. Parent provider/model updates do not overwrite Claude agents |
| `--effort` | From agent `reasoning:` when set (`low`, `medium`, `high`, `xhigh`, `max`). Omitted when unset so Claude keeps its default. `off` and `minimal` never reach spawn |
| `--max-turns` | Exact configured step/turn cap from the bound launch data. If the installed binary rejects the flag, the run fails with a clear error |
| cwd | Explicit project directory |
| prompt | Written on stdin, not argv |

Stderr goes to `log.txt` in the run directory. Cancel kills the child. Terminal success or failure comes from the stream `result` message (`subtype` / `is_error`), not exit code alone.

### Usage, limits, and resume

Per-run usage (turns, tokens, cost) comes from Claude's terminal result and is stored on `result.json`. Cache read/write token fields stay separate on attachment usage events; `input_tokens` on the status file is the total input including cache so attach metrics stay consistent.

`/limits` shows last-observed Claude rate-limit windows reported during a run (window name, status, reset time, age). It does not invent a remaining percentage and does not spawn a probe run. If nothing has been observed yet, Claude limits are absent until a claude-cli run reports them.

When a run finishes, `result.json` may include `claude_session_id`. Attach and the parent completion entry show it so you can reopen the full Claude transcript with:

```bash
claude --resume <session-id>
```

Default concurrency is one global pool of 4 delegated runs (`RHO_AGENT_CONCURRENCY` overrides that total). Claude-cli runs also take a nested Claude permit capped at 2 by default (`RHO_CLAUDE_AGENT_CONCURRENCY` overrides that nested cap). The Claude pool is always `min(total, claude_cap)`, so overrides never open a 2N fan-out window and Claude never exceeds the global total.

### Auth ownership

| Action | Owner |
| --- | --- |
| Sign in | Claude Code via `/login claude-code` (terminal handoff to `claude auth login --claudeai`) |
| Sign out | Claude Code via `/logout claude-code` or `claude auth logout` (global, not Rho-only) |
| Credential storage | Claude binary only. Rho never sees or stores the token |

## Attachment and artifacts

Observe any delegated run without owning its execution:

```bash
rho attach abc123
```

The read-only attachment TUI follows durable artifacts under `~/.rho/subagents/<id>/`:

- `result.json` - live status, agent ID, semantic fingerprint, usage, final result, and optional `claude_session_id`
- `events.jsonl` - display events used by attachment
- `log.txt` - Claude stderr for `runtime: claude-cli` runs

Detaching does not cancel execution. Herdr panes also run `rho attach <id>` and never own the delegated task. Artifacts remain available for post-run inspection and may contain prompts or workspace content.

A direct automation run can persist the same status contract:

```bash
rho run --agent explorer --output-file /tmp/result.json "where is auth handled?"
```

Root session metadata stores the selected agent ID and fingerprint. Resume fails explicitly when that identity is missing or when the selected definition changed. Unchanged default Rho definitions still resume when the session stores the pre-runtime-axis v1 fingerprint.
