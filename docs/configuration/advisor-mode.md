# Advisor mode

Parent: [Configuration](/configuration).

Advisor mode gives the agent an `advisor` tool backed by a second model. When the agent calls it, Rho sends the session transcript to that model and returns its guidance as the tool result. The idea is to let a stronger reviewer check the plan before the agent commits to it, when it is stuck, and before it declares the work done.

- Rho runs the advisor itself. There is no server-side advisor and no provider beta flag, so any model in Rho's [catalog](/authentication-and-models#selecting-models) can be the advisor, on any provider.
- The tool takes no parameters. Rho serializes the transcript, so nothing the agent writes reaches the advisor unedited. The transcript covers the system prompt, your requests, every tool call, every result, and the turn in flight.
- The advisor runs bare: one request, no tools, guidance text only. It cannot read files, run commands, or edit anything.
- Advisor mode needs an advisor model. Set `[internal_agents.advisor]` or choose one from `/advisor`, `/agents`, or **Agent behavior** > **Advisor mode** in `/config`. With `advisor_mode = true` and no model, Rho shows `advisor: no model` in the status line and offers no tool.
- The advisor must resolve to the `rho` runtime. A `claude-cli` advisor is rejected with a clear error.
- Changes apply before the next turn. The session ID and history are kept.
- Advice appears in the transcript as an ordinary tool card, collapsed past the [output limit](#tool-output-limit) and expandable with `ctrl+o`.
- Advisor calls are billed to the advisor model's provider and recorded in the [usage ledger](/usage-ledger) under the `advisor` purpose.
- [Automation runs](/automation-cli) honor `advisor_mode` too, so `rho run` gets the same tool. Subagents and workflow runs do not: the advisor reviews the root session, and a child run has its own.

See [`/advisor`](/interactive-tui#commands) for the interactive command.
