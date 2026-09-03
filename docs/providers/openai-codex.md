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

`/login openai-codex` starts Rho's Codex OAuth flow and always shows the authorize URL. On a machine with a browser, Rho opens it and still prints the link. Remote or headless sessions skip the browser and use device-code automatically (`rho login openai-codex`). `--device-auth` forces device-code on a graphical session.

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

- `gpt-6-astra` (default; reasoning effort `low` through `max`)
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

Supported GPT-5.4, GPT-5.5, GPT-5.6, and GPT-6 Astra Codex models can use OpenAI's faster priority tier at a higher credit rate. In the TUI, use `/fast on` or `/fast off`. Running `/fast` with no argument toggles the mode. Rho saves the choice as `model.fast_mode`, shows `(fast)` after the active model name, and sends `service_tier: "priority"` on later supported Codex turns.

## Notes

- On `gpt-6-astra`, `/reasoning` changes are sent as `configuration_update` items so the prompt cache prefix is preserved.
- As a subscription auth mode, the statusline estimates an equivalent API cost from [models.dev](https://models.dev/) pricing (including long-context rate tiers when available) and labels it `(sub)`.
- [`/limits`](/interactive-tui#commands) reports the usage windows for Codex OAuth when you are logged in.
- Context windows come from cached model metadata. Set `usable_context_window` in `~/.rho/models.toml` to raise or cap a model. See [local model metadata](/configuration#local-model-metadata).

## Mid-turn steering (`gpt-6-astra`)

On `gpt-6-astra` over the Codex websocket, steering entered during a model turn is forwarded as `response.steer`. The original response ends `incomplete` with reason `steered` (or completes if it finished first). The server then continues automatically with the steer prepended; Rho reuses that continuation instead of sending another `response.create`. Already-streamed text is not rewritten. If the original turn ended waiting for a client tool result, Rho replays a full next request so the server does not prepend an orphaned steer.

Steering is queued in the TUI as today. When the backend accepts it mid-turn, the pending-input row shows `delivered` until the steer is applied at the turn boundary. Disconnecting the websocket drops any unacked steer; Rho then applies it locally on the next step.
