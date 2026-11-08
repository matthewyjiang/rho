# Agent Client Protocol

Parent: [Integrations](/integrations).

`rho acp` runs Rho inside an editor or desktop app that speaks the [Agent Client Protocol](https://agentclientprotocol.com/) (ACP), such as Zed or T3 Code. The host starts the process, shows the conversation, and asks you for approvals. `rho acp` is a client of the per-user [rho daemon](/daemon), which it starts when none runs. The daemon runs the sessions, with the model, tools, and config they would use in the TUI.

```mermaid
flowchart TD
    host[Editor host] -->|stdio JSON-RPC| acp["rho acp"]
    acp -->|daemon protocol| daemon[rho daemon]
    daemon --> store["~/.rho/sessions"]
    daemon --> model[Model and provider from config]
```

Sessions keep running in the daemon when the host disconnects. Other clients can follow the same session: another `rho acp`, [`rho attach`](/automation-cli#attaching-to-a-daemon-session), or anything else that speaks the [daemon protocol](/protocol).

`rho acp` runs on Linux and macOS. The daemon does not support Windows yet ([#1665](https://github.com/matthewyjiang/rho/issues/1665)), so neither does `rho acp`.

This page covers setup and day-to-day use. Host-specific behavior lives on [T3 Code](/integrations/acp/t3-code). Protocol details for people building or debugging a host are in the [ACP host reference](/integrations/acp/reference).

## Set up a host

Every host launches the same command:

```bash
rho acp
```

Each session opens with the configuration, agents, and instructions of its workspace, as `rho` launched there would use them. `rho acp` takes `--agent`, `--add-dir`, `--no-tools`, `--no-system-prompt`, and `--no-subagents`, and applies them to every session it opens. It refuses `--provider`, `--model`, `--auth`, `--reasoning`, `--permission-mode`, `--config`, and `--save`: choose the model and mode in the host. Sign in to your provider before the first session; see [Sign in](#sign-in).

### Zed

Add Rho as a custom agent in Zed's `settings.json` (**Agent Settings → External Agents → Add Agent → Add Custom Agent**):

```json
{
  "agent_servers": {
    "Rho": {
      "type": "custom",
      "command": "rho",
      "args": ["acp"]
    }
  }
}
```

Start a Rho thread from the Agent Panel's new-thread menu. Zed lists saved Rho sessions in its thread history, uses Rho's generated titles as thread names, and shows compaction progress. To see the messages Zed and Rho exchange, run `dev: open acp logs`.

### T3 Code

Add Rho as an ACP agent with the command `rho` and the arguments `acp --t3code`. The connection negotiates ACP v2 when T3 Code asks for it, with v1 still available to older hosts. Rho subagents run as T3 child threads; background results do not wake an idle thread yet ([#1670](https://github.com/matthewyjiang/rho/issues/1670)). See [T3 Code](/integrations/acp/t3-code).

### Other hosts

Point the host at `rho acp`. Rho speaks ACP protocol version 1, and the version 2 draft for hosts that ask for it, over stdio. Rho never asks the host to read files or run commands: tools run in the daemon, on the files on disk. stdout carries only protocol messages and logs go to stderr, so do not wrap the command in anything that prints to stdout.

## Sign in

Logins run in the daemon, and credentials go to the daemon's [credential store](/daemon#credentials), never over ACP.

- **OAuth in the host's own sign-in UI.** Hosts that can show a link from the agent, such as Zed, sign in to OAuth providers without a terminal. Rho picks browser or device code the same way `rho login <provider>` does. You get a browser callback on a desktop. Over SSH or headless you get device code, and the host also shows the code. Open the link, approve, and the host finishes signing in.
- **A terminal for everything else.** API keys, and OAuth on hosts that cannot show links, sign in through a terminal that runs `rho acp --login <auth-profile>`, on hosts that have one. API-key input stays hidden.

When credentials are missing, unreadable, or refused by the provider (a revoked key or an expired OAuth session), the prompt tells the host that sign-in is needed instead of failing with an internal error. Zed opens its sign-in screen. After you sign in, keep going. You do not need to reconnect.

- Signing in does not change the configured provider or model. Sign in to the provider your config selects.
- On a host with neither kind of sign-in, run `rho login <provider>` in a terminal, then connect.
- Logging out from the host deletes the saved credential of the auth profile your workspace config selects; with none selected, the host gets an error. Every session the daemon runs, and every other Rho process, then needs to sign in again; open sessions ask on their next message.

## What works in the host

| Feature | In the host |
| --- | --- |
| Sessions | Run in the daemon and saved under `~/.rho/sessions`, the same as the TUI. They show up in the host's history, in `rho sessions list`, and in `rho -R`. Loading a session replays its messages and tool cards. See [Sessions](/sessions). |
| Permission modes | `bypass`, `auto`, `allow_edits`, `plan`, and `supervised` appear in the host's mode picker. Tools that need approval ask the host, which offers allow once, allow for the session, and reject. |
| Plan mode | When the plan is ready, the host asks you to approve it into a mode or keep planning. Approving runs the implementation in the same prompt. |
| Model, reasoning, fast mode | Host pickers list your Rho model catalog, favorites first, plus the current model's reasoning levels and fast mode when it has one. |
| Questions | On hosts that support forms, such as Zed and T3 Code, questions from the model or an MCP server appear as a form. |
| Checklist | The model's `todo` list appears as the host's plan. |
| File changes | Edits and writes show up as diffs. Tools read and write the files on disk, so they do not see edits the host has not saved. |
| Context and cost | The host gets the context fill and the session's cumulative cost, read from the same usage ledger as `/spend`. |
| Workspace roots | Extra roots of a multi-root workspace join the session like `--add-dir` and stay with it across reloads. |
| MCP servers | Servers from your Rho config and servers the host sends both connect. A host server replaces a config server with the same name. |
| Commands and skills | The host's command menu lists the commands below and your skills. |

Changes you make in host pickers apply to that session, for every client following it. They never rewrite `config.toml`. A change made while a prompt is running takes effect from the next prompt. The session keeps them when you load it again, including after the daemon restarts. Sessions started with `/agent:` reload as that agent, with its prompt, tools, and model, and those started with a [per-call model](/integrations/acp/reference#per-call-model) keep their saved model.

The host owns approvals. `rho acp` can ask you to approve a tool. Headless `rho run` cannot. An approval or question waits in the daemon until a client answers it, even after the host disconnects.

Closing a thread, or quitting the host, cancels the prompt that host is running and detaches. The session stays in the daemon, and loading it again picks it up. If `rho acp` is killed outright (`SIGKILL`) or crashes, nothing cancels the prompt: its turn keeps running in the daemon.

## Commands

The host's command menu lists these. Pick one, or type it at the start of a message:

| Command | Effect |
| --- | --- |
| `/compact [instructions]` | Compacts older context, optionally saying what to keep, and reports what it did. Cancel stops it before it commits. |
| `/init` | Surveys the repository and writes `AGENTS.md` through the `rho-init` skill, as in the TUI. Refused in `plan` mode, or when the active agent is missing any of the skill, write, or file-edit tools. |
| `/prompt:<name> [args]` | Runs a [prompt template](/configuration#prompt-templates). |
| `/mcp:<server>:<prompt> [args]` | Runs an MCP server prompt. Arguments follow the TUI rules. |
| `/agent:<name> <task>` | Turns a new, empty session into the [agent](/subagents) `<name>` and runs `<task>`. The session keeps that agent's prompt, tools, and model, including when loaded again. Add `--model <reference> --` before the task to pick the model for this one session. |
| `/workflow:<name> [inputs JSON]` | Starts a workspace [workflow](/workflows#start-a-workflow-by-name) in the background, such as `/workflow:review {"target": "src"}`. Listed only when the active agent has the `workflow` tool; refused in plan mode. |

Other TUI commands, such as `/model`, are rejected with an error that names the command; for `/model`, use the host's model picker. Text that is not a known command, such as `/usr/bin`, goes to the model unchanged. The session saves what you typed, not its expansion. Edge cases are in the [host reference](/integrations/acp/reference#commands).

### Skills

When the active agent has the `skill` tool, the menu also lists one `$<name>` entry per [skill](/skills). Mentioning `$<name>` anywhere in a message loads that skill before the model starts, following the TUI's [mention rule](/skills#load-a-skill-yourself).

## Background work

Background processes, workflows, and subagents run in the daemon and report back as they do in the TUI, and each result shows up in the host as a finished tool card.

Rho keeps the current prompt open while work it is waiting on is still running, and the model continues once it finishes:

- Subagents hold the prompt open until they finish, whichever prompt started them.
- Workflows and background processes started during the prompt hold it open too. Older processes, such as a dev server, do not; their results arrive with your next message.
- A workflow started with `/workflow:` ends the prompt right away. Its result reaches the model with your next message, and that prompt does not wait for the run to finish.
- Cancelling stops the wait, not the work. Subagents and processes keep running. The next prompt waits for those subagents and delivers their results; processes report in it without holding it open. Closing the session does not stop them either. Stop subagents with the `agents` tool.

Under `--t3code`, subagents run as [T3 threads](/integrations/acp/t3-code#subagents-run-as-t3-threads) instead and never hold a prompt open.

## Not covered by ACP

- `rho acp` is not an MCP server. See [Model Context Protocol](/integrations/mcp) for using MCP servers in Rho.
- For scripts and CI, use [`rho run`](/automation-cli). `rho acp` does not change `rho run --output jsonl`.
