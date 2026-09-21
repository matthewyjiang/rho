# Model prompts

Parent: [Configuration](/configuration).

Markdown files in `~/.rho/model-prompts/` add or replace Rho's behavioral instructions for one provider and model. This is separate from [prompt templates](/configuration#prompt-templates), which expand reusable user messages.

## Edit from the CLI

```bash
rho model-prompt edit
rho model-prompt edit --provider openai-codex
rho model-prompt edit --provider openai-codex --model gpt-6-astra
rho model-prompt edit --model @local
```

Without `--model`, the command opens a searchable picker. It does not edit the configured model automatically. Type to filter, Enter to edit, Esc or Ctrl+C to cancel. `--provider` narrows the list. The picker uses locally known models, including cached provider models, and does not require login or a model-list fetch. Pass an exact `--model` or `@alias` to skip the picker. Non-interactive callers must do that. Provider and model flags may go before `model-prompt`, but do not pass the same flag in both places.

The command uses `$VISUAL`, then `$EDITOR`. Editor arguments work, for example `EDITOR='code --wait'`. The editor must wait until you finish. Rho validates and saves when the process exits. The command works offline. It does not start a session, read provider credentials, or change the configured model. `rho --config /path/to/config.toml model-prompt edit` selects another config file.

Rho finds an existing file by its frontmatter, even if you renamed it. If none matches, it creates a draft with the resolved provider and model and `mode: append`. A valid save writes `<provider>_<model>.md`. Filename characters other than ASCII letters, numbers, `.`, `-`, and `_` become `-`, with consecutive replacements collapsed. A leading `.` gets an `_` prefix. Occupied names get `-2.md`, `-3.md`, and so on.

Edits use a private draft and an atomic save. An unchanged draft cancels creation or leaves the existing file alone. Invalid edits, a changed provider or model, and editor failures keep the draft and report its recovery path. Rho refuses to overwrite a file changed during editing, and refuses read-only or symlink targets. A running session keeps its loaded instructions until the next reload. There is no TUI command for this.

## File format

Identity comes from required YAML frontmatter. Filenames are arbitrary.

```markdown
---
provider: openai-codex
model: gpt-6-astra
mode: append
---
Prefer direct implementation over extended planning.
When a tool fails, inspect the error before retrying.
```

`provider` and `model` must match resolved IDs, not a display name or alias. `mode` defaults to `append`:

- `append` puts the body after Rho's default behavioral prompt, before runtime and project instructions.
- `replace` substitutes the body for that default behavioral block. It does not remove tool contracts, working directory, model identity, rendering guidance, `AGENTS.md`, skills, MCP instructions, or additional agent instructions. It does not change permission enforcement.

This directory is global only. Rho does not load project-local model prompts, recurse into subdirectories, or apply wildcards or provider-wide inheritance. Every `.md` file needs valid frontmatter and a nonempty body. Unknown fields or modes, unreadable files, and duplicate provider/model pairs are errors, including errors in files for other models. A missing directory or no matching file uses the default prompt.

## When Rho reloads

Rho reads these files at startup, `/new`, session resume, conversation-tree branch selection, and model switches. Not on every turn. A switch prepares the target prompt before changing the active model. An invalid catalog leaves the previous model and prompt active. An in-progress turn finishes before an interactive switch takes effect. Recovery after a failed save restores saved state without reloading prompt files.

The next request uses the target model's prompt. Rho removes the previous model's custom instructions rather than appending another patch. Conversation messages and tool results remain, so a switch is not a clean behavioral reset. Switching back reads that model's file again. There is no file watcher.

Prompt-source diagnostics name the selected file and whether it appends or replaces. Session snapshots record its display path, mode, SHA-256 fingerprint, and source byte accounting. Resume uses current file contents and reports changes from a recorded fingerprint.

Failed-save recovery restores the assembled system text from saved history, including provenance and source accounting. It also rebinds the advisor's executor prompt. Recovery does not need the original prompt file. Older snapshots without source accounting clear that diagnostic list rather than showing sources from the abandoned prompt. Launch-owned whole-prompt replacements and `--no-system-prompt` stay in force for new conversations after recovery.

These rules apply to interactive, headless, ACP, and native Rho subagent sessions that use normal prompt assembly. Each subagent matches its own resolved model. `--no-system-prompt` and agent definitions with an explicit whole-prompt replacement bypass model prompt files. Summarization, title generation, and delegated external CLI runtimes keep their own prompt behavior.
