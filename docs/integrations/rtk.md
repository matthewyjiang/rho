# RTK

Parent: [Integrations](/integrations).

[RTK](https://github.com/rtk-ai/rtk) rewrites common shell commands into token-efficient forms before they run. When the `rtk` binary is on your `PATH`, Rho uses it automatically for agent shell tools. You do not run `rtk init`, install host hooks, or change prompts.

## Install RTK

Install the `rtk` binary with the method you prefer from the [RTK project](https://github.com/rtk-ai/rtk), then confirm it is on `PATH`:

```bash
rtk --version
```

Rho needs RTK **0.23 or newer** so `rtk rewrite` is available. Older versions are treated as unavailable.

## What Rho does

When RTK is available and enabled:

1. Before `bash` or `powershell` runs a command, Rho calls `rtk rewrite` on the command string.
2. If rewrite returns a different non-empty command, Rho runs that rewritten form.
3. After the command finishes, Rho writes RTK-compatible analytics records so `rtk gain` and `rtk discover` can account for Rho shell use.

Commands that already start with `rtk ` are left unchanged. Empty commands are not rewritten. Rewrite is skipped when the process environment sets `RTK_DISABLED=1`.

Rho does not rewrite:

- [inline shell](/inline-shell) commands you type in the TUI
- in-process tools such as [`grep` and `glob`](/tools-workspace/search) (prefer those for workspace search)
- SDK coding-tool shells that must authorize and execute the same immutable process description

Rewritten commands still go through the normal shell path, so RTK records savings through its usual `rtk gain` path.

### Example

Without RTK, an agent might run:

```bash
git status
```

With RTK available, Rho may rewrite that to:

```bash
rtk git status
```

The agent still requested `git status`. The rewrite is an execution detail.

## Configuration

`[behavior].rtk` in `~/.rho/config.toml` defaults to `true`:

```toml
[behavior]
rtk = true
```

Set `rtk = false` to leave shell commands unchanged even when the binary is installed. This toggle is not in the `/config` browser yet; edit `config.toml` or set `RTK_DISABLED=1`. See [configuration](/configuration#rtk).

For a one-off disable without editing config, set `RTK_DISABLED=1` in the environment.

## Analytics and discover

Rewritten runs participate in RTK's savings history through the RTK binary.

Rho also writes discover-compatible command records under the Claude projects directory so `rtk discover` can include Rho shell commands:

```text
~/.claude/projects/<encoded-cwd>/rho-sessions/rho-<pid>-<uuid>.jsonl
```

Set `CLAUDE_CONFIG_DIR` to override the default `~/.claude` root used by both Rho and RTK.

These records store the command string and a placeholder sized to the tool output length. They do not copy command output text.

## Check the integration

In the interactive TUI, run `/doctor`. The **rtk** row reports:

| Status | Meaning |
| --- | --- |
| `available` | `rtk --version` succeeds and supports rewrite |
| `unavailable` | Binary missing, too old, or not runnable |

Unavailable is not an error. Rho runs shell commands without rewrite until RTK is installed.

## Related

- [Integrations](/integrations) - other built-in integrations
- [Configuration](/configuration#rtk) - the `rtk` config key
- [Tools and workspace](/tools-workspace) - shell tools and permission modes
- [Search tools](/tools-workspace/search) - in-process search that does not need RTK
- [Interactive TUI](/interactive-tui) - `/doctor` and `/config`
