# Getting started

Rho is a lightweight agent harness for interactive coding sessions and one-shot terminal prompts.

## Quick path

1. [Install Rho](/installation).
2. Set up [authentication and models](/authentication-and-models).
3. Run `rho` to open the [interactive TUI](/interactive-tui).
4. Use [tools and workspace](/tools-workspace) behavior to understand how Rho reads, edits, and runs commands in your project.

## Choose a workflow

- Use the [interactive TUI](/interactive-tui) when you want an ongoing session with streaming output, tool calls, slash commands, and [session resume](/sessions).
- Use [automation and CLI](/automation-cli) when you want a single answer for a script, hook, alias, pipeline, or CI job.

## After the first run

Rho stores persistent [configuration](/configuration) in `~/.rho/config.toml`. The [model catalog](/authentication-and-models#selecting-models) and `/model` command can update the active provider and model from inside the [interactive TUI](/interactive-tui#commands).

For local project work, see [development](/development).
