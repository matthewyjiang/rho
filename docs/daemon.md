# Daemon

The rho daemon is one long-lived background process per user and rho home (`~/.rho`, or `$RHO_HOME`). It owns agent work so that frontends (the interactive TUI, `rho acp`, and other clients) can come and go without stopping it. Frontends talk to it over a private Unix socket using rho's versioned JSON-RPC protocol, described in the [protocol reference](/protocol) and published as the [`rho-protocol`](https://crates.io/crates/rho-protocol) crate.

## Commands

| Command | What it does |
| --- | --- |
| `rho daemon` | Run the daemon in the foreground until it is stopped. Same as `rho daemon run`. `rho daemon run --unload-idle-after <DURATION>` sets how long a session sits idle before it is [unloaded](#unloading-idle-sessions), or discarded if it saved nothing (`60s` by default). |
| `rho daemon start` | Start the daemon in the background unless one is running, wait until it answers, and print its pid. Run again, it starts no second daemon and still exits 0, so scripts can run it unconditionally. |
| `rho daemon status` | Show the running daemon's pid, version, protocol, uptime, jobs, a pending [upgrade](#upgrading-in-place), socket, and [credential store](#credentials), with why the daemon cannot reach the store when it cannot. Add `--json` for one JSON document. |
| `rho daemon stop` | Ask the daemon to shut down and wait for it to exit. |
| `rho daemon restart` | Restart the daemon [in place](#upgrading-in-place) and wait until it answers again. Turns under way are interrupted and recorded `interrupted`. With no daemon running, it starts one. |
| `rho mcp serve` | Serve the daemon's sessions as an MCP server, so another agent can start, follow, and steer them. See [MCP server](/mcp-server). |
| `rho attach <ID>` | Follow a session's turn until it ends, and exit as it ended; answer its requests, steer it, or cancel it from the terminal; Ctrl+C detaches. See [Attaching to a daemon session](/automation-cli#attaching-to-a-daemon-session). |

`status`, `stop`, and `attach` never start a daemon. With none running, they print `rho daemon is not running` and exit with status 1.

Only one daemon runs per rho home. A second `rho daemon run` fails with the pid, version, and socket of the one already running.

The daemon shuts down gracefully on `rho daemon stop`, `SIGTERM`, or `SIGINT`: it detaches every client following a session with `rho/session/detached` (reason `daemon_stopping`), stops accepting connections and reading requests, answers the requests it has already received, and removes its socket. A client that has stopped reading, such as a suspended TUI, is dropped once a message to it has waited 1 second, so it cannot hold the stop open. It then stops the [turns](#sessions-and-jobs) still running, waits up to 4 seconds for them to save, records them `interrupted`, and exits with status 0. A request that never finishes holds that shutdown open. A `SIGTERM` or `SIGINT` that arrives once shutdown is under way, whether a second signal or any signal after `rho daemon stop`, makes the daemon exit at once with status 1; the next daemon to start removes the socket it left behind and records the turns that daemon left open as `interrupted`.

Stopping the daemon ends every process its sessions' tools started: the command a turn is running and the background processes a session's `process` tool started, each with its whole process group, so their children go too. A process that left its group, for example through `setsid`, keeps running. Unloading an idle session ends none of them, since a session whose background process still runs is never idle. A daemon that crashes, is killed with `SIGKILL`, or is forced to exit by a second signal cannot end them: its tools' processes keep running, as they do when any rho process is killed that way, and the next daemon does not track them.

## Experimental daemon-backed TUI

On Unix, `RHO_TUI_DAEMON=1 rho` opens the interactive TUI as a daemon client. Without that environment variable, the TUI keeps its in-process behavior. The flag is an unfinished migration, not a drop-in replacement yet.

The attached session supplies the current model, reasoning level, fast mode, permission mode, and their available choices. `/model`, `/fast`, `/permissions`, the session rows at the top of `/config`, and the model, reasoning, and permission shortcuts configure that session rather than the unused local runtime. The model picker lists the models the daemon offers under your pinned models, with the same pinned and all views and pin shortcut as without the daemon, and a model switch reports the model prompt file the session now runs on. Changes requested during a turn apply after it ends; the TUI remains responsive to approvals and cancellation, and displays the old settings until the daemon confirms the change. Changes are session settings, not edits to the default configuration; the categories of `/config` and `/advisor` change the default configuration through the daemon ([below](#configuration-changes)). Context tokens and cumulative session cost feed the usual context and spend gauges, and title changes update the followed session's title live.

The command menu includes commands the daemon advertises, such as `/init`, `/compact`, prompt templates, MCP prompts, and command workflows. These are resolved by the daemon. `/compact` shows as it does without the daemon: its card alone, with no row for the command and no reply. Pure UI commands remain local. `/remember` and the output of a `!` shell command are added to the daemon's session, and `/title` renames it there; a `!!` command's output stays on the screen that ran it. `/info`, `/doctor`, `/limits`, and `/export` read the disk and config the daemon shares; `/info` asks the daemon for its Compaction section, which fills in once the turn under way ends, and `/side` runs its own read-only chat on a copy of the transcript. `/new`, `/resume`, the `/sessions` hub, and the `--resume` picker switch the daemon session the TUI follows: the session left behind detaches and stays in the daemon, and a new session keeps the model and its auth, reasoning, fast mode, and permission mode of the one before it. Deleting a session from a picker is refused while the daemon still has it loaded. The subagent rail lists the runs the followed session started, from the daemon's jobs, and `/attach` opens a run's transcript; a finished run leaves the rail ten seconds after it ends. A subagent's question is put to the session's clients like any other, named by a notice. `/workflow` lists the workspace's workflows, saved plans, and runs from the store the daemon shares; starting a workflow, running a plan, or resuming a run sends the daemon a prompt (`/workflow:<name>`, `/workflow:plan/<plan id>`, `/workflow:run/<run id> [recover]`), so the daemon drives the run and it outlives the TUI. The workflow rail lists the runs the followed session started, from the daemon's jobs, and keeps a finished one until the daemon gives the session its result, as in process. A turn does not wait for a workflow run or background process it started: it ends while they run on, and once one finishes with no turn under way, the daemon starts a turn that gives the model the result, as the in-process TUI does, whether or not a TUI is attached ([`idle_wake`](/protocol#a-session-s-setup)). `/tree` lists the session's turns from its file and has the daemon continue from the one chosen; the screen then replays the conversation as it stands there. The daemon's turns take [workspace checkpoints](/interactive-tui) as in-process turns do; `/rewind` lists and previews them from the session's files, and the daemon restores the files and returns the conversation to before the turn. `/codemode` saves `codemode.mode` through the daemon, which every loaded session takes at its next turn. `/agents` edits definitions on disk and tells the daemon, which tells the model what changed; `/create-agent` and `/agents create` run in the daemon, which starts the guided creator or names the tools the session's agent lacks. `/skill:<name>` loads the skill in the daemon's session, or says the session has no skill tool. `/add-dir` widens the daemon's session, `/context` and `/hooks` show what the daemon reports for it, and `/refresh-models` refreshes the model lists on the disk the daemon shares, then has every loaded session's options read them again. `/login` and `/logout` keep their pickers and prompts, and the daemon checks and saves the key, runs a provider's interactive login (the browser opens where the TUI runs), and signs out; its sessions use the credential from their next prompt, so a login does not swap the running provider and a logout of the profile in use shows as the next prompt's authentication error. A session whose saved model has no login opens signed out, as in process, and offers `/login`. A resumed session whose conversation holds provider-native context its model cannot use asks how to continue, and each send is confirmed, as in process; the daemon reports what would be omitted. The resume dialog offers the model that produced the context, when the session may switch to it, to compact on that model and then continue, and to continue. After a compaction there, the session returns to its model even if the compaction failed; in process it then stays on the model that produced the context. `--provider`, `--model`, `--auth`, `--reasoning`, and `--permission-mode` change the session once it opens, and a value the daemon refuses fails the launch; on a session resumed mid-turn they apply when the turn ends, and with the bare `--resume` picker they are refused. `--auth` alone keeps a resumed session's model when the auth is its provider's. `--no-tools`, `--no-system-prompt`, `--no-subagents`, and `--no-save` leave those parts out of the session. A `--no-save` session lives in the daemon's memory only: quitting the TUI cancels its turn, since nothing could follow it again, the daemon discards it once it has sat idle with no TUI attached for one to two [idle periods](#unloading-idle-sessions), and a daemon restart loses its conversation, which the TUI says. `--agent` with `--resume`, `--continue`, or `/resume` must name the agent the session was saved as. `--config` is refused, since every session of a daemon shares the config in its Rho home; set `RHO_HOME` to run on another. Full provider and runtime configuration and reconnection after a lost connection remain unported.

`/mcp` opens the existing inventory overlay using the attached daemon session's servers, not the unused local runtime or a freshly read client configuration. It includes disabled and failed servers, exported tools, live tool changes and keepalive details, and current prompt/resource inventory. Each opening requests a fresh snapshot with `rho/session/mcp`; it remains usable during a turn even when a settings change is waiting to apply. Close and reopen the overlay to refresh it. The composer reads the same servers: an `@` mention lists and attaches their resources, and an MCP prompt command completes its arguments from them, through `rho/session/mcp/catalog`, `rho/session/mcp/resource`, and `rho/session/mcp/complete`. The listing is read again each time a mention or an MCP prompt command begins. A text resource is cut to the configured `max_output_bytes`, as in process. An image or other binary resource larger than a protocol line (16 MiB) is refused with its size.

Background processes the session's `process` tool starts run in the daemon. The activity rail lists them and a click opens the read-only peek view of one's output, as without the daemon; the TUI reads both with `rho/session/processes`.

Quitting detaches and leaves any running turn in the daemon. The exit receipt shows `rho --resume <short id>` once a new session has a saved prompt; quitting an untouched session shows no resume hint. Reopening with `RHO_TUI_DAEMON=1 rho --resume <id>` catches up with the turn, including a reply completed while the TUI was closed.

If the daemon detaches a client because it fell behind (`lagged`), the TUI automatically attaches again after its last event. A tail replay extends the existing transcript; a reset replay replaces it when the old cursor is no longer available. Open input requests and turn state are restored from the attachment. When the daemon [restarts in place](#upgrading-in-place) (`restarting`), the TUI shows that it is reconnecting, attaches again once the restarted daemon answers, and replaces the transcript with the session's reset replay; a prompt or settings change sent meanwhile waits for the reattach, and a prompt the restarting daemon refused is sent again to the restarted one, which runs it once: if the turn had already saved the prompt, the transcript shows it interrupted instead ([#1777](https://github.com/matthewyjiang/rho/issues/1777)). A session that had saved nothing yet, which only `rho daemon restart` restarts under, cannot be reopened, so the TUI goes on in a new session. A failed reattach stops following and shows an error rather than retrying indefinitely; other detach reasons and a lost connection still stop following.

## Sessions and jobs

A session is a conversation: its saved transcript and, while the daemon has it loaded, its live runtime. Requests name a session by its session id, the id `rho sessions` and `/resume` show. A loaded session's turns run in the daemon, so a turn keeps running when the client that started it disconnects, and another client can follow it later. The daemon tracks every unit of work it runs as a job with its own id: the turns of one session, or one subagent run that a session started.

Clients open, prompt, cancel, steer, withdraw steers from, and follow sessions with the `rho/session/*` methods, list, rename, and delete saved sessions, loaded or not, with `rho/session/list`, `rho/session/rename`, and `rho/session/delete`, and list jobs with `rho/job/list`; the [protocol reference](/protocol) describes every method, its errors, and the events a client following a session receives.

A session opens with the configuration, agents, and instructions that `rho` launched in its workspace would use, read when the session is opened. It runs as the agent its client names, else the default agent, with the directories, MCP servers, and parts left out its client chose ([protocol](/protocol#a-session-s-setup)) and the configured permission mode, until a client changes its model, reasoning level, fast mode, or permission mode between turns with `rho/session/configure` ([protocol](/protocol#changing-a-session-s-settings)). A prompt's `content` reaches the model as the [protocol reference](/protocol#content-blocks) describes. A prompt that starts with a slash command resolves as in [`rho run`](/automation-cli#sessions-commands-and-questions): `/init`, `/create-agent`, prompt templates, MCP prompts, `$skill` mentions, `/compact`, `/workflow:<name>`, and `/agent:<name>` work, while the TUI's own commands are refused ([protocol](/protocol#methods)). The session's file is written with its first prompt, so `rho sessions` lists it from then on. After its first answered turn, the daemon names the session as the TUI would, and attached clients get the title as a `title_changed` event; clients that attach later find it in the attach response.

A session's job state is the state of its current or last turn:

| State | Meaning |
| --- | --- |
| `idle` | Opened; no turn has started. |
| `running` | A turn is under way. |
| `waiting_for_input` | The turn waits for a tool approval, a plan approval, or an answer to a question. |
| `completed` | The last turn finished. |
| `failed` | The last turn failed; the job's `error` says why. |
| `cancelled` | Someone cancelled the last turn. |
| `timed_out` | The daemon cancelled the last turn at the time limit its prompt set, as `rho run --detach --timeout` does. |
| `interrupted` | The daemon stopped, or died, before the last turn ended. |

A new prompt moves a session's job from any state but `queued`, `running`, and `waiting_for_input` back to `running`, or to `queued` for a session [another session started](#sessions-other-sessions-start). `rho daemon status` counts the jobs that are `queued`, `running`, or `waiting_for_input`.

### Sessions other sessions start

A session an agent starts from its own session, as the `sessions` tool's `new` does, is a job of kind `session` whose `parent` is the job writing the starting session, so `rho/job/list` shows who started what. A session started from one the daemon does not run, such as an in-process rho's, has no `parent` to show but is budgeted the same. Either way the daemon holds it to the grant it started with, whoever prompts it later, and for a parent it runs, starts it with no more than the parent's turn could give ([a session's grant](/protocol#a-session-s-grant)).

Its turns share the [subagents' budget](#subagents): each turn is `queued`, with its `queue` place, until it gets a slot, then `running`. A prompt that has to wait answers once the turn queued; a turn cancelled while it waits ends `cancelled` without running, and a prompt's `timeout_ms` counts the wait. Such a turn can itself wait on others, so it holds its slot only while it runs alone: while it waits for a person's answer, or while sessions or subagents it started are under way, it lends the slot back, and once it runs alone again it takes a slot again in line, working on meanwhile. A slot is only ever held by a turn that waits on no other slot, so a budget of any size cannot deadlock, even when every slot's turn starts work of its own.

The price is that `agent_concurrency` bounds the started sessions' turns that run alone, and the subagent runs, but not a turn that has lent its slot: that turn works on while what it started is under way and until it takes a slot again, so each started session with work of its own under way can run one agent beyond the budget. Session control, through which agents start sessions, bounds how many such sessions there can be with its nesting depth and per-caller session caps ([Limits](/mcp-server#limits)).

### Subagents

Subagents that a session starts run in the daemon, and each run is a job of kind `subagent` whose `parent` is the session's job. Its `session` is the session the subagent writes; a subagent on another runtime (Claude, Cursor, Antigravity) keeps that runtime's own and has none. Its state is that of its one run: `queued` until it gets a slot, then `running`, then `completed`, `failed`, `cancelled`, or `interrupted`. A session whose client's host runs its children, as [T3 Code](/integrations/acp/t3-code#subagents-run-as-t3-threads) does, starts no subagent jobs for its `agent` tool: those children are the host's threads. The agent nodes of its workflow runs still run as subagent jobs.

Every session's subagents, and the agent nodes of the workflow runs it starts, share one budget, `agent_concurrency` in your config (10 by default), so at most that many run at once across all your projects. Claude-cli subagents first take a slot in a nested budget, 2 by default and never more than `agent_concurrency` (`RHO_CLAUDE_AGENT_CONCURRENCY` in the daemon's environment), then wait for a slot in the shared one. Each budget gives its slots in the order runs joined its queue. While a run waits, its job usually lists its `queue` place (it can be briefly absent, for example while a run moves between budgets): the budget it waits on, that budget's limit, and its position in line. A change to `agent_concurrency` applies to every session's subagents as soon as the daemon sees it; runs already going keep their slots.

### Workflow runs

A workflow run a session starts with the `workflow` tool (`start`, `run`, or `resume`) runs in the daemon, in the session's workspace, and is a job of kind `workflow` whose `parent` is the session's job and whose `workflow_run` names the run. It is `running` while the daemon drives the run, then `completed` when the run succeeded, `cancelled` when it was cancelled, or `failed`, with the reason, when it failed, was denied or blocked, or could not be driven. Each agent node is a `subagent` job whose `parent` is the workflow job, and takes its slot in the shared budget like any subagent. A node's `timeout_seconds` counts from when it joins the line, so a wait for a slot uses part of it; a node that times out before it gets one fails naming the budget, its limit, and the node's place in line.

A run's capability approvals follow its permission mode as for any background run: nobody is asked. A run the daemon's stop cuts short ends `interrupted` and resumes like a run whose process died: the session's `workflow` tool or `rho workflow resume` picks it up, with `recover_uncertain` (`--recover-uncertain`) once you have checked that no process an interrupted node started is still running. The daemon does not resume runs on its own. A resume through a daemon session's `workflow` tool drives the run in the daemon as a new `workflow` job; `rho workflow resume` drives it in your terminal.

A session's turn that asks for approval, a plan approval, or answers to questions waits in `waiting_for_input` until a client answers or the turn is cancelled. It is never denied because no client is attached: a client that leaves and comes back finds it still open. The one exception is the user's own: with [`[questionnaire] timeout_seconds`](/configuration#questionnaire-timeout) set, a question whose agent offered fallback answers is answered with them once the timeout passes, unless a client answers or [holds](/protocol#timeouts) it first. Any connected client can answer, whether or not it follows the session, and the first answer wins; see [Answering a turn's requests](/protocol#answering-a-turn-s-requests).

### Following a session

Any number of clients can follow a session with `rho/session/attach`: they receive its conversation as events, live, and a client that comes back resumes where it left off, or gets the saved conversation again when the daemon no longer buffers what it missed. See [Following a session](/protocol#following-a-session) in the protocol reference. From a terminal, [`rho run --detach`](/automation-cli#running-in-the-daemon) starts one and [`rho attach`](/automation-cli#attaching-to-a-daemon-session) follows it. Editors and desktop apps open, load, and follow sessions through [`rho acp`](/integrations/acp), which starts the daemon when none runs; closing a thread there detaches, and the session stays in the daemon.

Each job's record is a JSON file in `~/.rho/daemon/jobs/` (`$RHO_HOME/daemon/jobs/` with `RHO_HOME`), rewritten on every state change. Records are kept, even when their session is deleted, so a daemon lists the jobs earlier daemons ran; only the job of a session the daemon [discards unsaved](#unloading-idle-sessions) goes. A prompt or an attach for a saved session the daemon has not loaded, such as one an earlier daemon ran or one the TUI saved, reopens it in its own workspace, whatever workspace the client is in. It runs as the agent it was saved as, and one `/agent:<name>` rebound runs as that agent again under the tools of the agent it was rebound from, never more; if either agent's definition changed or is gone, the reopen fails with `SessionOpenFailed` (-32008), as the TUI's resume does, rather than run the session as another agent. It keeps the directories and parts left out it was saved with, adding those of the attach that reopens it, and takes MCP servers and whether its client shows questions from that attach alone, so a prompt reopens it without them ([protocol](/protocol#a-session-s-setup)). Its next turns are recorded on its last session job, so a session keeps one job across daemons; a session no daemon ran gets a new one. Cancel, steer, unsteer, answer, hold, and `rho/session/configure` never reopen a session, since one that is not loaded has no turn to act on; a client attaches before it changes a session's settings. Nor does `rho/session/observe`, which a client that only reads a session uses: it replays an unloaded session from its file and leaves it unloaded, starting none of its MCP servers, OpenAPI sources, or hooks.

A reopen is refused when:

- another process, such as the TUI or a foreground `rho run`, has the session open: `SessionInUse` (-32012);
- another job of the daemon's, such as a subagent run, writes it: `SessionBusy` (-32004);
- it is a subagent's or workflow node's session, which only the session that started it continues: `SessionOpenFailed` (-32008);
- nothing is saved under the id: `SessionNotFound` (-32003).

While the daemon has a session loaded, other processes cannot open it; see [Unloading idle sessions](#unloading-idle-sessions). A reopened session's job state is the daemon's last record of it: a session continued in the TUI since then still reports how the daemon's last turn ended, until the TUI runs through the daemon ([#1667](https://github.com/matthewyjiang/rho/issues/1667)).

### Unloading idle sessions

A session sits idle while no turn is under way, no subagent or workflow node writes it, no client follows it, and no subagent, workflow run, or background process its prompts started is still running or holding a result for the next prompt. Once a session has been idle for 60 seconds, the daemon unloads it: it closes the session's MCP servers and lets go of the session's file, so the TUI or another rho process can open it. The session's job and saved conversation stay, and the next prompt or attach reopens it, which takes a few tens of milliseconds plus the startup of its MCP servers. The daemon checks once per idle period, so a session unloads between one and two idle periods after it went idle, and the daemon log records each unload with how long the session was idle. A session that has not saved its file yet, because it has had no prompt or its turns saved nothing, has nothing to reopen from, so the daemon discards it instead and logs it as discarded: it closes the session's MCP servers and forgets its job, so `rho/job/list` stops listing it and a later prompt or attach gets `SessionNotFound`. A client that follows an unsaved session keeps it loaded, as it keeps any session. `rho daemon run --unload-idle-after <DURATION>`, such as `--unload-idle-after 10m`, changes the idle period (at least `100ms`); a daemon that a frontend started automatically uses the default.

A reopen starts the session afresh from its file and the current config. Settings a client chose with `rho/session/configure` carry over: the session keeps its model, reasoning level, fast mode, and permission mode with its file from the first change on, as they stand at each save, a plan approval's mode switch included. A session no client changed takes the configured settings again. Approvals granted for the session do not carry over, and a session whose agent definition changed since is refused, as for any reopen.

### Environment

A session's commands run with its client's environment, not the daemon's: each frontend sends its environment variables when it opens, prompts, or attaches to a session, so `bash` finds the same `PATH`, mise shims, `SSH_AUTH_SOCK`, and virtualenv as the terminal you prompted from ([#1659](https://github.com/matthewyjiang/rho/issues/1659)). Each prompt brings the prompting client's environment, so after `mise use` or `export FOO=bar` the next prompt's commands see the change, including those of subagents still running from an earlier prompt; a background process keeps the environment it started with. A command you allowed for the session is asked about again when a prompt brings a different environment, since the environment can change what the command does. An attach changes the environment only when it reopens a session the daemon had unloaded. Provider credentials, such as an exported `OPENAI_API_KEY`, are left out of commands' environments, as tools in every rho session go without them; the session's own models sign in with the ones in the environment that opened it ([Credentials](#credentials)). The daemon never logs or saves the environment, so a session reopened after a restart takes the environment of the client that reopens it.

A session's MCP servers, hooks, and OpenAPI sources use its environment too. MCP servers and OpenAPI sources take the environment current when the session's tool sources load: when the session opens or reopens, the opening client's, or when `/agent:<name>` rebinds it mid-prompt, the prompting client's. An MCP server's executable is looked up on that environment's `PATH`, and the server takes its base variables, `env_from_env`, and `headers_from_env` from it and keeps them while it runs; an OpenAPI source reads its environment variables from it. A hook takes its base variables and its `env` names from the environment current when it runs. A variable you name this way is read even when it is a provider credential. A session opened or reopened by a client whose environment sets `RHO_IN_HOOK`, such as `rho` run by a hook, runs no hooks, so a hook cannot trigger itself through the daemon; a daemon a hook started automatically does not inherit the marker, so the daemon's other sessions keep theirs.

CLI subagents ([Claude Code](/subagents/claude-cli), [Cursor](/subagents/cursor), and [Antigravity](/subagents/antigravity)) and a [Claude Code advisor](/configuration/advisor-mode#claude-code-as-the-advisor) use the session's environment as it is when they start: their executable is looked up on its `PATH`, and they run with all of it, provider credentials such as `ANTHROPIC_API_KEY` or `CURSOR_API_KEY` included, since they sign in themselves, as they would from your terminal. A workflow run that a session's `workflow` tool or `/workflow:<name>` starts or resumes takes the session's environment as it is at that moment and keeps it until the run stops: its hooks, command nodes, rho agent nodes, and CLI agent nodes all run in it, and later prompts do not change it. Planning looks up command executables, their `#!/usr/bin/env` interpreters, and CLI agent binaries on that environment's `PATH` and freezes them, and each node checks them again on the same `PATH` before it starts. A run you start or resume with `rho workflow` uses your terminal's environment instead. Rho never saves the environment with a run, so a run resumed from another session or terminal takes that one's; when its `PATH` finds a different interpreter for a frozen script, the run fails closed with a `frozen filesystem identity drift` error rather than running a script under an interpreter it was not planned with.

Project trust (`RHO_TRUST_PROJECT_HOOKS`, `RHO_TRUST_PROJECT_AGENTS`, `RHO_TRUST_PROJECT_PLUGINS`, and `RHO_TRUST_PROJECT_OPENAPI`) comes from the session's environment too, not the daemon's ([#1779](https://github.com/matthewyjiang/rho/issues/1779)). A session reads it once, from the client that opens it or reopens it after the daemon unloaded it, and keeps it while it stays loaded, since it loads its hooks, plugins, and agents when it opens. A workflow run a session starts or resumes has the session's trust; one you start with `rho workflow` has your terminal's. Whoever started the daemon decides nothing: a session opened from a terminal without the variables trusts nothing, even when the daemon was started from one that set them.

A prompt from a client whose trust is not the session's is refused, whether that client trusts less or more: the session would otherwise run a project hook for a client that never trusted it, or leave out one the client expected. The error names the variables that differ, for example that the session trusts its project through `RHO_TRUST_PROJECT_HOOKS`, which the prompting client's environment does not set. Open a session of your own from that terminal, or prompt with the session's variables set. A refused prompt starts no turn and changes nothing for the clients attached to the session. Attaching to a session and following it need no matching trust, and steering a running turn carries no environment, so it is not checked. Because the daemon unloads a session left idle, the client whose prompt or attach reopens it decides its trust from then on: a session opened as a project agent reopens only for a client that trusts the project's agents, and once a client without trust has reopened a session by attaching to it, a prompt from the trusting client that opened it first is refused until the daemon unloads the session again. Replaying a saved session the daemon has not loaded starts nothing of it, so the replay finds a project agent the session was saved as without asking for trust.

## Starting automatically

Frontends that use the daemon, such as `rho run --detach` and `rho daemon start`, start it when none is running, so you rarely run `rho daemon` yourself. If several start at once, they agree on a single daemon. A frontend that finds the previous daemon still shutting down waits for it to exit, up to 5 seconds, and then starts the new one; a daemon still holding its lock after that fails the frontend with the limit and the time it waited. A frontend that finds a daemon of an older protocol version that nobody is using replaces it the same way ([Versions](#versions)). The started daemon runs `rho daemon run` from the same `rho` executable in its own process group, so `Ctrl+C` in your terminal does not stop it.

A daemon a frontend starts does not inherit that frontend's environment ([#1779](https://github.com/matthewyjiang/rho/issues/1779)). It serves every later client, so it starts with only the variables that describe the daemon and the machine, and everything else stays with the frontend and reaches a session as [its client's environment](#environment):

| Kept | Variables |
|---|---|
| Where its files are and who it runs as | `HOME`, `USER`, `LOGNAME`, `PATH`, `TMPDIR`, `XDG_RUNTIME_DIR`, `XDG_CACHE_HOME`, `XDG_CONFIG_HOME`, `GIT_CONFIG_GLOBAL`, `LD_LIBRARY_PATH`, `DYLD_LIBRARY_PATH`; `RHO_HOME` is set to the home the frontend resolved |
| Locale | `LANG`, `LC_ALL`, `LC_CTYPE`, `TZ` |
| How it reaches the network | `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`, `NO_PROXY` and their lowercase forms, `SSL_CERT_FILE`, `SSL_CERT_DIR`, `RHO_SSRF_ALLOW_RANGES` |
| The daemon's own settings | `RHO_LOG`, `RHO_CLAUDE_AGENT_CONCURRENCY`, `RHO_MODELS_PATH`, `RUST_BACKTRACE` |

Nothing of the starting frontend's identity is on the list: not its project trust, and not its [credentials](#credentials), which are API keys and tokens such as `OPENAI_API_KEY`, `GITHUB_TOKEN`, and `CODEX_ACCOUNT_ID`, the store choice `RHO_CREDENTIAL_STORE`, and the Ollama device key directory `OLLAMA_DEVICE_KEY_DIR`. Each session takes those from the client that opens it. `DBUS_SESSION_BUS_ADDRESS` is not on the list either, and no session supplies it: the daemon reaches the OS keyring on the default session bus ([Credentials](#credentials)). `RHO_SSRF_ALLOW_RANGES` is the one setting here that every session shares: it widens what every session's web tools may reach. Any other variable the daemon's own code reads from its environment has to be set where you start the daemon yourself: `rho daemon run`, or a service manager, keeps the whole environment it was given.

## Upgrading in place

Updating rho replaces its executable, which leaves a running daemon on the old version. The daemon notices on its own, whatever replaced the executable (`rho update`, a package manager, `cargo install`), and upgrades without interrupting work ([#1713](https://github.com/matthewyjiang/rho/issues/1713)):

- **Noticing.** Once a second, the daemon checks the path it was started from: the one a service unit's `ExecStart` names, or, for a daemon a frontend started, that frontend's `rho` executable with symlinks resolved. A new file there is a replacement, which the daemon runs with `--version` first; one that does not print a rho version is never executed, and the log says why. A client of a newer rho that the daemon accepts makes it check at once.
- **Waiting.** Until nothing would be cut short, the daemon keeps serving and taking new work. `rho daemon status` shows `upgrade  pending to <version> (<N> in progress)`, as does `upgrade` in `rho/daemon/status`. What keeps it waiting: jobs queued, running, or waiting for input (a job waiting for input is not yet asked again after a restart, [#1661](https://github.com/matthewyjiang/rho/issues/1661)); interactive logins under way; and sessions with work of their own, which are the same ones [unloading](#unloading-idle-sessions) keeps loaded: background processes, subagents, or workflow runs still running or holding a result, or a settings change being applied. A session a client follows that has saved nothing yet, because it has had no prompt, also keeps it waiting, since a restarted daemon could not reopen it for that client.
- **Re-executing.** At the first check that finds nothing it would cut short, the daemon detaches its clients with the reason `restarting`, closes their connections, unloads its sessions, and executes the new binary as the same process. It keeps its pid, so a service manager sees the same service, and it hands the new binary its lock and listening socket, so no other daemon can start in between and a client that connects meanwhile waits until the new binary answers rather than finding nothing there. Job records and saved sessions carry over, and sessions reopen on their next prompt or attach.

`rho daemon restart` re-executes at once instead, without waiting: it interrupts the turns under way, which are recorded `interrupted` as a stop records them, and the sessions take prompts again once the daemon is back. It runs whatever executable is at the daemon's path, so it also applies an upgrade the daemon has not checked yet. When the restarted daemon is still older than the `rho` you ran it from, `restart` stops it and starts that `rho` instead. That is an ordinary stop and start, not an in-place restart: the pid changes, and a client that connects in between finds no daemon and waits for the new one, as after any stop. If the turns under way do not stop within 4 seconds, the daemon exits instead of re-executing, as a stop does, and the next connection starts a new daemon.

An installer that puts the new version at another path, such as a versioned directory or a symlink switched to it (mise, Homebrew), leaves a frontend-started daemon's path unchanged, so the daemon cannot upgrade itself; it logs that a newer client connected, or that it refused one of a newer protocol. Run `rho daemon restart` from the new version, which then starts it in the old daemon's place. When the new version also speaks a newer protocol, a frontend replaces a daemon nobody is using itself, as [Versions](#versions) describes. Windows has no in-place restart yet ([#1665](https://github.com/matthewyjiang/rho/issues/1665)).

## Versions

Every connection begins with a handshake that compares protocol versions, so a frontend and a daemon of different rho builds learn there whether they can work together, rather than at the first request one of them does not understand ([#1776](https://github.com/matthewyjiang/rho/issues/1776)).

The protocol is unstable while its version is 0.x: a frontend and daemon work together only when their protocol versions are equal. Until the 3.0 beta the version is 0.1 and a change to the protocol does not raise it, so two builds can report the same version and still differ: run a frontend and a daemon from the same build. From protocol 1.0 on, they work together when their major versions match.

| The frontend finds | What happens |
| --- | --- |
| A daemon of a compatible protocol version | It connects, whatever rho version each side is. A daemon whose executable was replaced [upgrades itself](#upgrading-in-place) once nothing would be cut short. |
| An older daemon nobody is using: nothing in progress and no other client connected | A frontend that [starts the daemon](#starting-automatically) stops it and starts its own rho in its place, then connects. `rho daemon status` and `rho attach`, which never start a daemon, report the daemon as the next row does. |
| An older daemon in use | The frontend fails with the fix and what it costs, for example ``rho daemon speaks protocol 0.1, older than this client's 0.2; run `rho daemon restart` to replace it, which interrupts 2 in progress (jobs, logins, or background work) and disconnects 1 other client``. Run it, or wait until the daemon is no longer in use. |
| A newer daemon | The frontend fails and says to update it, or to restart it if it was updated while running. A TUI left open across an update is such a frontend once the daemon upgrades to a new protocol version: its reattach fails with that message, and reopening it with `--resume` continues the session. A frontend never replaces a newer daemon; only that older rho's own `rho daemon restart` does. |

What counts as in progress is what an [upgrade](#upgrading-in-place) waits for: jobs queued, running, or waiting for input, interactive logins under way, and sessions with work of their own. The other clients are the frontends connected to the daemon, such as an open TUI, a `rho acp` adapter an editor started, or `rho mcp serve`, whether or not they are doing anything. They speak the daemon's protocol, so replacing the daemon leaves each without one it can use until it is restarted from the newer rho; that is why a frontend never replaces a daemon another is connected to, and why the refusal counts them.

The daemon reports both counts when it refuses the handshake, so they are the counts at that moment, and its log records the refusal. A frontend that replaces a daemon stops it as `rho daemon stop` does and starts its own as it starts any daemon: the pid changes, a turn that began after the daemon reported none is recorded `interrupted`, not lost, and a daemon that ran under a service manager or with `rho daemon run` options is followed by one that does not.

`rho daemon stop` and `rho daemon restart` work on a daemon of any protocol version.

What each protocol version may change is described in [Versions and compatibility](/protocol#versions-and-compatibility).

## Socket and permissions

The daemon's socket, `daemon.sock`, lives in a runtime directory of its own rho home:

- `$XDG_RUNTIME_DIR/rho/<home id>` when `XDG_RUNTIME_DIR` is set to an absolute path, as on most Linux desktops. `<home id>` is the first 16 hex digits of the SHA-256 of the rho home's real path, so every rho home gets its own socket.
- Otherwise `<rho home>/run`.

`RHO_HOME` only picks the rho home; the socket follows from the home as above.

The rest of the daemon's files live in the rho home's state directory, `~/.rho/daemon` (`$RHO_HOME/daemon` with `RHO_HOME`):

| File | Purpose |
| --- | --- |
| `daemon.lock` | Held by the running daemon, so only one daemon runs per rho home even when two resolve different runtime directories. |
| `daemon.json` | The running daemon's pid, version, protocol, start time, and socket. |
| `jobs/` | One record per job (see [Sessions and jobs](#sessions-and-jobs)). |

A `spawn.lock` in the runtime directory lets concurrent frontends start exactly one daemon.

Only your user can reach the daemon:

- The runtime directory and the state directory are created with mode `0700`. If one already exists, rho refuses to use it unless it is a real directory (not a symlink) that you own and that grants no group or other access.
- The socket has mode `0600`.
- The daemon checks the user id of every connecting process and closes connections from other users.
- The daemon listens on no TCP port.

Unix socket paths are limited to 107 bytes on Linux and 103 on macOS. If the socket path would be longer, rho fails with the limit and the actual length; point `RHO_HOME` or `XDG_RUNTIME_DIR` at a shorter directory.

The daemon is not supported on Windows yet ([#1665](https://github.com/matthewyjiang/rho/issues/1665)), and neither are the clients that need it, such as `rho acp`.

## Logs

When the daemon runs under systemd with stderr connected to the journal (`JOURNAL_STREAM` matches stderr), it logs to stderr and journald records it. Otherwise it appends to `~/.rho/logs/daemon.log` (`$RHO_HOME/logs/daemon.log` with `RHO_HOME`). When a turn fails to authenticate, clients are told only how to log in, and the provider's own diagnostic is logged here as a warning. It may echo credentials, so treat the log as private. The daemon creates the file readable only by you (mode 0600). Under journald that mode does not apply: anyone who can read your journal can read the diagnostic, and the journal keeps it by its own retention rules.

`RHO_LOG` sets the log filter with the same syntax as for other rho commands, for example `rho=debug`. The daemon defaults to `info`.

## Credentials

A session's credentials are those of the client that opens it ([#1779](https://github.com/matthewyjiang/rho/issues/1779)). An API key or token in that client's environment, such as `OPENAI_API_KEY`, `GITHUB_TOKEN`, or a web-search key, signs in that session, its subagents, and the models that run beside it, and wins over a saved credential there; it reaches no other client's session. A client that sets none gets none from anyone else: its sessions read the [credential store](/authentication-and-models#where-credentials-live), and without a saved credential they ask you to sign in. Like project trust, a session keeps the credentials of the environment it opened with while it stays loaded, so a later prompt from a client with another key runs on the opener's; the client that reopens an unloaded session brings its own. A session an agent starts through the `sessions` tool opens with the environment of the prompt that is running, so it signs in as that prompt's client. Reading a saved session the daemon has not loaded needs no credential, since it runs nothing. A credential variable in the daemon's own environment signs in no session, and the daemon's log says so at startup when it finds one: put the key in the environment you run rho from, or save it with `rho login`.

The store is the daemon's own unless the client says otherwise: the one its config selects, or `RHO_CREDENTIAL_STORE` in the environment of a daemon you started yourself, else the OS store. A client whose environment sets `RHO_CREDENTIAL_STORE` gets that store for its sessions and its sign-ins instead.

The [Ollama Cloud device key](/providers/ollama-cloud#setup-with-a-device-key) is a file, not a store entry, and its directory is the client's too ([#1789](https://github.com/matthewyjiang/rho/issues/1789)): a client whose environment sets `OLLAMA_DEVICE_KEY_DIR` signs in with, and has its sessions read, the key in that directory, and a client that sets none uses `~/.ollama` in the daemon's home. Set an absolute path: the daemon resolves a relative one against its own working directory, not yours. The daemon's own `OLLAMA_DEVICE_KEY_DIR` is never read for a session. That you signed in is still recorded in the store, not with the key, so clients that share a store share the record: after one signs in, another that already has a key in its own directory counts as signed in with that key, and a logout from either signs both out.

A frontend signs in with an API key, or out, through [`rho/auth/key` and `rho/auth/logout`](/protocol#signing-in-and-out), or runs a provider's interactive login in the daemon through [`rho/auth/login`](/protocol#logging-in), which relays the login's URL and code to the frontend; `rho login` does that, starting the daemon when none runs ([#1662](https://github.com/matthewyjiang/rho/issues/1662)), and so does [`rho acp`](/integrations/acp#sign-in) for an ACP host's sign-in. Each one sends its environment and writes the store that client's sessions read, a login once the provider approved it, and applies to every session reading that store from the session's next prompt, whichever client opened it; a refresh of a provider's OAuth tokens does too. Any such change rebuilds every running session's provider at its next prompt, never during a turn, whichever auth profile it uses. MCP servers' OAuth tokens and web-search keys feed no provider, so saving them rebuilds nothing. A key set in the signing-in client's environment cannot be replaced or removed this way, since it would still win in that client's sessions: unset it there first.

The daemon may not reach the store its config selects. A daemon a frontend started does not take that frontend's `DBUS_SESSION_BUS_ADDRESS`, so on Linux it reaches the Secret Service only through the default session bus at `$XDG_RUNTIME_DIR/bus`; started from an SSH login, or anywhere without a desktop session, it has none, and the keyring connects once per process, so that daemon cannot reach it until it restarts. The daemon fails closed, as every rho process does: a sign-in to a store it cannot reach is refused with `CredentialStoreUnavailable`, an interactive login before the provider is asked, so you approve nothing that cannot be saved, and it never falls back to the file store on its own. `rho daemon status` shows the daemon's own store, what chose it (default, config, or `RHO_CREDENTIAL_STORE` in the daemon's environment), and why it cannot be reached, as does [`rho doctor`](/automation-cli). When nothing chose a store, in your shell or for the daemon, `rho login` asks with the daemon's answer, offering the file store as the default when the OS store is unreachable, and the daemon saves the choice through [`rho/auth/choose_store`](/protocol#credential-store). When config or `RHO_CREDENTIAL_STORE` already chose a store the daemon cannot reach, `rho login` says where to choose the file store instead. A store chosen from outside the daemon, with `rho credential-store set file` or an edit of `config.toml`, is the one the next session it opens reads, and the next sign-in writes; no restart is needed. The check behind the status writes nothing, so a locked keyring shows as reachable until a write is refused. Keychain access from a daemon started on macOS has not been verified yet.

A `/login` or `/logout` in a TUI runs in that process until the TUI becomes a daemon client ([#1667](https://github.com/matthewyjiang/rho/issues/1667)). It changes the same store, but providers keep the credentials they were built with, so the daemon's running sessions do not notice at once. New sessions read the change. A running session rereads the store once the provider refuses its credentials, or once a prompt found none there, as after a logout, so a new login fixes it at the next prompt, while a session signed out elsewhere keeps working until the provider refuses its key.

## Configuration changes

The daemon serves and changes [configuration](/configuration) for its clients over [`rho/config/*`](/protocol#settings), and reads `config.toml` again whenever it changed, by a client or by hand, the next time a request, a session opening, or a turn touches it. Each setting says when a change takes effect: `agent_concurrency` at once, the advisor and automatic compaction settings and the codemode mode at the next turn of every loaded session, and the rest in sessions opened later. A hand edit that no longer loads keeps the last good settings in force; `rho/config/get` lists its errors until you fix it. If `config.toml` never loaded since the daemon started, sessions do not open. Daemon-level settings, which are the `RHO_LOG` filter, the socket location (`XDG_RUNTIME_DIR`, `RHO_HOME`), and the credential store, are read once at startup. Restart the daemon with `rho daemon stop` to change them; a [sign-in or sign-out](#credentials) through the daemon also moves it to the credential store the config selects then, and `rho/auth/choose_store` moves it to the store a client chose. New sessions read that store, while running sessions keep the credentials they hold until a write moves them or their provider refuses them.

## Checking the daemon

[`rho doctor`](/automation-cli) reports on the daemon without starting one, each row with what to do about it; `rho doctor --json` carries the same rows in a `daemon` section, the credential store's under `authentication`.

| Row | Reports | An issue when |
| --- | --- | --- |
| `Daemon` | Whether one runs, with its pid, rho version, and protocol version. | Its protocol is incompatible with this `rho`'s (fails; see [Versions](#versions)), unless it is an older daemon nobody is using, which the next client replaces (warns). It listens on another socket than this shell resolves, because it started with another `XDG_RUNTIME_DIR` (fails). Its rho version is another one (warns). A daemon that is not running is not an issue: the next client starts it. |
| `Daemon socket` | Whether the socket and its directory are yours alone. | Either is open to group or others, owned by someone else, or not what it should be (fails; see [Socket and permissions](#socket-and-permissions)). |
| `Service` | Whether the [service](#running-as-a-service) for `rho mcp serve --http` is saved under the name the examples use, `rho-mcp.service` or `dev.rho.mcp.plist`, and on Linux whether it is enabled. It reads the files in your own unit directory and asks no service manager, so a unit under another name or installed system-wide is not found. | A systemd unit is saved but not enabled (warns). No service is not an issue: the daemon needs none. |
| `Daemon environment` | Which of the variables the daemon reads from [its own environment](#starting-automatically) differ from this shell's, by name only. | A network variable or daemon setting differs, such as `HTTPS_PROXY` or `RHO_LOG` (warns): the daemon keeps what it started with. Variables such as `PATH` that only differ between shells are listed without a warning, as sessions use their client's. Linux only: elsewhere the row says the environment cannot be read. |
| `Daemon credential store` | The store the daemon keeps provider credentials in and what chose it. | The daemon cannot reach it (see [Credentials](#credentials)). |

On Windows the report has one `Daemon` row saying the daemon is not supported yet.

## Running as a service

The daemon needs no service of its own: every frontend starts it on demand. Only a process that has to listen before any client shows up needs a service manager, and today that is [`rho mcp serve --http`](/mcp-server#over-http). It is a daemon client like any other, so its first tool call starts the daemon from inside its service, and its unit is all the always-on setup there is. Use `rho daemon start` in a script that wants the daemon running before anything connects.

Both examples below make the same three choices:

- **Stopping the service leaves the daemon running.** The daemon the server starts runs in a process group of its own, but systemd stops every process in a service's control group by default, and a new process group does not leave it: restarting or stopping the MCP server would stop the daemon and every turn it runs. `KillMode=process` makes systemd stop only the server. launchd kills what is left of a job's process group when the job exits; the daemon is not in the server's group, and `AbandonProcessGroup` keeps launchd from killing anything the server leaves behind all the same. Stop the daemon itself with `rho daemon stop`.
- **`PATH` is set.** A service starts with the service manager's bare environment, not your shell's, and the sessions the server starts run with the server's environment: their tools, [MCP servers](/integrations/mcp), and [CLI subagents](/subagents) find executables on its `PATH` (see [Environment](#environment)). Set it to the directories your tools live in, and add any variable sessions need that your shell would give them, such as a provider key.
- **The server listens on loopback.** Reach it from elsewhere through a tunnel, as [Over HTTP](/mcp-server#over-http) describes.

### systemd

Save as `~/.config/systemd/user/rho-mcp.service`, changing `ExecStart` to where `rho` is installed and `PATH` to your own:

```ini
[Unit]
Description=rho MCP server
After=network.target

[Service]
ExecStart=%h/.local/bin/rho mcp serve --http 127.0.0.1:8765
# The sessions this server starts look up executables on this PATH.
Environment=PATH=%h/.local/bin:/usr/local/bin:/usr/bin:/bin
# Stop only the server: the daemon it started keeps running its turns.
KillMode=process
Restart=on-failure

[Install]
WantedBy=default.target
```

Then start it, and keep it running across logins:

```bash
systemctl --user daemon-reload
systemctl --user enable --now rho-mcp
loginctl enable-linger "$USER"   # optional: keep it running while you are logged out
```

`systemctl --user restart rho-mcp` restarts the server and leaves the daemon and its running turns alone. The server logs to the journal (`journalctl --user -u rho-mcp`); the daemon it starts writes its own log to `~/.rho/logs/daemon.log`, as [Logs](#logs) describes.

### launchd

Save as `~/Library/LaunchAgents/dev.rho.mcp.plist`, changing the `rho` path, `PATH`, and the log path to your own:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>dev.rho.mcp</string>
  <key>ProgramArguments</key>
  <array>
    <string>/opt/homebrew/bin/rho</string>
    <string>mcp</string>
    <string>serve</string>
    <string>--http</string>
    <string>127.0.0.1:8765</string>
  </array>
  <!-- The sessions this server starts look up executables on this PATH. -->
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key>
    <string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin</string>
  </dict>
  <!-- Stop only the server: the daemon it started keeps running its turns. -->
  <key>AbandonProcessGroup</key>
  <true/>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardErrorPath</key>
  <string>/Users/you/Library/Logs/rho-mcp.log</string>
</dict>
</plist>
```

Then load it:

```bash
launchctl bootstrap "gui/$(id -u)" ~/Library/LaunchAgents/dev.rho.mcp.plist
launchctl kickstart -k "gui/$(id -u)/dev.rho.mcp"   # restart the server
```

The server logs to `StandardErrorPath`; the daemon it starts writes its own log to `~/.rho/logs/daemon.log`, as [Logs](#logs) describes.
