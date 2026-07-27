# Ollama Cloud

Rho can use [Ollama Cloud](https://docs.ollama.com/cloud) through its OpenAI-compatible API at `https://ollama.com`. This path does not need a local Ollama server for inference.

For local models, or cloud models routed through a signed-in local Ollama host, see [Ollama](/providers/ollama).

## At a glance

| Setting | API key | Device key |
| --- | --- | --- |
| Provider | `ollama-cloud` | `ollama-cloud-device` |
| Auth | `ollama-cloud-api-key` | `ollama-cloud-device` |
| Login | `/login ollama-cloud` | `/login ollama-cloud-device` |
| Environment override | `OLLAMA_API_KEY` | — |
| Default API base | `https://ollama.com/v1` | `https://ollama.com/v1` |
| Model list | Hosted catalog from `/v1/models` | Hosted catalog from `/v1/models` |

## Setup with an API key

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

## Setup with a device key

Ollama's local install keeps an Ed25519 device key at `~/.ollama/id_ed25519`. After that public key is registered with your ollama.com account, Rho can sign Cloud API requests the same way the Ollama CLI does.

1. Prefer the Rho login flow:

```text
/login ollama-cloud-device
```

Rho opens the Ollama connect page (or prints the URL with `--device-auth` / headless login), waits until the device is approved, and stores a short session marker.

2. Or reuse an existing Ollama sign-in. If you already ran `ollama signin` and `~/.ollama/id_ed25519` is registered, Rho can use that key without creating an API key. Run `/login ollama-cloud-device` once so Rho records the session, or select the provider after the key is present.

3. Select a model:

```text
/model ollama-cloud-device/<hosted-model>
```

`/logout ollama-cloud-device` clears Rho's session marker. It does not delete `~/.ollama/id_ed25519` or disconnect the key from ollama.com.

## Environment and automation

For CI or development, set `OLLAMA_API_KEY`. It overrides a stored API key:

```bash
export OLLAMA_API_KEY="<api-key>"
rho --provider ollama-cloud \
  --auth ollama-cloud-api-key \
  --model ollama-cloud/kimi-k2.6 \
  run "review this project"
```

API keys are sent as Bearer tokens. Device-key auth signs each request with the local key and a `ts` query parameter; do not put either credential in `config.toml` or commit it to source control.

Device-key login from a remote shell:

```bash
rho login ollama-cloud-device --device-auth
```

## Models and compatibility

Use `/config` and choose **Refresh model lists** to fetch the current Ollama Cloud catalog. Hosted model ids can change as Ollama adds or retires cloud models; refresh before selecting a newly published model.

Ollama Cloud uses the same OpenAI-compatible chat path as local Ollama. Prefer models with reliable tool-call support for coding work. Rho does not claim that every hosted model supports tools, images, reasoning controls, parallel calls, or usage data.

If Ollama omits optional usage data, Rho still handles the response.
