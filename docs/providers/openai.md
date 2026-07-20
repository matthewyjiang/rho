# OpenAI

OpenAI uses API-key auth. For shared concepts such as credential storage and model selection, see [authentication and models](/authentication-and-models).

## At a glance

| Setting | Value |
| --- | --- |
| Provider | `openai` |
| Auth | `api-key` |
| Environment override | `OPENAI_API_KEY` |
| API base | `https://api.openai.com/v1` |
| Model list | Refreshable after authentication |

## Sign in

```text
/login openai
```

`/login openai` opens a masked API-key entry box in the [interactive TUI](/interactive-tui). Credentials are stored in the native OS credential store, not in config or transcripts.

## Sign out

```text
/logout openai
```

`/logout openai` deletes the stored OpenAI API key. If an environment override is still present, the provider stays available.

## Environment override

```bash
OPENAI_API_KEY=...
```

Environment variables are CI/development escape hatches and override stored credentials. For normal interactive setup, prefer `/login`.

## Models

OpenAI can refresh its provider model list through **Refresh model lists** in `/config`. Switch to an OpenAI model with:

```text
/model openai/gpt-5.6-sol
```

For a non-interactive run, pass the matching provider, auth mode, and model. These flags also update the persistent default:

```bash
rho --provider openai --auth api-key --model gpt-5.6-sol run "hello"
```

Provide `OPENAI_API_KEY` in the automation environment or log in once through the TUI so Rho can read the stored key.

## Notes

- OpenAI API-key requests use the Chat Completions API and do not currently send a [reasoning](/configuration#reasoning-options) configuration.
- Pricing-sensitive models such as `openai/gpt-5.6-sol` use safer effective context windows below their advertised maximums to avoid long-context pricing thresholds.
