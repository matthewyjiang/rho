# Automation and CLI

Use `rho run` for non-interactive automation. It sends one prompt, prints the final answer to stdout, and exits.

```bash
rho run "summarize this repository"
printf 'summarize this repository' | rho run --stdin
rho run "review this diff" --stdin < diff.txt
```

Use the [interactive TUI](/interactive-tui) when you want an ongoing session. Use `rho run` when you want a single answer for a script, hook, alias, pipeline, or CI job.

## CLI reference

Rho accepts global options before an optional subcommand. Provider, model, auth, and reasoning selections update the saved defaults; security and session-control switches apply only to the current invocation.

### Global options

| Option | Description |
| --- | --- |
| `--provider <PROVIDER>` | Select the provider for the current session or run. |
| `--model <MODEL>` | Select a model. A provider/model name can be used when switching providers. |
| `--config <CONFIG>` | Read and save configuration at a specific path instead of `~/.rho/config.toml`. |
| `--auth <AUTH>` | Select an auth mode: `api-key`, `codex`, `anthropic-api-key`, `github-copilot`, `xai-api-key`, `xai-oauth`, `moonshot-api-key`, `openrouter-api-key`, or `kimi-oauth`. |
| `--reasoning <LEVEL>` | Select a reasoning level: `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, or `max`. |
| `--no-system-prompt` | Do not send Rho's system prompt, including `AGENTS.md` and skill context. Current invocation only. |
| `--no-tools` | Do not expose tools to the model. Current invocation only. |
| `-R`, `--resume [<ID>]` | Resume a session by UUID or UUID prefix. Without an ID, open a picker. Interactive sessions only. |
| `-h`, `--help` | Show help for Rho or a subcommand. |

### Commands

| Command | Description |
| --- | --- |
| `rho` | Start an interactive TUI session in the current working directory. |
| `rho run [OPTIONS] [PROMPT]...` | Send one prompt, optionally append stdin, print the final answer, and exit. |
| `rho login <PROVIDER>` | Authenticate a provider from a browser or device-code flow. Add `--device-auth` for remote or headless sessions. |
| `rho update` | Update Rho using the detected installation method. |
| `rho help [COMMAND]` | Show help for Rho or a subcommand. |

Provider, model, auth, and reasoning options are described further in [authentication and models](/authentication-and-models) and [configuration](/configuration). For provider-specific automation caveats, see the [provider pages](/authentication-and-models#providers). For example, [GitHub Copilot](/providers/github-copilot#automation) needs a prior `/login` or a `GITHUB_COPILOT_TOKEN` override.

`--no-system-prompt` and `--no-tools` only affect the current invocation and are not written to config. `--resume` cannot be combined with a subcommand such as `run` or `update`.

## `rho login`

Log in to a provider from the command line. Browser-based providers open a local browser flow; use `--device-auth` on remote or headless systems:

```bash
rho login openai-codex
rho login openai-codex --device-auth
rho login xai-oauth --device-auth
```

API-key providers are usually easier to configure interactively with `/login` in the TUI or with their documented environment-variable override. See [authentication and models](/authentication-and-models) for provider-specific details.

## `rho update`

`rho update` checks the latest GitHub release and dispatches to the detected installation method:

- Cargo installs run `cargo install rho-coding-agent --locked`, adding `--root <cargo-root>` when the current executable is from a non-default Cargo install root.
- pacman installs run `sudo pacman -Sy mjiang-extras/rho-coding-agent` so pacman can refresh package databases and sync only `rho-coding-agent` from `mjiang-extras`, without performing a full system upgrade. Pacman may prompt for your password.
- Scoop installs show `scoop update; scoop update rho`, or `scoop update; scoop update -g rho` for global installs, so Scoop refreshes buckets before updating the package.
- install-script installs download the official install script to a temporary file and run it with `RHO_INSTALL_DIR` set to the current executable directory.

On Windows, `rho update` prints the detected update command instead of running it automatically.

Set `RHO_INSTALL_METHOD` to `cargo`, `pacman`, `scoop`, `scoop-global`, or `script` to override detection.

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

### Automation output and exit contract

For the 1.0 CLI contract, `rho run` writes exactly one final assistant answer and
a trailing newline to stdout. Reasoning, provider activity, tool lifecycle,
diagnostics, and errors never write to stdout. Actionable errors go to stderr.
This keeps command substitution, pipes, and redirected output stable.

A successful run exits with status 0. Provider, configuration, tool setup, and
output failures exit with status 1. SIGINT exits with 130 and SIGTERM exits with
143 after SDK shutdown and managed-process cleanup.

Rho 1.0 does not add a JSON Lines or other machine-readable event mode. The
versioned Rust `RunEvent` API is the machine-readable streaming contract. A
future CLI event protocol must be explicitly opted into, independently
versioned, and must not change default stdout behavior.
