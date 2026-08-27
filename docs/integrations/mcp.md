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

To reach a cleartext non-loopback host (a LAN IP, an internal hostname), set `allow_insecure_http = true` on that server. Headers and bearer tokens then travel unencrypted. Plugin-provided servers cannot set this flag. OAuth discovery endpoints still require HTTPS or loopback even when the MCP URL is opted in:

```toml
[mcp.servers.lan]
transport = "streamable_http"
url = "http://10.0.0.5:3000/mcp"
allow_insecure_http = true
```

`headers_from_env` maps HTTP header names to ambient variable names. Put the complete header value in the environment, such as `Bearer ...`. Rho does not store it in config or diagnostics. Automatic HTTP redirects are disabled, so configured headers cannot be replayed to another origin.
`headers` supplies literal header values with the configuration. Prefer `headers_from_env` for anything secret. On a name collision, environment-derived headers override literal ones.

As with stdio, configuration is the trust boundary for the remote session; tool calls do not re-request network capability.

## OAuth

A server that issues its own credentials can be authorized with OAuth 2.1 instead of a header. Declaring the `oauth` table is the opt-in:

```toml
[mcp.servers.remote]
transport = "streamable_http"
url = "https://mcp.example.com/mcp"

[mcp.servers.remote.oauth]
# Both keys are optional. An empty table asks for full discovery.
client_id = "issued-out-of-band"
scopes = ["read", "write"]
```

Leave `client_id` out and Rho registers itself with the authorization server through dynamic client registration (RFC 7591). Leave `scopes` out and the server's own metadata chooses them. Nothing secret belongs in this table.

What happens on the first connection to such a server:

1. Rho sends one unauthenticated request and reads the `WWW-Authenticate` header of the 401. That header names the protected resource metadata document (RFC 9728), which names the authorization server, whose metadata (RFC 8414) names the endpoints.
2. Rho registers a client if none is configured.
3. Rho opens your browser for the authorization code flow with PKCE (S256) and a loopback redirect on an ephemeral port, and includes the `resource` indicator (RFC 8707) so the token is bound to this server.
4. The tokens go into the same credential store as your provider logins, under the server identity. Later sessions reuse them and refresh an expired access token without asking again.

Rules the flow holds to:

- Every OAuth endpoint must use HTTPS, with the same loopback exception the MCP URL gets. A discovery document cannot move any leg of the exchange onto plaintext HTTP.
- A server that publishes no metadata is refused. Rho does not guess authorization endpoints from the MCP URL.
- The authorization server metadata must carry an issuer that matches where it was discovered, and tokens minted by a previous issuer are discarded if a server is repointed.
- The loopback listener answers one exact callback path and requires the CSRF state, so a stray browser request cannot finish the login.
- Discovery and loading stored credentials are each bounded at 60 seconds (120 seconds combined before login), and the browser login at 5 minutes, so a login nobody finishes cannot hang session startup.
- Tokens never reach config, `rho mcp list --json`, `rho mcp show`, or logs.

A configured `Authorization` header wins over the `oauth` table and suppresses the flow entirely. You already said which credential to send, so Rho sends it and never opens a browser.

The browser login needs a person, so it only runs when Rho has a terminal and is not in CI. `rho mcp list --connect` and `rho mcp show --connect` never open a browser: a server with no stored token is reported as failed, with an error telling you to start Rho interactively and authorize it once.

## Handshake

Rho identifies itself in `initialize` as `rho` with the running version, and declares the client capabilities it actually answers:

- `roots`: Rho advertises the session workspace as one `file://` root. The declaration sets `listChanged: false`, because a session's workspace never changes and Rho would never send the notification.
- `elicitation`: declared only in a run that can put a question in front of a person, and only in form mode with `schemaValidation: false`. See [Questions from a server](#questions-from-a-server).
- `sampling`: declared only to a server that opted in with `sampling = "ask"`, and only in a run that has a model to spend. See [Completions for a server](#completions-for-a-server).

Rho never declares a capability it would then always refuse. A run that cannot show a questionnaire, such as `rho mcp list --connect` or an automation run started without a host-input responder, declares no `elicitation`, so a well-behaved server never asks.

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

Enabled servers initialize independently. Interactive sessions paint the TUI first and connect in the background. Composer text, slash commands, and pickers stay available while that happens. Pressing `enter` on a model prompt waits until connect finishes or times out, then starts the turn so the model sees the discovered tools. Automation (`rho run`) still connects before the first request.

Each server has a two-minute startup budget for connection, handshake, and discovery. A timeout logs the server identity and limit. Rho does not retry during startup. A malformed entry, failed executable, failed connection, authentication error, handshake error, or `tools/list` error disables only that server. Other MCP servers and built-in tools continue to load. After a session is established, discovery failures and timeouts attempt a bounded close instead of relying only on Drop.

Rho attaches a progress token to every tool call, so a server may report `notifications/progress` against it. Progress reaches the tool card while the call runs. Counts appear only when the server supplies a total.

Cancelling a turn sends `notifications/cancelled` for the in-flight call, so the server can stop its own work instead of continuing on an abandoned request. The same happens when a call exceeds its two-minute budget.

Streamable HTTP sessions are pinged once a minute, because an idle proxy can drop a remote connection with no local signal. A failed ping appears in `/mcp` and `rho mcp show`. Stdio servers need no ping: a dead child is observable directly.

Rho exports discovered tools as:

```text
mcp__<server_identity>__<tool_name>
```

Components containing only ASCII letters, digits, and `_` remain unchanged, unless they contain the `__` separator. Rho encodes every other component, and components beginning with the reserved `_rho_` prefix, as `_rho_` followed by the lowercase hexadecimal UTF-8 bytes. This encoding keeps distinct server and remote tool names distinct. Descriptions include the owning server identity for diagnostics. `allow` is an optional allowlist; `deny` always wins.

MCP tool calls use Rho's native tool registry, cancellation, and shutdown path. MCP error results and transport failures become tool failures without stopping sibling servers.

### Tool declarations

Rho reads more than the name, description, and input schema:

- **`title`** appears in the exported description, so the model sees the readable name next to the server identity.
- **Annotations** (`readOnlyHint`, `destructiveHint`, `openWorldHint`) appear as `Server hint:` lines in the description and as notices on the tool card. They are hints from a server Rho does not control, so they never relax a permission or skip an approval. A read-only hint changes the card's icon and nothing else.
- **`outputSchema`** sets a contract. A server that declares one must return `structuredContent` that validates against that schema; a missing or invalid result fails the call rather than passing a half-answer to the model.

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

Binary payloads never reach the model as base64. A returned image rides on the tool card as an asset, and the model gets a short descriptor it can reason about. The card shows one image, so Rho keeps only the first image that fits a 4 MiB encoded-byte budget and describes later or oversized images without retaining their bytes.

When a result carries `structuredContent`, that JSON is what the model reads. Servers are asked to mirror structured content as text for older clients; Rho drops the mirrored copy so one result does not cost the context twice.

### Tool lists that change mid-session

A server that declares `tools.listChanged` may send `notifications/tools/list_changed`. Rho re-runs `tools/list` for that server and reconciles the result:

- **Revised tools** keep working. A new description or input schema reaches the model on the next turn, because Rho reads each tool's definition when it builds a turn.
- **Withdrawn tools** stay registered under their exported name and fail with a clear reason if called.
- **Added tools** cannot join the registry mid-session, because a session's tool set is fixed once it starts. `/mcp` and `rho mcp show` list them and say a restart is needed.

## Prompts

A server that declares the `prompts` capability offers its prompts as slash commands:

```text
/mcp:<server_identity>:<prompt_name>
```

They appear in the command palette next to built-in commands, prompt templates, and skills. Rho lists a server's prompts once, at connect, so typing `/` matches without a round-trip. `notifications/prompts/list_changed` refreshes the list.

Arguments follow the command. A prompt that takes exactly one argument takes everything you type as that argument's value, because `key=value` for a single field is friction with no purpose:

```text
/mcp:docs:search how do sessions resume
```

A prompt with several arguments reads whitespace-separated pairs:

```text
/mcp:tickets:triage id=4821 severity=high
```

Leaving out a required argument reports which one is missing and starts no turn.

### Argument suggestions

A server that also declares the `completions` capability can suggest values for
the argument you are filling in. Put the cursor in a value and the palette
offers what the server sent back:

```text
/mcp:tickets:triage id=4821 severity=hi
                                    ^ high, highest
```

Picking one replaces that value alone. The command and any other arguments you
have already typed stay as they are.

The suggestion is a round-trip, so it arrives a moment after the keystroke that
asked for it. Rho keeps one request in flight at a time and reuses answers it
already holds, so typing fast costs one request per reply rather than one per
character. A server that declares no `completions` capability is never asked,
and a request that fails or arrives late simply leaves the palette empty:
nothing blocks, and no error interrupts what you are writing.

Rho fetches the prompt when you submit, not when you complete the command, because `prompts/get` is a round-trip to the server. The returned messages become one user turn. A message the server marked as coming from the assistant is labelled as such, so the model reads it as prior context rather than as a request from you. Prompt text is capped by `max_output_bytes`, like tool output.
## Resources

Tools are for the model. Resources are for you: a server publishes documents,
records, or images, and you decide which of them a message carries.

At connect, Rho lists the resources of every server that declares the
`resources` capability, from both `resources/list` and
`resources/templates/list`. The listing is held for the session and re-listed
when a server sends `notifications/resources/list_changed`, so matching a
resource costs no round-trip.

### Picking one

Type `@` in the message box. The palette that already completes workspace file
paths also offers server resources, matched on their URI against whatever you
type after the `@`. Resources are listed first, because a workspace holds far
more files than a server holds resources.

Each row shows the resource URI, the server that offers it, and the server's own
name for it. Enter or Tab picks the highlighted row.

What picking does depends on the row:

| Row | What Enter does |
| --- | --- |
| workspace file | writes `@path` into the message, unchanged from before |
| resource | reads it from the server and attaches the content, removing the `@` token |
| resource **template** | writes the template URI into the message for you to fill in |

A template URI carries [RFC 6570](https://www.rfc-editor.org/rfc/rfc6570)
placeholders, such as `db://users/{id}`, so there is nothing to read until you
replace them. Rho inserts it as text and marks the row `· template`.

### While the read runs

`resources/read` is a round-trip, so the resource appears in the composer right
away as `[resource: <uri> · reading]`. You can keep typing. The message cannot be
sent until the read finishes, and Backspace on the attachment cancels it, the
same as any other attachment.

A read that fails removes the attachment and reports the reason, so the composer
never gets stuck holding a resource that never arrived.

### What arrives

| What the server returned | What the message carries |
| --- | --- |
| text | a text attachment holding the body |
| a single `image/*` blob | an image the model can look at |
| any other blob | a text attachment holding `[resource <mime>, <size>]` |
| several bodies | one text attachment holding all of them |

Image blobs are passed to the provider exactly as the server encoded them.
Resource text is capped by the same `max_output_bytes` limit as tool output, so
one large resource cannot swamp the turn it joins.
## Questions from a server

A server may interrupt a tool call with `elicitation/create` to ask the user something. Rho shows the request as a form on the tool card that caused it, titled with the asking server's identity so it is never mistaken for one of Rho's own questions.

**How Rho decides which call the question belongs to.** The protocol carries nothing in an `elicitation/create` that names the `tools/call` it came from. Rho therefore records the in-flight calls of each session and answers only when exactly one call is running on that server. With no call running, or with several, Rho declines rather than interrupting a caller it guessed at. Two servers asking at the same time never confuse each other, because the record is per session.

**What Rho does with the answer.**

| What happened | What the server is told |
| --- | --- |
| The user filled the form in | `accept`, with the answers typed to the schema |
| The user dismissed the form | `cancel`, which also ends the turn |
| Rho could not ask, or could not type the answer | `decline` |

Rho declines rather than failing the request, because a decline is a first-class MCP answer that lets the server carry on without the information.

**Fidelity limit.** Rho's questionnaire is choice-only: every question is a list of values, optionally with free text, and every answer arrives as text. An elicitation schema is richer than that, so the mapping is lossy in one direction:

| Schema field | What the user sees |
| --- | --- |
| `enum` (single or multi select) | the choices, with titles when the schema supplies them |
| `boolean` | a yes/no confirm |
| `string` | free text |
| `number`, `integer` | free text, parsed back to a number |

Constraints such as `minLength`, `minimum`, `maxItems`, and `format` are shown to the user as help text and are **not enforced**, which is why Rho declares `schemaValidation: false`. Rho does guarantee the JSON *type* of every field it sends back: a `number` field never returns the string `"3"`. An answer that cannot carry its declared type declines the request instead of sending something the server did not ask for.

A schema Rho cannot render at all, such as one with no properties or an enum with no values, is declined whole rather than shown as a partial form. URL-mode elicitation is always declined: Rho opens no browser, and accepting without opening one would tell the server the user had answered.

While a form is open, the tool call's two-minute budget is paused. The budget exists to bound an unresponsive server, and a turn waiting on a person is not unresponsive.

## Completions for a server

A server may ask Rho's model to write something with `sampling/createMessage`. This spends the user's tokens on work the user did not ask for, so it is behind **two independent gates that both have to open**.

**Gate one, config.** The server opts in per entry. The default is `deny`, and a server left at the default never sees `sampling` in Rho's declared capabilities:

```toml
[mcp.servers.reviewer]
transport = "stdio"
command = "reviewer-mcp"
sampling = "ask"
```

Plugin-provided servers cannot opt themselves in; only the selected config file can.

**Gate two, the user.** Every individual request still raises a question naming the server, the number of messages, and the token ceiling it asked for. A refusal rejects the request and no model call happens. The gate is a question rather than a permission prompt on purpose: Rho's default `bypass` permission mode allows every capability by policy, so an approval prompt would never reach a person there, and token spend a server asked for is not something that mode ever opted into.

Sampling is routed to the in-flight tool call the same way elicitation is, and refuses under the same rules: no call in flight, or more than one, and the request is rejected. A run with no model bound, such as `rho run` or `rho mcp list --connect`, rejects every sampling request and declares no `sampling` capability. Sampling spend is recorded in the usage ledger under the `mcp_sampling` purpose, so it is attributable apart from the user's own turns.

**Fidelity limits.**

- **`modelPreferences` is ignored.** The provider, model, and credentials are the user's configuration. Letting a server steer them would let it choose which of the user's accounts pays and which model sees the prompt. Rho always uses the session's current model and reports its name in the result.
- **The conversation is flattened.** Rho's one-shot path takes a system prompt and one user turn, so a multi-turn sampling conversation is rendered as labelled text. Non-text blocks are named but not sent; Rho does not forward a server's images or tool results into the user's model.
- **`maxTokens` is not enforced.** Rho shows the requested ceiling in the confirmation but has no per-request token cap to apply.
- **`includeContext`, `temperature`, `stopSequences`, `tools`, and `toolChoice` are not honored.** Rho declares no sampling sub-capabilities, so a server should not expect them.

A sampling call is bounded at three minutes. Cancelling the turn cancels it.

## Inspect status

There is no marketplace in Rho. Configure servers in the selected config file, then inspect config or live load status:

- **Interactive:** `/mcp` lists configured servers, transport, status, errors, exported tool names, and any prompts and resources the server offers, for the current session. A server that is still connecting shows as connecting, not as a failure. `/doctor` includes an MCP health row.
- **CLI:** `rho mcp list` prints configured servers from the selected config and plugins without starting them. `rho mcp show <id>` prints one server, including its prompts and resources when started with `--connect`. Pass `--connect` on either command to start enabled servers and report live status and discovered tools. Both accept `--json`.

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
Rho creates it immediately before it starts each enabled stdio server. If directory preparation fails, Rho disables only that server. A plugin with no valid MCP servers adds no MCP startup work, and the zero-server fast path above still applies. Servers from a project's plugin packages start only in trusted workspaces (`RHO_TRUST_PROJECT_PLUGINS=1`).
