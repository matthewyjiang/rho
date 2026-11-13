# Mistral AI

Rho supports [Mistral AI](https://mistral.ai/) La Plateforme as a first-party provider. Rho talks to Mistral's Chat Completions API at `https://api.mistral.ai/v1`.

## Provider details

| Setting | Value |
| --- | --- |
| Provider | `mistral` |
| Auth | `mistral-api-key` |
| Environment override | `MISTRAL_API_KEY` |
| API base | `https://api.mistral.ai/v1` |
| Model source | Mistral `/models` |
| Metadata | [models.dev `mistral`](https://models.dev/providers/mistral) |
| Default model | `mistral-medium-latest` |

Create an API key in the [Mistral console](https://console.mistral.ai/api-keys), then store it with Rho. Do not put the key in `config.toml`.

## Interactive login

In the TUI, run:

```text
/login mistral
```

Rho stores the key and refreshes the model list. Select a model with `/model`, for example:

```text
/model mistral/mistral-medium-latest
```

Remove the stored key with:

```text
/logout mistral
```

## Environment and automation

For CI or development, set `MISTRAL_API_KEY`. It overrides a stored key.

```bash
export MISTRAL_API_KEY="<api-key>"
rho --provider mistral \
  --auth mistral-api-key \
  --model mistral/mistral-medium-latest \
  run "review this project"
```

## Models and reasoning

Use `/config` and choose **Refresh model lists** to fetch the current catalog from `https://api.mistral.ai/v1/models`. Rho lists only models that Mistral marks as supporting chat and function calling, so embedding, OCR, and moderation models stay out of the picker.

Mistral lists most models under several ids, such as `mistral-medium-latest`, `mistral-medium-2604`, and `mistral-medium-3.5`. The picker shows each model once, under the id that models.dev describes. Any of its other ids still works as `/model mistral/<id>` and in `config.toml`, and Rho sends that id to Mistral unchanged, so a `-latest` alias keeps following Mistral's latest release.

Cost, context, and reasoning controls come from models.dev under `mistral`. Mistral reasoning levels depend on the model, and Rho sends only the `reasoning_effort` values that models.dev lists for it. For example, Mistral Small and Medium accept `none` and `high`, and Mistral Large 4 accepts `low`, `medium`, and `high`. For models that do not reason, Rho sends no reasoning control.

Mistral returns reasoning as `thinking` content chunks. Rho shows them as reasoning and sends them back to the same model on later turns, as Mistral's [reasoning guide](https://docs.mistral.ai/inference/conversations/reasoning) recommends.
