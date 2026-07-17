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
model-policy: inherit
reasoning: high
tools: [read_file, list_dir, bash]
---
Review the requested changes. Do not modify files.
```

Definitions are discovered deterministically from built-ins, `~/.agents/agents`, `~/.rho/agents`, and trusted project `.agents/agents` directories, with later sources taking precedence. Project definitions are ignored unless `RHO_TRUST_PROJECT_AGENTS=1`, so an untrusted checkout cannot affect prompts, models, or tools. Duplicate IDs within one precedence level are errors. The file name supplies `id` when the field is omitted.

Supported fields are:

| Field | Required | Meaning |
| --- | --- | --- |
| `id` | no | Stable lowercase identifier; defaults to the file name |
| `description` | yes | Description shown by the `agent` tool |
| `prompt` | no | `extend` (default) or `replace` |
| `model-policy` | no | `inherit`, `prefer`, `require`, or `select` |
| `model` | policy-dependent | Model selected by non-inherit policies |
| `provider` | no | Provider selected with the model |
| `reasoning` | no | `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, or `max` |
| `tools` | no | `all` or an explicit capability list; `shell` resolves to the platform shell |

Unknown fields, values, and tool references fail before provider execution. Definitions contain no credentials or mutable runtime state. A semantic fingerprint covers behaviorally relevant fields, not file paths or formatting.

## Binding and security

Every invocation goes through the same binder. It resolves model and reasoning policy, renders prompt policy, and intersects requested tools with capabilities supplied by the host. Host policy is always the upper authority boundary.

Delegated invocations do not receive `agent`, `agents`, or interactive questionnaire capabilities, so they cannot recursively delegate. Each delegated run owns a fresh SDK runtime, session, tool registry, cancellation token, event stream, and usage accounting. Immutable configuration and provider infrastructure may be shared.

## Delegating work

The `agent` tool accepts an `agent_id`, prompt, and optional `background` flag:

- Foreground delegation waits on the run handle and returns its final result.
- Background delegation returns a six-character run ID immediately and sends a completion notification later.

Both modes use the same in-process `AgentExecutor`; Rho never starts a CLI child for internal delegation. The `agents` tool lists, inspects, or cancels handles tracked by `SubagentManager`. Parent shutdown cancels active handles and waits for bounded cleanup.

Pass `--no-subagents` to remove delegation capabilities from a root invocation.

## Attachment and artifacts

Observe any delegated run without owning its execution:

```bash
rho attach abc123
```

The read-only attachment TUI follows durable artifacts under `~/.rho/subagents/<id>/`:

- `result.json` - live status, agent ID, semantic fingerprint, usage, and final result
- `events.jsonl` - display events used by attachment

Detaching does not cancel execution. Herdr panes also run `rho attach <id>` and never own the delegated task. Artifacts remain available for post-run inspection and may contain prompts or workspace content.

A direct automation run can persist the same status contract:

```bash
rho run --agent explorer --output-file /tmp/result.json "where is auth handled?"
```

Root session metadata stores the selected agent ID and fingerprint. Resume fails explicitly when that identity is missing or when the selected definition changed.
