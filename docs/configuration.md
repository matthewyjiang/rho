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
| Project and global instructions | [AGENTS.md](#project-and-global-instructions), `/init`, and `/remember [global] <text>` |
| Model-scoped system instructions | [Model prompts](/configuration/model-prompts) |
| Web search | [Web search](/configuration/web-search) |
| SpaceXAI image generation | [SpaceXAI](/providers/xai#notes) |
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
| Appearance | Theme, zen mode, reasoning display, cache miss notices, header hints, notifications, collapsed tool-output lines, output streaming |
| Agent behavior | Permission mode, Auto classifier, advisor mode, delegation, concurrent agents, questionnaire timeout |
| Context & limits | Auto compaction, max output bytes, prompt history |
| Tools | Inline shell, edit tool, web search, and SpaceXAI image generation when the conversation provider is SpaceXAI |
| Providers | Login, logout, model-list refresh, models.dev catalog refresh, startup update check |

Apply timing:

- Before the next turn: permission mode, edit tool, advisor mode, web search.
- Immediately, including mid-turn: reasoning, theme, zen, reasoning display, cache miss notices, header hints, notifications, output streaming, concurrent agents.
- Next session: `enable_subagents`, `max_output_bytes`, SpaceXAI image generation.
- Restart: keybindings, and any direct edit of `config.toml`.

When `cache_miss_notices` is on, a completed turn that re-billed a large uncached prompt, over 20K tokens or $0.10, inserts a transcript notice. `/info` always shows session and latest-request cache hit rates, plus re-billed totals once misses were counted.

`show_header_hints` controls the keyboard hint block under the session header. It defaults to on. Turning it off hides the shortcut list; a signed-out session still shows its `/login` hints.

`notifications` defaults to on. While the terminal is unfocused, Rho notifies when an approval or questionnaire opens or a turn finishes; a goal run or queued follow-ups notify once, when Rho waits for you again. iTerm2, WezTerm, Ghostty, and kitty get an OSC 9 desktop notification. Other terminals, and anything inside tmux or screen, get a terminal bell. Rho only knows the terminal is unfocused if it reports focus changes; in tmux, set `focus-events on`. Under [Herdr](/integrations/herdr) Rho sends none, because Herdr shows pane state itself.

`/login`, `/logout`, and `/model` remain shortcuts for credentials and the conversation model. The matching `/config` rows open the same pickers. Use `/agents` to inspect reserved internal agents and set their model overrides.

## Project and global instructions

Rho loads instructions from `AGENTS.md` only. With the default system prompt, discovery runs in this order:

1. Global instructions at `~/.rho/AGENTS.md`.
2. Project `AGENTS.md` files from the git root through the current working directory, including each directory on that path. Outside a git repository, only the current directory's `AGENTS.md` is discovered.

More specific files appear later and take precedence when instructions conflict. Rho does not load `CLAUDE.md`, `.cursor/rules`, or `.github/copilot-instructions.md` as instructions.

Run `/init` in the [interactive TUI](/interactive-tui#commands) to survey the repository and create a concise `AGENTS.md` at its git root (or in the current directory outside a repository). The survey focuses on build/test/lint commands, layout, non-obvious conventions, and contribution rules. Existing instructions keep their structure and voice; `/init` makes targeted edits rather than replacing them. Other tools' instruction files may inform the survey, but the output is always `AGENTS.md`.

`/init` starts a model turn using a built-in skill and the active agent's file tools. It is unavailable during a running turn, in Plan permission mode, or when the active agent lacks the required skill, write, or file-edit tools. Normal write permissions still apply.

Instructions are cached for the current session and re-read from disk whenever you start or switch to a different session: `/new` (or `/clear`), `/resume` of another session, or cross-session tree selection. After `/init` or a manual instruction edit, start or switch sessions to load the updated global and project files, or restart Rho. Model switches and same-session tree navigation do not reload `AGENTS.md`. `--no-system-prompt` omits instruction files entirely.

Use `/remember <text>` in the [interactive TUI](/interactive-tui#commands) to append one bullet to the Git-root `AGENTS.md` (or the current directory's file outside a repository). Use `/remember global <text>` for an instruction that applies across projects:

```text
/remember use max jobs 8
/remember global keep explanations concise
```

The text must be a non-empty single line. Rho creates the file if needed, preserves existing instructions, and adds the new instruction to the current conversation without starting a model turn. The file is re-read whenever you start or switch to a different session, as described above. `/remember` is available only between model turns. To revise or remove instructions, edit `AGENTS.md` directly. Global instructions always live at `~/.rho/AGENTS.md`, even when `RHO_HOME` redirects Rho's data directory.

## CLI overrides

`--provider`, `--model`, `--auth`, and `--reasoning` override the loaded config for the current invocation. Add `--save` to write them as the future default.

```bash
rho --provider openai --auth api-key --model gpt-5.6-sol
rho --reasoning high
rho --provider openai --auth api-key --model gpt-5.6-sol --save
```

`--config` selects the file to load and save. It does not override those keys:

```bash
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

Image input comes from models.dev `modalities.input`. When the catalog lists a model as text-only, Rho replaces images from `read_file`, MCP tools, and attachments with a short text note before sending the request. Models with no catalog row still receive images. Set `image_input` to override the catalog:

```toml
[models."cliproxyapi/my-text-model"]
image_input = false
```

## Internal agent models

Rho uses reserved internal agents for session titles, `/goal` completion, the [`advisor`](/configuration/advisor-mode) tool, Auto permission classification, and [compaction summaries](/configuration/compaction#summarizer-model). Most roles follow the active conversation provider, model, and auth. Run `/agents`, select the role, and press Enter to choose a model. **Use conversation model** removes that role's override. Changes apply to the next invocation and save at once.

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

`workspace_rewind` enables native file-tool checkpoints and `/rewind`. It defaults to `true`. Toggle **Workspace rewind** under Agent behavior in `/config`, or opt out explicitly:

```toml
[behavior]
workspace_rewind = false
```

Restart Rho after changing it. The legacy `experimental_workspace_rewind` key is accepted but ignored (including saved `false` defaults), and the next config save removes it. Only `workspace_rewind = false` opts out. Checkpoints from the old experimental version lack pre-turn conversation boundaries and cannot be rewound; they are rejected before any files change.

Checkpoints capture original file contents for `write` and the selected native edit tool. Shell (`bash`), process, Git, network, database, service, and other untracked tool effects are **not captured**; recorded untracked effects show a **limited** badge in `/rewind`. `/tree` changes conversation state only. `/rewind` restores captured files and returns the conversation to **before** the selected turn, preserving the old branch. Conflicting or unsupported paths stay unchanged; a partial restore does not select a different conversation state.

Checkpoint journals live at `<session>/workspace-checkpoints/checkpoints.jsonl`. The per-file capture budget is **2 MiB**: larger files are marked unsupported, and a transcript notice names the budget, limit, and file size. The serialized per-session journal budget is **64 MiB**: a turn that would exceed it is not appended, and a visible notice names the budget, limit, requested total, and turn size. Further capture pauses for that running session; earlier turns remain rewindable. See [workspace checkpoints](/sessions#workspace-checkpoints) for storage and privacy details.

`advisor_mode` controls whether the advisor tool is available. It defaults to `false`. See [Advisor mode](/configuration/advisor-mode).

## Codemode

The `codemode` tool composes native and MCP sibling tools in a Starlark script. It is available whenever tools are enabled. `[codemode] mode` matches Pi's `codemode.mode` and controls how the other tools are presented:

```toml
[codemode]
mode = "on" # or "only"
```

- `on` (default): native sibling tools such as `read_file`, `write`, and `bash` stay declared next to `codemode`; the model may use either.
- `only`: only `codemode` and `tool_search` are declared. Native siblings remain executable through script `call_tool`; `tool_search` discovers their names and result schemas without advertising them directly.

`/codemode on|only` changes the next model request without rebuilding the runtime and saves the preference. Neither mode is a permission level: nested calls inherit `permission_mode`. MCP tools are script-only in both modes and discoverable with `tool_search` or script `search_tools`/`list_tools`. Discovery does not promote tools into the direct model list.

Scripts call one tool with `call_tool(name, args)` or run independent calls concurrently with `call_tools([(name, args), ...])`, which returns results in order. Pass exact tool names and argument dictionaries. Results are native Starlark dictionaries with `"is_error"`, `"content"`, and `"data"` keys; use bracket indexing, not dot access. A batch runs up to 4 calls at once, the same width as a model-issued parallel tool batch, and the whole batch counts toward the 64-call limit per script before any call starts. Script `list_tools()` and `search_tools(query)` return `{name, description}` rows; `describe_tool(name)` adds the result schema. A script that fails keeps its printed output and call log. See [Codemode](/tools-workspace/codemode) for the full guide.

## RTK

`rtk` enables built-in [RTK](/integrations/rtk) command rewriting when the `rtk` binary is available. It defaults to `true`. Set `rtk = false` to leave shell commands unchanged.

## Tool output limit

`max_output_bytes` is how much output Rho keeps from [tool](/tools-workspace) calls such as command output, file reads, and loaded skills. It defaults to `64000`.

`max_tool_output_lines` is how many wrapped screen rows of a tool card show inline before the TUI collapses the rest. It defaults to `10` and is clamped to at least one line on load. Delegated agent prompts use the same limit: they stream from the beginning, then keep a stable collapsed preview. Click the card or press `Ctrl+O` to expand the full prompt while it streams or after launch. Once the agent starts or finishes, run receipts and results appear before the prompt so they remain visible when collapsed.

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
