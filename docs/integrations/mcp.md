# Model Context Protocol

Rho can connect native agents to ordinary [Model Context Protocol (MCP)](https://modelcontextprotocol.io/) servers. MCP support does not require an Agent Plugin package.

## Configuration source and precedence

Add servers under `[mcp.servers]` in Rho's config file. Rho reads `~/.rho/config.toml` by default. To keep project MCP settings with a project, select that file through the existing config override:

```bash
rho --config .rho/config.toml
```

An explicit `--config` file replaces the default user config. Rho does not merge the two files. The selected file is the only ordinary MCP configuration source for that run. Plugin packages contribute servers separately through [Agent Plugins](/integrations/plugins); they merge with the servers below at session start.

Each table key is the server's stable identity. Identities may contain ASCII letters, digits, `-`, and `_`. Set `enabled = false` to keep an entry without starting it.

When no enabled server exists, Rho does not construct an MCP runtime, spawn a process or task, connect to a URL, run the handshake or `tools/list`, add MCP tools or prompt text, or install MCP shutdown work.

## stdio servers

Rho executes `command` directly and passes `args` as separate arguments. It never invokes a shell. Bare commands use the platform executable search path.

```toml
[mcp.servers.filesystem]
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/work/project"]
cwd = "/work/project"

[mcp.servers.filesystem.tools]
allow = ["read_file", "list_directory"]
deny = []
```

Rho starts the child with the same sanitized base environment hooks use (paths, home, locale, and on Windows the loader/interpreter variables). It does not copy credentials or other variables by default. `env` adds literal values. Use `env_from_env` for secrets so they do not enter the config file. Its keys are child variable names and its values are ambient variable names:

```toml
[mcp.servers.github]
transport = "stdio"
command = "github-mcp-server"
args = ["stdio"]
env_from_env = { GITHUB_PERSONAL_ACCESS_TOKEN = "GITHUB_TOKEN" }
```

The explicit overlay is applied after sanitization, so it may intentionally restore a variable under the name a server expects. **Configuration is the trust boundary:** enabling a server is permission to start it for discovery and to expose its tools. Tool calls are RPCs on that already-running session; they do not re-request process or network capabilities and are not blocked by plan mode. The process is not an operating-system sandbox and runs with the current user's rights.

Rho closes MCP sessions during normal session shutdown. The stdio transport closes the child's input, waits for clean exit, and kills the child if it does not exit through the transport's cleanup path. Initialization failures also drop and clean up the child.

## Streamable HTTP servers

Use `streamable_http` for current MCP Streamable HTTP. This is not the legacy HTTP+SSE transport. Non-loopback endpoints must use HTTPS. Loopback `http://localhost`, `127.0.0.0/8`, and `::1` endpoints are allowed for local development.

```toml
[mcp.servers.remote]
transport = "streamable_http"
url = "https://mcp.example.com/mcp"
headers_from_env = { Authorization = "MCP_AUTHORIZATION" }
headers = { X-Tenant = "public-tenant" }

[mcp.servers.remote.tools]
deny = ["delete_account"]
```

`headers_from_env` maps HTTP header names to ambient variable names. Put the complete header value in the environment, such as `Bearer ...`. Rho does not store it in config or diagnostics. Automatic HTTP redirects are disabled, so configured headers cannot be replayed to another origin.
`headers` supplies literal header values with the configuration. Prefer `headers_from_env` for anything secret. On a name collision, environment-derived headers override literal ones.

Authentication discovery and OAuth are not implemented; supply server-issued credentials through environment-backed headers. As with stdio, configuration is the trust boundary for the remote session; tool calls do not re-request network capability.

## Handshake

Rho identifies itself in `initialize` as `rho` with the running version, and declares the client capabilities it actually answers:

- `roots`: Rho advertises the session workspace as one `file://` root. The declaration sets `listChanged: false`, because a session's workspace never changes and Rho would never send the notification.

A server's `instructions` from `initialize` reach the model. Rho fences each server's text in the system prompt and marks it as documentation from that server, not as instructions from the user.

## Server logs

Set `log_level` on a server to send `logging/setLevel` after the handshake. Levels are `debug`, `info`, `notice`, `warning`, `error`, `critical`, `alert`, and `emergency`. Leaving it unset keeps the server's own default.

```toml
[mcp.servers.indexer]
transport = "stdio"
command = "indexer-mcp"
log_level = "info"
```

Server log messages enter Rho's tracing output under the `rho::mcp::server` target with the server identity and the server's logger name. Severities above `error` map onto `error`. A server that does not declare the `logging` capability is left alone.

## Discovery and tool calls

Enabled servers initialize independently at session startup because Rho needs `tools/list` before the first model request. Each server has a two-minute startup budget for connection, handshake, and discovery. A timeout logs the server identity and limit. Rho does not retry during startup. A malformed entry, failed executable, failed connection, authentication error, handshake error, or `tools/list` error disables only that server. Other MCP servers and built-in tools continue to load. After a session is established, discovery failures and timeouts attempt a bounded close instead of relying only on Drop.

Rho attaches a progress token to every tool call, so a server may report `notifications/progress` against it. Progress reaches the tool card while the call runs. Counts appear only when the server supplies a total.

Cancelling a turn sends `notifications/cancelled` for the in-flight call, so the server can stop its own work instead of continuing on an abandoned request. The same happens when a call exceeds its two-minute budget.

Streamable HTTP sessions are pinged once a minute, because an idle proxy can drop a remote connection with no local signal. A failed ping appears in `/mcp` and `rho mcp show`. Stdio servers need no ping: a dead child is observable directly.

Rho exports discovered tools as:

```text
mcp__<server_identity>__<tool_name>
```

Components containing only ASCII letters, digits, and `_` remain unchanged. Rho encodes every other component, and components beginning with the reserved `_rho_` prefix, as `_rho_` followed by the lowercase hexadecimal UTF-8 bytes. This encoding keeps distinct server and remote tool names distinct. Descriptions include the owning server identity for diagnostics. `allow` is an optional allowlist; `deny` always wins.

MCP tool calls use Rho's native tool registry, cancellation, and shutdown path. MCP error results and transport failures become tool failures without stopping sibling servers.

### Tool declarations

Rho reads more than the name, description, and input schema:

- **`title`** appears in the exported description, so the model sees the readable name next to the server identity.
- **Annotations** (`readOnlyHint`, `destructiveHint`, `openWorldHint`) appear as `Server hint:` lines in the description and as notices on the tool card. They are hints from a server Rho does not control, so they never relax a permission or skip an approval. A read-only hint changes the card's icon and nothing else.
- **`outputSchema`** sets a contract. A server that declares one must return `structuredContent`; a result without it fails the call rather than passing a half-answer to the model.

### Results

Each content block is rendered for what it is, rather than serialized as a JSON envelope:

| Block | What the model reads | What the card shows |
| --- | --- | --- |
| `text` | the text | the text |
| `image` | `[image image/png, 41.2 KB]` | the image |
| `audio` | `[audio audio/wav, 1.1 MB]` | the descriptor |
| `resource` (text) | `[resource <uri>]` and the body | the same |
| `resource` (blob) | `[resource <uri>] [resource <mime>, <size>]` | the image, if it is one |
| `resource_link` | the URI, name, and description | the same |

Binary payloads never reach the model as base64. A returned image rides on the tool card as an asset, and the model gets a short descriptor it can reason about.

When a result carries `structuredContent`, that JSON is what the model reads. Servers are asked to mirror structured content as text for older clients; Rho drops the mirrored copy so one result does not cost the context twice.

### Tool lists that change mid-session

A server that declares `tools.listChanged` may send `notifications/tools/list_changed`. Rho re-runs `tools/list` for that server and reconciles the result:

- **Revised tools** keep working. A new description or input schema reaches the model on the next turn, because Rho reads each tool's definition when it builds a turn.
- **Withdrawn tools** stay registered under their exported name and fail with a clear reason if called.
- **Added tools** cannot join the registry mid-session, because a session's tool set is fixed once it starts. `/mcp` and `rho mcp show` list them and say a restart is needed.

## Inspect status

There is no marketplace in Rho. Configure servers in the selected config file, then inspect config or live load status:

- **Interactive:** `/mcp` lists configured servers, transport, status, errors, and exported tool names for the current session. `/doctor` includes an MCP health row.
- **CLI:** `rho mcp list` prints configured servers from the selected config and plugins without starting them. `rho mcp show <id>` prints one server. Pass `--connect` on either command to start enabled servers and report live status and discovered tools. Both accept `--json`.

Use `/mcp` when you already have a session open. Use `rho mcp list` from a shell to verify config before starting the TUI, and `rho mcp list --connect` when you need a live probe.

## Runtime differences

Native Rho agents receive these tools. Claude CLI agents do not: Rho does not pass its MCP configuration to Claude, inherit Claude's MCP configuration, or treat Claude's opaque `mcp__...` names as native support. This prevents one configured server from loading through both runtimes. For a Claude CLI agent session, `/mcp` still shows configured entries as not loaded.

## Plugin-provided servers

[Agent Plugins](/integrations/plugins) packages can declare MCP servers in a
`mcp.json` file. Rho translates valid entries into the same configuration
model above, so transports, startup budgets, tool namespacing, permissions,
failure isolation, and shutdown behave identically. Plugin servers appear in
inventories as `<plugin-name>/<server-name>` and expand the `${PLUGIN_ROOT}`
and `${PLUGIN_DATA}` placeholders. Discovery does not create `${PLUGIN_DATA}`.
Rho creates it immediately before it starts each enabled stdio server. If directory preparation fails, Rho disables only that server. A plugin with no valid MCP servers adds no MCP startup work, and the zero-server fast path above still applies.
