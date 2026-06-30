# Installation

Install the latest prebuilt Rho binary on macOS and Linux:

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

The macOS and Linux installer writes to `$HOME/.local/bin` by default. The Windows installer writes to `%LOCALAPPDATA%\Programs\rho\bin` and adds that directory to your user `PATH`.

To use a different directory, set `RHO_INSTALL_DIR`:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://raw.githubusercontent.com/matthewyjiang/rho/main/scripts/install.sh | RHO_INSTALL_DIR=/usr/local/bin sh
```

```powershell
$env:RHO_INSTALL_DIR = "$env:LOCALAPPDATA\Programs\rho\bin"
$installer = Join-Path ([IO.Path]::GetTempPath()) "rho-install.ps1"
Invoke-WebRequest -Uri "https://raw.githubusercontent.com/matthewyjiang/rho/main/scripts/install.ps1" -OutFile $installer
& $installer
Remove-Item $installer
```

To install a specific release, set `RHO_VERSION`. Accepted forms include `v0.9.0`, `0.9.0`, and the full release tag `rho-coding-agent-v0.9.0`:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://raw.githubusercontent.com/matthewyjiang/rho/main/scripts/install.sh | RHO_VERSION=v0.9.0 sh
```

```powershell
$env:RHO_VERSION = "v0.9.0"
$installer = Join-Path ([IO.Path]::GetTempPath()) "rho-install.ps1"
Invoke-WebRequest -Uri "https://raw.githubusercontent.com/matthewyjiang/rho/main/scripts/install.ps1" -OutFile $installer
& $installer
Remove-Item $installer
```

If a prebuilt binary is not available for your platform, install from crates.io with Cargo:

```bash
cargo install rho-coding-agent
```

Run Rho directly:

```bash
rho
```

If Cargo's bin directory is not on your `PATH`, add it before running the [interactive TUI](/interactive-tui) or [automation commands](/automation-cli):

```bash
export PATH="$HOME/.cargo/bin:$PATH"
```

Next, configure [authentication and models](/authentication-and-models).
