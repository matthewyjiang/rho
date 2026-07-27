# Ollama Cloud

Rho can use [Ollama Cloud](https://docs.ollama.com/cloud) through its OpenAI-compatible API. This path talks to `https://ollama.com` directly with an API key. It does not need a local Ollama server.

For local models, or cloud models routed through a signed-in local Ollama host, see [Ollama](/providers/ollama).

## At a glance

| Setting | Value |
| --- | --- |
| Provider | `ollama-cloud` |
| Auth | `ollama-cloud-api-key` |
| Environment override | `OLLAMA_API_KEY` |
| Default API base | `https://ollama.com/v1` |
| Model list | Hosted catalog from `/v1/models` |

## Setup

1. Create an API key at [ollama.com/settings/keys](https://ollama.com/settings/keys).
2. In Rho, run:

```text
/login ollama-cloud
```

Paste the key when prompted. Rho stores it in the configured credential store and refreshes the hosted model list.

3. Select a model:

```text
/model ollama-cloud/<hosted-model>
```

for example `/model ollama-cloud/kimi-k2.6` or `/model ollama-cloud/kimi-k2.7-code`.

Remove the stored key with:

```text
/logout ollama-cloud
```

## Environment and automation

For CI or development, set `OLLAMA_API_KEY`. It overrides a stored key:

```bash
export OLLAMA_API_KEY="<api-key>"
rho --provider ollama-cloud \
  --auth ollama-cloud-api-key \
  --model ollama-cloud/kimi-k2.6 \
  run "review this project"
```

Rho sends the key as a Bearer token. Do not put the key in `config.toml` or commit it to source control.

## Models and compatibility

Use `/config` and choose **Refresh model lists** to fetch the current Ollama Cloud catalog. Hosted model ids can change as Ollama adds or retires cloud models; refresh before selecting a newly published model.

Ollama Cloud uses the same OpenAI-compatible chat path as local Ollama. Prefer models with reliable tool-call support for coding work. Rho does not claim that every hosted model supports tools, images, reasoning controls, parallel calls, or usage data.

If Ollama omits optional usage data, Rho still handles the response.
