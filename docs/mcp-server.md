# MCP server

`rho mcp serve` turns Rho into a [Model Context Protocol](https://modelcontextprotocol.io/) server. Another agent, such as Claude Code, Codex, or a Rho session, can then start Rho sessions in any workspace, follow their turns, and read their answers. The sessions run in the [daemon](/daemon), which the server starts if needed.

The server speaks stdio by default. For hosts that connect by URL, use [`--http`](#over-http). A Rho agent gets the same tools through its [`sessions` tool](/sessions#starting-and-driving-other-sessions).

## Tools

| Tool | What it does | Access |
| --- | --- | --- |
| `list` | Lists sessions with their id, workspace, state, and parent session. Filter by `workspace` or `state`. | read |
| `read` | Returns a session's transcript as text, a page at a time. | read |
| `new` | Starts a session in `workspace` (an absolute path) with a first `prompt`. Optional `agent` and `model`. `worktree: true` runs it in a new [git worktree](/daemon#sessions-in-worktrees). | write |
| `prompt` | Starts the session's next turn with `text`. | write |
| `steer` | Sends `text` to the running turn. | write |
| `cancel` | Stops the turn in progress. | write |
| `wait` | Waits for the turn to end or ask for input. See [Following a turn](#following-a-turn). | read |

Over stdio the host has write access unless you start the server [read-only](#read-only-over-stdio). Over HTTP each [token](#tokens) has read or write access.

## Following a turn

`new` and `prompt` return as soon as the turn starts. `wait` follows it, and returns when the turn ends, when it needs a person, or after `timeout_secs` (30 by default, at most 50):

```json
{
  "session": "8c1e…",
  "state": "completed",
  "done": true,
  "outcome": {"type": "answered", "text": "…", "stop": "end_turn"},
  "cursor": "4f0a…:42"
}
```

- If `done` is false, the turn is still running. Call `wait` again with the returned `cursor`.
- `outcome.text` is the final answer. `read` returns the whole conversation.
- `inputs` lists the approvals or questions the turn waits on.

## Approvals stay with people

No tool answers a session's approvals or questions. When a turn asks for one, `wait` returns with `done: true`, `state: "waiting_for_input"`, and the request in `inputs`. A person answers it from `rho attach <session>` or the TUI, and the next `wait` follows the turn from there.

To let a session work unattended, give it a [permission mode](/configuration/permissions) that does not ask, as you would for `rho run`.

## Limits

Sessions can start sessions, so the server checks two budgets on every `new` and `prompt`:

- Session depth. A session started by a person or an outside agent is depth 1, a session it starts is depth 2, and so on. `--max-depth` sets the limit, 3 by default.
- Concurrent sessions. Each caller may run `--max-sessions` sessions at once, 10 by default. Each HTTP token is a separate caller. A session counts until its turn ends, including time waiting for a person.

A request past a budget is refused with the budget, its limit, and the value asked for:

```text
session depth budget: limit 3, asked 4; a session at depth 3 may not start or prompt another
concurrent sessions budget for token "ci": limit 10, asked 11
```

These limits stop runaway recursion. They are not a sandbox, since a session with a shell can reach the daemon directly.

## Sessions' environment

Sessions get the server's environment, including its `PATH`, as if you started them from the server's shell. Under a service manager that is the service's environment; see [Running as a service](/daemon#running-as-a-service).

## Registering over stdio

The host starts `rho mcp serve` itself. If the host does not inherit your shell's `PATH`, give it the full path to `rho`.

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

Rho, in `~/.rho/config.toml`, so one Rho session can drive others:

```toml
[mcp.servers.rho]
transport = "stdio"
command = "rho"
args = ["mcp", "serve"]
```

A Rho host starts stdio servers with a [sanitized environment](/integrations/mcp#stdio-servers), and the sessions this server starts inherit it. List any variables their tools need in `env_from_env`.

Add `--max-depth` or `--max-sessions` after `serve` to change the [limits](#limits).

### Read-only over stdio

Add `--scope read` for a host that should watch sessions but never start or change one:

```bash
claude mcp add --transport stdio rho-watch -- rho mcp serve --scope read
```

The write tools are refused:

```text
this server's caller has read access; `new` needs write
```

`--scope write` is the default. `--scope` does not work with `--http`, where each [token](#tokens) carries its own scope.

## Over HTTP

```bash
rho mcp serve --http 127.0.0.1:8765
```

The server prints its URL, `http://127.0.0.1:8765/mcp`, on stderr. Port `0` picks a free port.

It binds only to a loopback address. To reach it from another machine, use a tunnel such as an SSH forward. [Running as a service](/daemon#running-as-a-service) has a systemd unit and a launchd agent that keep it running.

### Tokens

Every HTTP request needs a bearer token. Create one and name the workspaces it may reach:

```bash
rho mcp token rotate laptop --scope write --workspace ~/src/app --workspace ~/src/lib
```

`rotate` prints the secret once on stdout. Rho keeps only a hash of it, in your [credential store](/authentication-and-models#where-credentials-live). Clients send it as `Authorization: Bearer <secret>`.

- `--scope read`, the default, allows `list`, `read`, and `wait`. `--scope write` also allows `new`, `prompt`, `steer`, and `cancel`.
- `--workspace` grants a directory and everything under it. A new token needs at least one. The token cannot see or touch sessions anywhere else.
- Running `rotate` on an existing token issues a new secret. It keeps the token's scope and workspaces unless you pass new ones.

```bash
rho mcp token list           # names, scopes, and workspaces; --json for JSON
rho mcp token revoke laptop  # reject further requests with this token
```

Rotations and revokes apply from the next request, without a restart. They do not stop work already in progress. A request with a missing or unknown token gets `401 Unauthorized`.

### Token scope {#scope-follows-the-sessions-a-token-starts}

A turn that a token starts or prompts runs with the token's scope and workspaces. Any `rho mcp serve` the session calls holds it to that scope too. A session the token started keeps that limit for good, so a person who prompts it later does not give it their own access.

Like the [limits](#limits), this is not a sandbox. A write token can also steer a turn someone else started.

### Hosts and origins

The server accepts requests whose `Host` is `localhost`, `127.0.0.1`, or `::1`. Behind a tunnel that keeps the public hostname, allow that name:

```bash
rho mcp serve --http 127.0.0.1:8765 --allow-host mcp.example.com
```

Requests with an `Origin` header get `403 Forbidden` unless `--allow-origin` names the origin, so a web page cannot drive the server. Non-browser MCP hosts send no `Origin`. Both flags can be repeated.

### Registering by URL

These examples assume the token's secret is in `RHO_MCP_TOKEN`.

Claude Code:

```bash
claude mcp add --transport http rho https://mcp.example.com/mcp \
  --header "Authorization: Bearer $RHO_MCP_TOKEN"
```

Codex, in `~/.codex/config.toml`:

```toml
[mcp_servers.rho]
url = "https://mcp.example.com/mcp"
bearer_token_env_var = "RHO_MCP_TOKEN"
```

Rho, in `~/.rho/config.toml` (see [Streamable HTTP servers](/integrations/mcp#streamable-http-servers)), with `RHO_MCP_AUTHORIZATION="Bearer <secret>"` in Rho's environment:

```toml
[mcp.servers.rho]
transport = "streamable_http"
url = "https://mcp.example.com/mcp"
headers_from_env = { Authorization = "RHO_MCP_AUTHORIZATION" }
```
