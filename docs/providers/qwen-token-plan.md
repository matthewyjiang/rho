# Qwen Token Plan

Rho supports [QwenCloud Token Plan](https://docs.qwencloud.com/token-plan/overview) through its OpenAI-compatible Chat Completions API.

## Provider details

| Setting | Value |
| --- | --- |
| Provider | `qwen-token-plan` |
| Auth | `qwen-token-plan-api-key` |
| Environment override | `QWEN_TOKEN_PLAN_API_KEY` |
| Default API base | `https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1` |
| Model source | Token Plan `/models` API |

Create a Token Plan dedicated API key on the [API Keys page](https://home.qwencloud.com/api-keys). Token Plan keys are not interchangeable with Coding Plan or pay-as-you-go keys. Model access depends on your plan, so Rho fetches the model list after login instead of keeping a static list.

## Interactive login

In the TUI, run:

```text
/login qwen-token-plan
```

Rho asks for:

1. Your Token Plan API key
2. The OpenAI-compatible endpoint you were given

Paste the endpoint exactly as shown in the QwenCloud console or docs. The common international value is:

```text
https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1
```

Keep the `/compatible-mode/v1` path. Rho appends `/models` for discovery and `/chat/completions` for agent turns. The URL must use `http` or `https` and cannot contain credentials, a query, or a fragment.

Rho stores the key in the configured credential store and the endpoint under `[providers.qwen-token-plan].base_url` in `config.toml`. It then refreshes the models available to your account. Select one with `/model`, for example:

```text
/model qwen-token-plan/qwen3.7-plus
```

Remove the stored key with:

```text
/logout qwen-token-plan
```

Logout does not clear the saved endpoint. Edit or remove `[providers.qwen-token-plan].base_url` in config if you need a different base URL without logging in again.

## Environment and automation

For CI or development, set `QWEN_TOKEN_PLAN_API_KEY`. It overrides a stored key. Set the endpoint in config:

```toml
[providers.qwen-token-plan]
base_url = "https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1"
```

```bash
export QWEN_TOKEN_PLAN_API_KEY="<api-key>"
rho --provider qwen-token-plan \
  --auth qwen-token-plan-api-key \
  --model qwen-token-plan/qwen3.7-plus \
  run "review this project"
```

Rho sends the key as a Bearer token. Do not put the key in `config.toml` or commit it to source control.

## Models and reasoning

Use `/config` and choose **Refresh model lists** to fetch the current models for your Token Plan account. Rho maps models.dev reasoning metadata for the `alibaba-token-plan` catalog:

- `qwen3.8-max` and `qwen3.8-max-preview`: `low`, `medium`, and `xhigh` (API default is `xhigh` when the field is omitted)
- Models that only advertise a thinking toggle may not expose a full effort cycle until models.dev lists exact effort values

See [Token Plan overview](https://docs.qwencloud.com/token-plan/overview) and the [Token Plan FAQ](https://docs.qwencloud.com/token-plan/faq) for quota, region, and key rules.
