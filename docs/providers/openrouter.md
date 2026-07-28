# OpenRouter

Rho supports OpenRouter through its OpenAI-compatible Chat Completions API. OpenRouter is one provider with two auth modes.

## At a glance

| Method | Provider | Auth | Login |
| --- | --- | --- | --- |
| API key | `openrouter` | `openrouter-api-key` | `/login openrouter-api-key` |
| OAuth | `openrouter` | `openrouter-oauth` | `/login openrouter-oauth` |

Both methods use:

- Environment override: `OPENROUTER_API_KEY`
- API base: `https://openrouter.ai/api/v1`
- A model list that Rho can refresh after login

## Login and model selection

Run `/login` and select **OpenRouter**, then choose **API Key** or **OAuth**. `/login openrouter` opens the same method picker. You can also target either method at once:

```text
/login openrouter-api-key
/login openrouter-oauth
```

API-key login opens a masked key entry box. OAuth opens OpenRouter in your browser. Rho uses S256 PKCE, listens on an unused localhost port for the redirect, exchanges the code for a user-controlled OpenRouter API key, and saves that key in the configured credential store. The callback listener closes when login ends.

OpenRouter does not offer a device-code flow. Browser login therefore needs a browser that can reach the localhost callback. On a remote or headless host, use API-key login or set `OPENROUTER_API_KEY`.

The OAuth key and a manually entered key have separate credential-store entries. Run bare `/logout` to choose a stored mode, or use `/logout openrouter-api-key` and `/logout openrouter-oauth` to target one. For CI and development, `OPENROUTER_API_KEY` overrides either stored key.

OpenRouter model IDs often contain a slash. Both auth modes use the same provider model reference:

```text
/model openrouter/anthropic/claude-sonnet-4
```

Legacy `openrouter-oauth/...` model references still load and normalize to `openrouter/...`.

Rho fetches the model list from OpenRouter's `/models` endpoint after login. Choose **Refresh model lists** in `/config` when models change. Rho sends turns to `/chat/completions`.

## Automation

You can complete browser OAuth from the command line:

```sh
rho login openrouter-oauth
```

Do not pass `--device-auth`, since OpenRouter does not support device login. Then select the provider, OAuth mode, and model:

```sh
rho --provider openrouter --auth openrouter-oauth --model anthropic/claude-sonnet-4 run "hello"
```

For API-key automation, use:

```sh
rho --provider openrouter --auth openrouter-api-key --model anthropic/claude-sonnet-4 run "hello"
```

The retired `--provider openrouter-oauth` value remains a compatibility alias and normalizes to `provider = "openrouter"` with `auth = "openrouter-oauth"`.
