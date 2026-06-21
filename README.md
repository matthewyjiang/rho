# Rho

Rho is a tiny YOLO coding agent harness for Rust.

## Install

From this repo:

```bash
cargo install --path .
```

Then run it directly:

```bash
rho
```

If Cargo's bin directory is not on your `PATH`, add it:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
```

## Usage

With an OpenAI API key:

```bash
export OPENAI_API_KEY=...
rho --model gpt-5.5
```

Or use Codex OAuth from an existing Codex CLI login:

```bash
codex login
rho --auth codex --model gpt-5.5
```

Rho reads `CODEX_ACCESS_TOKEN` or `~/.codex/auth.json`. If the default API base is unchanged, Codex auth uses `https://chatgpt.com/backend-api/codex/responses`.

Codex reasoning summaries are configurable in `~/.rho/config.toml`:

```toml
reasoning_effort = "medium"  # set to "none" to omit
reasoning_summary = "auto"   # set to "none" to omit
```

Running `rho` opens an inline terminal interface. Finalized conversation output is written into normal terminal scrollback, while the active assistant response and composer stay inline below it. Assistant responses stream as tokens arrive, and reasoning deltas are shown when the model provides them. Useful keys:

```text
enter    send the current prompt
shift-enter insert a newline
alt-enter insert a newline fallback
paste    insert pasted text, including newlines, without submitting
arrows   move within the prompt
alt-arrows move by word
alt-backspace delete previous word
home/end jump to prompt start/end
esc      interrupt a streaming response
wheel    scroll terminal history
ctrl-r   reset conversation history
ctrl-c   clear the input line, press twice to exit
```

For non-interactive automation, use `rho run`:

```bash
rho run "summarize this repository"
printf 'summarize this repository' | rho run --stdin
rho run "review this diff" --stdin < diff.txt
```

The CLI is intentionally non-interactive. The TUI is the main mode of use.

## Config

Rho stores its persistent config at `~/.rho/config.toml` by default. Passing `--provider` or `--model` updates that file and makes the choice the future default.

```toml
provider = "openai"
model = "gpt-5.5"
max_output_bytes = 12000
auth = "api-key" # or "codex"
```

You can still load and save a specific config file with:

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

Rho v0.1 intentionally uses one OpenAI-compatible provider and no approvals, permissions, policies, allowlists, denylists, or sandboxing.
