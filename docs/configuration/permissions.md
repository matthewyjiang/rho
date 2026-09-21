# Permission modes

Parent: [Configuration](/configuration).

`permission_mode` decides whether Rho allows, classifies, denies, or asks before a security-sensitive tool capability. Set it with `/permissions`, `/config` → **Agent behavior**, or `[behavior].permission_mode`.

Allowed values are `bypass`, `auto`, `allow_edits`, `plan`, and `supervised`. A missing value defaults to `bypass`. An unrecognized value is a configuration error. `allow-edits` is an alias for `allow_edits`.

[`rho acp`](/integrations/acp) can ask the editor host. Headless `rho run` cannot prompt, so operations that need approval fail closed.

| Mode | Config | Default | What it does |
| --- | --- | --- | --- |
| Bypass | `bypass` | yes | No policy checks. Every capability is allowed. |
| Auto | `auto` | no | Same gate as Allow edits. A classifier model decides the rest. |
| Allow edits | `allow_edits` | no | In-workspace writes to git-tracked files are allowed. Later writes to a path already allowed this session are also allowed. Other new files, processes, outside reads, and unknown capabilities need approval. |
| Plan | `plan` | no | File writes, process execution, and reads outside the workspace are denied. |
| Supervised | `supervised` | no | Those same operations need approval. |

Gitignored and untracked paths still ask, as do writes outside the workspace, including writes to global `AGENTS.md`, skill trees, and agent definitions.

These reads do not prompt in checked modes: workspace-scoped reads, `~/.rho/AGENTS.md`, skill trees (`~/.rho/skills`, `~/.agents/skills`), and agent definitions (`~/.rho/agents`, `~/.agents/agents`). Network access, skills, and instruction discovery do not prompt either. There is no attach-directory command. Switch mode to read a path outside the workspace.

The status line shows **Bypass** in warning style. The other modes appear dim.

## Auto

Auto uses the Allow edits gate. A classifier model reviews what that gate does not allow, instead of opening the approval UI.

The classifier has two stages. A fast low-reasoning screen answers `allow` or `escalate` in one token. Only an escalation pays for a second review at the configured classifier reasoning level. The stages share a transcript cache breakpoint, so raising that reasoning keeps the screen cheap and skips a message-cache hit on the review.

A denial returns a tool error and the run continues. After three consecutive denials, or twenty total, Rho opens the human approval prompt in the TUI, or fails closed in a headless run. A human decision clears both counts.

Auto needs a classifier model. Rho does not pick one. `/config` and interactive startup open the picker when none is set. Headless `rho run` fails at startup without one. Escaping the startup picker falls back to Supervised.

The classifier sees completed questionnaire answers next to the questions. Approving an action there covers that action, not unrelated ones. You do not need to repeat it in chat. Unanswered questions and defaults are not consent. Ordinary tool output stays out of the classifier transcript.

Set the model under **Agent behavior** in `/config`, or as `[internal_agents.permission-classifier]`.

## Change the mode

`/permissions` shows the current mode. `/permissions bypass|auto|allow_edits|plan|supervised` changes it and saves the choice. `/permissions auto` asks for a classifier model if none is set. Cancelling keeps the previous mode. The command is unavailable during a model turn.

`--permission-mode` overrides one invocation and is not saved.

An interactive change applies before the next turn. The session ID and history stay. Every remembered **Allow for session** approval is cleared. Path grants stay bound to the approver that allowed them, so a classifier grant does not skip the human gate after a switch to Allow edits. A human grant may. A reset or a different resumed session starts with no inherited path grants.

## Approval prompt

When a checked mode needs a person, the composer opens an approval prompt. It leads with the path or command and focuses **Deny**. Choose **Allow once**, **Allow for session**, or **Deny**, then press Enter.

**Allow for session** remembers only that exact capability request for the current session. Page Up and Page Down scroll long details without hiding the choices. **Deny** rejects that operation and lets the run continue. Escape denies it and cancels the run.

## Not a sandbox

Permission modes are application policy checks. Rho and its tools still run as the current user. The policy only covers tools that declare and authorize their capabilities. Unrecognized capability classes fail closed: Plan denies them, Supervised and Allow edits ask, and Auto sends them to the classifier.
