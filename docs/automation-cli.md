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
  run     Run one non-interactive automation prompt and print the final answer
  update  Update rho using the detected installation method
  help    Print this message or the help of the given subcommand(s)

Options:
      --provider <PROVIDER>
      --model <MODEL>
      --config <CONFIG>
      --auth <AUTH>            [possible values: api-key, codex, anthropic-api-key, github-copilot]
      --no-system-prompt       Do not send rho's system prompt, including AGENTS.md and skill context
      --no-tools               Do not expose any tools to the model
      --reasoning <REASONING>  Override reasoning level: off, minimal, low, medium, high, xhigh, or max
  -R, --resume [<ID>]          Resume an existing session by UUID or UUID prefix. Omit the ID to choose from a picker
  -h, --help                   Print help
```

Provider, model, auth, and reasoning override options affect [authentication and models](/authentication-and-models) and persistent [configuration](/configuration). For GitHub Copilot automation, use `/login github-copilot` in the TUI first or provide `GITHUB_COPILOT_TOKEN` as a bearer-token override, then select models as `github-copilot/<model>`.

`--no-system-prompt` and `--no-tools` only affect the current run and are not written to config.

## `rho update`

`rho update` checks the latest GitHub release and dispatches to the detected installation method:

- Cargo installs run `cargo install rho-coding-agent --locked`, adding `--root <cargo-root>` when the current executable is from a non-default Cargo install root.
- pacman installs run `sudo pacman -Syu rho-coding-agent` so pacman can refresh package databases and prompt for your password.
- install-script installs download the official install script to a temporary file and run it with `RHO_INSTALL_DIR` set to the current executable directory.

Set `RHO_INSTALL_METHOD` to `cargo`, `pacman`, or `script` to override detection.

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
