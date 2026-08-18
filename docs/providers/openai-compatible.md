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

Keep the `/v1` suffix. Rho appends `/models` for discovery and `/chat/completions` for agent turns. The URL must use `http` or `https` and cannot contain credentials, a query, or a fragment.

Names must be lowercase letters, digits, and hyphens, start with a letter, and must not match a built-in provider. Restart Rho after you edit this table, including an existing `base_url`. Direct edits to `config.toml` do not update a running process. Creating or updating a host through `/login` applies immediately.

## Authentication

Custom hosts default to `auth = "none"` and send no `Authorization` header. If the host requires a key, store one with `/login <name>` or during onboarding. That selects `{name}-api-key` and sends `Authorization: Bearer <key>`. Secrets stay in the credential store, not in config.

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

Leave the `/login` key field blank to keep or switch back to `none`. `/logout <name>` deletes a stored key. The CI/development override is `RHO_<NAME>_API_KEY`, with the provider name uppercased and hyphens turned into underscores (`RHO_VLLM_API_KEY`).

## Use it

After a restart, or immediately after `/login` onboarding, Rho fetches `/v1/models` in the background for each custom host so the picker can fill in. You can also refresh by hand in `/config` if the host was down at startup.

```text
/model vllm/qwen2.5-coder
```

## Models

Rho fetches `/v1/models` in the background at startup for every custom host so the picker can fill in. A down host is skipped so startup still succeeds; refresh later in `/config` once it is up. Opening `/model` before the fetch lands can show a stale or empty custom list. The host must support tool calls if you want a coding agent.

Rho sends `reasoning_effort` on each turn, including `"none"` when reasoning is off. Shift+Tab and `/config` cycle the level. Hosts that do not accept that field may reject the request; pin levels in `~/.rho/models.toml` if you need a smaller set.

## Automation

```bash
rho --provider vllm --model vllm/qwen2.5-coder run "review this project"
```
