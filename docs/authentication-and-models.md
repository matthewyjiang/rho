# Authentication and models

Rho supports OpenAI API key auth and Codex OAuth auth. The selected provider, model, and auth mode are stored in [configuration](/configuration).

## OpenAI API key

Set an OpenAI API key and choose a model:

```bash
export OPENAI_API_KEY=...
rho --provider openai --auth api-key --model gpt-5.5
```

The `openai` provider uses API-key authentication.

## Codex OAuth

Rho can use an existing Codex CLI login:

```bash
codex login
rho --provider openai-codex --auth codex --model gpt-5.5
```

Rho reads `CODEX_ACCESS_TOKEN` or `~/.codex/auth.json`. If the default API base is unchanged, Codex auth uses:

```text
https://chatgpt.com/backend-api/codex/responses
```

## Providers and model catalog

Rho's implemented providers are:

| Provider | Auth mode | Use case |
| --- | --- | --- |
| `openai` | `api-key` | OpenAI API-key models |
| `openai-codex` | `codex` | Codex OAuth models |

Rho uses a built-in static model catalog instead of querying providers for model listings. Codex models live under the `openai-codex` provider, separate from API-key OpenAI models.

In the [interactive TUI](/interactive-tui), use [`/model`](/interactive-tui#commands) to open a picker filtered to models that match the active auth mode. Use `/model provider/model` to switch explicitly, including to another provider:

```text
/model openai/gpt-5.5
/model openai-codex/gpt-5.5
```

A bare model id works when it uniquely matches the catalog. Uncataloged bare model ids stay on the current provider as an escape hatch for newly released models.

For persistent defaults, see [configuration](/configuration). For one-shot prompts, see [automation and CLI](/automation-cli).
