# Meta Model API

Rho supports [Meta Model API](https://developer.meta.com/ai/products/meta-model-api/) through its OpenAI-compatible Chat Completions API.

## Provider details

| Setting | Value |
| --- | --- |
| Provider | `meta` |
| Auth | `meta-api-key` |
| Environment override | `MODEL_API_KEY` |
| API base | `https://api.meta.ai/v1` |
| Model source | Meta Model API `/models` |

Create an API key in the [Model API dashboard](https://dev.meta.ai/). Meta documents the key as `MODEL_API_KEY`. Rho stores it in the configured credential store after login and sends it as a Bearer token.

## Interactive login

In the TUI, run:

```text
/login meta
```

Rho asks for your Model API key, stores it, and refreshes the models available to your account. Select one with `/model`, for example:

```text
/model meta/muse-spark-1.2
```

Remove the stored key with:

```text
/logout meta
```

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
