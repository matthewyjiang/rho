# MCP server

`rho mcp serve` serves the [daemon](/daemon)'s sessions as a [Model Context Protocol](https://modelcontextprotocol.io/) server, so another agent, such as Claude Code, Codex, or a rho session, can start rho sessions in any workspace, follow their turns, and read what they answered. It is a daemon client like `rho attach`, and it starts the daemon on its first tool call when none is running.

It serves over stdio, for a host that starts it as a command.

The daemon, and so this server, is not supported on Windows yet ([#1665](https://github.com/matthewyjiang/rho/issues/1665)).

## Tools

| Tool | What it does |
| --- | --- |
| `list` | The daemon's sessions, most recently changed first: each one's id, workspace, state, and when it last changed. Filter by `workspace` or `state`; `limit` is 20 by default and at most 1000. |
| `read` | A session's transcript as text: who said what, and the tools it called. Pages by characters: `start` (0 by default) and `chars` (4096 by default, at most 64000); pass the result's `next_start` to read on. |
| `new` | Starts a session in `workspace`, an absolute path to an existing directory, with a first `prompt`. `agent` picks one of the workspace's agents by id and `model` one of its models, as `provider/model` or a model id only one provider offers; both default to the workspace's. |
| `prompt` | Starts a session's next turn with `text`. |
| `steer` | Gives the running turn `text` for its next model request. |
| `cancel` | Stops the session's turn in progress, if any. |
| `wait` | Waits for the session's turn to end or ask for input, up to `timeout_secs`, then reports where it is. See [Following a turn](#following-a-turn). |

## Following a turn

`new` and `prompt` return once the turn starts. Call `wait` to follow it:

```json
{"session": "8c1e…", "timeout_secs": 30}
```

`wait` returns when the turn ends, when it waits for a person, or when `timeout_secs` passes (30 seconds by default, at most 50), whichever is first:

```json
{
  "session": "8c1e…",
  "state": "completed",
  "done": true,
  "outcome": {"type": "answered", "text": "…", "stop": "end_turn"},
  "cursor": "4f0a…:42"
}
```

- `done` is false while the turn still runs. Call `wait` again with the `cursor` it returned, which resumes after the events this call already saw, so a turn that ended between two calls is still reported.
- `outcome` is how the turn ended, its final answer included; it is present only once the turn has ended.
- `inputs` lists what the turn waits on a person for: an approval, a plan approval, or questions. See [Approvals stay with people](#approvals-stay-with-people).

The 50-second cap keeps a `wait` inside the shortest per-call timeout of the hosts rho knows: Codex gives an MCP tool call 60 seconds by default. A longer turn takes several `wait` calls, each resuming from the last one's cursor.

`read` returns the whole conversation so far, and `outcome.text` only the final answer.

## Approvals stay with people

No tool answers a session's approvals or questions, and the server has no other way to. A turn that asks for one stops `wait` with `done: true`, `state: "waiting_for_input"`, and the open requests in `inputs`. A person answers them from a rho frontend, such as `rho attach <session>` or the TUI, and the next `wait` follows the turn on.

To let a session work unattended, give it a [permission mode](/configuration/permissions) that does not ask, as you would for `rho run`.

## Limits

Sessions that start sessions can recurse: an agent driving rho through this server may run a session that drives rho through its own. The server bounds that with two budgets, enforced on every `new` and `prompt`:

- **Session depth.** A session the server starts or prompts runs one level deeper than the server's caller: depth 1 when a person or an outside agent runs the server, depth 2 when a depth-1 session does. A session carries its depth to the processes it starts in `RHO_SESSION_DEPTH`, its MCP servers included, so a `rho mcp serve` it runs knows where it sits. `--max-depth` sets the deepest a session may run, 3 by default.
- **Concurrent sessions.** The server's caller may have as many sessions running at once as `--max-sessions`, 10 by default, the same as the daemon's default agent concurrency. A session counts from when the caller starts or prompts it until its turn ends, a turn waiting for a person included; prompting a session the caller already has running adds none.

A start past either budget is refused with the budget, its limit, and the value asked for:

```text
session depth budget: limit 3, asked 4; a session at depth 3 may not start or prompt another
concurrent sessions budget for this server's caller: limit 10, asked 11
```

The depth guards against runaway recursion, not against a session's own tools: a session with a shell can still reach the daemon's socket directly.

An argument past its limit, such as a `wait` longer than 50 seconds, is refused the same way, naming the limit and the value asked for.

## Sessions' environment

Sessions run with the server's environment, as if you had started them from the shell that runs the server: their tools, MCP servers, and [CLI subagents](/subagents) look up executables on its `PATH` and read its variables. See [Environment](/daemon#environment).

## Registering over stdio

The host starts `rho mcp serve` itself. Give it the full path to `rho` if the host does not run with your shell's `PATH`.

Claude Code:

```bash
claude mcp add --transport stdio rho -- rho mcp serve
```

Codex, in `~/.codex/config.toml`:

```toml
[mcp_servers.rho]
command = "rho"
args = ["mcp", "serve"]
```

Rho, in `~/.rho/config.toml`, so one rho session can drive others; see [Model Context Protocol](/integrations/mcp#stdio-servers):

```toml
[mcp.servers.rho]
transport = "stdio"
command = "rho"
args = ["mcp", "serve"]
```

A rho session starts its stdio servers with a [sanitized environment](/integrations/mcp#stdio-servers), so the sessions such a server starts get that environment too. List the variables their tools need in the server's `env_from_env`.

Pass `--max-depth` and `--max-sessions` after `serve` to change the [limits](#limits).
