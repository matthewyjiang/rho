# Providers

This index lists every first-party provider Rho ships. Shared concepts such as credential stores, model selection, first-run setup, and environment overrides live in [authentication and models](/authentication-and-models). Each provider page covers login, logout, auth modes, and model notes for that backend.

| Provider | Auth modes | Page |
| --- | --- | --- |
| `openai` | `api-key` | [OpenAI](/providers/openai) |
| `openai-codex` | `codex` | [OpenAI (Codex OAuth)](/providers/openai-codex) |
| `anthropic` | `anthropic-api-key` | [Anthropic](/providers/anthropic) |
| `google` | `google-api-key` | [Google Gemini](/providers/google-gemini) |
| `github-copilot` | `github-copilot` | [GitHub Copilot](/providers/github-copilot) |
| `xai` | `xai-api-key`, `xai-oauth` | [xAI](/providers/xai) |
| `poolside` | `poolside-api-key` | [Poolside](/providers/poolside) |
| `openrouter` | `openrouter-api-key`, `openrouter-oauth` | [OpenRouter](/providers/openrouter) |
| `ollama` | `none`, optional `ollama-api-key` | [Ollama](/providers/ollama) |
| `ollama-cloud` | `ollama-cloud-api-key`, `ollama-cloud-device` | [Ollama Cloud](/providers/ollama-cloud) |
| `moonshot` | `moonshot-api-key` | [Moonshot and Kimi Code](/providers/moonshot-kimi) |
| `kimi-code` | `kimi-oauth` | [Moonshot and Kimi Code](/providers/moonshot-kimi) |
| `qwen-token-plan` | `qwen-token-plan-api-key` | [Qwen Token Plan](/providers/qwen-token-plan) |
| `meta` | `meta-api-key` | [Meta Model API](/providers/meta) |
| `opencode-go` | `opencode-go-api-key` | [OpenCode Go](/providers/opencode-go) |

User-defined OpenAI-compatible hosts can be created from `/login` (**Custom**, then **Chat Completions** or **Responses**) or by adding `[providers.custom.<name>]` with a `base_url`. They default to Chat Completions; set `api = "responses"` in config, or choose **Responses** in `/login`, to use the Responses API. An API key is optional. See [Custom OpenAI-compatible hosts](/providers/openai-compatible).

## Recommended next steps

1. Read [authentication and models](/authentication-and-models) for credential storage and model selection.
2. Open the provider page that matches your account.
3. Run `rho` and finish setup with `/login` and `/model`, or pass provider flags to [automation](/automation-cli).
