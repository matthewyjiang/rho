# Authentication and models

Rho supports OpenAI API-key auth, Rho-owned Codex OAuth auth, Anthropic API-key auth, and GitHub Copilot auth. Provider, model, and auth mode are stored in [configuration](/configuration). Secrets are not stored in config.

## Interactive login

Use `/login` in the interactive TUI:

```text
/login
/login openai
/login openai-codex
/login anthropic
/login github-copilot
```

`/login` opens a provider picker. Direct args support provider names only:

| Command | Auth flow |
| --- | --- |
| `/login openai` | masked OpenAI API-key entry |
| `/login openai-codex` | browser-based Codex OAuth owned by Rho |
| `/login anthropic` | masked Anthropic API-key entry |
| `rho login openai-codex --device-auth` | Codex device-code login for remote/headless sessions |
| `/login github-copilot` | GitHub device code login for GitHub Copilot |

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
```

`/logout` opens the same provider picker style as `/login`. Direct args support provider names only. If an environment override is still present, the provider remains available after deleting the stored credential.

## Environment overrides

Environment variables are CI/development escape hatches and override stored credentials:

```bash
OPENAI_API_KEY=...
ANTHROPIC_API_KEY=...
CODEX_ACCESS_TOKEN=...
CODEX_ACCOUNT_ID=... # optional for Codex
GITHUB_COPILOT_TOKEN=...
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

OpenAI, Anthropic, and GitHub Copilot can refresh provider model lists with `/refresh-model-list [provider]`. GitHub Copilot falls back to a conservative static allowlist when a dynamic model cache is unavailable or refresh fails, so `/model github-copilot/<model>` and the picker can work before the first refresh.

GitHub Copilot support uses GitHub Copilot endpoints, not GitHub Models endpoints.

Use `/model provider/model` to switch explicitly, including to another provider:

```text
/model openai/gpt-5.5
/model openai-codex/gpt-5.5
/model anthropic/claude-sonnet-4-5
/model github-copilot/gpt-4.1
```

A bare model id works when it uniquely matches the catalog for the active selection rules. Uncataloged bare model ids stay on the current provider as an escape hatch for newly released models.

Rho uses cached model metadata and built-in overrides to choose effective context windows for status display and [auto compaction](/configuration#auto-compaction). Pricing-sensitive models such as `openai/gpt-5.5` and `openai-codex/gpt-5.5` use safer effective windows below their advertised maximums.

For persistent defaults, see [configuration](/configuration). For one-shot prompts, see [automation and CLI](/automation-cli).
