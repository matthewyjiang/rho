# xAI

xAI is one provider with API-key and OAuth auth modes. OAuth works with models available to a SuperGrok or X Premium+ subscription. For shared concepts such as credential storage and model selection, see [authentication and models](/authentication-and-models).

## At a glance

| Method | Provider | Auth | Environment override |
| --- | --- | --- | --- |
| API key | `xai` | `xai-api-key` | `XAI_API_KEY` |
| OAuth | `xai` | `xai-oauth` | `XAI_ACCESS_TOKEN` |

Both modes use `https://api.x.ai/v1` and the static model allowlist maintained by Rho.

## Sign in

Run `/login`, select **xAI**, then choose **API Key** or **OAuth**. `/login xai` opens the same method picker. You can also target either method directly:

```text
/login xai-api-key
/login xai-oauth
```

API-key login opens a masked key entry box. `/login xai-oauth` opens Rho's browser-based xAI OAuth flow, or automatically uses xAI's device-code flow in SSH and headless environments. You can also request the OAuth device-code flow explicitly:

```bash
rho login xai-oauth --device-auth
```

Credentials are stored in the configured credential store, not in config or transcripts.

## Sign out

Delete the stored credential for one method at a time:

```text
/logout xai-api-key
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

xAI uses a static allowlist rather than a refreshable API list: `grok-4.6`, `grok-4.5`, `grok-build-0.1`, `grok-composer-2.5-fast`, and `grok-4.3`. Both auth modes use the same provider model reference:

```text
/model xai/grok-4.6
```

For a non-interactive run, pass the provider, matching auth mode, and model. These flags also update the persistent default:

```bash
rho --provider xai --auth xai-api-key --model grok-4.6 run "hello"
rho --provider xai --auth xai-oauth --model grok-4.6 run "hello"
```

The retired `xai-oauth` provider value remains a compatibility alias. Config, CLI flags, favorites, and model references normalize it to `provider = "xai"` with `auth = "xai-oauth"`.

Provide the matching environment override or log in once so Rho can read the stored credential.

## Notes

- With OAuth, the statusline estimates an equivalent API cost from [models.dev](https://models.dev/) pricing (including long-context rate tiers when available) and labels it `(sub)`.
- [`/limits`](/interactive-tui#commands) reports the usage windows for xAI OAuth when you are logged in.
- Both auth modes attach xAI's hosted `x_search` tool on every Responses create turn. Hosted X Search is a provider amenity outside the agent tool allowlist: it remains available even when client tools are restricted or empty, and disappears as soon as the session switches away from xAI. It is independent of the client `web_search` tool. Activity streams as typed `HostedToolActivity { name: "x_search", detail }` run events.
- Both auth modes use xAI [server-side context compaction](https://docs.x.ai/developers/advanced-api-usage/context-compaction) (`POST /v1/responses/compact`) when automatic or manual compaction runs. The compact request body is only `model` plus full `input` (system messages included). The response is a single encrypted compaction item that replaces the prior window; host-owned system prompts are still retained client-side for portable handoff. The encrypted item only replays on a compatible xAI Responses turn for the same provider identity and model.
