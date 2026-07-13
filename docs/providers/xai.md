# xAI

xAI uses OAuth (`provider = "xai"`, `auth = "xai-oauth"`) for models available to a SuperGrok or X Premium+ subscription. For shared concepts such as credential storage and model selection, see [authentication and models](/authentication-and-models).

## Sign in

```text
/login xai
```

`/login xai` opens Rho's browser-based xAI OAuth flow, or automatically uses xAI's device-code flow in SSH and headless environments. You can also request the device-code flow explicitly:

```bash
rho login xai --device-auth
```

Credentials are stored in the native OS credential store, not in config or transcripts.

## Sign out

```text
/logout xai
```

`/logout xai` deletes stored xAI tokens. If an environment override is still present, the provider stays available.

## Environment override

```bash
XAI_ACCESS_TOKEN=...
```

Environment variables are CI/development escape hatches and override stored credentials. For normal interactive setup, prefer `/login`.

## Models

xAI OAuth uses a static allowlist rather than a refreshable API list: `grok-4.5`, `grok-build-0.1`, `grok-composer-2.5-fast`, and `grok-4.3`. Switch to an xAI model with:

```text
/model xai/grok-4.5
```

Or from the CLI, which also updates the persistent default:

```bash
rho --provider xai --auth xai-oauth --model grok-4.5
```

## Notes

- As a subscription auth mode, the statusline estimates an equivalent API cost from [models.dev](https://models.dev/) pricing (including long-context rate tiers when available) and labels it `(sub)`.
- [`/limits`](/interactive-tui#commands) reports the usage windows for xAI OAuth when you are logged in.
