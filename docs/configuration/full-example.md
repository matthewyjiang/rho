# Configuration file example

Parent: [Configuration](/configuration).

```toml
[model]
provider = "openai"
model = "gpt-5.6-sol"
auth = "api-key" # or "none", "codex", "anthropic-api-key", "google-api-key", "github-copilot", "xai-api-key", "xai-oauth", "moonshot-api-key", "ollama-api-key", "ollama-cloud-api-key", "ollama-cloud-device", "poolside-api-key", "openrouter-api-key", "openrouter-oauth", "kimi-oauth", "qwen-token-plan-api-key", "meta-api-key", "minimax-api-key", or "opencode-go-api-key"
reasoning = "medium" # off, minimal, low, medium, high, xhigh, or max
fast_mode = false # priority service for supported Codex models; uses credits at a higher rate
favorite_models = []

[model.aliases]
# deep = "anthropic/claude-opus-4-8"
# fast = "gpt-5.6-luna"

[display]
show_reasoning_output = true
zen_mode = false
theme = "terminal" # terminal, a built-in id from /theme, or a custom ~/.rho/themes/<id>.json stem
max_tool_output_lines = 10
prompt_history_limit = 1000
cache_miss_notices = false

[output]
max_output_bytes = 64000

[compaction]
auto_compact = false
compact_threshold_percent = 85
compact_target_percent = 50

[internal_agents.session-title]
# provider = "openai"
# model = "gpt-5.6-sol"
# auth = "api-key"

[internal_agents.goal-judge]
# provider = "openai"
# model = "gpt-5.6-sol"
# auth = "api-key"

[internal_agents.advisor]
# provider = "anthropic"
# model = "claude-fable-5"
# auth = "anthropic-api-key"
# reasoning = "high"

[internal_agents.permission-classifier]
# provider = "openai"
# model = "gpt-5.6-luna"
# auth = "api-key"
# reasoning = "low"

[web_search]
hosted = true # provider-hosted search when the chat path supports it
provider = "auto" # backup only: auto, openai, exa, brave, or disabled

# MCP stays inert while this table has no enabled server entries.
# See /integrations/mcp for stdio and Streamable HTTP examples.
[mcp.servers]

[providers.ollama]
base_url = "http://127.0.0.1:11434/v1"

[providers.custom.composer]
base_url = "http://127.0.0.1:8787/v1"
# catalog = "llmgateway" # optional models.dev slug for context, price, and reasoning
# catalog_mode = "model-id" # look up unsplit slug/model ids in models.dev

[behavior]
advisor_mode = false
check_for_updates = true
enable_subagents = true
experimental_workspace_rewind = false
edit_tool = "auto" # auto, hashline, apply_patch, or str_replace
permission_mode = "bypass" # bypass, auto, allow_edits, plan, or supervised
rtk = true
inline_shell = "bash" # bash default on macOS/Linux; powershell on Windows
# credential_store = "os" # or "file"; omit until first /login chooses

[prompt_templates]
review = "Review this code for correctness, security, and maintainability."
"explain-tests" = "Explain how these tests cover the expected behavior."

[keybindings]
reset_conversation = "ctrl+r"
open_editor = "ctrl+g"
jump_to_bottom = "ctrl+end"
toggle_tool_output = "ctrl+o"
insert_newline = "ctrl+j"
paste_image = "ctrl+v"
edit_pending_input = "alt+up"
manage_pending_input = "alt+q"
cycle_pinned_model = "ctrl+p"
cycle_pinned_model_back = "ctrl+shift+p"
```

Settings are grouped by purpose so the file is easier to scan and edit by hand. Rho still reads the previous flat format and rewrites it into groups the next time it saves config.

Keybindings use `+`-separated modifiers and keys. Supported modifiers are `ctrl`, `alt`, and `shift`; supported named keys include `enter`, `esc`, `tab`, arrow keys, `home`, `end`, `pageup`, `pagedown`, `backspace`, and `delete`. Single-character keys can be used directly. Keybinding changes take effect when Rho starts.

The full saved file can also include model overrides for reserved internal agents. Each entry under `[internal_agents]` selects the provider, model, and auth used by that role. An internal agent with no entry follows the active conversation selection. `[providers.ollama].base_url` and `[providers.custom.<name>].base_url` set OpenAI-compatible endpoints used for those hosts' chat, model refresh, and health checks. First-run setup does not write `[providers.ollama]`; `/login ollama` stores the API base and an optional key. `[providers.custom.<name>].catalog` optionally borrows a models.dev provider for context, price, and reasoning. Rho still reads the old `[title]` and flat `title_provider`, `title_model`, and `title_auth` settings, then migrates them to `[internal_agents.session-title]` when it next saves config. Web search API keys are normally stored in the configured credential store rather than config.

Ollama's provider-specific API base uses its own section and does not affect other providers. It appears after `/login ollama` or a hand edit:

```toml
[providers.ollama]
base_url = "http://127.0.0.1:11434/v1"
```

Custom OpenAI-compatible hosts use a name you choose. They speak Chat Completions unless you pick **Custom · Responses** in `/login`, or set `api = "responses"`. `catalog` is optional and borrows a [models.dev](https://models.dev/) provider for context, price, and reasoning. `catalog_mode = "model-id"` looks up the unsplit `slug/model` id instead and cannot be combined with `catalog`:

```toml
[providers.custom.composer]
base_url = "http://127.0.0.1:8787/v1"

[providers.custom.cliproxyapi]
base_url = "http://127.0.0.1:8317/v1"
catalog = "llmgateway"

[providers.custom.litellm]
base_url = "http://127.0.0.1:4000/v1"
api = "responses"
```

See [Ollama](/providers/ollama) and [Custom OpenAI-compatible hosts](/providers/openai-compatible) for setup, model refresh, and endpoint limits.
