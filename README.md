# Rho

Rho is a lightweight agent harness inspired by Pi, built in Rust to stay fast and memory-efficient.

## Install

Recommended install path on macOS and Linux:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://raw.githubusercontent.com/matthewyjiang/rho/main/scripts/install.sh | sh
```

On Windows PowerShell:

```powershell
$installer = Join-Path ([IO.Path]::GetTempPath()) "rho-install.ps1"
Invoke-WebRequest -Uri "https://raw.githubusercontent.com/matthewyjiang/rho/main/scripts/install.ps1" -OutFile $installer
& $installer
Remove-Item $installer
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

## Docs

See the docs site: <https://matthewyjiang.github.io/rho/>

## Development

```bash
cargo build
cargo test
```
