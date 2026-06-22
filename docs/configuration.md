# Configuration

Rho stores persistent config at `~/.rho/config.toml` by default.

```toml
provider = "openai"
model = "gpt-5.5"
max_output_bytes = 12000
auth = "api-key" # or "codex"
reasoning = "medium" # off, minimal, low, medium, high, or xhigh
auto_compact = false
compact_threshold_percent = 85
compact_recent_messages = 8
```

## CLI overrides

Passing `--provider`, `--model`, or `--auth` updates the config file and makes the choice the future default.

```bash
rho --provider openai --auth api-key --model gpt-5.5
rho --provider openai-codex --auth codex --model gpt-5.5
```

These values select [authentication and models](/authentication-and-models).

You can load and save a specific config file with:

```bash
rho --config ~/.rho/config.toml
```

## TUI updates

In the [interactive TUI](/interactive-tui), [`/config`](/interactive-tui#commands) opens a picker for configuration values. The `reasoning` row cycles through `off`, `minimal`, `low`, `medium`, `high`, and `xhigh`, saves immediately, and applies to the current session. The `max_output_bytes` row opens a numeric input and saves for the next session.

[`/login`](/interactive-tui#commands) stores credentials in the OS credential store, not in this config file. [`/logout`](/interactive-tui#commands) deletes stored credentials. [`/model`](/interactive-tui#commands) saves the selected `provider` and `model`. The picker shows entries from Rho's static [model catalog](/authentication-and-models#providers-and-model-catalog) for providers with available auth, and `/model provider/model` can switch explicitly.

## Reasoning options

`reasoning` is the user-facing thinking level. Supported values are `off`, `minimal`, `low`, `medium`, `high`, and `xhigh`. For supported OpenAI Responses providers, `off` omits the reasoning object. Other levels send `reasoning.summary = "auto"`; `minimal` maps to effort `low`, while `low`, `medium`, `high`, and `xhigh` map to matching effort values.

## Tool output limit

`max_output_bytes` controls how much output Rho keeps from [tool](/tools-workspace) calls such as command output and file reads.

## Auto compaction

`auto_compact` enables summarizing older conversation history when the estimated current context approaches the model window. It is disabled by default. `compact_threshold_percent` controls the trigger point, and `compact_recent_messages` controls how many recent messages are kept verbatim after older history is summarized.
