# OpenCode Zen

Rho supports [OpenCode Zen](https://opencode.ai/docs/zen/) as a first-party provider. Zen is OpenCode's pay-as-you-go gateway: you add credits in the OpenCode console and pay per request for curated coding models from `https://opencode.ai/zen/v1`. For the subscription plan, see [OpenCode Go](/providers/opencode-go).

## Provider details

| Setting | Value |
| --- | --- |
| Provider | `opencode` |
| Auth | `opencode-api-key` |
| Environment override | `OPENCODE_ZEN_API_KEY` |
| API base | `https://opencode.ai/zen/v1` |
| Model source | OpenCode Zen `/models`, minus free models |
| Metadata | [models.dev `opencode`](https://models.dev/providers/opencode) |

Sign in to the [OpenCode console](https://opencode.ai/auth), add credits, copy the API key, and store it with Rho. The console issues one key for both Zen and Go. Rho sends the key as a Bearer token for Chat Completions and Responses, as `x-api-key` for Anthropic Messages, and as `x-goog-api-key` for Gemini. Do not put the key in `config.toml`.

## Interactive login

In the TUI, run `/login` and choose **OpenCode**, then **Zen pay-as-you-go**. You can also skip the picker:

```text
/login opencode
```

If you already logged in to OpenCode Go, leave the field blank and press Enter to reuse that key. Type a different key to store it instead. Each login keeps its own copy, so Zen models appear only after you log in to Zen, and logging out of one leaves the other signed in.

Rho stores the key and refreshes the models available on Zen. Select one with `/model`, for example:

```text
/model opencode/claude-sonnet-5
```

Remove the stored key with:

```text
/logout opencode
```

## Environment and automation

For CI or development, set `OPENCODE_ZEN_API_KEY`. It overrides a stored Zen key. Go reads `OPENCODE_API_KEY`, so you can export one without listing the other gateway's models.

```bash
export OPENCODE_ZEN_API_KEY="<api-key>"
rho --provider opencode \
  --auth opencode-api-key \
  --model opencode/claude-sonnet-5 \
  run "review this project"
```

## Models and wire shape

Use `/config` and choose **Refresh model lists** to fetch the current Zen catalog. Rho does not ship a baked default model; the first cached `/models` id is used until you pick one.

Cost, context, and reasoning controls come from models.dev under `opencode`. The same catalog names the AI SDK package for each model. Rho uses that package to choose Chat Completions, OpenAI Responses, Anthropic Messages, or Gemini `generateContent`, the same way it does for [OpenCode Go](/providers/opencode-go#models-and-wire-shape). New Zen models work after a models.dev refresh without a Rho update, as long as they use one of those packages.

Every adapter sends Rho's user agent and an `x-opencode-session` header built from the conversation's prompt cache key.

## Free models

Zen lists free models (such as Big Pickle and the `-free` ids) in its `/models` response, but serves them only to the OpenCode client. Requests from other clients fail with `403 FreeTierError`. Rho hides models that models.dev prices at $0 from the Zen model list. A model without a models.dev entry stays listed until the catalog catches up. To use OpenCode's free models, use the [OpenCode client](https://opencode.ai).

Zen has no usage windows, so `/limits` does not report it. Check your balance in the OpenCode console.
