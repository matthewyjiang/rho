# Rho

Rho is a lightweight agent harness inspired by Pi, built in Rust to stay fast and memory-efficient.

## Install

Recommended install path on macOS and Linux:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://raw.githubusercontent.com/matthewyjiang/rho/main/scripts/install.sh | sh
```

On Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/matthewyjiang/rho/main/scripts/install.ps1 | iex
```

Or with Scoop:

```powershell
scoop bucket add rho https://github.com/matthewyjiang/rho
scoop install rho
```

Or install from crates.io with Cargo:

```bash
cargo install rho-coding-agent
```

## Usage

```bash
rho
```

For one-off prompts:

```bash
rho run "summarize this repository"
```

### Inline shell

Prefix TUI input with `!` to run a command locally and add its command, output, and exit status to model context. Use `!!` to run it without adding anything to context. The composer uses distinct colors and labels for the two modes.

Rho defaults to Bash on Unix-like systems and PowerShell on Windows. Choose another detected shell under `/config` -> **Inline shell**.

## Docs

See the docs site: <https://matthewyjiang.github.io/rho/>

## Development

```bash
cargo build
cargo test
```
