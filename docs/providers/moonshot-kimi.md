# Moonshot and Kimi Code

Rho supports Moonshot's OpenAI-compatible Chat Completions API through two explicit provider selections. Neither selection uses the Anthropic Messages API.

## At a glance

### Moonshot

| Setting | Value |
| --- | --- |
| Provider | `moonshot` |
| Auth | `moonshot-api-key` |
| Environment override | `MOONSHOT_API_KEY` |
| API base | `https://api.moonshot.ai/v1` |
| Model list | Refreshable after authentication |

### Kimi Code

| Setting | Value |
| --- | --- |
| Provider | `kimi-code` |
| Auth | `kimi-oauth` |
| Environment override | `KIMI_ACCESS_TOKEN` |
| API base | `https://api.kimi.com/coding/v1` |
| Model list | Refreshable after authentication |

Both providers send turns to `/chat/completions` and fetch models from `/models`. Their model lists can be updated through **Refresh model lists** in `/config` after authentication. Rho preserves account-specific context limits returned by the model-list API. For Kimi Code, account-specific reasoning choices replace general models.dev reasoning metadata when available.

Although Moonshot and Kimi Code share an OpenAI-compatible transport, they use separate reasoning contracts:

- Moonshot Kimi K3 requests use the top-level `reasoning_effort` field.
- Kimi Code K3 requests use `thinking: { "type": "enabled", "effort": "<level>" }`. Turning reasoning off sends `thinking: { "type": "disabled" }` without an effort.

Rho does not send Anthropic-style adaptive thinking or `output_config.effort` to either provider.

## Moonshot API key

In the interactive TUI, run:

```text
/login moonshot
/model moonshot/<model-id>
```

Rho stores the key in the OS credential store. For CI and development, `MOONSHOT_API_KEY` overrides the stored key. Remove the stored key with `/logout moonshot`.

For automation, select the matching provider and auth mode:

```sh
rho --provider moonshot --auth moonshot-api-key --model <model-id> run "hello"
```

## Kimi Code OAuth

Kimi Code uses the RFC 8628 device authorization flow implemented by [MoonshotAI/kimi-code](https://github.com/MoonshotAI/kimi-code). Run either of these commands:

```text
/login kimi-code
```

```sh
rho login kimi-code
```

Visit the displayed Kimi authorization URL and enter the displayed code if requested. Rho stores the access and refresh tokens in the OS credential store, refreshes expiring tokens, and persists replacements. `KIMI_ACCESS_TOKEN` is a non-persistent CI/development override and cannot be refreshed.

Select a model after login:

```text
/model kimi-code/<model-id>
```

For Kimi K3, Rho uses the authenticated `context_length` returned for the current account. If that field is unavailable, Rho uses a conservative 256K effective window while retaining the model's advertised 1M maximum. K3 reasoning choices come from the authenticated model list when available. Enabled requests carry the selected effort in Kimi Code's native `thinking` object. Turning reasoning off sends the native disabled mode, which Kimi documents as routing the request to K2.6.

For automation after a prior login or with `KIMI_ACCESS_TOKEN`:

```sh
rho --provider kimi-code --auth kimi-oauth --model <model-id> run "hello"
```

Use `/limits` in the TUI to inspect the weekly and rolling usage windows reported by Kimi Code. Remove stored OAuth tokens with `/logout kimi-code`. An active `KIMI_ACCESS_TOKEN` override remains available after logout.
