# Computer use

Rho can interact with desktop apps through **Cua Driver**. Rho manages the connection and tool access; Cua Driver performs desktop automation. `/computer setup` can install a missing driver with your confirmation or connect an existing installation.

Computer use starts off until you choose otherwise. Enable it explicitly inside an interactive Rho session:

```text
/computer setup
/computer on
```

`/computer setup` detects the driver, offers a separately authorized installation if needed, then asks for desktop access and verifies the connection. An existing driver skips installation. `/computer on` asks you to review the access scope before connecting. Confirmation saves this session's access as on and makes on the default for new sessions on this machine. Other saved sessions keep their own on/off choice. Cancel is selected by default; cancelling does not start the driver or grant access. If the terminal clips the confirmation, Rho blocks the grant and asks you to enlarge it so the full disclosure is visible.

Connection runs in the background so the composer remains usable. Once connected, the model receives a `computer` tool for discovering and calling supported Cua operations. Use an image-capable model for visual tasks. A persistent indicator near the composer shows connecting, enabled, or access-off/disconnecting state, even as other status messages change. `/computer` holds connection details, permission status, and diagnostic commands. Errors and actionable warnings appear in the transcript.

Rho adds a runtime-authored capability notice to model context, including the actual tool schema when access is enabled. Live state overrides stale notices from resumed history. Turning access off during a turn updates the next scheduled provider request; the change does not start a new turn by itself.

## Access and privacy

Enabling computer use grants access to the **local desktop**, including signed-in apps and browsers. It is not a workspace sandbox or an isolated virtual desktop.

This is a session-wide grant. Rho does not ask for separate approval before each computer action, including in supervised mode. Cua's own permission policies and OS permissions still apply. Plan mode never permits this grant.

Captured images are sent to your configured model provider and retained in session history. Driver output, screen text, web pages, and clipboard contents are untrusted tool data, not instructions. Enable the feature only when that scope is appropriate for your task.

Consent stays on this machine, separately from config and conversation history. `~/.rho/computer-use.toml` stores the default for new sessions. `~/.rho/computer-use-sessions/<uuid>.toml` stores each session's own on/off choice. Both paths use `$RHO_HOME` instead of `~/.rho` when an absolute Rho home override is set. Project configuration and conversation history cannot opt in. A missing default means off. A resumed session without its own record, including a legacy session, starts off even if the new-session default is on. Invalid or unreadable preferences leave access off with a nonfatal startup warning. Native interactive sessions with tools enabled connect automatically when their saved choice is on. Startup connection errors do not prevent using Rho.

`/computer off`, `/computer stop`, and dashboard `r` revoke the current connection, save this session as off, and set the new-session default to off. They do not change other sessions' records. Revocation happens before saving. If saving fails, access remains revoked now and Rho warns that a saved session choice or the new-session default may still be on. An enable-save failure also revokes the current grant rather than silently keeping session-only access.

Session switches close the current connection. `/new` and fresh launches inherit the last explicitly chosen default. `/resume` and `--resume` restore the selected session's own saved on/off choice instead. Restoring a session does not change the new-session default; only explicit access toggles change it. For example, enable session A, then disable session B. Resuming A enables access, but new sessions still start off. Entering plan mode revokes the current connection without changing saved choices. Subagents, side chats, workflows, ACP sessions, and `rho run` do not inherit this managed connection. `--no-tools` and plan mode cannot enable it. Saved consent never bypasses those restrictions.

This is a Rho tool-registration boundary, not an OS sandbox. An independently configured MCP server or an agent with shell access may still invoke an installed automation program outside this integration.

## Commands

| Command | Effect |
| --- | --- |
| `/computer` or `/computer status` | Open the dashboard with session state, a direct revoke action, driver detection, and access details |
| `/computer setup` | Install a missing driver after confirmation, then configure session access and verify the connection |
| `/computer on` | Review and confirm desktop access, save this session and the new-session default as on, then connect |
| `/computer off` or `/computer stop` | Cancel installation, revoke access, disconnect, and save this session and the new-session default as off |
| `rho computer status [--json]` | Detect the executable without starting it or inspecting the desktop |
| `rho computer setup` | Show read-only guidance directing you to the interactive setup flow |

`/computer off` works during a model turn. It prevents later computer calls through that grant. It cannot retract input already handed to the OS, and an in-flight action may finish during driver cleanup. Status remains `closing (access revoked)` until cleanup completes; a new grant cannot start before that. The runtime removes the revoked tool at the next idle boundary, before another model request.

CLI status is a local installation check, not a query of another running Rho session. A successful MCP handshake also does not prove that the OS has granted capture or input permissions.

The interactive dashboard uses the same overlay as `/limits` and `/doctor`, without adding status output to the transcript. Session state and available actions appear before diagnostics and privacy details. Scroll with arrow keys, PgUp/PgDn, Home/End, or the mouse wheel; Enter or Esc closes it. It can be opened during a running turn, and closing it does not interrupt the model. Press `r` to revoke access immediately and save this session and the new-session default as off without another confirmation. Initial consent requires explicit confirmation through `/computer setup` or `/computer on`.

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

After installation, Rho asks separately for desktop access. If you are typing or another overlay is open, it preserves that UI and asks you to continue with `/computer setup`. Installation does not change saved desktop consent. Confirming access saves this session and the new-session default as on in machine-local records; no persistent MCP configuration is written.

Rho detects `cua-driver` in absolute directories on `PATH`, then the standard `~/.local/bin/cua-driver` installation. Empty and relative `PATH` entries are ignored so a repository-local program cannot be selected accidentally. On Windows, detection also checks `~/.cua-driver/bin` (the managed installation location) and `%LOCALAPPDATA%/Programs/Cua/cua-driver/bin`; the executable is `cua-driver.exe`. Rho starts its stdio connection using:

```sh
cua-driver mcp
```

Connection verification checks the MCP handshake and supported tools, not OS capture/input permissions. Rho does not change OS permissions or select unrestricted mode. Run `cua-driver doctor` for installation diagnostics. On macOS, use `cua-driver permissions status` after starting the daemon and grant Accessibility and Screen Recording in System Settings as needed; Cua's daemon-backed permission flow remains in charge. Outside the separately authorized installer, Rho does not stop a shared Cua daemon: closing its MCP connection does not close other clients' sessions.

On Linux, the driver child receives the available desktop connection variables `DISPLAY`, `WAYLAND_DISPLAY`, `XDG_RUNTIME_DIR`, `XAUTHORITY`, `DBUS_SESSION_BUS_ADDRESS`, and `HYPRLAND_INSTANCE_SIGNATURE`. Rho also forwards an existing `CUA_DRIVER_RS_ENABLE_WAYLAND` value. Other process environment filtering remains unchanged. A headless shell or missing OS permissions may allow discovery but prevent actual observation or input.

When `WAYLAND_DISPLAY` is nonempty, Rho defaults its managed driver child to `CUA_DRIVER_RS_ENABLE_WAYLAND=1`. This selects Cua's experimental native Wayland backend instead of the X11 path, even when the desktop also exports `DISPLAY` for XWayland. X11-only and headless sessions get no automatic opt-in. An existing `CUA_DRIVER_RS_ENABLE_WAYLAND` value always takes precedence, including `0`, `false`, or an empty value. To disable native Wayland explicitly:

```sh
CUA_DRIVER_RS_ENABLE_WAYLAND=0 rho
```

This default applies only after you grant desktop access. It does not change global settings, the installer environment, or unrestricted mode. The Hyprland instance signature lets Cua find the correct compositor IPC socket. Forwarding it does not install a compositor plugin or guarantee capture and input support. If using a terminal server such as Herdr, start that server from the desktop session too; attaching a desktop client does not necessarily update the server's environment.

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
{"action":"call","tool":"get_window_state","arguments":{"pid":12345,"window_id":67890}}
```

The PID and window ID above are illustrative; the model must discover the real target. Available operations vary by driver version and platform. Rho only exposes an explicit set of observation, input, navigation, and app-launch operations. `launch_app` can open apps, files, and URLs. On Linux it also accepts commands and extra arguments, so this grant is not limited to installed-app IDs. Launch calls are mutating operations: failure or cancellation revokes access, and Rho never replays them automatically. Unknown operations and driver administration, recording, permission changes, and session lifecycle controls are excluded. Calls cannot supply another session's identifier.

Actions are serialized within the managed session. Rho does not coordinate desktop ownership with other Rho processes or other computer-use clients. Failed or cancelled dispatched calls revoke the grant unless they are audited observations. Observation calls, including screenshots returned to the model, keep the grant on failure or cancellation. Screenshots requested with `screenshot_out_file` still revoke on failure or cancellation because they may have written a file. Rho classifies calls by their tool name and arguments, not by server hints or error text. The model cannot reconnect itself or automatically replay an input action. Rho reports a revocation at the next idle boundary and retains its reason in the dashboard until a new grant. Check the desktop for partial effects before explicitly enabling access again.

Supported typed image content from computer calls becomes model-visible image input, while ordinary MCP output remains presentation-only. Model observations preserve the original payloads independently of the preview card's image selection and size budget. A screenshot omitted from the preview can still reach the model; unsupported image formats are identified in the text output. Provider image limits still apply.
