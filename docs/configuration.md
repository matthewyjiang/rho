# Configuration

Rho stores persistent config at `~/.rho/config.toml` by default.

```toml
[model]
provider = "openai"
model = "gpt-5.6-sol"
auth = "api-key" # or "codex", "anthropic-api-key", "github-copilot", "xai-api-key", "xai-oauth", "moonshot-api-key", "openrouter-api-key", or "kimi-oauth"
reasoning = "medium" # off, minimal, low, medium, high, xhigh, or max
favorite_models = []

[model.aliases]
# deep = "anthropic/claude-opus-4-8"
# fast = "gpt-5.6-luna"

[display]
show_reasoning_output = true
max_tool_output_lines = 10

[output]
max_output_bytes = 12000

[compaction]
auto_compact = false
compact_threshold_percent = 85
compact_target_percent = 50

[title]
# provider = "openai"
# model = "gpt-5.6-sol"
# auth = "api-key"

[web_search]
provider = "auto" # auto, openai, exa, brave, or disabled

[behavior]
check_for_updates = true
enable_subagents = true
permission_mode = "auto" # auto, plan, or supervised
rtk = true

[prompt_templates]
review = "Review this code for correctness, security, and maintainability."
"explain-tests" = "Explain how these tests cover the expected behavior."

[keybindings]
reset_conversation = "ctrl+r"
jump_to_bottom = "ctrl+g"
toggle_tool_output = "ctrl+o"
insert_newline = "ctrl+j"
paste_image = "ctrl+v"
edit_pending_input = "alt+up"
manage_pending_input = "alt+q"
```

Settings are grouped by purpose so the file is easier to scan and edit by hand. Rho still reads the previous flat format and rewrites it into groups the next time it saves config.

Keybindings use `+`-separated modifiers and keys. Supported modifiers are `ctrl`, `alt`, and `shift`; supported named keys include `enter`, `esc`, `tab`, arrow keys, `home`, `end`, `pageup`, `pagedown`, `backspace`, and `delete`. Single-character keys can be used directly. Keybinding changes take effect when Rho starts.

The full saved file can also include optional title-generation and web-search settings. `provider`, `model`, and `auth` under `[title]` override the model used to generate session titles; when they are omitted, Rho uses the active provider/model/auth selection. Web search API keys are normally stored in the OS credential store rather than config.

## Prompt templates

The easiest way to add a reusable prompt is to create a Markdown or text file. The filename becomes the slash command and the file contents become its prompt:

- `~/.rho/prompts/review.md` makes `/prompt:review` available everywhere.
- `.rho/prompts/review.md` makes `/prompt:review` available in that project and its subdirectories.
- A project file overrides a global file with the same name.

For example, `~/.rho/prompts/review.md` could contain:

```text
Review this code for correctness, security, and maintainability.
```

Templates can also be defined inline in `config.toml` when a separate file would be unnecessary:

```toml
[prompt_templates]
review = "Review this code for correctness, security, and maintainability."
```

Inline config templates override files with the same name. Typing `/prompt:review src/config.rs` expands to `Review this code for correctness, security, and maintainability. src/config.rs`. Press `tab` in the command palette to expand without sending, or press `enter` to expand and send. Template names may contain letters, numbers, `-`, and `_`, and cannot duplicate built-in command names. Restart Rho after adding or editing templates.

## CLI overrides

Passing `--provider`, `--model`, `--auth`, or `--reasoning` updates the config file and makes the choice the future default.

```bash
rho --provider openai --auth api-key --model gpt-5.6-sol
rho --reasoning high
```

These values select [authentication and models](/authentication-and-models). For the exact `--provider`/`--auth`/`--model` combination each provider expects, see its [provider page](/authentication-and-models#providers).

You can load and save a specific config file with:

```bash
rho --config ~/.rho/config.toml
```

`--no-system-prompt`, `--no-tools`, and `--no-subagents` are only available on the command line and apply only to the current run. `--no-subagents` has the same tool and prompt behavior as setting `enable_subagents = false`.

## Model aliases

`[model.aliases]` defines short names for concrete models so a pinned model id lives in one place instead of being repeated across config and agent definitions. An alias value is either `provider/model` or a bare model id, which keeps whichever provider is otherwise selected. Model ids may contain `/`, as OpenRouter ids commonly do:

```toml
[model.aliases]
deep = "anthropic/claude-opus-4-8"
fast = "gpt-5.6-luna"
openrouter-deep = "openrouter/anthropic/claude-sonnet-4"
```

Reference an alias with an `@` prefix. The explicit prefix distinguishes aliases from concrete model ids and makes a missing or misspelled alias an immediate configuration error:

```toml
[model]
model = "@deep"

[title]
model = "@fast"
```

The same syntax works with `rho --model @deep` and with `model: @deep` in [agent definition frontmatter](/subagents). Updating a model is then a one-line change to the alias table rather than an edit per file.

Rho resolves aliases to concrete ids before any model-specific behavior, holds no opinion about which model a name should map to, and never rewrites your mapping. A concrete model id is always interpreted literally, even when an alias has the same name. The `/config` picker shows the active mapping, and saving config preserves the `@deep` reference rather than its expansion while the selected concrete model still matches. Alias values must be concrete models and therefore cannot begin with `@`. Every provider-qualified alias is validated when configuration loads, including aliases that are not currently selected.

## Title model

Rho can use a separate model for generating session titles. The optional `title_provider`, `title_model`, and `title_auth` settings persist that selection. Title generation does not need the same flagship model as the conversation, so using a cheaper model such as `gpt-5.6-luna` can reduce title-generation cost while keeping the main session on `gpt-5.6-sol`. Choose **Session title model** in `/config` to select from available catalog and cached models.

If no title model settings are present, Rho falls back to the active provider, model, and auth.

## Web search

`provider` under `[web_search]` controls the built-in [web search tool](/tools-workspace#built-in-tools). Supported values are `auto`, `openai`, `exa`, `brave`, and `disabled`. Unknown values are normalized back to `auto` when config is loaded.

Legacy flat `web_search_openai_api_key`, `web_search_exa_api_key`, and `web_search_brave_api_key` values are migrated to the OS credential store when loaded. Empty strings are ignored. Set `provider = "disabled"` under `[web_search]` to remove the web search tool from the tool registry while keeping other workspace tools enabled.

`enable_subagents` controls whether the `agent` and `agents` tools are available. It defaults to `true`. Set it to `false` to remove both tools and instruct the model not to attempt to use subagents. Restart Rho after changing this setting.

## Permission modes

`permission_mode` must be `auto`, `plan`, or `supervised`. Missing values default to `auto`; an unrecognized value is a configuration error. The setting controls whether Rho allows, denies, or asks before security-sensitive tool capabilities:

- `auto` is the default and preserves unrestricted tool behavior.
- `plan` allows investigation but denies file writes and process execution.
- `supervised` asks for confirmation before file writes and process execution. Reads, network access, skills, and instruction discovery do not prompt.

Change the mode from the **Permission mode** row in `/config`. An interactive mode change applies before the next turn and preserves the current session ID and history, but clears every remembered **Allow for session** approval. In a supervised approval prompt, choose **Allow once**, **Allow for session**, or **Deny**. A session approval remembers only the exact structured capability request for the current session. Pressing Escape denies the request and cancels the current run; choosing **Deny** with Enter rejects only that operation so the run can continue.

Non-interactive `rho run` sessions cannot display approval prompts. Supervised operations that require approval therefore fail closed instead of being approved automatically.

Permission modes are application policy checks, not an operating-system sandbox. Rho and its tools still run with the current user's permissions, and tools must correctly declare and authorize their capabilities for the policy to cover them. In restricted modes, capability classes that this Rho version does not recognize fail closed: Plan denies them and Supervised requires approval.

## TUI updates

In the [interactive TUI](/interactive-tui), [`/config`](/interactive-tui#commands) opens a picker for configuration values. Its model and provider rows change the conversation and title models, refresh cached model lists, and manage provider credentials. The `permission_mode` row applies the selected policy before the next turn. The `reasoning` row cycles through `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, and `max`, saves immediately, and applies to the current session. The `show_reasoning_output` row toggles whether reasoning text is shown in the TUI and applies on the next model call. The `check_for_updates` row toggles startup update checks against GitHub releases. The `enable_subagents` row controls whether agent tools are available in the next session. The auto compaction rows toggle compaction and edit its threshold and target percentages. The `max_output_bytes` row opens a numeric input and saves for the next session.

[`/login`](/interactive-tui#commands), [`/logout`](/interactive-tui#commands), and [`/model`](/interactive-tui#commands) remain direct shortcuts for provider credentials and conversation-model selection. The corresponding `/config` rows provide the same picker flows, while **Session title model** saves optional title-generation model settings. Model pickers show entries from Rho's [model catalog](/authentication-and-models#selecting-models) and cached dynamic provider model lists for providers with available auth, and `/model provider/model` can switch explicitly. See the [provider pages](/authentication-and-models#providers) for per-provider auth and model details.

## Reasoning options

`reasoning` is the user-facing thinking level. Supported values are `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, and `max`. For supported OpenAI Responses providers, `off` omits the reasoning object and other levels send `reasoning.summary = "auto"` with the matching effort value.

Rho reads each model's available effort values from cached [models.dev](https://models.dev/) metadata. The interactive reasoning control skips levels the current model does not advertise, so models without `minimal`, `xhigh`, or `max` do not expose those choices. `off` remains available for every model: Rho omits reasoning by default, or sends `effort: "none"` when the model explicitly advertises that value. Switching models also normalizes an unavailable selection to the closest lower supported level. When capability metadata is unavailable or uses an unsupported reasoning scheme, Rho preserves the full level list rather than guessing. You can override metadata locally with `supported_reasoning_levels = ["off", "low", "medium", "high"]` in a model entry in `~/.rho/models.toml` (or the file selected by `RHO_MODELS_PATH`).

`show_reasoning_output` controls whether streamed reasoning text is displayed and stored in the TUI transcript. When reasoning text is hidden, the TUI shows `Thinking...` in its place until the reasoning phase finishes. It defaults to `true`. Changing it from `/config` applies to the next model call.

## Update checks

`check_for_updates` controls whether Rho checks the latest GitHub release at TUI startup. It defaults to `true`. When a newer version is available, the session header shows an update notice and points to `rho update`.

## Tool output limit

`max_output_bytes` controls how much output Rho keeps from [tool](/tools-workspace) calls such as command output and file reads.

`max_tool_output_lines` controls how many lines of a tool result are shown inline before the TUI collapses the rest. It defaults to `10` and is clamped to at least one line when config is loaded.

## RTK

`rtk` enables built-in RTK command rewriting when the `rtk` binary is available. It defaults to `true`; set `rtk = false` to leave shell commands unchanged.

Rewritten commands run through the RTK binary, so their savings are recorded by `rtk gain`. Rho also writes RTK-compatible command records and output sizes under the Claude projects directory so `rtk discover` can include Rho shell commands. Command output is not copied into these compatibility records. Set `CLAUDE_CONFIG_DIR` to override the default `~/.claude` location used by both Rho and RTK.

## Auto compaction

`auto_compact` enables summarizing older conversation history when the estimated current context approaches the effective model window. It is disabled by default. `compact_threshold_percent` controls the trigger point. `compact_target_percent` controls the post-compaction target as a percent of the effective model window; it must stay below the threshold, so values at or above `compact_threshold_percent` are clamped to one below it when the config is loaded or saved. Rho keeps the recent verbatim tail by token budget and safe tool-call boundaries, not by message count. Context estimates are anchored to the most recent provider-reported token usage when available.

Auto compaction affects only future model context. Session files remain append-only and keep the original transcript entries, then append a replacement-history entry used for resume. It is not a privacy or deletion feature.

Model metadata supplies the effective context window when available. Pricing-sensitive models such as `openai/gpt-5.6-sol` and `openai-codex/gpt-5.6-sol` use safer effective windows below the advertised maximum to avoid long-context pricing thresholds.
