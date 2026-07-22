# OpenAI (Codex OAuth)

Codex OAuth uses Rho-owned OAuth and signs in with an OpenAI account subscription rather than an API key. For shared concepts such as credential storage and model selection, see [authentication and models](/authentication-and-models).

## At a glance

| Setting | Value |
| --- | --- |
| Provider | `openai-codex` |
| Auth | `codex` |
| Environment override | `CODEX_ACCESS_TOKEN` |
| API base | `https://chatgpt.com/backend-api/codex` |
| Model list | Static allowlist maintained by Rho |

## Sign in

```text
/login openai-codex
```

`/login openai-codex` starts Rho's browser-based Codex OAuth flow. For remote or headless sessions where a browser is not available, use the device-code flow:

```bash
rho login openai-codex --device-auth
```

Credentials are stored in the configured credential store, not in config or transcripts.

### Device-code authorization

Device-code access is managed by OpenAI. See [Codex authentication](https://learn.chatgpt.com/docs/auth) for current setup and troubleshooting guidance. If device-code login is unavailable for the account or managed workspace, use the regular browser callback instead:

```bash
rho login openai-codex
```

## Sign out

```text
/logout openai-codex
```

`/logout openai-codex` deletes stored Codex tokens. If an environment override is still present, the provider stays available.

## Environment override

```bash
CODEX_ACCESS_TOKEN=...
CODEX_ACCOUNT_ID=... # optional for Codex
```

Environment variables are CI/development escape hatches and override stored credentials. For normal interactive setup, prefer `/login`.

## Models

Codex OAuth uses this static model allowlist rather than a refreshable API list:

- `gpt-5.6-sol`
- `gpt-5.6-terra`
- `gpt-5.6-luna`
- `gpt-5.5`
- `gpt-5.4`
- `gpt-5.4-mini`
- `gpt-5.3-codex-spark`

Switch to a Codex model with:

```text
/model openai-codex/gpt-5.6-sol
```

For a non-interactive run, pass the matching provider, auth mode, and model. These flags also update the persistent default:

```bash
rho --provider openai-codex --auth codex --model gpt-5.6-sol run "hello"
```

Run `rho login openai-codex` first or provide `CODEX_ACCESS_TOKEN` in the automation environment.

## Notes

- As a subscription auth mode, the statusline estimates an equivalent API cost from [models.dev](https://models.dev/) pricing (including long-context rate tiers when available) and labels it `(sub)`.
- [`/limits`](/interactive-tui#commands) reports the usage windows for Codex OAuth when you are logged in.
- Pricing-sensitive models such as `openai-codex/gpt-5.6-sol` use safer effective context windows below their advertised maximums to avoid long-context pricing thresholds.
