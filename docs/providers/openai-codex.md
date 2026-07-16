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

### Device-code authorization

For remote or headless sessions, enable Codex device authorization before entering the code:

1. Open ChatGPT and go to **Settings → Security and login**.
2. Enable **Device code authorization for Codex**.
3. Return to the terminal and run `rho login openai-codex --device-auth` again.
4. Open `https://auth.openai.com/codex/device`.
5. Enter the fresh device code printed by Rho, then approve the authorization.

Always rerun the login command after enabling the setting. A code issued before the setting changed
may have expired or may no longer be accepted.

If **Device code authorization for Codex** is unavailable for the account or managed workspace, use
the regular browser callback instead:

```bash
rho login openai-codex
```

### macOS Keychain

On macOS, Rho stores Codex OAuth credentials in the login Keychain. A Keychain dialog asking for
access to the item named `rho` expects the Mac login password, not an OpenAI, GitHub, API-key, or
passkey credential.

- Choose **Allow** to approve one Keychain operation.
- Choose **Always Allow** only when you trust the installed Rho executable and want to avoid a
  second prompt when Rho reads or updates the credential.
- Choose **Deny** when the request is unexpected or the executable is not trusted.

If Rho reports `provider failed: credential store operation failed`, quit any existing Rho
sessions, unlock the login Keychain, and repeat `rho login openai-codex`. Complete the browser flow
before retrying the provider. A failed device-code login does not test the provider or any proposed
shell command because execution stops while Rho is acquiring credentials.

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
