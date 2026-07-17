# Anthropic

Anthropic uses API-key auth. For shared concepts such as credential storage and model selection, see [authentication and models](/authentication-and-models).

## At a glance

| Setting | Value |
| --- | --- |
| Provider | `anthropic` |
| Auth | `anthropic-api-key` |
| Environment override | `ANTHROPIC_API_KEY` |
| API base | `https://api.anthropic.com/v1` |
| Model list | Refreshable after authentication |

## Sign in

```text
/login anthropic
```

`/login anthropic` opens a masked API-key entry box in the [interactive TUI](/interactive-tui). Credentials are stored in the native OS credential store, not in config or transcripts.

## Sign out

```text
/logout anthropic
```

`/logout anthropic` deletes the stored Anthropic API key. If an environment override is still present, the provider stays available.

## Environment override

```bash
ANTHROPIC_API_KEY=...
```

Environment variables are CI/development escape hatches and override stored credentials. For normal interactive setup, prefer `/login`.

## Models

Anthropic can refresh its provider model list with `/refresh-model-list anthropic`. Switch to an Anthropic model with:

```text
/model anthropic/claude-sonnet-4-5
```

For a non-interactive run, pass the matching provider, auth mode, and model. These flags also update the persistent default:

```bash
rho --provider anthropic --auth anthropic-api-key --model claude-sonnet-4-5 run "hello"
```

Provide `ANTHROPIC_API_KEY` in the automation environment or log in once through the TUI so Rho can read the stored key.
