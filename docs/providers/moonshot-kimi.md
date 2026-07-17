# Moonshot and Kimi Code

Rho supports Moonshot's OpenAI-compatible Chat Completions API through two explicit provider selections. Neither selection uses the Anthropic Messages API.

| Provider | Auth | Environment override | API base | Model list |
| --- | --- | --- | --- | --- |
| `moonshot` | `moonshot-api-key` | `MOONSHOT_API_KEY` | `https://api.moonshot.ai/v1` | Refreshable after authentication |
| `kimi-code` | `kimi-oauth` | `KIMI_ACCESS_TOKEN` | `https://api.kimi.com/coding/v1` | Refreshable after authentication |

Both providers send turns to `/chat/completions` and fetch models from `/models`. Their model lists can be updated with `/refresh-model-list moonshot` and `/refresh-model-list kimi-code` after authentication. Rho preserves account-specific context limits returned by the model-list API and combines them with general model capabilities from models.dev.

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
/refresh-model-list kimi-code
/model kimi-code/<model-id>
```

For Kimi K3, Rho uses the authenticated `context_length` returned for the current account. If that field is unavailable, Rho uses a conservative 256K effective window while retaining the model's advertised 1M maximum. K3 reasoning choices follow models.dev capabilities and currently expose thinking off or max; Rho sends these through Kimi's native `thinking.type` field.

For automation after a prior login or with `KIMI_ACCESS_TOKEN`:

```sh
rho --provider kimi-code --auth kimi-oauth --model <model-id> run "hello"
```

Use `/limits` in the TUI to inspect the weekly and rolling usage windows reported by Kimi Code. Remove stored OAuth tokens with `/logout kimi-code`. An active `KIMI_ACCESS_TOKEN` override remains available after logout.
