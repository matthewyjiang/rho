# Authentication and models

Rho supports OpenAI API-key auth, Rho-owned Codex OAuth auth, Anthropic API-key auth, GitHub Copilot auth, and xAI OAuth for SuperGrok or X Premium+ subscriptions. Provider, model, and auth mode are stored in [configuration](/configuration). Secrets are not stored in config.

## Interactive login

Use `/login` in the interactive TUI:

```text
/login
/login openai
/login openai-codex
/login anthropic
/login github-copilot
/login xai
```

`/login` opens a provider picker. Direct args support provider names only:

| Command | Auth flow |
| --- | --- |
| `/login openai` | masked OpenAI API-key entry |
| `/login openai-codex` | browser-based Codex OAuth owned by Rho |
| `/login anthropic` | masked Anthropic API-key entry |
| `rho login openai-codex --device-auth` | Codex device-code login for remote/headless sessions |
| `/login github-copilot` | GitHub device code login for GitHub Copilot |
| `/login xai` | browser-based xAI OAuth for SuperGrok or X Premium+ |
| `rho login xai --device-auth` | xAI device-code login for remote/headless sessions |

Rho stores credentials in the native OS credential store through an OS-agnostic abstraction. If no OS credential store is available, login fails closed with setup guidance. Rho does not add a plaintext or encrypted file fallback.

Successful login normally stores credentials only. It does not switch the active provider/model, because provider switching is model-driven through `/model`. If Rho started without usable auth and is running on an unauthenticated placeholder, a successful login selects that provider's default model so the session becomes usable.

## Logout

Use `/logout` to delete stored credentials from the device:

```text
/logout
/logout openai
/logout openai-codex
/logout anthropic
/logout github-copilot
/logout xai
```

`/logout` opens a provider picker containing only providers with stored credentials that can be deleted. Direct args support provider names only. If an environment override is still present, the provider remains available after deleting the stored credential.

## Environment overrides

Environment variables are CI/development escape hatches and override stored credentials:

```bash
OPENAI_API_KEY=...
ANTHROPIC_API_KEY=...
CODEX_ACCESS_TOKEN=...
CODEX_ACCOUNT_ID=... # optional for Codex
GITHUB_COPILOT_TOKEN=...
XAI_ACCESS_TOKEN=...
```

`GITHUB_COPILOT_TOKEN` is treated as a GitHub Copilot API bearer token. It is not refreshed or stored by Rho. Stored `/login github-copilot` credentials can be exchanged for short-lived Copilot API tokens and refreshed once after an unauthorized response.

For normal interactive setup, prefer `/login`.

## Providers and model catalog

Rho's implemented providers are:

| Provider | Auth mode | Use case |
| --- | --- | --- |
| `openai` | `api-key` | OpenAI API-key models |
| `openai-codex` | `codex` | Codex OAuth models |
| `anthropic` | `anthropic-api-key` | Anthropic API-key models |
| `github-copilot` | `github-copilot` | GitHub Copilot models |
| `xai` | `xai-oauth` | xAI models available to a SuperGrok or X Premium+ subscription |

OpenAI, Anthropic, GitHub Copilot, and xAI can refresh provider model lists with `/refresh-model-list [provider]`. xAI refreshes from the authenticated account's language-model catalog, including aliases, so newly available text models appear without a Rho release. `grok-4.5` remains available as the built-in fallback before the first refresh.

GitHub Copilot support uses GitHub Copilot endpoints, not GitHub Models endpoints.

Use `/model provider/model` to switch explicitly, including to another provider:

```text
/model openai/gpt-5.5
/model openai-codex/gpt-5.5
/model anthropic/claude-sonnet-4-5
/model github-copilot/gpt-4.1
/model xai/grok-4.5
```

A bare model id works when it uniquely matches the catalog for the active selection rules. Uncataloged bare model ids stay on the current provider as an escape hatch for newly released models.

Rho uses cached model metadata and built-in overrides to choose effective context windows for status display and [auto compaction](/configuration#auto-compaction). The same metadata supplies each model's available [reasoning effort levels](/configuration#reasoning-options), allowing the TUI to skip unsupported choices without model-name allowlists. Pricing-sensitive models such as `openai/gpt-5.5` and `openai-codex/gpt-5.5` use safer effective windows below their advertised maximums.

For persistent defaults, see [configuration](/configuration). For one-shot prompts, see [automation and CLI](/automation-cli).
