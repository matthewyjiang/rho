# MiniMax

Rho supports [MiniMax](https://platform.minimax.io/) as a first-party provider. MiniMax serves its M-series coding models (MiniMax-M3, M2.x) from an Anthropic-compatible endpoint at `https://api.minimax.io/anthropic/v1`.

## Provider details

| Setting | Value |
| --- | --- |
| Provider | `minimax` |
| Auth | `minimax-api-key` |
| Environment override | `MINIMAX_API_KEY` |
| API base | `https://api.minimax.io/anthropic/v1` |
| Model source | MiniMax `/models` |
| Metadata | [models.dev `minimax`](https://models.dev/providers/minimax) |
| Default model | `MiniMax-M3` |

Create an API key in the [MiniMax console](https://platform.minimax.io/user-center/basic-information/interface-key), then store it with Rho. Do not put the key in `config.toml`.

## Interactive login

In the TUI, run:

```text
/login minimax
```

Rho stores the key and refreshes the model list. Select a model with `/model`, for example:

```text
/model minimax/MiniMax-M3
```

Remove the stored key with:

```text
/logout minimax
```

## Environment and automation

For CI or development, set `MINIMAX_API_KEY`. It overrides a stored key.

```bash
export MINIMAX_API_KEY="<api-key>"
rho --provider minimax \
  --auth minimax-api-key \
  --model minimax/MiniMax-M3 \
  run "review this project"
```

## Models and wire shape

Use `/config` and choose **Refresh model lists** to fetch the current catalog from `https://api.minimax.io/anthropic/v1/models`.

Cost, context, and reasoning controls come from models.dev under `minimax`. Rho speaks Anthropic Messages on the MiniMax Anthropic-compatible base, including native thinking blocks. MiniMax-M3 supports toggling thinking; M2.x models always think.

See MiniMax's [API overview](https://platform.minimax.io/docs/api-reference/api-overview) for rate limits and plan details.
