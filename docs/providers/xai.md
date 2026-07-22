# xAI

xAI supports API-key authentication and OAuth for models available to a SuperGrok or X Premium+ subscription. For shared concepts such as credential storage and model selection, see [authentication and models](/authentication-and-models).

## At a glance

### API key

| Setting | Value |
| --- | --- |
| Provider | `xai` |
| Auth | `xai-api-key` |
| Environment override | `XAI_API_KEY` |
| API base | `https://api.x.ai/v1` |
| Model list | Static allowlist maintained by Rho |

### OAuth

| Setting | Value |
| --- | --- |
| Provider | `xai-oauth` |
| Auth | `xai-oauth` |
| Environment override | `XAI_ACCESS_TOKEN` |
| API base | `https://api.x.ai/v1` |
| Model list | Static allowlist maintained by Rho |

## Sign in

Run `/login`, select **xAI**, then choose **API Key** or **OAuth**. You can also target either method directly:

```text
/login xai
/login xai-oauth
```

`/login xai` opens a masked API-key entry box. `/login xai-oauth` opens Rho's browser-based xAI OAuth flow, or automatically uses xAI's device-code flow in SSH and headless environments. You can also request the OAuth device-code flow explicitly:

```bash
rho login xai-oauth --device-auth
```

Credentials are stored in the configured credential store, not in config or transcripts.

## Sign out

Delete the stored credential for one method at a time:

```text
/logout xai
/logout xai-oauth
```

If the corresponding environment override is still present, that method stays available.

## Environment overrides

```bash
XAI_API_KEY=...
XAI_ACCESS_TOKEN=...
```

`XAI_API_KEY` selects API-key authentication. `XAI_ACCESS_TOKEN` is the OAuth CI/development override. Environment variables override stored credentials for their respective methods. For normal interactive setup, prefer `/login`.

## Models

xAI uses a static allowlist rather than a refreshable API list: `grok-4.5`, `grok-build-0.1`, `grok-composer-2.5-fast`, and `grok-4.3`. Select the provider that matches the authentication method:

```text
/model xai/grok-4.5
/model xai-oauth/grok-4.5
```

For a non-interactive run, pass the matching provider, auth mode, and model. These flags also update the persistent default:

```bash
rho --provider xai --auth xai-api-key --model grok-4.5 run "hello"
rho --provider xai-oauth --auth xai-oauth --model grok-4.5 run "hello"
```

Provide the matching environment override or log in once so Rho can read the stored credential.

## Notes

- With OAuth, the statusline estimates an equivalent API cost from [models.dev](https://models.dev/) pricing (including long-context rate tiers when available) and labels it `(sub)`.
- [`/limits`](/interactive-tui#commands) reports the usage windows for xAI OAuth when you are logged in.
