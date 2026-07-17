# OpenRouter

Rho supports OpenRouter through its OpenAI-compatible Chat Completions API using API-key authentication.

## At a glance

| Setting | Value |
| --- | --- |
| Provider | `openrouter` |
| Auth | `openrouter-api-key` |
| Environment override | `OPENROUTER_API_KEY` |
| API base | `https://openrouter.ai/api/v1` |
| Model list | Refreshable after authentication |

## Login and model selection

In the interactive TUI, run:

```text
/login openrouter
/refresh-model-list openrouter
/model openrouter/<model-id>
```

Rho stores the API key in the OS credential store. For CI and development, `OPENROUTER_API_KEY` overrides the stored key. Remove the stored key with `/logout openrouter`.

OpenRouter model IDs commonly contain a slash, so a complete selection can look like:

```text
/model openrouter/anthropic/claude-sonnet-4
```

The model list is fetched from OpenRouter's `/models` endpoint after authentication. Run `/refresh-model-list openrouter` when models are added or removed. Rho sends turns to `/chat/completions`.

## Automation

Select the OpenRouter provider, API-key auth mode, and a model:

```sh
rho --provider openrouter --auth openrouter-api-key --model anthropic/claude-sonnet-4 run "hello"
```

Provide `OPENROUTER_API_KEY` in the automation environment or log in once through the TUI so Rho can read the stored key.
