# OpenAI (Codex OAuth)

Codex OAuth uses Rho-owned OAuth (`provider = "openai-codex"`, `auth = "codex"`). It signs in with an OpenAI account subscription rather than an API key. For shared concepts such as credential storage and model selection, see [authentication and models](/authentication-and-models).

## Sign in

```text
/login openai-codex
```

`/login openai-codex` starts Rho's browser-based Codex OAuth flow. For remote or headless sessions where a browser is not available, use the device-code flow:

```bash
rho login openai-codex --device-auth
```

Credentials are stored in the native OS credential store, not in config or transcripts.

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

Codex OAuth uses a static model allowlist rather than a refreshable API list. Switch to a Codex model with:

```text
/model openai-codex/gpt-5.6-sol
```

Or from the CLI, which also updates the persistent default:

```bash
rho --provider openai-codex --auth codex --model gpt-5.6-sol
```

## Notes

- As a subscription auth mode, the statusline estimates an equivalent API cost from [models.dev](https://models.dev/) pricing (including long-context rate tiers when available) and labels it `(sub)`.
- [`/limits`](/interactive-tui#commands) reports the usage windows for Codex OAuth when you are logged in.
- Pricing-sensitive models such as `openai-codex/gpt-5.6-sol` use safer effective context windows below their advertised maximums to avoid long-context pricing thresholds.
