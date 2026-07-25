# Installation

Install the latest prebuilt Rho binary on macOS and Linux:

```bash
curl -fsSL https://matthewyjiang.github.io/rho/install.sh | sh
```

On Windows PowerShell:

```powershell
irm https://matthewyjiang.github.io/rho/install.ps1 | iex
```

The macOS and Linux installer writes to `$HOME/.local/bin` by default. The Windows installer writes to `%LOCALAPPDATA%\Programs\rho\bin` and adds that directory to your user `PATH`.

After installing the binary, Rho leaves the credential backend unset and uses the OS store by default. The first interactive `/login` asks which backend to use. The OS store is recommended when available. You can also opt into a local file protected by filesystem permissions but not encrypted at rest.

Set `RHO_CREDENTIAL_STORE=os|file` during install, or run `rho credential-store set os` or `rho credential-store set file`, to make this choice without the login picker:

```bash
curl -fsSL https://matthewyjiang.github.io/rho/install.sh | RHO_CREDENTIAL_STORE=file sh
```

```powershell
$env:RHO_CREDENTIAL_STORE = "file"; irm https://matthewyjiang.github.io/rho/install.ps1 | iex
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
curl -fsSL https://matthewyjiang.github.io/rho/install.sh | RHO_INSTALL_DIR=/usr/local/bin sh
```

```powershell
$env:RHO_INSTALL_DIR = "$env:LOCALAPPDATA\Programs\rho\bin"; irm https://matthewyjiang.github.io/rho/install.ps1 | iex
```

To install a specific release, set `RHO_VERSION`. Accepted forms include `v0.9.0`, `0.9.0`, and the full release tag `rho-coding-agent-v0.9.0`:

```bash
curl -fsSL https://matthewyjiang.github.io/rho/install.sh | RHO_VERSION=v0.9.0 sh
```

```powershell
$env:RHO_VERSION = "v0.9.0"; irm https://matthewyjiang.github.io/rho/install.ps1 | iex
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

## Claude Code binary (optional)

Agent definitions with `runtime: claude-cli` need the `claude` binary on `PATH`. Rho does not ship or install it. Install Claude Code from Anthropic's docs ([setup](https://code.claude.com/docs/en/setup)), for example:

```bash
curl -fsSL https://claude.ai/install.sh | bash
```

```powershell
irm https://claude.ai/install.ps1 | iex
```

Confirm with `claude --version`. Sign in from Rho with `/login claude-code` (terminal handoff; Claude Code stores the credential). Details: [Claude Code runtime sign-in](/authentication-and-models#claude-code-runtime-sign-in) and [Claude Code as a delegated runtime](/subagents#claude-code-as-a-delegated-runtime).

Next, configure [authentication and models](/authentication-and-models). To embed Rho as a headless Rust library instead of installing the CLI, start with [SDK installation and support](/sdk/installation).
