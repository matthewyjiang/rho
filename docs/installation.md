# Installation

Install the latest prebuilt Rho binary on macOS and Linux:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://raw.githubusercontent.com/matthewyjiang/rho/main/scripts/install.sh | sh
```

On Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/matthewyjiang/rho/main/scripts/install.ps1 | iex
```

The macOS and Linux installer writes to `$HOME/.local/bin` by default. The Windows installer writes to `%LOCALAPPDATA%\Programs\rho\bin` and adds that directory to your user `PATH`.

After installing the binary, an interactive script install checks whether the native OS credential store works and asks which credential backend to use. The OS store is recommended when available. If it is unavailable, you can opt into a local file protected by filesystem permissions but not encrypted at rest. When no choice is saved, Rho uses the OS store only.

Set `RHO_CREDENTIAL_STORE=os|file` to make this choice without a prompt:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://raw.githubusercontent.com/matthewyjiang/rho/main/scripts/install.sh | RHO_CREDENTIAL_STORE=file sh
```

```powershell
$env:RHO_CREDENTIAL_STORE = "file"; irm https://raw.githubusercontent.com/matthewyjiang/rho/main/scripts/install.ps1 | iex
```

For more detail, see [where credentials live](/authentication-and-models#where-credentials-live).

You can also install Rho with [Scoop](https://scoop.sh/) on Windows:

```powershell
scoop bucket add rho https://github.com/matthewyjiang/rho
scoop install rho
```

Or install the manifest directly:

```powershell
scoop install https://raw.githubusercontent.com/matthewyjiang/rho/main/bucket/rho.json
```

To use a different directory, set `RHO_INSTALL_DIR`:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://raw.githubusercontent.com/matthewyjiang/rho/main/scripts/install.sh | RHO_INSTALL_DIR=/usr/local/bin sh
```

```powershell
$env:RHO_INSTALL_DIR = "$env:LOCALAPPDATA\Programs\rho\bin"; irm https://raw.githubusercontent.com/matthewyjiang/rho/main/scripts/install.ps1 | iex
```

To install a specific release, set `RHO_VERSION`. Accepted forms include `v0.9.0`, `0.9.0`, and the full release tag `rho-coding-agent-v0.9.0`:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://raw.githubusercontent.com/matthewyjiang/rho/main/scripts/install.sh | RHO_VERSION=v0.9.0 sh
```

```powershell
$env:RHO_VERSION = "v0.9.0"; irm https://raw.githubusercontent.com/matthewyjiang/rho/main/scripts/install.ps1 | iex
```

If your platform has no prebuilt binary, install from crates.io with Cargo:

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

Next, configure [authentication and models](/authentication-and-models). To embed Rho as a headless Rust library instead of installing the CLI, start with [SDK installation and support](/sdk/installation).
