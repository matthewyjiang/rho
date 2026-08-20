# Custom OpenAI-compatible hosts

You can point Rho at any local or remote host that speaks OpenAI Chat Completions. Add a name and a base URL in config, or create the host from `/login`. Requests send an `Authorization` header only when you store an API key.

This is not the first-party [OpenAI](/providers/openai) provider.

## Define a host

The fastest path is `/login` in the [interactive TUI](/interactive-tui). Choose **Custom Chat Completions**, name the provider, enter its base URL, then enter an API key or leave that field blank.

You can also edit `~/.rho/config.toml`. The table key is the provider name used in `/model`.

```toml
[providers.custom.vllm]
base_url = "http://127.0.0.1:8000/v1"
```

A mixed proxy that is not itself in [models.dev](https://models.dev/) can borrow another catalog for context windows, prices, and reasoning lists. Set `catalog` to that models.dev provider slug. Model ids must match the borrowed catalog (`gpt-5.6-sol`, not `openai/gpt-5.6-sol`):

```toml
[providers.custom.cliproxyapi]
base_url = "http://127.0.0.1:8317/v1"
catalog = "llmgateway"
```

`llmgateway` is a mixed models.dev catalog with bare model ids. `openrouter` only matches if the host uses OpenRouter-style `owner/model` ids. `openai-codex` borrows Rho's Codex catalog, including built-in window overrides. Requests still go to the custom host; only metadata is borrowed. For one model that should use a different slug, set `catalog` on that row in `~/.rho/models.toml`. See [local model metadata](/configuration#local-model-metadata).

If the host already pushes `provider/model` ids (`anthropic/claude-sonnet-4-5`), set `catalog_mode = "model-id"` instead of a borrowed slug. Rho splits on the first `/` and looks that pair up in models.dev (`foo/bar/baz` → `foo` / `bar/baz`). A host cannot set both `catalog` and `catalog_mode = "model-id"`. A bare id with no slash misses catalog metadata and inserts a transcript notice. Per-model `catalog` in `models.toml` still wins. Open `/config`, choose **Providers**, then **Refresh models.dev catalog** to redownload that snapshot on demand.

Keep the `/v1` suffix. Rho appends `/models` for discovery and `/chat/completions` for agent turns. The URL must use `http` or `https` and cannot contain credentials, a query, or a fragment.

Names must be lowercase letters, digits, and hyphens, start with a letter, and must not match a built-in provider. Restart Rho after you edit this table, including an existing `base_url`. Direct edits to `config.toml` do not update a running process. Creating or updating a host through `/login` applies immediately.

## Authentication

Custom hosts default to `auth = "none"` and send no `Authorization` header. If the host requires a key, store one with `/login <name>` or during onboarding. That selects `{name}-api-key` and sends `Authorization: Bearer <key>`. Restart keeps a keyed profile. Startup only promotes leftover `none` when a key is stored; it does not write `none` over a keyed profile. Secrets stay in the credential store, not in config.

```toml
[model]
provider = "vllm"
model = "qwen2.5-coder"
auth = "none"
```

With a stored key:

```toml
[model]
provider = "vllm"
model = "qwen2.5-coder"
auth = "vllm-api-key"
```

Leave the `/login` key field blank to keep a stored or env-supplied key, or to run keyless when none is set. `/logout <name>` deletes a stored key. The CI/development override is `RHO_<NAME>_API_KEY`, with the provider name uppercased and hyphens turned into underscores (`RHO_VLLM_API_KEY`). The override applies to hosts defined in `config.toml`; Rho also strips those names from agent command environments so a tool cannot read them.

## Use it

After a restart, or immediately after `/login` onboarding, Rho fetches `/v1/models` in the background for each custom host so the picker can fill in. You can also refresh by hand in `/config` if the host was down at startup.

```text
/model vllm/qwen2.5-coder
```

## Models

Rho fetches `/v1/models` in the background at startup for every custom host so the picker can fill in. A down host is skipped so startup still succeeds; refresh later in `/config` once it is up. Opening `/model` before the fetch lands can show a stale or empty custom list. The host must support tool calls if you want a coding agent. Proxies such as CLIProxyAPI, and many local servers, omit tool-call ids, use sparse indexes, or skip `{}` for zero-argument tools. Rho fills those in so the tool loop can continue. First-party OpenAI-compatible providers stay strict.

Rho sends `reasoning_effort` on each turn, including `"none"` when reasoning is off. Shift+Tab and `/config` cycle the level. Hosts that do not accept that field may reject the request; pin levels in `~/.rho/models.toml` if you need a smaller set.

When the session has a prompt cache key, Rho also sends `prompt_cache_key` on Chat Completions so compatible hosts and proxies can pin prompt cache across turns. The field is omitted when there is no key.

## Automation

```bash
rho --provider vllm --model vllm/qwen2.5-coder run "review this project"
```
