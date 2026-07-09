# Configuration

Rho stores persistent config at `~/.rho/config.toml` by default.

```toml
provider = "openai"
model = "gpt-5.5"
max_output_bytes = 12000
max_tool_output_lines = 10
auth = "api-key" # or "codex", "anthropic-api-key", or "github-copilot"
reasoning = "medium" # off, minimal, low, medium, high, xhigh, or max
show_reasoning_output = true
check_for_updates = true
auto_compact = false
compact_threshold_percent = 85
compact_target_percent = 50
title_provider = "openai"
title_model = "gpt-5.5"
title_auth = "api-key"
web_search_provider = "auto" # auto, openai, exa, brave, or disabled
web_search_openai_api_key = ""
web_search_exa_api_key = ""
web_search_brave_api_key = ""
rtk = true
```

The full saved file can also include optional title-generation and web-search settings. `title_provider`, `title_model`, and `title_auth` override the model used to generate session titles; when they are omitted, Rho uses the active provider/model/auth selection. The web search API-key fields are optional convenience settings for tool access; leave them unset unless you want Rho's built-in web search tool to use a specific provider credential.

## CLI overrides

Passing `--provider`, `--model`, `--auth`, or `--reasoning` updates the config file and makes the choice the future default.

```bash
rho --provider openai --auth api-key --model gpt-5.5
rho --provider openai-codex --auth codex --model gpt-5.5
rho --provider anthropic --auth anthropic-api-key --model claude-sonnet-4-5
rho --provider github-copilot --auth github-copilot --model gpt-4.1
rho --reasoning high
```

These values select [authentication and models](/authentication-and-models).

You can load and save a specific config file with:

```bash
rho --config ~/.rho/config.toml
```

`--no-system-prompt` and `--no-tools` are only available on the command line and apply only to the current run.

## Title model

Rho can use a separate model for generating session titles. The optional `title_provider`, `title_model`, and `title_auth` settings persist that selection. Use `/title-model` in the TUI to choose from available catalog and cached models, or pass a direct provider/model name:

```text
/title-model openai/gpt-5.5
/title-model anthropic/claude-sonnet-4-5
```

If no title model settings are present, Rho falls back to the active provider, model, and auth.

## Web search

`web_search_provider` controls the built-in [web search tool](/tools-workspace#built-in-tools). Supported values are `auto`, `openai`, `exa`, `brave`, and `disabled`. Unknown values are normalized back to `auto` when config is loaded.

Optional `web_search_openai_api_key`, `web_search_exa_api_key`, and `web_search_brave_api_key` values let the web search tool use provider-specific credentials from config. Empty strings are ignored. Set `web_search_provider = "disabled"` to remove the web search tool from the tool registry while keeping other workspace tools enabled.

## TUI updates

In the [interactive TUI](/interactive-tui), [`/config`](/interactive-tui#commands) opens a picker for configuration values. The `reasoning` row cycles through `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, and `max`, saves immediately, and applies to the current session. The `show_reasoning_output` row toggles whether reasoning text is shown in the TUI and applies on the next model call. The `check_for_updates` row toggles startup update checks against GitHub releases. The auto compaction rows toggle compaction and edit its threshold and target percentages. The `max_output_bytes` row opens a numeric input and saves for the next session.

[`/login`](/interactive-tui#commands) stores credentials in the OS credential store, not in this config file. [`/logout`](/interactive-tui#commands) deletes stored credentials. [`/model`](/interactive-tui#commands) saves the selected `provider` and `model`. [`/title-model`](/interactive-tui#commands) saves optional title-generation model settings. The picker shows entries from Rho's static [model catalog](/authentication-and-models#providers-and-model-catalog) and cached dynamic provider model lists for providers with available auth, and `/model provider/model` can switch explicitly. GitHub Copilot uses `github-copilot/<model>` names from the refreshed Copilot API model list.

## Reasoning options

`reasoning` is the user-facing thinking level. Supported values are `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, and `max`. For supported OpenAI Responses providers, `off` omits the reasoning object. Other levels send `reasoning.summary = "auto"`; `minimal` maps to effort `low`, while the remaining levels map to matching effort values. Codex applies model-specific support: GPT-5.6 models accept `max`, while older Codex models such as GPT-5.5 stop at `xhigh`.

`show_reasoning_output` controls whether streamed reasoning text is displayed and stored in the TUI transcript. It defaults to `true`. Changing it from `/config` applies to the next model call.

## Update checks

`check_for_updates` controls whether Rho checks the latest GitHub release at TUI startup. It defaults to `true`. When a newer version is available, the session header shows an update notice and points to `rho update`.

## Tool output limit

`max_output_bytes` controls how much output Rho keeps from [tool](/tools-workspace) calls such as command output and file reads.

`max_tool_output_lines` controls how many lines of a tool result are shown inline before the TUI collapses the rest. It defaults to `10` and is clamped to at least one line when config is loaded.

## RTK

`rtk` enables built-in RTK command rewriting when the `rtk` binary is available. It defaults to `true`; set `rtk = false` to leave shell commands unchanged.

## Auto compaction

`auto_compact` enables summarizing older conversation history when the estimated current context approaches the effective model window. It is disabled by default. `compact_threshold_percent` controls the trigger point. `compact_target_percent` controls the post-compaction target as a percent of the effective model window; it must stay below the threshold, so values at or above `compact_threshold_percent` are clamped to one below it when the config is loaded or saved. Rho keeps the recent verbatim tail by token budget and safe tool-call boundaries, not by message count. Context estimates are anchored to the most recent provider-reported token usage when available.

Auto compaction affects only future model context. Session files remain append-only and keep the original transcript entries, then append a replacement-history entry used for resume. It is not a privacy or deletion feature.

Model metadata supplies the effective context window when available. Pricing-sensitive models such as `openai/gpt-5.5` and `openai-codex/gpt-5.5` use safer effective windows below the advertised maximum to avoid long-context pricing thresholds.
