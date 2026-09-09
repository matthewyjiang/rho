# Computer use

Rho can interact with desktop apps through **Cua Driver**. Rho manages the connection and tool access; Cua Driver performs desktop automation. The driver is installed separately.

Computer use starts off. Enable it explicitly inside an interactive Rho session:

```text
/computer setup
/computer on
```

Connection runs in the background so the composer remains usable. Once connected, the model receives a `computer` tool for discovering and calling supported Cua operations. Use an image-capable model for visual tasks.

## Access and privacy

Enabling computer use grants access to the **local desktop**, including signed-in apps and browsers. It is not a workspace sandbox or an isolated virtual desktop.

This is a session-wide grant. Rho does not ask for separate approval before each computer action, including in supervised mode. Cua's own permission policies and OS permissions still apply. Plan mode never permits this grant.

Captured images are sent to your configured model provider and retained in session history. Driver output, screen text, web pages, and clipboard contents are untrusted tool data, not instructions. Enable the feature only when that scope is appropriate for your task.

The grant lasts for the current interactive session. It is not saved in config or restored from conversation history. New sessions, session switches, and entering plan mode revoke it. Subagents, side chats, workflows, ACP sessions, and `rho run` do not inherit this managed connection. `--no-tools` and plan mode cannot enable it.

This is a Rho tool-registration boundary, not an OS sandbox. An independently configured MCP server or an agent with shell access may still invoke an installed automation program outside this integration.

## Commands

| Command | Effect |
| --- | --- |
| `/computer` or `/computer status` | Show this session's state and driver detection without connecting |
| `/computer setup` | Show setup and privacy guidance without installing anything |
| `/computer on` | Grant desktop access for this session and connect |
| `/computer off` or `/computer stop` | Revoke access, cancel pending connection or computer actions, and disconnect |
| `rho computer status [--json]` | Detect the executable without starting it or inspecting the desktop |
| `rho computer setup` | Show setup guidance outside the TUI |

`/computer off` works during a model turn. It prevents later computer calls through that grant. It cannot retract input already handed to the OS, and an in-flight action may finish during driver cleanup. Status remains `closing (access revoked)` until cleanup completes; a new grant cannot start before that. The runtime removes the revoked tool at the next idle boundary, before another model request.

CLI status is a local installation check, not a query of another running Rho session. A successful MCP handshake also does not prove that the OS has granted capture or input permissions.

## Driver setup

Follow [Cua Driver's setup instructions](https://cua.ai/docs/how-to-guides/driver/connect-your-agent). Rho detects `cua-driver` in absolute directories on `PATH`, then the standard `~/.local/bin/cua-driver` installation. Empty and relative `PATH` entries are ignored so a repository-local program cannot be selected accidentally. On Windows, the executable is `cua-driver.exe`. Rho starts its stdio connection using:

```sh
cua-driver mcp
```

Rho does not install or upgrade the driver, change OS permissions, select unrestricted mode, or stop a shared Cua daemon. On macOS, Cua's normal daemon-backed permission flow remains in charge. Closing Rho closes its own MCP connection, not other clients' sessions.

On Linux, the driver child receives the available desktop connection variables such as `DISPLAY`, `WAYLAND_DISPLAY`, `XDG_RUNTIME_DIR`, `XAUTHORITY`, and `DBUS_SESSION_BUS_ADDRESS`. Other process environment filtering remains unchanged. A headless shell or missing OS permissions may allow discovery but prevent actual observation or input.

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

Actions are serialized within the managed session. Rho does not coordinate desktop ownership with other Rho processes or other computer-use clients. Failed or cancelled actions revoke the grant because their effects may be uncertain; the model cannot reconnect itself or automatically replay an input action. The user must explicitly enable access again.

Supported typed image content from computer calls becomes model-visible image input, while ordinary MCP output remains presentation-only. Model observations preserve the original payloads independently of the preview card's image selection and size budget. A screenshot omitted from the preview can still reach the model; unsupported image formats are identified in the text output. Provider image limits still apply.
