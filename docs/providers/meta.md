# Meta Model API

Rho supports [Meta Model API](https://developer.meta.com/ai/products/meta-model-api/) through its OpenAI-compatible Chat Completions API.

## Provider details

| Setting | Value |
| --- | --- |
| Provider | `meta` |
| Auth | `meta-api-key`, `meta-muse` |
| Environment override | `MODEL_API_KEY` for `meta-api-key`, `META_API_KEY` for `meta-muse` |
| API base | `https://api.meta.ai/v1` |
| Model source | Meta Model API `/models` |

Create an API key in the [Model API dashboard](https://dev.meta.ai/). Meta documents the key as `MODEL_API_KEY`. Rho stores it in the configured credential store after login and sends it as a Bearer token.

## Interactive login

In the TUI, run:

```text
/login meta-api-key
```

`/login meta` asks which method to use. Rho asks for your Model API key, stores it, and refreshes the models available to your account. Select one with `/model`, for example:

```text
/model meta/muse-spark-1.2
```

Remove the stored key with:

```text
/logout meta-api-key
```

## Muse subscription

A Muse Code subscription is a separate login from a pay-as-you-go key. Rho opens a device-code sign-in, stores the session, and mints a short-lived Model API key for requests. That key is replaced before it expires. When the session itself expires, sign in again.

In the TUI, run:

```text
/login meta-muse
```

`/login meta` asks you to choose the API key or the subscription. Remove the stored session with:

```text
/logout meta-muse
```

`META_API_KEY` overrides a stored subscription session for `meta-muse`. `MODEL_API_KEY` remains the override for `meta-api-key`.

```bash
rho --provider meta \
  --auth meta-muse \
  --model meta/muse-spark-1.2 \
  run "review this project"
```

Subscription usage bills to the Muse plan. Rho still estimates an equivalent API cost. `/info` marks that estimate as a subscription.

## Environment and automation

For CI or development, set `MODEL_API_KEY`. It overrides a stored key.

```bash
export MODEL_API_KEY="<api-key>"
rho --provider meta \
  --auth meta-api-key \
  --model meta/muse-spark-1.2 \
  run "review this project"
```

Do not put the key in `config.toml` or commit it to source control.

## Models and reasoning

Use `/config` and choose **Refresh model lists** to fetch the current models for your account. When the cache is empty, or when the cache includes it, Rho defaults to `muse-spark-1.2`. If only older Muse Spark builds are present, the first cached model is used instead.

Rho reads Muse Spark reasoning efforts from models.dev under the `meta` catalog (`minimal`, `low`, `medium`, `high`, `xhigh`). The models do not advertise a full off control. For persisted config and defaults, Rho maps an out-of-set level to the nearest advertised effort and sends Chat Completions `reasoning_effort`. An explicit unsupported choice is rejected instead of silently rewritten. When catalog metadata is still unknown, Rho omits the wire field and the API uses its default depth.

Rho uses the Chat Completions surface (`/v1/chat/completions`). Meta also exposes Responses and Anthropic Messages endpoints; those are not used by this provider path.

See Meta's [quickstart](https://ai.developer.meta.com/docs/quickstart/) and [models](https://ai.developer.meta.com/docs/models/) docs for regions, pricing, and the live model list.
