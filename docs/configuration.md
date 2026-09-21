# Configuration

Rho stores persistent config at `~/.rho/config.toml` by default.

Change day-to-day settings from the TUI with `/config`, `/model`, `/login`, and the other commands below. Use this page for the file layout, CLI overrides, and keys that do not have their own page. A complete sample is in [Configuration file example](/configuration/full-example).

Secrets are never stored in config. See [authentication and models](/authentication-and-models).

Unknown keys in `config.toml` are a load error, so typos fail loudly. Values that Rho clamps or normalizes warn at load time. Both `--save` and `/config` rewrite only the known schema and discard unknown keys, comments, and formatting.

## Common settings

| Goal | Where |
| --- | --- |
| Provider, model, reasoning | `[model]`, `/model`, or `/config` → **Models** |
| Theme, zen, reasoning display, output streaming | `[display]` or `/config` → **Appearance**. See [Theme](/interactive-tui/theme), [Transcript display](/interactive-tui/transcript), and [Text streaming](/interactive-tui#text-streaming) |
| Permission mode | [Permission modes](/configuration/permissions) |
| Concurrent agents, subagents, rewind | [Behavior](#behavior) |
| Questionnaire timeout | [Questionnaire timeout](#questionnaire-timeout) |
| Prompt templates | [Prompt templates](#prompt-templates) |
| Model-scoped system instructions | [Model prompts](/configuration/model-prompts) |
| Web search | [Web search](/configuration/web-search) |
| xAI image generation | [xAI](/providers/xai#notes) |
| Edit tool | [Edit tool](#edit-tool) |
| MCP servers | `[mcp.servers]`. Inspect with `/mcp` or `rho mcp list`. See [Model Context Protocol](/integrations/mcp) |
| Auto compaction | [Auto compaction](/configuration/compaction) |
| Per-model context or reasoning | [Local model metadata](#local-model-metadata) |
| Model aliases | [Model aliases](#model-aliases) |
| Internal agent models | [Internal agent models](#internal-agent-models) |
| Keybindings | `[keybindings]`. Restart required |
| Update checks | [Update checks](#update-checks) |
| RTK rewrite | [RTK](/integrations/rtk) |

## Changing settings

In the [interactive TUI](/interactive-tui), [`/config`](/interactive-tui#commands) opens a category browser. Type to find a category, Enter to open it, Esc to go back. Space toggles an on/off row in place. Changes save as soon as they change.

| Category | Contains |
| --- | --- |
| Models | Conversation model and reasoning level |
| Appearance | Theme, zen mode, reasoning display, cache miss notices, collapsed tool-output lines, output streaming |
| Agent behavior | Permission mode, Auto classifier, advisor mode, delegation, concurrent agents, questionnaire timeout |
| Context & limits | Auto compaction, max output bytes, prompt history |
| Tools | Inline shell, edit tool, web search, and xAI image generation when the conversation provider is xAI |
| Providers | Login, logout, model-list refresh, models.dev catalog refresh, startup update check |

Apply timing:

- Before the next turn: permission mode, edit tool, advisor mode, web search.
- Immediately, including mid-turn: reasoning, theme, zen, reasoning display, cache miss notices, output streaming, concurrent agents.
- Next session: `enable_subagents`, `max_output_bytes`, xAI image generation.
- Restart: keybindings, and any direct edit of `config.toml`.

When `cache_miss_notices` is on, a completed turn that re-billed a large uncached prompt, over 20K tokens or $0.10, inserts a transcript notice. `/info` always shows session and latest-request cache hit rates, plus re-billed totals once misses were counted.

`/login`, `/logout`, and `/model` remain shortcuts for credentials and the conversation model. The matching `/config` rows open the same pickers. Use `/agents` to inspect reserved internal agents and set their model overrides.

## CLI overrides

`--provider`, `--model`, `--auth`, and `--reasoning` override the loaded config for the current invocation. Add `--save` to write them as the future default.

```bash
rho --provider openai --auth api-key --model gpt-5.6-sol
rho --reasoning high
rho --provider openai --auth api-key --model gpt-5.6-sol --save
rho --config ~/.rho/config.toml
```

The exact `--provider`, `--auth`, and `--model` combination for each provider is on its [provider page](/authentication-and-models#providers).

`--no-system-prompt`, `--no-tools`, `--no-subagents`, and `--agent` apply only to the current run and are not saved. `--no-system-prompt` and `--no-tools` must come before a subcommand (`rho --no-tools run "..."`). `--no-subagents` and `--agent` may appear before or after the subcommand. `--no-subagents` matches `enable_subagents = false`.

## Reasoning options

`reasoning` is the thinking level: `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, and `max`.

Rho reads each model's advertised effort values from cached [models.dev](https://models.dev/) metadata and skips the rest in the TUI. `off` stays available for every model. Rho omits reasoning by default, or sends `effort: "none"` when the model advertises that value. Override the list with `supported_reasoning_levels` in `~/.rho/models.toml`. See [Local model metadata](#local-model-metadata).

For supported OpenAI Responses providers, `off` omits the reasoning object. Other levels send `reasoning.summary = "auto"` with the matching effort.

Local Ollama and [config-defined OpenAI-compatible hosts](/providers/openai-compatible) send `reasoning_effort`, including `"none"`, when capability metadata is missing or the model supports reasoning. Rho omits the field when metadata says reasoning is not configurable, because those APIs treat a missing field as thinking on. Switching models maps an unavailable level to the closest lower supported level. When metadata is missing or uses an unsupported scheme, Rho keeps the full list rather than guessing.

Whether reasoning text is shown, zen mode, and output streaming are display settings. See [Transcript display](/interactive-tui/transcript#display-modes) and [Text streaming](/interactive-tui#text-streaming).

## Advisor mode

Advisor mode gives the agent an `advisor` tool backed by a second model. That model reviews the session transcript and has no tools of its own.

Details: [Advisor mode](/configuration/advisor-mode).

## Prompt templates

A Markdown or text file becomes a slash command. The filename is the command. The file contents are the prompt.

- `~/.rho/prompts/review.md` makes `/prompt:review` available everywhere.
- `.rho/prompts/review.md` makes `/prompt:review` available in that project and its subdirectories.
- A project file overrides a global file with the same name.

```text
Review this code for correctness, security, and maintainability.
```

Inline config overrides a file with the same name:

```toml
[prompt_templates]
review = "Review this code for correctness, security, and maintainability."
```

`/prompt:review src/config.rs` expands to the template text plus `src/config.rs`. Tab in the command palette expands without sending. Enter expands and sends. Names may contain letters, numbers, `-`, and `_`, and cannot duplicate built-in command names. Restart Rho after adding or editing templates.

## Model aliases

`[model.aliases]` maps a short name to a concrete model so a pinned id lives in one place. A value is `provider/model` or a bare model id, which keeps whichever provider is otherwise selected. Model ids may contain `/`.

```toml
[model.aliases]
deep = "anthropic/claude-opus-4-8"
fast = "gpt-5.6-luna"
openrouter-deep = "openrouter/anthropic/claude-sonnet-4"
```

Reference an alias with `@`. The prefix distinguishes aliases from concrete ids, so a missing alias is a configuration error.

```toml
[model]
model = "@deep"

[internal_agents.session-title]
model = "@fast"
```

The same syntax works with `rho --model @deep`, `/model @deep`, and `model: @deep` in [agent definition frontmatter](/subagents). Rho resolves aliases before any model-specific behavior and never rewrites the mapping. A concrete model id is always literal, even when an alias has the same name. Saving config keeps the `@deep` reference rather than its expansion while the selected concrete model still matches. Alias values must be concrete models, so they cannot begin with `@`. Every provider-qualified alias is validated at load, including aliases that are not selected.

## Local model metadata

`~/.rho/models.toml` overrides catalog fields for one model. `RHO_HOME` moves this file with the rest of `~/.rho`. `RHO_MODELS_PATH` selects a different file.

Without this file, Rho uses the catalog window. For GPT-5.5 and GPT-5.6 that is the [models.dev](https://models.dev/) input limit. Codex GPT-5.5 keeps a 400k effective window because that is the product limit. Set `usable_context_window` to raise or cap the budget Rho shows and uses for [auto compaction](/configuration/compaction):

```toml
[models."openai-codex/gpt-5.6-sol"]
usable_context_window = 272000
```

The key is `provider/model`. You can also set `effective_context_window` and `supported_reasoning_levels`. Set `catalog` to a models.dev provider slug, or to `provider/model`, to borrow that catalog row for a custom host. Local values win over catalog data. Restart Rho or switch models after you edit the file.

```toml
[models."cliproxyapi/claude-sonnet-4-5"]
catalog = "anthropic"

[models."cliproxyapi/opus-thinking"]
catalog = "anthropic/claude-opus-4-6"
```

The catalog window can sit above a model's long-context price tier. Rho does not clamp it.

## Internal agent models

Rho uses reserved internal agents for session titles, `/goal` completion, the [`advisor`](/configuration/advisor-mode) tool, and Auto permission classification. Most roles follow the active conversation provider, model, and auth. Run `/agents`, select the role, and press Enter to choose a model. **Use conversation model** removes that role's override. Changes apply to the next invocation and save at once.

`advisor` and `permission-classifier` have no default and no conversation-model fallback. Advisor mode stays inactive until a model is chosen. Auto opens the classifier picker when no model is set. Cancelling from `/config` keeps the previous mode. Cancelling the startup picker falls back to Supervised. The classifier defaults to low reasoning when a model is first selected.

The advisor picker also lists `claude-code/…` rows when the `claude` binary is installed. Choosing one runs the advisor on [Claude Code](/configuration/advisor-mode#claude-code-as-the-advisor). When the advisor model supports configurable reasoning, Rho carries the previous level, or the advisor default, onto the new model.

`/config` → **Agent behavior** exposes advisor model, advisor reasoning, permission classifier model, and permission classifier reasoning.

```toml
[internal_agents.session-title]
provider = "openai"
model = "gpt-5.6-luna"
auth = "api-key"
```

Model aliases work here. Rho still reads the old `[title]` section and flat title settings, and rewrites them as `[internal_agents.session-title]` on the next save.

## Edit tool

`edit_tool` under `[behavior]` selects the file edit tool exposed to the model. It defaults to `auto`. Only one edit tool is registered at a time.

| Value | Exposed tool | Format |
| --- | --- | --- |
| `auto` | preferred for the active provider | Built-in catalog. Switches when the provider changes |
| `hashline` | `edit` | Snapshot-tagged, line-anchored `PUT` and `CUT` |
| `apply_patch` | `apply_patch` | Codex-style, multi-file patch documents |
| `str_replace` | `str_replace` | Exact `old_string` to `new_string` replacement in one file |

```toml
[behavior]
edit_tool = "auto"
```

`auto` is a preference, not a tool name. Rho keeps `auto` in config and advertises the concrete format the active provider's models were trained on. Custom providers can override that choice:

```toml
[providers.custom.vllm]
base_url = "http://127.0.0.1:8000/v1"
edit_tool = "apply_patch"
```

Built-in providers have no per-provider override. They use this table unless you pin a global `behavior.edit_tool`.

| Provider | Preferred format | Why |
| --- | --- | --- |
| `openai-codex` | `apply_patch` | Codex trains on Codex-style patches |
| `anthropic` | `str_replace` | Claude Code trains on exact string replace |
| `xai` | `str_replace` | First-party agent tooling uses string replace |
| all others | `hashline` | Rho default when no first-party match is known |

Pinned values stay fixed across provider changes. From `/config`, the change applies before the next turn: the tool list rebuilds and the session gets a short notice with the new schema. Auto also switches live when you change providers mid-session. Direct `config.toml` edits need a restart. Format details: [Hash-line edit format](/tools-workspace/edit-format).

## Behavior

`enable_subagents` controls the `agent` and `agents` tools. It defaults to `true`. Set it to `false` to remove both tools and tell the model not to delegate. The change applies to the next session. `--no-subagents` does the same for one run.

`agent_concurrency` is the maximum number of delegated agents that may run at once, including background runs. It defaults to `10` and must stay between `1` and `64`. `/config` applies immediately: in-flight agents keep their slots, and queued agents start as capacity frees. `RHO_AGENT_CONCURRENCY` is no longer read. Claude-cli runs also take a nested cap of 2 by default (`RHO_CLAUDE_AGENT_CONCURRENCY`), always `min(total, claude_cap)`. Cursor runs take only the global pool. There is no nested Cursor cap yet.

`inline_shell` selects the shell for `!` and `!!` in the TUI. It defaults to `bash` on macOS and Linux and `powershell` on Windows. See [inline shell](/inline-shell).

`experimental_workspace_rewind` enables native file-tool checkpoints and `/rewind`. It defaults to `false`. Restart after changing it. Checkpoints cover `write` and the selected edit tool. Rho warns when a turn ran a shell command, because shell, Git, process, network, database, and service effects cannot be restored. `/tree` branches conversation state only. `/rewind` branches conversation state and restores captured files. Git commands stay separate.

`advisor_mode` controls whether the advisor tool is available. It defaults to `false`. See [Advisor mode](/configuration/advisor-mode).

## RTK

`rtk` enables built-in [RTK](/integrations/rtk) command rewriting when the `rtk` binary is available. It defaults to `true`. Set `rtk = false` to leave shell commands unchanged.

## Tool output limit

`max_output_bytes` is how much output Rho keeps from [tool](/tools-workspace) calls such as command output, file reads, and loaded skills. It defaults to `64000`.

`max_tool_output_lines` is how many lines of a tool result show inline before the TUI collapses the rest. It defaults to `10` and is clamped to at least one line on load.

`prompt_history_limit` is how many sent composer prompts Rho keeps in `~/.rho/prompt-history.sqlite3` for up-arrow recall across sessions. `$RHO_HOME` moves that file. It defaults to `1000`. `0` disables persistence. Values above `10000` clamp to `10000` on load. `/config` → **Context & limits** edits the cap and can clear saved history. Lowering the cap below the number of stored prompts asks first, then deletes the oldest extras. Clear also asks first. This is separate from `/new` and `/clear`, which reset the conversation, not composer recall.

## Update checks

`check_for_updates` controls whether Rho checks the latest GitHub release at TUI startup. It defaults to `true`. A newer version shows an update notice in the session header and points to `rho update`. Change it from **Providers** in `/config`.

## Questionnaire timeout

Questionnaires wait for an answer by default. To let an untouched form use explicit fallback answers, set a positive whole number of seconds:

```toml
[questionnaire]
timeout_seconds = 60
```

That value is an example, not a default. Omit `timeout_seconds` to disable automatic answers. Zero, negative values, fractions, and strings are invalid. In `/config` → **Agent behavior** → **Questionnaire timeout**, enter positive seconds or clear the field. Changes apply when the next form opens, including forms from delegated agents, not to an already-open form.

What the TUI shows, and what does not count as consent: [Questionnaire fallbacks](/interactive-tui#questionnaire-fallbacks).
