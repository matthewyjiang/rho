# Ollama

Rho can use models served by Ollama through its OpenAI-compatible API. Ollama runs locally by default and needs no API key.

For direct hosted access with an API key or device key and no local server, see [Ollama Cloud](/providers/ollama-cloud). If a local Ollama install is signed in with `ollama signin`, cloud models pulled as `*-cloud` still work through this local provider.

## At a glance

| Setting | Value |
| --- | --- |
| Provider | `ollama` |
| Auth | None |
| Default API base | `http://127.0.0.1:11434/v1` |
| Model list | Installed models from native `/api/tags` (falls back to `/v1/models`) |

## Setup

Install [Ollama](https://ollama.com/), start its server, and install a model that supports tool calls. For example:

```sh
ollama serve
ollama pull <tool-capable-model>
```

In Rho, open `/config` and choose **Refresh model lists**. The model picker then shows the models returned by Ollama. Startup also refreshes that list when the cache is stale. You can also select one directly:

```text
/model ollama/<tool-capable-model>
```

Do not run `/login` for Ollama. Rho sends no `Authorization` header and does not read or write an Ollama credential.

## Use another server

Set a provider-specific API base in `~/.rho/config.toml`:

```toml
[providers.ollama]
base_url = "http://192.168.1.20:11434/v1"
```

Keep the `/v1` suffix. Rho derives the native API root by removing that segment (`http://host:11434/v1` → `http://host:11434/api/tags` and `/api/show`) and still posts agent turns to `/v1/chat/completions`. Bases that do not end in `/v1` skip native discovery and keep the plain `/v1/models` list. `/doctor` still probes `/v1/models`. The URL must use `http` or `https` and cannot contain credentials, a query, or a fragment. Bearer tokens and custom headers for secured remote endpoints are not supported.

## Model metadata

Context window and thinking capability come from the local server. Complete `/api/tags` rows are enough; Rho calls `/api/show` only when tags omit context length or capabilities, reading `model_info.<arch>.context_length` when present. That value is the model's advertised maximum, not the server's current `num_ctx`. Models that advertise only `embedding` stay out of the picker.

## Model compatibility

Ollama's OpenAI-compatible endpoint can serve many model types, but not every model supports the tool calls needed by a coding agent. Choose a model with reliable tool-call support. Rho does not download models or claim that every installed model supports tools, images, reasoning controls, parallel calls, or usage data.

## Reasoning

Ollama accepts `reasoning_effort` values `low`, `medium`, `high`, `max`, and `none`. Rho sends the selected level when the model advertises a `thinking` capability. Shift+Tab and `/config` cycle it. When the model is missing from models.dev and the server does not advertise capabilities, Rho still sends the field so you can turn thinking off, mapping other Rho levels to the nearest accepted value. Rho omits the field for models whose metadata reports reasoning as not configurable. Omitting the field lets Ollama enable thinking on its own.

If Ollama omits optional usage data, Rho still handles the response. Run `/doctor` to distinguish a reachable server with installed models, a reachable server with no models, an unreachable server, and an invalid or unsuccessful response.
