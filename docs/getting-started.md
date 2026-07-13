# Getting started

Rho is a lightweight agent harness for interactive coding sessions and one-shot terminal prompts.

## Quick path

1. [Install Rho](/installation).
2. Set up [authentication and models](/authentication-and-models).
3. Run `rho` to open the [interactive TUI](/interactive-tui).
4. Read [tools and workspace](/tools-workspace) to understand how Rho reads, edits, and runs commands in your project, including its security and workspace boundaries.

## First successful run

For an interactive session, start Rho and authenticate from the command palette:

```text
rho
/login openai
/model openai/gpt-5.6-sol
```

For a one-shot API-key workflow, keep the key out of configuration and provide it through the provider's environment override:

```bash
OPENAI_API_KEY=... rho run "summarize this repository"
```

The active model and provider are saved when you choose them through `/model` or pass the corresponding CLI flags. See [authentication and models](/authentication-and-models) for other providers and auth modes.

## Choose a workflow

- Use the [interactive TUI](/interactive-tui) when you want an ongoing session with streaming output, tool calls, slash commands, and [session resume](/sessions).
- Use [automation and CLI](/automation-cli) when you want a single answer for a script, hook, alias, pipeline, or CI job.

## After the first run

Rho stores persistent [configuration](/configuration) in `~/.rho/config.toml`. The [model catalog](/authentication-and-models#selecting-models) and `/model` command can update the active provider and model from inside the [interactive TUI](/interactive-tui#commands).

For local project work, see [development](/development). If authentication, model selection, or environment setup fails, run `/doctor` in the TUI; use `/info` to inspect the active selection and `/limits` to inspect supported OAuth usage windows.
