# OpenCode Go

Rho supports [OpenCode Go](https://opencode.ai/docs/go/) as a first-party provider. Go is a Zen subscription that serves curated open coding models from `https://opencode.ai/zen/go/v1`.

## Provider details

| Setting | Value |
| --- | --- |
| Provider | `opencode-go` |
| Auth | `opencode-go-api-key` |
| Environment override | `OPENCODE_API_KEY` |
| API base | `https://opencode.ai/zen/go/v1` |
| Model source | OpenCode Go `/models` |
| Metadata | [models.dev `opencode-go`](https://models.dev/providers/opencode-go) |

Subscribe in the [OpenCode Zen console](https://opencode.ai/auth), copy the API key, and store it with Rho. Rho sends the key as a Bearer token for Chat Completions and Responses, and as `x-api-key` for Anthropic Messages. Do not put the key in `config.toml`.

## Interactive login

In the TUI, run:

```text
/login opencode-go
```

Rho stores the key and refreshes the models available on the Go gateway. Select one with `/model`, for example:

```text
/model opencode-go/kimi-k2.7-code
```

Remove the stored key with:

```text
/logout opencode-go
```

## Environment and automation

For CI or development, set `OPENCODE_API_KEY`. It overrides a stored key.

```bash
export OPENCODE_API_KEY="<api-key>"
rho --provider opencode-go \
  --auth opencode-go-api-key \
  --model opencode-go/kimi-k2.7-code \
  run "review this project"
```

## Models and wire shape

Use `/config` and choose **Refresh model lists** to fetch the current Go catalog. Rho does not ship a baked default model; the first cached `/models` id is used until you pick one.

Cost, context, and reasoning controls come from models.dev under `opencode-go`. The same catalog names the AI SDK package for each model. Rho uses that package to choose Chat Completions, OpenAI Responses, or Anthropic Messages. New Go models work after a models.dev refresh without a Rho update, as long as they use one of those packages.

See OpenCode's [Go documentation](https://opencode.ai/docs/go/) for subscription limits and the live model list.
