# Automation and CLI

Use `rho run` for non-interactive automation. It sends one prompt, prints the final answer to stdout, and exits.

```bash
rho run "summarize this repository"
printf 'summarize this repository' | rho run --stdin
rho run "review this diff" --stdin < diff.txt
```

Use the [interactive TUI](/interactive-tui) when you want an ongoing session. Use `rho run` when you want a single answer for a script, hook, alias, pipeline, or CI job.

## CLI reference

```text
Usage: rho [OPTIONS] [COMMAND]

Commands:
  run   Run one non-interactive automation prompt and print the final answer
  help  Print this message or the help of the given subcommand(s)

Options:
      --provider <PROVIDER>
      --model <MODEL>
      --config <CONFIG>
      --auth <AUTH>              [possible values: api-key, codex, anthropic-api-key, github-copilot]
      --no-system-prompt         Do not send rho's system prompt, including AGENTS.md and skill context
      --no-tools                 Do not expose any tools to the model
      --reasoning <REASONING>    Override reasoning level [possible values: off, minimal, low, medium, high, xhigh]
  -R, --resume <RESUME>          Resume an existing session by UUID or UUID prefix
  -h, --help                     Print help
```

Provider, model, auth, and reasoning override options affect [authentication and models](/authentication-and-models) and persistent [configuration](/configuration). For GitHub Copilot automation, use `/login github-copilot` in the TUI first or provide `GITHUB_COPILOT_TOKEN` as a bearer-token override, then select models as `github-copilot/<model>`.

`--no-system-prompt` and `--no-tools` only affect the current run and are not written to config.

## `rho run`

`rho run` accepts prompt text as arguments and can append stdin with `--stdin`:

```text
Usage: rho run [OPTIONS] [PROMPT]...

Arguments:
  [PROMPT]...  Prompt text to send to the agent

Options:
      --stdin  Read additional prompt text from stdin
  -h, --help   Print help
```

`rho run` uses the same [tools and workspace](/tools-workspace) behavior as the TUI when tools are enabled. It runs in the current working directory and can read files, write files, edit files, and run shell commands when the model chooses those tools.

Use `--no-tools` to remove tool access and send only the raw prompt and model response behavior.
