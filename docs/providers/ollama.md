# Ollama

Rho can use models served by Ollama through its OpenAI-compatible API. Ollama runs locally by default. An API key is optional.

For direct hosted access with an API key or device key and no local server, see [Ollama Cloud](/providers/ollama-cloud). If a local Ollama install is signed in with `ollama signin`, cloud models pulled as `*-cloud` still work through this local provider.

## At a glance

| Setting | Value |
| --- | --- |
| Provider | `ollama` |
| Auth | `none`, or optional `ollama-api-key` |
| Default API base | `http://127.0.0.1:11434/v1` |
| Model list | Models installed in Ollama, refreshed from `/v1/models` |
| Environment override | `RHO_OLLAMA_API_KEY` (optional key) |

## Setup

Install [Ollama](https://ollama.com/), start its server, and install a model that supports tool calls. For example:

```sh
ollama serve
ollama pull <tool-capable-model>
```

In Rho, run `/login ollama`. The first field is the API base, prefilled with the local default. Keep it or replace it with another host. The second field is an API key; leave it blank for a keyless local server.

```text
/login ollama
```

Then open `/config` and choose **Refresh model lists**, or select a model directly:

```text
/model ollama/<tool-capable-model>
```

First-run setup does not write `[providers.ollama]`. The endpoint is stored only after this login, or when you add the table by hand.

## Use another server

`/login ollama` is the usual way to set a custom API base. You can also edit `~/.rho/config.toml`:

```toml
[providers.ollama]
base_url = "http://192.168.1.20:11434/v1"
```

Keep the `/v1` suffix. Rho appends `/models` for discovery and `/chat/completions` for agent turns. The setting applies only to Ollama and is also used by `/doctor` when it checks the server. The URL must use `http` or `https` and cannot contain credentials, a query, or a fragment.

A stored key is sent as a Bearer token. Leave the key blank, or omit it, when the host does not require one.

## Model compatibility

Ollama's OpenAI-compatible endpoint can serve many model types, but not every model supports the tool calls needed by a coding agent. Choose a model with reliable tool-call support. Rho does not download models or claim that every installed model supports tools, images, reasoning controls, parallel calls, or usage data.

## Reasoning

Ollama accepts `reasoning_effort` values `low`, `medium`, `high`, `max`, and `none`. Rho sends the selected level when the model capability profile permits it. Shift+Tab and `/config` cycle it. When the model is missing from models.dev, Rho still sends the field so you can turn thinking off, mapping other Rho levels to the nearest accepted value. Rho omits the field for models whose metadata reports reasoning as not configurable. Omitting the field lets Ollama enable thinking on its own.

If Ollama omits optional usage data, Rho still handles the response. Run `/doctor` to distinguish a reachable server with installed models, a reachable server with no models, an unreachable server, and an invalid or unsuccessful response.
