---
name: rho-config
description: Help configure rho, including the interactive /config browser, model and provider selection, credential storage, model aliases, permission mode, direct edits to config.toml, the global ~/.rho directory layout, and the user's global ~/.rho/AGENTS.md. Use when the user wants to change rho behavior, set a default model or provider, adjust reasoning, toggle compaction or web search, change permission mode, add global instructions or prompt templates, or understand which settings need a restart.
---

# Rho configuration

Help the user configure rho. Determine what they want to change, then guide them to the right mechanism. Prefer the interactive `/config` browser or direct config-file edits over telling the user to run one-off commands, unless a command is the natural fit.

## The global `~/.rho` directory

`~/.rho` is rho's home data and settings directory. `RHO_HOME` overrides its location; every path below moves under `$RHO_HOME` when it is set. What lives there:

- `config.toml` - persistent settings (see below).
- `AGENTS.md` - your global instructions, applied to every session (see below).
- `agents/` - user-defined agents (for example `~/.rho/agents/planner.md`).
- `skills/` - skills available only to rho.
- `prompts/` - prompt template files; each filename becomes a slash command.
- `hooks.toml` - user hooks, always eligible.
- `models.toml` - local model catalog overrides (or the file `RHO_MODELS_PATH` selects). Use this to set a per-model `usable_context_window`, `supported_reasoning_levels`, or `catalog` slug.
- `credentials/` - secrets when the `file` credential backend is selected (`credentials/secrets.json`).
- `sessions/` - saved session transcripts and workspace keys.
- `subagents/` - global delegated runs.
- `usage.sqlite3` - the usage ledger.
- `prompt-history.sqlite3` - sent composer prompts for up-arrow recall across sessions.
- `cache/`, `web-access/`, `workflows/` - cached and runtime data; do not edit these by hand.

`RHO_HOME` also moves the usage ledger and other data roots. Credentials and the ledger live under `$RHO_HOME` too.

## The global `AGENTS.md`

`~/.rho/AGENTS.md` holds instructions that apply to every rho session regardless of project. It is loaded as a project-instruction file before any project `AGENTS.md`, so project files appear later and take precedence on conflict. Edit it to set house rules that should always apply: prose style, commit conventions, safety boundaries, build hygiene, or any standing policy. Keep it focused; everything in it is injected into the system prompt for every session.

To add or change global instructions, edit `~/.rho/AGENTS.md` directly with the file tools, or open it for the user and make the edit. Offer a concrete draft when the user describes a rule in prose. Project `AGENTS.md` files live at `<project>/AGENTS.md`; the global file sits in the home directory, not in any project.

## Where settings live

Rho stores persistent config at `~/.rho/config.toml` by default. `RHO_HOME` overrides the config directory; `rho --config <path>` loads and saves a specific file. Restart-only settings are the values the running process used at startup, so a config edit does not take effect until rho starts again. Some settings apply live; say which one you are recommending.

## The four ways to configure

1. **Interactive TUI**: `/config` opens a category browser (Models, Appearance, Agent behavior, Context & limits, Tools, Providers). Type to filter, press `enter` to open a category, press `space` to toggle an on/off setting, and `esc` to return. `/login`, `/logout`, `/model`, and `/agents` are direct shortcuts. This is the easiest path for a user already in the TUI.
2. **Command line**: `--provider`, `--model`, `--auth`, and `--reasoning` update the config file and become the future default. `rho credential-store` shows or sets the credential backend.
3. **Direct file edit**: edit `~/.rho/config.toml` by hand. Group settings by purpose. Use this for settings the TUI does not expose, such as `[model.aliases]`, `[internal_agents]` overrides, `[prompt_templates]`, `[keybindings]`, `[providers.ollama].base_url`, and `[providers.custom.<name>].base_url` / `catalog`. Per-model context windows, reasoning lists, and catalog rematches live in `~/.rho/models.toml`, not `config.toml`.
4. **Environment**: `RHO_CREDENTIAL_STORE=os|file` overrides the saved credential backend; `RHO_MODELS_PATH` selects a custom models file; `RHO_TRUST_PROJECT_AGENTS=1` and `RHO_TRUST_PROJECT_HOOKS=1` trust project agent definitions and hooks.

## Inspect the current live config

Use the read-only `rho` tool with action `config` to see the sanitized live configuration (model, provider, reasoning, compaction, edit tool, web search, update checks, subagents, rtk, output limits). It excludes credentials and user content. It reports the values the running process uses; restart-only settings may differ from what is saved for the next startup.

## Common tasks

- **Set the default model or provider**: use `/model provider/model` in the TUI, or `rho --provider <provider> --model <model> --auth <auth>`. For the exact `--provider`/`--auth`/`--model` combination a provider expects, check the provider page.
- **Change reasoning**: cycle `reasoning` (`off`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`) in `/config` under Models, or set it in config. `reasoning` applies to the current session. Change `show_reasoning_output`, `zen_mode`, and `theme` under Appearance; those apply immediately when changed through `/config`. Direct configuration-file edits take effect on the next startup.
- **Manage credentials**: `/login [provider]` and `/logout [provider]`. The credential backend is `behavior.credential_store` (`os` or `file`). When unset, rho asks where to store secrets at first login.
- **Model aliases**: define `[model.aliases]` in config and reference them with an `@` prefix, for example `model = "@deep"`. Alias values must be concrete models and cannot begin with `@`. Update the alias table in one place to change the model everywhere it is referenced.
- **Per-model context window**: edit `~/.rho/models.toml` (not `config.toml`). Example: `[models."openai-codex/gpt-5.6-sol"]` with `usable_context_window = 272000` to cap, or a larger value to raise. Restart rho or switch models after editing.
- **Internal agent models**: use `/agents`, select `session-title` or `goal-judge`, and press `enter` to choose a model, or edit `[internal_agents.<role>]` in config. Pick **Use conversation model** to remove an override.
- **Permission mode**: `permission_mode` must be `bypass`, `auto`, `allow_edits`, `plan`, or `supervised`. `allow-edits` is accepted as an alias of `allow_edits`. Set it under Agent behavior in `/config`, or in config. `bypass` allows every capability. `auto` uses the same write and process gate as `allow_edits` and classifies only the requests that gate does not allow. `allow_edits` allows in-workspace writes to git-tracked, non-symlink files and later writes to a path already allowed this session. Untracked and gitignored paths, writes outside the workspace, and process execution still ask first. `plan` denies file writes and process execution. `supervised` asks before file writes and process execution. The change applies before the next turn and clears session approvals.
- **Auto compaction**: under Context & limits in `/config`. `compact_target_percent` must stay below `compact_threshold_percent`; values at or above the threshold are clamped.
- **Web search**: under Tools in `/config`. `hosted` enables provider-hosted search; `provider` selects the backup backend (`auto`, `openai`, `exa`, `brave`, `disabled`). Set `hosted` to `false` and `provider` to `disabled` to disable search entirely.
- **Edit tool**: under Tools in `/config`, choose `auto`, `hashline`, `apply_patch`, or `str_replace`. Exactly one edit schema is exposed under its own tool name (`edit`, `apply_patch`, or `str_replace`). `auto` keeps that preference in config and advertises the preferred format for the active provider, switching live when the provider changes. Prefer `auto` so models get the edit tool their first-party harness trained them on (Codex → `apply_patch`, Anthropic/xAI → `str_replace`, otherwise Rho `hashline`). Pinned formats stay fixed. The change applies before the next turn: the tool list rebuilds and the session gets a short notice with the new tool schema. It cannot change while a model turn is running.
- **Prompt templates**: add a file under `~/.rho/prompts/` or `<project>/.rho/prompts/`, or define `[prompt_templates]` inline in config. The filename or key becomes the slash command. Restart rho after adding or editing templates.
- **Global instructions**: to change rules that apply to every session, edit `~/.rho/AGENTS.md` (see above).
- **Keybindings**: edit `[keybindings]` in config. Values use `+`-separated modifiers and keys. Keybinding changes take effect at startup.

## Applying a change

State which mechanism you recommend and, when relevant, when it takes effect. Settings that apply to the current session or the next turn need no restart, including `advisor_mode` and `edit_tool`; `enable_subagents`, web search hosted state, templates, and keybindings apply on the next session or at startup. When the user edits config directly, tell them a restart may be required and offer to check whether the setting is restart-only.
