# Automation and CLI

Use `rho run` for non-interactive automation. It sends one prompt and exits. By default, it prints the final answer to stdout. Add `--output jsonl` for a versioned event stream.

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
| `--auth <AUTH>` | Select an auth profile and its matching provider profile: `api-key`, `codex`, `anthropic-api-key`, `google-api-key`, `github-copilot`, `xai-api-key`, `xai-oauth`, `moonshot-api-key`, `openrouter-api-key`, `openrouter-oauth`, or `kimi-oauth`. |
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
| `rho credential-store probe [auto|os|file]` | Test a credential backend with a temporary secret. |
| `rho credential-store set <auto|os|file>` | Save the credential backend used by Rho. |
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
      --stdin                 Read additional prompt text from stdin
      --output <FORMAT>       Output format: text or jsonl [default: text]
      --max-steps <N>         Override the model-step limit for this run
      --timeout <DURATION>    Set a wall-clock limit, such as 30s or 20m
      --output-file <PATH>    Update a delegated-run status file during the run
  -h, --help                  Print help
```

`rho run` uses the same [tools and workspace](/tools-workspace) behavior as the TUI when tools are enabled. It runs in the current working directory and can read files, write files, edit files, and run shell commands when the model chooses those tools.

Use `--no-tools` to remove tool access and send only the raw prompt and model response behavior.

### Automation output

The default `--output text` contract has not changed: `rho run` writes one final
assistant answer and a trailing newline to stdout. Reasoning, provider activity,
tool lifecycle events, diagnostics, and errors stay off stdout. Actionable errors
go to stderr. This keeps command substitution, pipes, and redirected output
stable.

Use `--output jsonl` when a script needs progress or terminal state:

```bash
rho run --output jsonl --max-steps 12 --timeout 20m \
  "implement the issue"

# Read the authoritative final answer.
rho run --output jsonl "summarize this repository" \
  | jq -r 'select(.type == "run.completed") | .text'
```

JSONL mode writes one JSON object per physical line and flushes each object.
Every object has `schema_version` (currently `1`), a run-local monotonic `seq`,
and a stable `type`. The stream can contain these event types:

- `run.started`, with run and session IDs and the workspace path
- `assistant.text_delta` and `assistant.text_reset`
- `tool.started`, `tool.updated`, and `tool.finished`
- one final `run.completed`, `run.failed`, or `run.stopped`

Assistant deltas include an `attempt` number. A provider retry emits
`assistant.text_reset` before a new attempt starts. Delta boundaries can change
between releases, and retried text can be discarded. Use `run.completed.text`
as the final answer. `run.failed` and `run.stopped` may omit `text`.

Tool events omit arguments and raw output. Progress includes only safe, bounded
fields. Provider and fatal errors use Rho's existing sanitization. Assistant text
is free-form model output and can contain data from the workspace, so do not
send the JSONL stream to a system that should not receive that data.

`--output-file` has a separate contract. It updates the existing mutable status
artifact used for delegated runs, while `--output jsonl` writes an immutable
event stream to stdout. You can use both at once.

A broken stdout pipe cancels the run and starts normal tool, subagent, and
managed-process cleanup. A timeout starts after CLI and configuration validation.
Cleanup can finish shortly after the deadline.

### Exit status

Exit codes are part of the automation contract:

| Code | Meaning |
| ---: | --- |
| `0` | Normal model completion |
| `1` | Authentication, provider, tool-host, output, or another run failure |
| `2` | Invalid invocation or configuration |
| `124` | Timeout or model-step limit reached |
| `130` | SIGINT, after cleanup |
| `143` | SIGTERM, after cleanup |

The terminal JSONL event gives a more exact reason, such as `completed`,
`max_steps`, `timeout`, `interrupted`, `authentication`, `provider_error`,
`tool_host_error`, `configuration_error`, `output_error`, or `other_error`.
A failed tool call can still lead to a successful run if the model recovers.

For CI, save the stream as an artifact and use the process status for the main
result:

```yaml
- name: Run Rho
  shell: bash
  run: |
    set -o pipefail
    rho run --output jsonl --timeout 20m "review this change" \
      | tee rho-events.jsonl
- name: Upload Rho events
  if: always()
  uses: actions/upload-artifact@v4
  with:
    name: rho-events
    path: rho-events.jsonl
```
