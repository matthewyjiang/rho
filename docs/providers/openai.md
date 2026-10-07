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

`/login openai` opens a masked API-key entry box in the [interactive TUI](/interactive-tui). Credentials are stored in the configured credential store, not in config or transcripts.

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

## Decision models

OpenAI also serves decision models over the [Decisions API](https://developers.openai.com/api/docs/guides/decisions) (`POST /v1/decisions`), which answer typed questions with a probability for each option. Rho can use `gpt-6-luna` there as the [permission screen](/configuration/permissions#decision-models). Pick it under **Decision models** in the screen-model picker, or set it in config:

```toml
[internal_agents.permission-classifier-screen]
provider = "openai"
model = "gpt-6-luna"
auth = "api-key"
kind = "decision"
```

`kind = "decision"` is required: an OpenAI entry without it is asked as a text model. The Decisions API bills input tokens only.

## Notes

- OpenAI API-key requests use the Chat Completions API and do not currently send a [reasoning](/configuration#reasoning-options) configuration.
- Context windows come from cached model metadata. Set `usable_context_window` in `~/.rho/models.toml` to raise or cap a model. See [local model metadata](/configuration#local-model-metadata).
