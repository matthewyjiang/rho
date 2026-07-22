# Poolside

Rho supports the [Poolside API Platform](https://platform.poolside.ai) through its OpenAI-compatible Chat Completions API.

## Provider details

| Setting | Value |
| --- | --- |
| Provider | `poolside` |
| Auth | `poolside-api-key` |
| Environment override | `POOLSIDE_API_KEY` |
| API base | `https://inference.poolside.ai/v1` |
| Model source | Poolside `/models` API |

Create an API key in [Poolside Platform](https://platform.poolside.ai). Poolside model access and model IDs depend on your account, so Rho fetches the model list after login instead of keeping a static list.

## Interactive login

In the TUI, run:

```text
/login poolside
```

Paste your Poolside API key when prompted. Rho stores it in the configured credential store and refreshes the models available to your account. You can then select one with `/model`, for example:

```text
/model poolside/laguna-m.1
```

Rho stores the unprefixed model id (`laguna-m.1`) in cache and config. User-facing selection still uses `poolside/laguna-m.1`, and Rho sends the namespaced wire id `poolside/laguna-m.1` to the Poolside API.

Remove the stored key with:

```text
/logout poolside
```

## Environment and automation

For CI or development, set `POOLSIDE_API_KEY`. It overrides a stored key:

```bash
export POOLSIDE_API_KEY="<api-key>"
rho --provider poolside \
  --auth poolside-api-key \
  --model poolside/laguna-m.1 \
  run "review this project"
```

Rho sends the key as a Bearer token. Do not put the key in `config.toml` or commit it to source control.

## Models and reasoning

Use `/config` and choose **Refresh model lists** to fetch the current models for your Poolside account. Poolside reasoning models expose a binary thinking control. Rho shows `Off` and `Max` as the available reasoning levels:

- `Off` sends `chat_template_kwargs.enable_thinking` as `false`.
- `Max` leaves `chat_template_kwargs` unset, which uses Poolside's default thinking mode.

The models.dev catalog marks Poolside reasoning models with `reasoning: true` but does not advertise effort levels. Rho maps that Poolside-specific capability to `Off` and `Max`; it does not send OpenRouter's `reasoning.effort` field to the direct Poolside API.

See [Poolside's API examples](https://docs.poolside.ai/api/openai-api-examples#send-a-chat-prompt) for request details.
