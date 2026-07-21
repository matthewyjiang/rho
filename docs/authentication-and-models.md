# Authentication and models

Rho supports several providers with different auth modes. This page covers the concepts shared across all of them. For provider-specific login, logout, environment overrides, and model selection, see the individual [provider pages](#providers).

Provider, model, and auth mode are stored in [configuration](/configuration). Secrets are never stored in config.

## Providers

Rho's implemented providers are:

| Provider | Auth mode | Details |
| --- | --- | --- |
| `openai` | `api-key` | [OpenAI](/providers/openai) |
| `openai-codex` | `codex` | [OpenAI (Codex OAuth)](/providers/openai-codex) |
| `anthropic` | `anthropic-api-key` | [Anthropic](/providers/anthropic) |
| `google` | `google-api-key` | [Google Gemini](/providers/google-gemini) |
| `github-copilot` | `github-copilot` | [GitHub Copilot](/providers/github-copilot) |
| `xai` | `xai-api-key` | [xAI](/providers/xai) |
| `xai-oauth` | `xai-oauth` | [xAI](/providers/xai) |
| `openrouter` | `openrouter-api-key` | [OpenRouter](/providers/openrouter) |
| `ollama` | None | [Ollama](/providers/ollama) |
| `moonshot` | `moonshot-api-key` | [Moonshot and Kimi Code](/providers/moonshot-kimi) |
| `kimi-code` | `kimi-oauth` | [Moonshot and Kimi Code](/providers/moonshot-kimi) |

OpenAI, Anthropic, Google Gemini, GitHub Copilot, Ollama, OpenRouter, Moonshot, and Kimi Code expose refreshable API model lists. Ollama needs no login; the other providers refresh after authentication. OpenAI Codex OAuth and xAI OAuth use static allowlists, so their available models are maintained by Rho rather than fetched through **Refresh model lists** in `/config`.

Each provider page documents whether authentication is required, how to select models, and any provider-specific setup.

## Where credentials live

Rho stores credentials in the native OS credential store through an OS-agnostic abstraction. If no OS credential store is available, login fails closed with setup guidance. Rho does not add a plaintext or encrypted file fallback.

On macOS, see Apple's [Keychain access prompt](https://support.apple.com/guide/keychain-access/if-youre-asked-for-access-to-your-keychain-kyca1243/mac) documentation when the OS asks whether to allow a credential-store operation.

For normal interactive setup, prefer `/login`. Environment variables are CI/development escape hatches and override stored credentials; each provider page lists the variables it reads. Command-line flags override values loaded from configuration for the current invocation, and flags that select provider, model, auth, or reasoning also become the saved default.

## Login and provider switching

`/login` opens a readable provider picker. Providers with multiple authentication methods open a second picker with prompts such as **API Key** and **OAuth**; providers with one method continue directly to that login flow. Direct args (`/login openai`, `/login anthropic`, and so on) target a single method. See each [provider page](#providers) for the exact flow.

Successful login normally stores credentials only. It does not switch the active provider/model, because provider switching is model-driven through `/model`. If Rho started without usable auth and is running on an unauthenticated placeholder, a successful login selects that provider's default model so the session becomes usable.

`/logout` opens a provider picker containing only providers with stored credentials that can be deleted. If an environment override is still present, the provider remains available after deleting the stored credential.

## Selecting models

Use `/model provider/model` to switch explicitly, including to another provider:

```text
/model openai/gpt-5.6-sol
/model openai-codex/gpt-5.6-sol
/model anthropic/claude-sonnet-4-5
/model google/gemini-3.1-flash-lite
/model github-copilot/gpt-4.1
/model openrouter/anthropic/claude-sonnet-4
/model ollama/<installed-model>
/model xai/grok-4.5
/model xai-oauth/grok-4.5
```

A bare model id works when it uniquely matches the catalog for the active selection rules. Uncataloged bare model ids stay on the current provider as an escape hatch for newly released models.

OpenAI, Anthropic, Google Gemini, GitHub Copilot, Ollama, OpenRouter, Moonshot, and Kimi Code can refresh their provider model lists through **Refresh model lists** in `/config`. Ollama reads its installed models without authentication. Codex OAuth and xAI OAuth use static allowlists instead. API-backed model lists can change as providers add or remove models; refresh them before selecting a newly released or newly installed model.

## Model metadata

Rho uses cached model metadata and built-in overrides to choose effective context windows for status display and [auto compaction](/configuration#auto-compaction). The same metadata supplies each model's available [reasoning effort levels](/configuration#reasoning-options), allowing the TUI to skip unsupported choices without model-name allowlists. Pricing-sensitive models such as `openai/gpt-5.6-sol` and `openai-codex/gpt-5.6-sol` use safer effective windows below their advertised maximums.

For subscription auth modes such as Codex OAuth and xAI OAuth, the statusline still estimates an equivalent API cost from [models.dev](https://models.dev/) pricing (including long-context rate tiers when available) and labels it `(sub)`. When a model is seen for the first time, Rho refreshes models.dev so newly added providers are not stuck on a stale local snapshot.

For persistent defaults, see [configuration](/configuration). For one-shot prompts, see [automation and CLI](/automation-cli).
