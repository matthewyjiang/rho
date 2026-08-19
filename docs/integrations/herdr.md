# Herdr

Parent: [Integrations](/integrations).

[Herdr](https://github.com/herdrdev/herdr) is a terminal workspace that hosts agent panes. When Rho runs inside a Herdr pane, integration turns on by itself. You do not install a plugin, run an init command, or edit Rho config.

Herdr support is Unix-only. On other platforms Rho ignores Herdr environment variables.

## What you get

| Feature | Behavior |
| --- | --- |
| Agent state | Rho reports `idle`, `working`, and `blocked` so Herdr can show pane status |
| Session identity | Rho reports the active session id (and the attach run id in `rho attach`) |
| Subagent attach | Click a subagent row in the activity rail to open the in-place attach view. `rho attach <id>` remains available for another terminal. |
| Image paste | A single-line paste of an image path becomes an attachment instead of plain text |
| Image previews | Kitty placements when Herdr can paint them; halfblock fallback when host cell metrics are missing |

Nothing extra is required after you start Rho under Herdr.

## How Rho detects Herdr

Rho enables Herdr when all of these are true:

- the process runs on Unix
- `HERDR_ENV=1`
- `HERDR_SOCKET_PATH` is a non-empty path to Herdr's local socket
- `HERDR_PANE_ID` is a non-empty pane id

Herdr sets those variables for processes it launches. You normally do not set them by hand.

Rho talks to Herdr over that Unix socket with short timeouts. Failed or timed-out reports are ignored so a stuck host never blocks the session.

## Agent state

Rho keeps Herdr in sync with the session:

| State | When |
| --- | --- |
| `working` | A model turn or tool run is in progress, or an attached subagent is still running |
| `blocked` | Rho waits on you (approval, questionnaire), auth is missing, a goal is blocked, or an attached run failed |
| `idle` | The session is resting and not waiting on user input |

On exit, Rho releases the agent marker for the pane.

Interactive sessions and `rho run` both report state when they detect Herdr. `rho attach <id>` reports state for the watched run and releases on detach.

## Watch a subagent

Activating a subagent row, or choosing one from `/attach`, opens a read-only attach view in the same terminal. The parent session keeps streaming. Press `q` or Escape to return. Detaching does not stop the delegated run.

`rho attach <id>` still starts the same viewer as its own process when you want another terminal. See [attachment and artifacts](/subagents/attachment-and-artifacts).

## Images and graphics

Hosts such as Herdr may paste clipboard images as a filesystem path. Rho treats a single-line paste of a PNG, JPEG, GIF, or WebP path as an image attachment. Details: [attachments](/interactive-tui/attachments).

For in-feed image previews, Rho probes whether Herdr can paint Kitty placements for the pane:

- paintable host metrics → Kitty placements through Herdr
- missing metrics → halfblock preview so reserved rows are not blank

See [documents and images](/tools-workspace/documents-and-images#where-thumbnails-paint).

## Check the integration

In the interactive TUI, run `/doctor`. The **Herdr** row reports:

| Status | Meaning |
| --- | --- |
| `not configured` | Rho is not running inside Herdr (healthy when you are outside Herdr) |
| `connected` | Herdr env is set and the socket accepted a connection |
| `unreachable` | Herdr env is set but the socket did not accept a connection |
| `unavailable` | Herdr is configured but reachability could not be determined |

## Related

- [Integrations](/integrations) - other built-in integrations
- [Interactive TUI](/interactive-tui) - session UI, `/doctor`, and `rho attach`
- [Subagent attachment](/subagents/attachment-and-artifacts) - run artifacts and detach behavior
- [Attachments](/interactive-tui/attachments) - image and document paste
- [Documents and images](/tools-workspace/documents-and-images) - thumbnail paint paths
- [Development](/development) - use PTY tests for regressions; use Herdr for exploratory checks
