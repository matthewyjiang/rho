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

The credential backend stays unset, and Rho uses the OS store until you choose. The first interactive `/login` asks. Set `RHO_CREDENTIAL_STORE=os|file` during install, or later run `rho credential-store set os` or `rho credential-store set file`, to skip that picker. `file` is a local file protected by filesystem permissions, not encrypted at rest. See [where credentials live](/authentication-and-models#where-credentials-live).

```bash
curl -fsSL https://matthewyjiang.github.io/rho/install.sh | RHO_CREDENTIAL_STORE=file sh
```

```powershell
$env:RHO_CREDENTIAL_STORE = "file"; irm https://matthewyjiang.github.io/rho/install.ps1 | iex
```

You can also install Rho with [Scoop](https://scoop.sh/) on Windows:

```powershell
scoop bucket add rho https://github.com/matthewyjiang/rho
scoop install rho
```

Or install the manifest directly:

```powershell
scoop install https://raw.githubusercontent.com/matthewyjiang/rho/main/bucket/rho.json
```

`rho update` recognizes custom Scoop roots from `SCOOP` and `SCOOP_GLOBAL`, as well as the default Scoop layout. It prints the Scoop update command, including `-g` for global installs, rather than replacing a Scoop-managed binary with the standalone installer.

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

You can also install Rho with [mise](https://mise.jdx.dev/) from its GitHub releases:

```bash
mise use -g 'github:matthewyjiang/rho[version_prefix=rho-coding-agent-v]'
```

`rho update` recognizes mise installs, including `MISE_DATA_DIR` and `MISE_INSTALLS_DIR` overrides, and uses `mise upgrade --global` (mise 2026.10.7 or newer) for that tool instead of overwriting mise's copy with the standalone installer. `rho uninstall` prints the matching `mise unuse -g` command, which also removes versions no other config needs.

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

## The daemon and services

Rho runs your sessions in a per-user [daemon](/daemon) that the first `rho` command starts. No install method adds a service or login item for it. The one optional service, a [systemd unit or LaunchAgent](/daemon#running-as-a-service) for `rho mcp serve --http`, is one you save yourself.

What an update does to a running daemon depends on how you installed Rho:

| Install method | After `rho update` |
| --- | --- |
| `install.sh`, a release archive on macOS or Linux, `cargo install`, or the Arch `PKGBUILD` | The daemon [restarts on the new binary](/daemon#upgrading-in-place) once no work would be cut short. `rho update` reports the restart or what the daemon waits for. |
| mise | `rho update` stops an idle daemon, and the next `rho` command starts the new one. If work is in progress, run `rho daemon restart` when it finishes. |
| Scoop, `install.ps1`, or a release archive on Windows | `rho update` stops an idle daemon and prints the update command to run. If work is in progress, run `rho daemon stop` when it finishes, then the command. |

An idle daemon has no queued, running, or waiting job, no login under way, and no session with its own background process, subagent, or workflow. The table applies too when you update with a package manager or the install script.

`rho uninstall` stops the daemon, which interrupts running turns, and deletes the saved `rho mcp serve --http` service (`rho-mcp.service` or `~/Library/LaunchAgents/dev.rho.mcp.plist`). `--dry-run` lists what it would remove. Remove a service saved under another name, or installed system-wide, yourself. See [`rho uninstall`](/automation-cli#rho-uninstall).

## Claude Code binary (optional)

Agent definitions with `runtime: claude-cli`, and a Claude Code advisor, need [`claude-agent-acp`](https://github.com/agentclientprotocol/claude-agent-acp) on `PATH`. It is the Agent Client Protocol adapter for Claude Code and bundles its own Claude Code CLI, so a separate `claude` install is not needed. Rho does not ship or install it. It needs Node.js or Bun:

```bash
npm install -g @agentclientprotocol/claude-agent-acp
# or
bun add -g @agentclientprotocol/claude-agent-acp
```

Confirm with `claude-agent-acp --cli --version`. Sign in from Rho with `/login claude-code` (terminal handoff; Claude Code stores the credential). Details: [Claude Code runtime sign-in](/authentication-and-models#claude-code-runtime-sign-in) and [Claude Code as a delegated runtime](/subagents/claude-cli).

## Cursor Agent binary (optional)

Agent definitions with `runtime: cursor` need the `cursor-agent` binary on `PATH`. Rho does not ship or install it. Install Cursor Agent from Cursor's docs, then confirm with `cursor-agent --version`. Sign in from Rho with `/login cursor` (terminal handoff; Cursor stores the credential in `~/.cursor`). Details: [Cursor Agent runtime sign-in](/authentication-and-models#cursor-agent-runtime-sign-in).

## Antigravity CLI (optional)

Agent definitions with `runtime: antigravity` need Google's `agy` CLI, version 1.5.0 or later, on `PATH`; Rho runs its built-in ACP server, `agy acp`. Rho does not ship or install it. Install it from [Google's releases](https://github.com/google-antigravity/antigravity-cli/releases) (update with `agy update`), then confirm with `agy --version`. Sign in from Rho with `/login antigravity` (terminal handoff to `agy acp --login`; agy stores the credential). Details: [Google Antigravity as a delegated runtime](/subagents/antigravity).

Earlier Rho releases downloaded a separate `agy_acp_server` into `~/.rho/runtimes/antigravity-acp/`. Rho no longer uses it; delete that directory to reclaim about 1 GB.

Next, configure [authentication and models](/authentication-and-models). To embed Rho as a headless Rust library instead of installing the CLI, start with [SDK installation and support](/sdk/installation).
