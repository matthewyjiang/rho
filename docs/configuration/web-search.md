# Web search

Parent: [Configuration](/configuration). Tool behavior: [Web access](/tools-workspace/web-access).

Choose **Tools → Web search** in `/config`. Mode, backend, and connection settings open a picker. Backend pages stay available in every mode. OpenAI and Exa pages show settings for both connections. Editing a backend does not select it. Changes during a turn apply to the next turn.

| Mode | Routing |
| --- | --- |
| `auto` | Prefer native search when the active chat path supports it. Otherwise use the selected backend. |
| `backend` | Always use the selected backend, even when the chat provider supports native search. |
| `off` | Disable web search. |

`backend` is one of `openai`, `exa`, `brave`, or `firecrawl`. A failed request does not fall through to another service. The model cannot override this with tool arguments. Native search means the chat provider's own search tool, not Firecrawl Cloud.

Force a self-hosted Firecrawl deployment:

```toml
[web_search]
mode = "backend"
backend = "firecrawl"

[web_search.firecrawl]
api_base_url = "http://localhost:3002"
```

Omit `api_base_url` to use Firecrawl Cloud at `https://api.firecrawl.dev`. Cloud requires a key. A self-hosted endpoint may omit authentication. Save keys in the TUI credential editor or set `FIRECRAWL_API_KEY`. Keys are not written into backend configuration tables.

## Backend connections

Each backend has its own settings page. Editing an endpoint or key does not select that backend. API base URLs accept HTTP or HTTPS and keep reverse-proxy path prefixes. Do not include the operation path. Firecrawl appends `v2/search`, for example. URLs must not contain embedded credentials, query strings, or fragments. Clear the URL or choose its reset row to restore the default.

| Backend | Default API base | Connection |
| --- | --- | --- |
| OpenAI | `https://api.openai.com/v1` | `api` or `codex` |
| Exa | `https://api.exa.ai` | `api` or `mcp` |
| Brave | `https://api.search.brave.com` | API |
| Firecrawl | `https://api.firecrawl.dev` | API, including self-hosted |

Set `web_search.openai.connection` to `codex` to use a Codex login. Codex uses its fixed ChatGPT endpoint. Custom API URLs never receive Codex OAuth tokens. API mode uses an API key.

Set `web_search.exa.connection` to `mcp` to use Exa MCP. `web_search.exa.mcp_url` defaults to `https://mcp.exa.ai/mcp`. Exa API mode does not switch to MCP when its key is missing. OpenAI and Exa both default to API connections.

TUI changes to mode, backend, endpoint, connection, and credentials apply before the next turn. An active turn keeps its existing route and credential snapshot. Future delegated agents inherit the applied settings. Already-started agents keep their bound settings. **Next turn route** shows the saved connection and resolved URL, including whether it is default or custom. If applying settings fails, the next turn does not start. Direct `config.toml` edits require a restart.

**Test connection** asks before sending a test query. The request may incur charges. It tests the backend even when search mode is Auto or Off. A successful health check does not prove that a self-hosted deployment can search.

Firecrawl uses the [v2 search API](https://docs.firecrawl.dev/api-reference/endpoint/search). Self-hosted support depends on the deployed version and its search infrastructure. This integration does not enable Firecrawl scraping. `includeContent` keeps Rho's existing page-fetch behavior.

## Migrating older settings

- Legacy `hosted = false` plus `provider = "disabled"` becomes `mode = "off"`. Legacy provider names stay case-insensitive. A backend-only or endpoint edit preserves that mode. Only an explicit `mode` overrides legacy routing.
- A concrete legacy provider becomes the selected backend. `hosted` determines Auto versus Backend mode.
- Legacy `provider = "auto"` becomes the OpenAI backend, with a warning. The old OpenAI, then Exa, then Brave failure chain is gone.
- Legacy native-only configuration (`hosted = true`, `provider = "disabled"`) requires an explicit mode. Rho rejects that migration instead of enabling a client backend. Set `mode = "off"` to prevent search, or choose Auto and a backend.
- Legacy OpenAI and Exa users get a warning to select their connection. Choose `codex` or `mcp` if that was the previously implicit transport.

Existing flat `web_search_openai_api_key`, `web_search_exa_api_key`, and `web_search_brave_api_key` values still migrate to the credential store. Empty strings are ignored.
