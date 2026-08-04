# Qwen Token Plan

Rho supports [QwenCloud Token Plan](https://docs.qwencloud.com/token-plan/overview) through its OpenAI-compatible Chat Completions API.

## Provider details

| Setting | Value |
| --- | --- |
| Provider | `qwen-token-plan` |
| Auth | `qwen-token-plan-api-key` |
| Environment override | `QWEN_TOKEN_PLAN_API_KEY` |
| API base | `https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1` |
| Model source | Token Plan `/models` API |

Create a Token Plan dedicated API key on the [API Keys page](https://home.qwencloud.com/api-keys). Token Plan keys are not interchangeable with Coding Plan or pay-as-you-go keys. Model access depends on your plan, so Rho fetches the model list after login instead of keeping a static list.

## Interactive login

In the TUI, run:

```text
/login qwen-token-plan
```

Rho asks for your Token Plan API key, stores it in the configured credential store, and refreshes the models available to your account. Select one with `/model`, for example:

```text
/model qwen-token-plan/qwen3.7-plus
```

Remove the stored key with:

```text
/logout qwen-token-plan
```

## Environment and automation

For CI or development, set `QWEN_TOKEN_PLAN_API_KEY`. It overrides a stored key.

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

When thinking models stream `reasoning_content` alongside tool calls, Rho retains that field as exact-model provider context and replays it on later assistant messages in the same tool loop. Without replay, multi-step tool use can degrade into empty assistant responses after a couple of tool rounds. Rho orchestration always uses the streaming path for this; non-stream `send_turn` / complete responses cannot carry provider context.

See [Token Plan overview](https://docs.qwencloud.com/token-plan/overview) and the [Token Plan FAQ](https://docs.qwencloud.com/token-plan/faq) for quota, region, and key rules.

> Note: QwenCloud **Coding Plan** is a separate product (different keys, base URL, and model allowlist). Rho's `qwen-token-plan` provider targets Token Plan endpoints only.
