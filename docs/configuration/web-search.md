# Web search

Parent: [Configuration](/configuration). Tool behavior: [Web access](/tools-workspace/web-access).

Change these under `/config` → **Tools**, in the **Web search** section: the mode, the backend, each connection, and each endpoint. Editing a backend's endpoint does not select it. Changes apply to sessions opened afterwards.

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

Omit `api_base_url` to use Firecrawl Cloud at `https://api.firecrawl.dev`. Cloud requires a key. A self-hosted endpoint may omit authentication. Set `FIRECRAWL_API_KEY`. Keys are not written into backend configuration tables.

## Backend connections

Each backend has its own endpoint and, for OpenAI and Exa, connection setting. Editing them does not select that backend. API base URLs accept HTTP or HTTPS and keep reverse-proxy path prefixes. Do not include the operation path. Firecrawl appends `v2/search`, for example. URLs must not contain embedded credentials, query strings, or fragments. Clear the URL to restore the default.

| Backend | Default API base | Connection |
| --- | --- | --- |
| OpenAI | `https://api.openai.com/v1` | `api` or `codex` |
| Exa | `https://api.exa.ai` | `api` or `mcp` |
| Brave | `https://api.search.brave.com` | API |
| Firecrawl | `https://api.firecrawl.dev` | API, including self-hosted |

Set `web_search.openai.connection` to `codex` to use a Codex login. Codex uses its fixed ChatGPT endpoint. Custom API URLs never receive Codex OAuth tokens. API mode uses an API key.

Set `web_search.exa.connection` to `mcp` to use Exa MCP. `web_search.exa.mcp_url` defaults to `https://mcp.exa.ai/mcp`. Exa API mode does not switch to MCP when its key is missing. OpenAI and Exa both default to API connections.

Changes to mode, backend, endpoint, and connection, from `/config` or a hand edit the daemon reads, apply to sessions opened afterwards. Running sessions and already-started agents keep their bound settings.

API keys come from the environment: `OPENAI_API_KEY`, `EXA_API_KEY`, `BRAVE_SEARCH_API_KEY` (or `BRAVE_API_KEY`), and `FIRECRAWL_API_KEY`. Keys saved by earlier versions stay in the credential store and keep working. Entering a key from the TUI is coming with daemon credentials ([#1662](https://github.com/matthewyjiang/rho/issues/1662)).

Firecrawl uses the [v2 search API](https://docs.firecrawl.dev/api-reference/endpoint/search). Self-hosted support depends on the deployed version and its search infrastructure. This integration does not enable Firecrawl scraping. `includeContent` keeps Rho's existing page-fetch behavior.

## Older settings

Rho 3.0 no longer reads the 2.x `hosted`, `provider`, and top-level `web_search_provider` keys. The [2.x upgrade](/configuration#upgrading-from-rho-2x) rewrites them the first time Rho 3.0 starts: `provider = "disabled"` becomes `mode = "off"`, and a concrete `provider` becomes `backend` with `mode = "auto"`, or `mode = "backend"` when `hosted` was `false`. Set by hand afterwards, they are unknown keys and the config fails to load.

The upgrade also moves flat `web_search_openai_api_key`, `web_search_exa_api_key`, and `web_search_brave_api_key` values to the credential store. Empty strings are ignored.
