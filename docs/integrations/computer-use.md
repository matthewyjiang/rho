# Computer use

Rho can interact with desktop apps through **Cua Driver**. Rho manages the connection and tool access; Cua Driver performs desktop automation. `/computer setup` can install a missing driver with your confirmation or connect an existing installation.

Computer use starts off. Enable it explicitly inside an interactive Rho session:

```text
/computer setup
/computer on
```

`/computer setup` detects the driver, offers a separately authorized installation if needed, then asks for session desktop access and verifies the connection. An existing driver skips installation. `/computer on` asks you to review the access scope and explicitly allow it for this session before connecting. Cancel is selected by default; cancelling does not start the driver or grant access. If the terminal clips the confirmation, Rho blocks the grant and asks you to enlarge it so the full disclosure is visible.

Connection runs in the background so the composer remains usable. Once connected, the model receives a `computer` tool for discovering and calling supported Cua operations. Use an image-capable model for visual tasks. A persistent indicator near the composer shows connecting, driver-connected, or access-off/disconnecting state, even as other status messages change.

## Access and privacy

Enabling computer use grants access to the **local desktop**, including signed-in apps and browsers. It is not a workspace sandbox or an isolated virtual desktop.

This is a session-wide grant. Rho does not ask for separate approval before each computer action, including in supervised mode. Cua's own permission policies and OS permissions still apply. Plan mode never permits this grant.

Captured images are sent to your configured model provider and retained in session history. Driver output, screen text, web pages, and clipboard contents are untrusted tool data, not instructions. Enable the feature only when that scope is appropriate for your task.

The grant lasts for the current interactive session. It is not saved in config or restored from conversation history. New sessions, session switches, and entering plan mode revoke it. Subagents, side chats, workflows, ACP sessions, and `rho run` do not inherit this managed connection. `--no-tools` and plan mode cannot enable it.

This is a Rho tool-registration boundary, not an OS sandbox. An independently configured MCP server or an agent with shell access may still invoke an installed automation program outside this integration.

## Commands

| Command | Effect |
| --- | --- |
| `/computer` or `/computer status` | Open the dashboard with session state, a direct revoke action, driver detection, and access details |
| `/computer setup` | Install a missing driver after confirmation, then configure session access and verify the connection |
| `/computer on` | Review and confirm desktop access for this session, then connect |
| `/computer off` or `/computer stop` | Cancel installation, revoke access, cancel pending connection or computer actions, and disconnect |
| `rho computer status [--json]` | Detect the executable without starting it or inspecting the desktop |
| `rho computer setup` | Show read-only guidance directing you to the interactive setup flow |

`/computer off` works during a model turn. It prevents later computer calls through that grant. It cannot retract input already handed to the OS, and an in-flight action may finish during driver cleanup. Status remains `closing (access revoked)` until cleanup completes; a new grant cannot start before that. The runtime removes the revoked tool at the next idle boundary, before another model request.

CLI status is a local installation check, not a query of another running Rho session. A successful MCP handshake also does not prove that the OS has granted capture or input permissions.

The interactive dashboard uses the same overlay as `/limits` and `/doctor`, without adding status output to the transcript. Session state and available actions appear before diagnostics and privacy details. Scroll with arrow keys, PgUp/PgDn, Home/End, or the mouse wheel; Enter or Esc closes it. It can be opened during a running turn, and closing it does not interrupt the model. Press `r` to revoke access immediately without another confirmation. Access still requires explicit confirmation through `/computer setup` or `/computer on`.

## Driver setup

Installation and desktop access are separate authorizations, both defaulting to Cancel. `/computer setup` never upgrades a detected driver. For a missing driver it offers to download and execute Cua's official installer. The confirmation and CLI setup guidance describe only your current platform:

| Platform | Installer | Managed installation locations |
| --- | --- | --- |
| Linux | `https://cua.ai/driver/install.sh` with Bash | `~/.cua-driver`, with a link in `~/.local/bin` |
| macOS | `https://cua.ai/driver/install.sh` with Bash | `~/.cua-driver` and `/Applications/CuaDriver.app`, with a link in `~/.local/bin` |
| Windows | `https://cua.ai/driver/install.ps1` with PowerShell | `%USERPROFILE%\.cua-driver`, with the executable in `%USERPROFILE%\.cua-driver\bin` |

Rho requests no PATH or shell-profile edits. On Windows it passes `-NoAutoStart`, but the upstream installer may still re-register an existing `cua-driver-serve` scheduled task and prompt for UAC elevation. Elevated work may run outside Rho's supervised job and continue after cancellation. Cancellation does not undo scheduled-task changes. The upstream installer may replace Cua files or stop old Cua daemons. You can instead install manually using [Cua Driver's setup instructions](https://cua.ai/docs/how-to-guides/driver/install).

Rho automatically disables Cua telemetry. Before the installer or any managed driver starts, it sets both `CUA_DRIVER_RS_TELEMETRY_ENABLED=false` and `CUA_TELEMETRY_ENABLED=false`, overriding caller opt-ins. After a successful installation, it runs the newly installed executable with `telemetry disable` to persist the opt-out. This command shares the installer's process-tree supervision, cancellation, and output log; a failure fails setup rather than reporting a completed installation. Failure or cancellation before this step completes can leave the saved preference unchanged. If a later `/computer setup` detects the driver, it skips installation and does not retry this persistence step. Run `cua-driver telemetry disable` manually to save the opt-out in that case. Rho still forces telemetry off for every managed connection.

For existing drivers, Rho applies the environment overrides when desktop access is confirmed. Detection does not execute the driver or change its saved preferences. The overrides do not reconfigure an already-running shared daemon or independently launched Cua clients. Telemetry opt-out does not disable Cua's separate GitHub update check; see Cua's [telemetry reference](https://cua.ai/docs/reference/cua-driver/telemetry).

Installation runs in the background with output retained in a private temporary log whose path appears in the transcript. The installer receives an allowlist of environment variables for platform paths, proxies, and certificates, plus Rho's forced telemetry opt-out, not model credentials or shell startup hooks. `/computer off` or dashboard `r` cancels its supervised process tree; session changes, entering plan mode, and shutdown also cancel pending installation. On Windows, this does not guarantee cancellation of elevated work launched outside the job. Cancellation does not roll back files already written. Rerun `/computer setup` to detect a completed installation or retry if the driver is still missing; detecting a driver does not repair an incomplete saved telemetry opt-out. Setup cannot start during a model turn or in plan mode.

After installation, Rho asks separately for desktop access. If you are typing or another overlay is open, it preserves that UI and asks you to continue with `/computer setup`. No persistent MCP configuration or desktop grant is written: Rho discovers the executable and manages the connection for this session.

Rho detects `cua-driver` in absolute directories on `PATH`, then the standard `~/.local/bin/cua-driver` installation. Empty and relative `PATH` entries are ignored so a repository-local program cannot be selected accidentally. On Windows, detection also checks `~/.cua-driver/bin` (the managed installation location) and `%LOCALAPPDATA%/Programs/Cua/cua-driver/bin`; the executable is `cua-driver.exe`. Rho starts its stdio connection using:

```sh
cua-driver mcp
```

Connection verification checks the MCP handshake and supported tools, not OS capture/input permissions. Rho does not change OS permissions or select unrestricted mode. Run `cua-driver doctor` for installation diagnostics. On macOS, use `cua-driver permissions status` after starting the daemon and grant Accessibility and Screen Recording in System Settings as needed; Cua's daemon-backed permission flow remains in charge. Outside the separately authorized installer, Rho does not stop a shared Cua daemon: closing its MCP connection does not close other clients' sessions.

On Linux, the driver child receives the available desktop connection variables such as `DISPLAY`, `WAYLAND_DISPLAY`, `XDG_RUNTIME_DIR`, `XAUTHORITY`, and `DBUS_SESSION_BUS_ADDRESS`. Other process environment filtering remains unchanged. A headless shell or missing OS permissions may allow discovery but prevent actual observation or input.

When Linux `DISPLAY` is unset or empty, `/computer on`, `/computer status`, and `rho computer status` warn that the X11 overlay cannot connect. CLI JSON includes a nullable `desktop_warning` field. This check does not rule out a Wayland session or verify desktop permissions. Launch Rho from the intended desktop session with its environment rather than guessing a display value.

Driver stderr is discarded so startup notices and warnings cannot overwrite the TUI. MCP protocol logging and connection errors still use Rho's reporting path.

## Tool behavior

The model first requests the supported operations:

```json
{"action":"list"}
```

When the combined schemas exceed the configured `max_output_bytes`, it gets operation names and can request one complete schema:

```json
{"action":"list","tool":"get_window_state"}
```

It then calls the exact operation with arguments matching that schema:

```json
{"action":"call","tool":"get_window_state","arguments":{"pid":12345}}
```

The PID above is illustrative; the model must discover the real target. Available operations vary by driver version and platform. Rho only exposes an explicit set of observation, input, and navigation operations. Unknown operations and driver administration, recording, permission changes, and session lifecycle controls are excluded. Calls cannot supply another session's identifier.

Actions are serialized within the managed session. Rho does not coordinate desktop ownership with other Rho processes or other computer-use clients. Failed or cancelled actions revoke the grant because their effects may be uncertain; the model cannot reconnect itself or automatically replay an input action. Rho reports the revocation at the next idle boundary and retains its reason in the dashboard until a new grant. Check the desktop for partial effects before explicitly enabling access again.

Supported typed image content from computer calls becomes model-visible image input, while ordinary MCP output remains presentation-only. Model observations preserve the original payloads independently of the preview card's image selection and size budget. A screenshot omitted from the preview can still reach the model; unsupported image formats are identified in the text output. Provider image limits still apply.
