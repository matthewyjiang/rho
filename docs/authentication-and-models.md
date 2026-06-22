# Authentication and models

Rho supports OpenAI API-key auth and Rho-owned Codex OAuth auth. Provider, model, and auth mode are stored in [configuration](/configuration). Secrets are not stored in config.

## Interactive login

Use `/login` in the interactive TUI:

```text
/login
/login openai
/login openai-codex
```

`/login` opens a provider picker. Direct args support provider names only:

| Command | Auth flow |
| --- | --- |
| `/login openai` | masked OpenAI API-key entry |
| `/login openai-codex` | browser-based Codex OAuth owned by Rho |

Rho stores credentials in the native OS credential store through an OS-agnostic abstraction. If no OS credential store is available, login fails closed with setup guidance. Rho does not add a plaintext or encrypted file fallback.

Successful login normally stores credentials only. It does not switch the active provider/model, because provider switching is model-driven through `/model`. If Rho started without usable auth and is running on an unauthenticated placeholder, a successful login selects that provider's default model so the session becomes usable.

## Logout

Use `/logout` to delete stored credentials from the device:

```text
/logout
/logout openai
/logout openai-codex
```

`/logout` opens the same provider picker style as `/login`. Direct args support provider names only. If an environment override is still present, the provider remains available after deleting the stored credential.

## Environment overrides

Environment variables are CI/development escape hatches and override stored credentials:

```bash
OPENAI_API_KEY=...
CODEX_ACCESS_TOKEN=...
CODEX_ACCOUNT_ID=... # optional for Codex
```

For normal interactive setup, prefer `/login`.

## Providers and model catalog

Rho's implemented providers are:

| Provider | Auth mode | Use case |
| --- | --- | --- |
| `openai` | `api-key` | OpenAI API-key models |
| `openai-codex` | `codex` | Codex OAuth models |

Rho uses a built-in static model catalog instead of querying providers for model listings. The `/model` picker shows models for providers that currently have credentials available through `/login` or env overrides. After logging into another provider, that provider's models appear in the picker. After logout, they disappear unless an env override still supplies auth.

Use `/model provider/model` to switch explicitly, including to another provider:

```text
/model openai/gpt-5.5
/model openai-codex/gpt-5.5
```

A bare model id works when it uniquely matches the catalog for the active selection rules. Uncataloged bare model ids stay on the current provider as an escape hatch for newly released models.

For persistent defaults, see [configuration](/configuration). For one-shot prompts, see [automation and CLI](/automation-cli).
