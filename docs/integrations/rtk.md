# RTK

Parent: [Integrations](/integrations).

[RTK](https://github.com/rtk-ai/rtk) rewrites common shell commands into token-efficient forms before they run. When the `rtk` binary is on your `PATH`, Rho uses it automatically for agent shell tools. You do not run `rtk init`, install host hooks, or change prompts.

## Install RTK

Install the `rtk` binary with the method you prefer from the [RTK project](https://github.com/rtk-ai/rtk), then confirm it is on `PATH`:

```bash
rtk --version
```

Rho needs RTK **0.23 or newer** so `rtk rewrite` is available. Older versions are treated as unavailable.

Rewritten commands run in `bash -lc`, so `rtk` must also be on your login shell's `PATH`, not only one set by `.bashrc` or an interactive-only activation.

## What Rho does

When RTK is available and enabled:

1. Before `bash` or `powershell` asks for permission, Rho calls `rtk rewrite` on the command string.
2. If rewrite returns a different non-empty command, that rewritten form is what the [permission mode](/configuration/permissions) checks and what runs. An approval prompt shows `rtk git status`, not `git status`.
3. After the command finishes, Rho writes RTK-compatible analytics records so `rtk gain` and `rtk discover` can account for Rho shell use.

The tool card and the transcript keep the command the agent wrote. [Hooks](/hooks/protocol) and the Auto classifier see the rewritten command, so a deny hook that anchors on `^git push` misses `rtk git push`. Match the command anywhere in `shell_command`, or strip a leading `rtk ` first.

Commands that already start with `rtk ` are left unchanged. Empty commands are not rewritten. Rewrite is skipped when the process environment sets `RTK_DISABLED=1`.

Rho does not rewrite:

- [inline shell](/inline-shell) commands you type in the TUI
- in-process tools such as [`grep` and `glob`](/tools-workspace/search) (prefer those for workspace search)

Rewritten commands still go through the normal shell path, so RTK records savings through its usual `rtk gain` path.

SDK hosts that build the shell tool with `rho_agent_tools::shell_tool` opt in with `ShellToolOptions::rtk(true)`. It is off by default.

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

### "No hook installed" warning

RTK 1.x warns ``No hook installed — run `rtk init -g` `` when `~/.claude` exists without its Claude Code hook. Rho's discover records create that directory, so `rtk gain` and `rtk discover` can show the warning on machines that only use Rho. Rho does not need the hook. To hide the warning, set this in RTK's own config (`rtk config` shows the path):

```toml
[hooks]
suppress_hook_warning = true
```

Or export `RTK_SUPPRESS_HOOK_WARNING=1`. Either one hides only the missing-hook warning, not RTK's outdated-hook prompt.

## Check the integration

In the interactive TUI, run `/doctor`. The **rtk** row reports:

| Status | Meaning |
| --- | --- |
| `available` | `rtk --version` succeeds and supports rewrite |
| `unavailable` | Binary missing, too old, or not runnable |
| `disabled` | `[behavior].rtk = false`; Rho does not probe the binary |

Unavailable and disabled are not errors; both show as informational rows. In both cases Rho runs shell commands unchanged.

## Related

- [Integrations](/integrations) - other built-in integrations
- [Configuration](/configuration#rtk) - the `rtk` config key
- [Tools and workspace](/tools-workspace) - shell tools and permission modes
- [Search tools](/tools-workspace/search) - in-process search that does not need RTK
- [Interactive TUI](/interactive-tui) - `/doctor` and `/config`
