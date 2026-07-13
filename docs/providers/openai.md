# OpenAI

OpenAI uses API-key auth (`provider = "openai"`, `auth = "api-key"`). For shared concepts such as credential storage and model selection, see [authentication and models](/authentication-and-models).

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

OpenAI can refresh its provider model list with `/refresh-model-list openai`. Switch to an OpenAI model with:

```text
/model openai/gpt-5.5
```

Or from the CLI, which also updates the persistent default:

```bash
rho --provider openai --auth api-key --model gpt-5.5
```

## Notes

- OpenAI API-key requests use the Chat Completions API and do not currently send a [reasoning](/configuration#reasoning-options) configuration.
- Pricing-sensitive models such as `openai/gpt-5.5` use safer effective context windows below their advertised maximums to avoid long-context pricing thresholds.
