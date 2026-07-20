# Google Gemini

Rho uses Google's native Gemini `generateContent` API. It supports streamed text, thought summaries, images, function calls and responses, thought-signature replay, and provider-reported token use.

| Setting | Value |
| --- | --- |
| Provider | `google` |
| Auth mode | `google-api-key` |
| Environment override | `GEMINI_API_KEY` |
| API | `https://generativelanguage.googleapis.com/v1beta` |

## Login

Create a Gemini API key in [Google AI Studio](https://aistudio.google.com/app/apikey), then run:

```text
/login google
```

Rho stores the key in the native OS credential store. For CI or local development, set `GEMINI_API_KEY` instead. The environment value takes priority over the stored key.

Google AI Studio offers a free tier for selected models. Google may use free-tier request content to improve its products, so use synthetic prompts and do not send private source code or secrets when testing on that tier.

## Logout

Run `/logout google` to remove the Google API key from the OS credential store. If `GEMINI_API_KEY` is set, that environment override remains active after logout.

## Models

Open `/config` and choose **Refresh model lists**. Rho reads Google's Models API, keeps text chat models that support `generateContent`, and caches their input and output token limits. Then select a model, for example:

```text
/model google/gemini-3.1-flash-lite
```

Google may add, rename, or retire models, so refresh the list when a model is missing. Refresh probes each candidate with a tiny `generateContent` call and hides models that Google reports as permanently unavailable for the current API key, such as retired Gemini 2.5 ids that still appear in the Models API. Temporary failures such as rate limits or high demand keep the model visible.

## Reasoning and tools

Rho maps its reasoning levels to Gemini 3 thinking levels and Gemini 2.5 thinking budgets. It rejects levels a model cannot honor rather than silently selecting a higher level. Thought summaries stay separate from raw reasoning, and Gemini thought signatures remain opaque. Rho replays those signatures only to the exact same provider, API, and model.

Tool declarations use Gemini function declarations. Tool calls and function responses retain their call IDs when Google supplies them. Rho creates a unique local ID when a response omits one, while leaving the ID absent on later Google requests.
