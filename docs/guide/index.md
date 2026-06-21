# Guide

## Install

Install from this repository:

```bash
cargo install --path .
```

Then run Rho directly:

```bash
rho
```

If Cargo's bin directory is not on your `PATH`, add it:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
```

## Authentication

### OpenAI API key

Set an OpenAI API key and choose a model:

```bash
export OPENAI_API_KEY=...
rho --model gpt-5.5
```

### Codex OAuth

Rho can also use an existing Codex CLI login:

```bash
codex login
rho --auth codex --model gpt-5.5
```

Rho reads `CODEX_ACCESS_TOKEN` or `~/.codex/auth.json`. If the default API base is unchanged, Codex auth uses:

```text
https://chatgpt.com/backend-api/codex/responses
```

## Usage

### Interactive TUI

Running `rho` opens an inline terminal interface. Finalized conversation output is written into normal terminal scrollback, while the active assistant response and composer stay inline below it.

See [Interactive TUI](/interactive-tui) for current behavior, keybindings, and implementation notes.

### Automation mode

Use `rho run` for non-interactive automation:

```bash
rho run "summarize this repository"
printf 'summarize this repository' | rho run --stdin
rho run "review this diff" --stdin < diff.txt
```

Automation mode prints the final answer to stdout and exits.

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
      --auth <AUTH>          [possible values: api-key, codex]
  -h, --help                 Print help
```

`rho run` accepts prompt text as arguments and can append stdin with `--stdin`:

```text
Usage: rho run [OPTIONS] [PROMPT]...

Arguments:
  [PROMPT]...  Prompt text to send to the agent

Options:
      --stdin  Read additional prompt text from stdin
  -h, --help   Print help
```

## Config

Rho stores persistent config at `~/.rho/config.toml` by default. Passing `--provider`, `--model`, or `--auth` updates that file and makes the choice the future default.

```toml
provider = "openai"
model = "gpt-5.5"
max_output_bytes = 12000
auth = "api-key" # or "codex"
reasoning_effort = "medium" # set to "none" to omit
reasoning_summary = "auto"  # set to "none" to omit
```

You can load and save a specific config file with:

```bash
rho --config ~/.rho/config.toml
```

## Tools

Rho currently ships five compiled-in tools:

```text
list_dir
read_file
write_file
edit_file
bash
```

These tools can read and modify files and run shell commands in the working directory. File write results include a unified diff so the model and transcript can inspect what changed. Rho does not currently sandbox or prompt for approval before tool calls.

## Development

Build and check the project with Cargo:

```bash
cargo build
cargo test
```

Run the local binary without installing:

```bash
cargo run --
cargo run -- run "summarize this repository"
```
