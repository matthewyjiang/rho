# Moonshot and Kimi Code

Rho supports Moonshot's OpenAI-compatible Chat Completions API through two explicit provider selections. Neither selection uses the Anthropic Messages API.

| Provider | Auth | API base |
| --- | --- | --- |
| `moonshot` | `moonshot-api-key` | `https://api.moonshot.ai/v1` |
| `kimi-code` | `kimi-oauth` | `https://api.kimi.com/coding/v1` |

Both providers send turns to `/chat/completions`. Their model lists can be updated with `/refresh-model-list moonshot` and `/refresh-model-list kimi-code` after authentication.

## Moonshot API key

In the interactive TUI, run:

```text
/login moonshot
/model moonshot/<model-id>
```

Rho stores the key in the OS credential store. For CI and development, `MOONSHOT_API_KEY` overrides the stored key. Remove the stored key with `/logout moonshot`.

For automation, select the matching provider and auth mode:

```sh
rho --provider moonshot --auth moonshot-api-key --model <model-id>
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

For automation after a prior login or with `KIMI_ACCESS_TOKEN`:

```sh
rho --provider kimi-code --auth kimi-oauth --model <model-id>
```

Use `/limits` in the TUI to inspect the weekly and rolling usage windows reported by Kimi Code. Remove stored OAuth tokens with `/logout kimi-code`. An active `KIMI_ACCESS_TOKEN` override remains available after logout.
