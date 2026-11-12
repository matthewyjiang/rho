# Configuration

Rho keeps agent settings in `~/.rho/config.toml` by default. The interactive TUI reads two more files of its own: `~/.rho/clients/tui.toml` for its display and composer settings, and `~/.rho/tui.toml` for the theme and keybindings. `RHO_HOME` moves all three.

Change day-to-day settings from the TUI with `/config`, `/model`, `/login`, and the other commands below. Use this page for the file layout, CLI overrides, and keys that do not have their own page. A complete sample is in [Configuration file example](/configuration/full-example).

Secrets are never stored in config. See [authentication and models](/authentication-and-models).

Unknown keys in any of these files are a load error, so typos fail loudly. A value outside its range is an error too, naming the key, the limit, and the value you set; Rho does not clamp. Every bad key is reported at once.

## Upgrading from Rho 2.x

Rho 2.x kept everything in `config.toml`. When Rho 3.0 starts and finds 2.x keys in `config.toml`, it rewrites the file to the 3.0 layout and prints what it moved. The check runs at every start, so a 2.x key added by hand later is rewritten the same way, with a new backup:

- `[display]` settings, `behavior.check_for_updates`, and `keybindings.editing_mode` move to `clients/tui.toml`.
- `display.theme` and the `[keybindings]` chords move to `tui.toml`.
- Top-level keys such as `provider`, `model`, and `permission_mode` move into their tables in `config.toml`.
- Plaintext `web_search_*_api_key` values move to the credential store.
- Web-search `hosted` and `provider` become `mode` and `backend`. The 2.x native-only pair (`hosted = true`, `provider = "disabled"`) becomes `mode = "off"`, because 3.0 cannot search natively without also enabling a backend.
- `edit_tool = "hashline"` becomes `"str_replace"`, and the retired `behavior.experimental_workspace_rewind` key is removed.

Before Rho rewrites a file, it copies the file beside itself as `<name>.2x.bak`, for example `config.toml.2x.bak`. An existing backup is never replaced. The backup of a file that held plaintext web-search keys still holds them, so delete it once the upgrade looks right. A value already set in `tui.toml` or `clients/tui.toml` wins over the 2.x value. Comments and key order are kept for keys that stay.

Only keys Rho 2.x wrote are rewritten. A key that neither version knows stays in the file and is still a load error.

`--save` and the daemon's settings writes (behind `/config`) rewrite only the settings that changed. Comments, key order, formatting, and tables Rho does not manage stay as you wrote them. A save that would leave an invalid file, such as a compaction target at or above the threshold, is refused and writes nothing.

## File layout

Agent settings in `config.toml` live in tables; no setting sits at the top level:

```toml
[model]
provider = "openai"
model = "gpt-5.6-sol"
auth = "api-key"
reasoning = "medium"
fast_mode = false
favorite_models = ["openai/gpt-5.6-sol"]

[behavior]
permission_mode = "supervised"
enable_subagents = true
agent_concurrency = 10
edit_tool = "auto"
rtk = true

[compaction]
auto_compact = true

[output]
max_output_bytes = 64000

[web_search]
mode = "auto"
```

The full set of tables is in [Configuration file example](/configuration/full-example).

The TUI's own settings sit at the top level of `~/.rho/clients/tui.toml`:

```toml
zen_mode = false
show_reasoning_output = true
output_streaming = "live"
show_header_hints = true
notifications = true
cache_miss_notices = false
max_tool_output_lines = 10
prompt_history_limit = 1000
editing_mode = "default"
check_for_updates = true
```

`~/.rho/tui.toml` holds the theme and keybinding chords:

```toml
theme = "terminal"

[keybindings]
cycle_permission_mode = "alt+m"
```

The TUI reads both files at startup. It hands its `clients/tui.toml` settings to the daemon, so `/config` saves them there and applies them live; see [Changing settings](#changing-settings). `tui.toml` stays hand-edited.

## Common settings

| Goal | Where |
| --- | --- |
| Provider, model, reasoning | `[model]`, `/model`, or `/config` → **Models** |
| Theme, zen, reasoning display, output streaming | `~/.rho/tui.toml` (theme), `~/.rho/clients/tui.toml`, or `/config` → **Display**. See [Theme](/interactive-tui/theme), [Transcript display](/interactive-tui/transcript), and [Text streaming](/interactive-tui#text-streaming) |
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
| OpenAPI sources | `[openapi.<source>]`. See [OpenAPI](/integrations/openapi) |
| Auto compaction | [Auto compaction](/configuration/compaction) |
| Per-model context or reasoning | [Local model metadata](#local-model-metadata) |
| Model aliases | [Model aliases](#model-aliases) |
| Internal agent models | [Internal agent models](#internal-agent-models) |
| Keybindings | `[keybindings]` in `~/.rho/tui.toml`. Restart required |
| Update checks | [Update checks](#update-checks) |
| RTK rewrite | [RTK](/integrations/rtk) |

## Changing settings

`/config` changes settings through the [daemon](/daemon#configuration-changes), which runs the TUI's session.

The root page lists the session's own options (model, reasoning, fast mode, permission mode), then one row per settings category, then the theme and **Clear prompt history**, which stay local to this terminal. Type to find a row, Enter to open it, Esc to go back.

| Category | Contains |
| --- | --- |
| Models | Default model, reasoning, and fast mode for new sessions |
| Agent | Permission mode, Auto classifier, advisor, delegation, concurrent agents, questionnaire timeout, workspace rewind, cache warming |
| Context | Auto compaction and max output bytes |
| Tools | Inline shell, edit tool, RTK, codemode, web search, SpaceXAI image generation |
| Display | The TUI's own settings in `clients/tui.toml`: zen mode, reasoning display, output streaming, header hints, notifications, cache miss notices, collapsed tool-output lines, prompt history, editing mode, update check |

The credential store is not on this screen: choose it with `rho credential-store set` or at your first login, which check the store first ([Credential store](/authentication-and-models#where-credentials-live)).

A category page shows each setting's value and, in its detail, when a change applies. Enter flips an on/off setting, as does Space before you type a filter. A choice or model opens a page of its values, with a **Default** row once the file sets it. A number or text opens a line editor; the prompt shows a number's range, and an empty entry removes the setting so its default applies.

Every change goes to the daemon, which writes the file and tells every client; a row shows its new value once the daemon confirms it. If the settings changed elsewhere first, nothing is written and the status says to try again. If a hand edit leaves the file invalid, the daemon keeps the last good settings and the TUI shows the problem.

When a change applies:

- Immediately: the Display settings, and concurrent agents.
- From the next turn of each loaded session: advisor mode, the advisor model and reasoning, auto compaction, and the codemode mode.
- New sessions: everything else, including the default model, permission mode, edit tool, and web search. `/model`, `/permissions`, and `/fast` change the current session.
- Restart: keybindings and the theme in `tui.toml`.

`/advisor` turns the advisor on or off, and `/advisor model` chooses its model. `/advisor on` without a model asks for one first, then saves both at once. See [Advisor mode](/configuration/advisor-mode).

When `cache_miss_notices` is on, a completed turn that re-billed a large uncached prompt, over 20K tokens or $0.10, inserts a transcript notice. `/info` always shows session and latest-request cache hit rates, plus re-billed totals once misses were counted.

`show_header_hints` controls the keyboard hint block under the session header. It defaults to on. Turning it off hides the shortcut list; a signed-out session still shows its `/login` hints.

`notifications` defaults to on. While the terminal is unfocused, Rho notifies when an approval or questionnaire opens or a turn finishes; queued follow-ups notify once, when Rho waits for you again. iTerm2, WezTerm, Ghostty, and kitty get an OSC 9 desktop notification. Other terminals, and anything inside tmux or screen, get a terminal bell. Rho only knows the terminal is unfocused if it reports focus changes; in tmux, set `focus-events on`. Under [Herdr](/integrations/herdr) Rho sends none, because Herdr shows pane state itself.

Separately from `notifications`, Rho reports its state to terminals that support the [Program Status Protocol](https://www.superlogical.com/rex/docs/build/program-status) (OSC 7501), such as Ghostty, so tabs and agent dashboards can show it:

| State | When |
| --- | --- |
| `working` | A turn is running |
| `blocked` | An approval or questionnaire waits for you, or no provider is signed in |
| `done` | A turn finished |
| `error` | A turn failed and was not retried |
| `idle` | Rho started, or you interrupted or cancelled the turn |

Like notifications, `done` and `error` are reported once Rho waits for you, so queued follow-ups do not report between turns. Rho clears its status on exit. Messages are the wait reason, the sign-in hint, or the first line of an error; prompts and assistant replies are never sent. Rho reports only after the terminal answers the protocol's support query at startup, and tmux and screen do not forward it. Set `RHO_PROGRAM_STATUS=1` to report without asking, for example when SSH latency delays the reply, or `RHO_PROGRAM_STATUS=0` to turn reports off.

`/login`, `/logout`, `/refresh-models`, and `/model` handle credentials, model lists, and the conversation model. Use `/agents` to inspect reserved internal agents and set their model overrides.

## Project and global instructions

Rho loads instructions from `AGENTS.md` only. With the default system prompt, discovery runs in this order:

1. Global instructions at `~/.rho/AGENTS.md`.
2. Project `AGENTS.md` files from the git root through the current working directory, including each directory on that path. Outside a git repository, only the current directory's `AGENTS.md` is discovered.

More specific files appear later and take precedence when instructions conflict. Rho does not load `CLAUDE.md`, `.cursor/rules`, or `.github/copilot-instructions.md` as instructions.

Run `/init` in the [interactive TUI](/interactive-tui#commands) to survey the repository and create a concise `AGENTS.md` at its git root (or in the current directory outside a repository). The survey focuses on build/test/lint commands, layout, non-obvious conventions, and contribution rules. Existing instructions keep their structure and voice; `/init` makes targeted edits rather than replacing them. Other tools' instruction files may inform the survey, but the output is always `AGENTS.md`.

`/init` starts a model turn using a built-in skill and the active agent's file tools. It is unavailable during a running turn, in Plan permission mode, or when the active agent lacks the required skill, write, or file-edit tools. Normal write permissions still apply.

Instructions are cached for the current session and re-read from disk whenever you start or switch to a different session: `/new` (or `/clear`), `/resume` of another session, or cross-session tree selection. After `/init` or a manual instruction edit, start or switch sessions to load the updated global and project files, or restart Rho. Model switches and same-session tree navigation do not reload `AGENTS.md`. [Added workspace directories](/tools-workspace#additional-workspace-directories) load their own `AGENTS.md` files too. `--no-system-prompt` omits instruction files entirely.

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

Without `--auth`, a provider selected by `--provider` or a `provider/model` reference uses an auth mode that has a stored credential or `RHO_<NAME>_API_KEY` set, the same way the `/model` picker does. It falls back to the provider's default auth only when none does. For example, `rho --model my-proxy/some-model run "hi"` sends the key saved with `/login my-proxy`.

`--config` selects the file to load and save. It does not override those keys:

```bash
rho --config ~/.rho/config.toml run "..."
```

The interactive TUI refuses `--config`, since its session runs in the [daemon](/daemon#daemon-backed-tui), which reads the config in its Rho home. Set `RHO_HOME` to run on another.

The exact `--provider`, `--auth`, and `--model` combination for each provider is on its [provider page](/authentication-and-models#providers).

`--no-system-prompt`, `--no-tools`, `--no-subagents`, and `--agent` apply only to the current run and are not saved. `--no-system-prompt` and `--no-tools` must come before a subcommand (`rho --no-tools run "..."`). `--no-subagents` and `--agent` may appear before or after the subcommand. `--no-subagents` matches `behavior.enable_subagents = false`.

## Reasoning options

`model.reasoning` is the thinking level: `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, and `max`.

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

`/prompt:review src/config.rs` expands to the template text plus `src/config.rs`. Tab in the command palette expands without sending. Enter expands and sends. Names may contain letters, numbers, `-`, and `_`, and cannot duplicate built-in command names.

Placeholders put arguments inside the template instead of after it:

- `$ARGUMENTS` is all text after the command.
- `$1`, `$2`, … are the whitespace-separated arguments. Double quotes group words with spaces: `/prompt:fix "login bug" src/auth.rs` makes `$1` = `login bug`. Single quotes stay literal, so apostrophes are safe.
- A missing argument expands to nothing.

```text
Compare $1 with $2. Focus on: $ARGUMENTS
```

A template with any placeholder no longer gets the text appended. Its palette entry lists the arguments it takes, such as `/prompt:compare [$1] [$2] [arguments]`, and Tab completes the command so you can type them first. Template files reload while Rho runs: palette discovery is cached for 2 seconds, while submitting `/prompt:<name>` refreshes the files immediately. Inline `[prompt_templates]` changes apply after a restart.

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

A custom host with `catalog_mode = "model-id"` looks up the unsplit `slug/model` id among models.dev serving providers first. When no provider publishes that id, as with `alibaba/qwen3.8-flash-next`, which only resellers serve, Rho falls back to the canonical [models.dev model page](https://models.dev/models/alibaba/qwen3.8-flash-next/) for the same id. That entry fills the context window, output limit, image input, and whether the model reasons. It has no prices or effort values, so cost stays unknown and, for reasoning models, the picker keeps every level. Set `catalog` or the fields above in `models.toml` to fill the rest.

Image input comes from models.dev `modalities.input`. When the catalog lists a model as text-only, Rho replaces images from `read_file`, MCP tools, and attachments with a short text note before sending the request. Models with no catalog row still receive images. Set `image_input` to override the catalog:

```toml
[models."cliproxyapi/my-text-model"]
image_input = false
```

## Internal agent models

Rho uses reserved internal agents for session titles, the [`advisor`](/configuration/advisor-mode) tool, Auto permission classification, and [compaction summaries](/configuration/compaction#summarizer-model). Most roles follow the active conversation provider, model, and auth. Run `/agents`, select the role, and press Enter to choose a model. **Use conversation model** removes that role's override. Changes apply to the next invocation and save at once.

`advisor` and `permission-classifier` have no default and no conversation-model fallback. Advisor mode stays inactive until a model is chosen. Interactive startup opens the classifier picker for Auto when no model is set; cancelling it falls back to Supervised. The classifier defaults to low reasoning when a model is first selected.

The advisor picker also lists `claude-code/…` rows when `claude-agent-acp` is installed. Choosing one runs the advisor on [Claude Code](/configuration/advisor-mode#claude-code-as-the-advisor). When the advisor model supports configurable reasoning, Rho carries the previous level, or the advisor default, onto the new model.

`/config` → **Agent** exposes advisor model, advisor reasoning, permission classifier model, and permission classifier reasoning.

```toml
[internal_agents.session-title]
provider = "openai"
model = "gpt-5.6-luna"
auth = "api-key"
```

Model aliases work here.

The same table sets the model of a read-only [plugin agent](/integrations/plugins#plugin-agent-models); `/agents` writes it when you pick one. An entry applies by ID to any Rho agent Rho does not reserve, so it also reaches an agent file of your own with that ID.

## Edit tool

`edit_tool` under `[behavior]` selects the file edit tool exposed to the model. It defaults to `auto`. Only one edit tool is registered at a time.

| Value | Exposed tool | Format |
| --- | --- | --- |
| `auto` | preferred for the active provider | Built-in catalog. Switches when the provider changes |
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
| all others | `str_replace` | Rho default when no first-party match is known |

Pinned values stay fixed across provider changes. A change from `/config` applies to new sessions. Auto also switches live when you change providers mid-session. Rho 3.0 removed the legacy `hashline` format. The [2.x upgrade](#upgrading-from-rho-2-x) rewrites `edit_tool = "hashline"` to `"str_replace"` at startup, globally and on a custom provider. Format details: [Edit formats](/tools-workspace/edit-format).

## Behavior

These keys live under `[behavior]`.

`enable_subagents` controls the `agent` and `agents` tools. It defaults to `true`. Set it to `false` to remove both tools and tell the model not to delegate. The change applies to the next session. `--no-subagents` does the same for one run.

`agent_concurrency` is the maximum number of delegated agents that may run at once, including background runs. It defaults to `10` and must stay between `1` and `64`. A change applies immediately: in-flight agents keep their slots, and queued agents start as capacity frees, in the order they joined the queue. The daemon shares one such budget across every session it hosts (see [Subagents](/daemon#subagents)). `RHO_AGENT_CONCURRENCY` is no longer read. Claude-cli runs also take a nested cap of 2 by default (`RHO_CLAUDE_AGENT_CONCURRENCY`), always `min(total, claude_cap)`. Cursor runs take only the global pool. There is no nested Cursor cap yet.

`inline_shell` selects the shell for `!` and `!!` in the TUI. It defaults to `bash` on macOS and Linux and `powershell` on Windows. See [inline shell](/inline-shell).

`workspace_rewind` enables native file-tool checkpoints and `/rewind`. It defaults to `true`. Toggle `/config` → **Agent** → **Workspace rewind**, or opt out explicitly:

```toml
[behavior]
workspace_rewind = false
```

Restart Rho after changing it. Checkpoints from the old experimental version lack pre-turn conversation boundaries and cannot be rewound; they are rejected before any files change.

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
- `only`: only `codemode` and `tool_search` are declared by default. Native siblings remain executable through script `call_tool`; top-level `tool_search` can explicitly load its matches for direct calls on the next model request.

`/codemode on|only` changes the next model request without rebuilding the runtime and saves the preference. Neither mode is a permission level: nested calls inherit `permission_mode`. MCP tools start deferred in both modes. Top-level `tool_search` loads only its returned matches, making them directly callable with full argument schemas on the next model request. Script-side `search_tools`, `list_tools`, and `describe_tool` remain read-only: they do not load tools, but scripts can call discovered tools immediately without loading direct schemas. See [Codemode](/tools-workspace/codemode) for the discovery and loading distinction.

Scripts call one tool with `call_tool(name, args)` or run independent calls concurrently with `call_tools([(name, args), ...])`, which returns results in order. Pass exact tool names and argument dictionaries. Results are native Starlark dictionaries with `"is_error"`, `"content"`, and `"data"` keys; use bracket indexing, not dot access. A batch runs up to 4 calls at once, the same width as a model-issued parallel tool batch, and the whole batch counts toward the 64-call limit per script before any call starts. Script `list_tools()` and `search_tools(query)` return `{name, description}` rows; `describe_tool(name)` adds the parameter and `data` schemas. A script that fails keeps its printed output and call log. See [Codemode](/tools-workspace/codemode) for the full guide.

## RTK

`behavior.rtk` enables built-in [RTK](/integrations/rtk) command rewriting when the `rtk` binary is available. It defaults to `true`. Set it to `false` to leave shell commands unchanged.

## Tool output limit

`output.max_output_bytes` is how much output Rho keeps from [tool](/tools-workspace) calls such as command output, file reads, and loaded skills. It defaults to `64000` and must be at least `1`.

`max_tool_output_lines` in `~/.rho/clients/tui.toml` is how many wrapped screen rows of a tool card show inline before the TUI collapses the rest. It defaults to `10` and must be at least `1`. Delegated agent prompts use the same limit: they stream from the beginning, then keep a stable collapsed preview. Click the card or press `Ctrl+O` to expand the full prompt while it streams or after launch. Once the agent starts or finishes, run receipts and results appear before the prompt so they remain visible when collapsed.

`prompt_history_limit` in `~/.rho/clients/tui.toml` is how many sent composer prompts Rho keeps in `~/.rho/prompt-history.sqlite3` for up-arrow recall across sessions. `$RHO_HOME` moves that file. It defaults to `1000`. `0` disables persistence. It must be at most `10000`. `/config` → **Display** → **Prompt history limit** edits the cap; a lower cap deletes the oldest extras when it applies. **Clear prompt history** on the root page of `/config` clears it after asking. This is separate from `/new` and `/clear`, which reset the conversation, not composer recall.

## Update checks

`check_for_updates` in `~/.rho/clients/tui.toml` controls whether Rho checks the latest GitHub release at TUI startup. It defaults to `true`. A newer version shows an update notice in the session header and points to `rho update`.

## Questionnaire timeout

Questionnaires wait for an answer by default. To let an untouched form use explicit fallback answers, set a positive whole number of seconds:

```toml
[questionnaire]
timeout_seconds = 60
```

That value is an example, not a default. Omit `timeout_seconds` to disable automatic answers. Zero, negative values, fractions, and strings are invalid. In `/config` → **Agent** → **Questionnaire timeout**, enter positive seconds, or clear the field to remove it. A change applies to new sessions.

A [daemon](/daemon) session enforces the timeout itself, with or without a client attached, using the value it was opened with. A client holds the question once you start on the form, and it then waits for your answer ([Timeouts](/protocol#timeouts)).

What the TUI shows, and what does not count as consent: [Questionnaire fallbacks](/interactive-tui#questionnaire-fallbacks).
