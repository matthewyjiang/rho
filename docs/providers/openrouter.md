# OpenRouter

Rho supports OpenRouter through its OpenAI-compatible Chat Completions API. You can use an API key or sign in through OpenRouter OAuth.

## At a glance

| Method | Provider | Auth | Login |
| --- | --- | --- | --- |
| API key | `openrouter` | `openrouter-api-key` | `/login openrouter` |
| OAuth | `openrouter-oauth` | `openrouter-oauth` | `/login openrouter-oauth` |

Both methods use:

- Environment override: `OPENROUTER_API_KEY`
- API base: `https://openrouter.ai/api/v1`
- A model list that Rho can refresh after login

## Login and model selection

Run `/login` and select **OpenRouter**, then choose **API Key** or **OAuth**. You can also start either method at once:

```text
/login openrouter
/login openrouter-oauth
```

API-key login opens a masked key entry box. OAuth opens OpenRouter in your browser. Rho uses S256 PKCE, listens on an unused localhost port for the redirect, exchanges the code for a user-controlled OpenRouter API key, and saves that key in the OS credential store. The callback listener closes when login ends.

OpenRouter does not offer a device-code flow. Browser login therefore needs a browser that can reach the localhost callback. On a remote or headless host, use API-key login or set `OPENROUTER_API_KEY`.

The OAuth key and a manually entered key have separate credential-store entries. `/logout openrouter` removes the manual key, while `/logout openrouter-oauth` removes the OAuth key. For CI and development, `OPENROUTER_API_KEY` overrides either stored key.

OpenRouter model IDs often contain a slash. Select a model under the provider that matches your login method:

```text
/model openrouter/anthropic/claude-sonnet-4
/model openrouter-oauth/anthropic/claude-sonnet-4
```

Rho fetches the model list from OpenRouter's `/models` endpoint after login. Choose **Refresh model lists** in `/config` when models change. Rho sends turns to `/chat/completions`.

## Automation

You can complete browser OAuth from the command line:

```sh
rho login openrouter-oauth
```

Do not pass `--device-auth`, since OpenRouter does not support device login. Then select the OAuth provider, auth mode, and model:

```sh
rho --provider openrouter-oauth --auth openrouter-oauth --model anthropic/claude-sonnet-4 run "hello"
```

For API-key automation, keep using:

```sh
rho --provider openrouter --auth openrouter-api-key --model anthropic/claude-sonnet-4 run "hello"
```
