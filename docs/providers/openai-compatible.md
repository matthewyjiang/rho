# Custom OpenAI-compatible hosts

You can point Rho at any local or remote host that speaks OpenAI Chat Completions. There is no login picker. Add a name and a base URL in config, like Ollama. Rho sends no `Authorization` header.

This is not the first-party [OpenAI](/providers/openai) provider.

## Define a host

Edit `~/.rho/config.toml`. The table key is the provider name used in `/model`.

```toml
[providers.custom.composer]
base_url = "http://127.0.0.1:8787/v1"

[providers.custom.vllm]
base_url = "http://127.0.0.1:8000/v1"
```

Keep the `/v1` suffix. Rho appends `/models` for discovery and `/chat/completions` for agent turns. The URL must use `http` or `https` and cannot contain credentials, a query, or a fragment.

Names must be lowercase letters, digits, and hyphens, start with a letter, and must not match a built-in provider. Restart Rho after you add or rename a host. Changing only `base_url` on an existing name is enough on the next request if Rho is already running that host; adding a new name still needs a restart.

## Use it

```toml
[model]
provider = "composer"
model = "composer-2.5"
auth = "none"
```

Or, after a restart, Rho fetches `/v1/models` for each custom host so the picker is populated. You can also refresh by hand in `/config` if the host was down at startup.

```text
/model composer/composer-2.5
```

Do not run `/login`. There is no API key and no credential store entry.

## Models

Rho fetches `/v1/models` at startup for every custom host. A down host is skipped so startup still succeeds; refresh later in `/config` once it is up. The host must support tool calls if you want a coding agent.

Rho sends `reasoning_effort` on each turn, including `"none"` when reasoning is off. Shift+Tab and `/config` cycle the level. Hosts that do not accept that field may reject the request; pin levels in `~/.rho/models.toml` if you need a smaller set.

## Example: API for Cursor

[API for Cursor](https://github.com/standardagents/composer-api) serves Cursor models from a local `/v1` server.

```toml
[providers.custom.composer]
base_url = "http://127.0.0.1:8787/v1"

[model]
provider = "composer"
model = "composer-2.5"
auth = "none"
```

## Automation

```bash
rho --provider composer --model composer/composer-2.5 run "review this project"
```
