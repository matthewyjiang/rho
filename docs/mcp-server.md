# MCP server

`rho mcp serve` serves the [daemon](/daemon)'s sessions as a [Model Context Protocol](https://modelcontextprotocol.io/) server, so another agent, such as Claude Code, Codex, or a rho session, can start rho sessions in any workspace, follow their turns, and read what they answered. It is a daemon client like `rho attach`, and it starts the daemon on its first tool call when none is running. A rho agent gets the same tools, with the same results and policy, as actions of its own [`sessions` tool](/sessions#starting-and-driving-other-sessions).

It serves over stdio by default, for a host that starts it as a command. `rho mcp serve --http` serves streamable HTTP on a loopback address instead, for hosts that reach MCP servers by URL; see [Over HTTP](#over-http).

The daemon, and so this server, is not supported on Windows yet ([#1665](https://github.com/matthewyjiang/rho/issues/1665)).

## Tools

| Tool | What it does | Needs |
| --- | --- | --- |
| `list` | The daemon's sessions, most recently changed first: each one's id, workspace, state, when it last changed, and the `parent` session whose agent started it, if one did. Filter by `workspace` or `state`; `limit` is 20 by default and at most 1000. | read |
| `read` | A session's transcript as text: who said what, and the tools it called. Pages by characters: `start` (0 by default) and `chars` (4096 by default, at most 64000); pass the result's `next_start` to read on. | read |
| `new` | Starts a session in `workspace`, an absolute path to an existing directory, with a first `prompt`. `agent` picks one of the workspace's agents by id and `model` one of its models, as `provider/model` or a model id only one provider offers; both default to the workspace's. | write |
| `prompt` | Starts a session's next turn with `text`. | write |
| `steer` | Gives the running turn `text` for its next model request. | write |
| `cancel` | Stops the session's turn in progress, if any. | write |
| `wait` | Waits for the session's turn to end or ask for input, up to `timeout_secs`, then reports where it is. See [Following a turn](#following-a-turn). | read |

The "Needs" column matters only [over HTTP](#over-http), where each token has read or write access. Over stdio every tool is available.

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

Every tool that names a `session` refuses one the daemon does not know, naming it, whichever workspaces the token may reach.

## Approvals stay with people

No tool answers a session's approvals or questions, and the server has no other way to. A turn that asks for one stops `wait` with `done: true`, `state: "waiting_for_input"`, and the open requests in `inputs`. A person answers them from a rho frontend, such as `rho attach <session>` or the TUI, and the next `wait` follows the turn on.

To let a session work unattended, give it a [permission mode](/configuration/permissions) that does not ask, as you would for `rho run`.

## Limits

Sessions that start sessions can recurse: an agent driving rho through this server may run a session that drives rho through its own. The server bounds that with two budgets, enforced on every `new` and `prompt`:

- **Session depth.** A session the server starts or prompts runs one level deeper than the session that calls: depth 1 when a person or an outside agent calls, depth 2 when a depth-1 session does. A session carries its depth to the processes it starts in `RHO_SESSION_DEPTH`, so a `rho mcp serve` it runs knows where it sits. A rho session also sends its current depth with every call to a rho MCP server, over stdio or HTTP, so the count follows the turn that calls: a session prompted again from deeper in a chain is held to its new depth by the server it already runs, and a call over HTTP counts from the calling session, not from wherever the listener was started. A call that sends none, such as an outside agent's over HTTP, is depth 0. `--max-depth` sets the deepest a session may run, 3 by default. `read` and `wait` never load a session the daemon has unloaded, so reading one starts none of its MCP servers at the reader's depth.
- **Concurrent sessions.** Each caller may have as many sessions running at once as `--max-sessions`, 10 by default, the same as the daemon's default agent concurrency. Over HTTP each token is a caller of its own. A session counts from when the caller starts or prompts it until its turn ends, a turn waiting for a person included; prompting a session the caller already has running adds none.

A start past either budget is refused with the budget, its limit, and the value asked for:

```text
session depth budget: limit 3, asked 4; a session at depth 3 may not start or prompt another
concurrent sessions budget for token "ci": limit 10, asked 11
```

The depth guards against runaway recursion, not against a session's own tools: a session with a shell can still reach the daemon's socket directly. The same holds for the [scope a token passes on](#scope-follows-the-sessions-a-token-starts).

An argument past its limit, such as a `wait` longer than 50 seconds, is refused the same way, naming the limit and the value asked for.

## Sessions' environment

Sessions run with the server's environment, as if you had started them from the shell that runs the server: their tools, MCP servers, and [CLI subagents](/subagents) look up executables on its `PATH` and read its variables. See [Environment](/daemon#environment). Under a service manager, that environment is the service's; [Running as a service](/daemon#running-as-a-service) shows how to set it.

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

## Over HTTP

```bash
rho mcp serve --http 127.0.0.1:8765
```

The server prints where it listens, `rho mcp serve listening on http://127.0.0.1:8765/mcp`, and serves MCP's streamable HTTP transport at `/mcp`. Port `0` picks a free port.

It binds a loopback address only and refuses any other, so nothing off the machine reaches it unless you set up a tunnel, such as an SSH forward or a tunnel service, to that address. Run it under a service manager to keep it listening; [Running as a service](/daemon#running-as-a-service) has a systemd unit and a launchd agent.

### Tokens

Every request needs a bearer token. Create one with the workspaces it may reach:

```bash
rho mcp token rotate laptop --scope write --workspace ~/src/app --workspace ~/src/lib
```

`rotate` prints the token's secret on stdout, alone, once; rho keeps only its SHA-256 hash, in the [credential store](/authentication-and-models#where-credentials-live) the config selects. Send it as `Authorization: Bearer <secret>`.

- `--scope read` (the default for a new token) lets the token `list`, `read`, and `wait`; `--scope write` also lets it `new`, `prompt`, `steer`, and `cancel`.
- `--workspace` names a directory whose sessions the token may reach, and those of every directory under it. A new token needs at least one. The token sees only those sessions in `list`, and every other tool refuses a session elsewhere, naming the directories the token may reach.
- Running `rotate` on an existing token gives it a new secret and keeps its scope and workspaces unless you pass them.

```bash
rho mcp token list           # names, scopes, and workspaces; --json for one document
rho mcp token revoke laptop  # reject subsequent requests with this token
```

The server reads the tokens on every request, so a rotation or a revoke applies to requests not yet accepted, without restarting the server. It does not cancel an already admitted tool call, close an already open SSE stream, or stop daemon work the token started.

### Scope follows the sessions a token starts

A session a token starts or prompts runs with the token's scope and workspaces, which it carries to the processes it starts in `RHO_SESSION_SCOPE` and sends with every call to a rho MCP server. Any `rho mcp serve` the session reaches holds the call to that scope as well as its own grant: the session's stdio server, which would otherwise have local write access, refuses a workspace outside the token's, and a call over HTTP with a broader token gets only what both allow. A scope a request sends can narrow a token, never widen it. A session a person prompts afterwards runs with that person's full access again. Over stdio, the depth and scope a server starts with are only a default: the calling rho session's own replace them on every call, so setting `RHO_SESSION_DEPTH` or `RHO_SESSION_SCOPE` in the server's config `env` does not pin them.

This keeps a token's limits on the sessions it starts, not their tools: like the [depth](#limits), it is not a sandbox, and a session with a shell can reach the daemon's socket directly. A token with write access may also steer a turn someone else started, which keeps that turn's own scope.

A request without a token, or with one rho does not know, gets `401 Unauthorized`. A token's scope or workspaces refusing a tool is the tool's error, which the model reads, not an HTTP error.

Each HTTP transport session belongs to the token name that initialized it. Another token name, or an unknown or deleted session ID, gets `404 Not Found` for requests, notifications, streams, resume, and deletion. Rotating a token keeps its existing transport sessions and applies its current grant to subsequent requests. Recreating a revoked token under the same name recreates that principal and can regain its surviving transport sessions.

Token mutations serialize through `.rho-mcp-tokens.lock` in the OS account's home, resolved from the account database rather than `HOME`, `RHO_HOME`, or `TMPDIR`. This keeps a service and a login shell on the same lock even when their environments differ. The home must be owned by the account and allow no group or other write access; the lock itself is private and cannot be a symlink.

### Hosts and origins

The server accepts requests whose `Host` is `localhost`, `127.0.0.1`, or `::1`. Behind a tunnel that keeps the public hostname, allow it with `--allow-host`:

```bash
rho mcp serve --http 127.0.0.1:8765 --allow-host mcp.example.com
```

A request that carries an `Origin` header, which a browser adds, is refused with `403 Forbidden` unless the origin is one `--allow-origin` names, so a web page you visit cannot drive the server. Hosts that are not browsers send no `Origin` and need no `--allow-origin`. Both flags may be repeated.

### Registering by URL

With the token's secret in `RHO_MCP_TOKEN`:

Claude Code:

```bash
claude mcp add --transport http rho https://mcp.example.com/mcp \
  --header "Authorization: Bearer $RHO_MCP_TOKEN"
```

Codex, in `~/.codex/config.toml`, reading the secret from the environment it runs in:

```toml
[mcp_servers.rho]
url = "https://mcp.example.com/mcp"
bearer_token_env_var = "RHO_MCP_TOKEN"
```

Rho, in `~/.rho/config.toml`; see [Streamable HTTP servers](/integrations/mcp#streamable-http-servers):

```toml
[mcp.servers.rho]
transport = "streamable_http"
url = "https://mcp.example.com/mcp"
headers_from_env = { Authorization = "RHO_MCP_AUTHORIZATION" }
```

with `RHO_MCP_AUTHORIZATION="Bearer <secret>"` in rho's environment.
