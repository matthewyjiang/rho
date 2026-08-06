# Background processes

Parent: [Tools and workspace](/tools-workspace).

The `process` tool has three actions. `start` launches a background shell command and returns its process ID; it accepts an optional timeout. `poll` requires a process ID and returns retained stdout and stderr, optionally continuing from a cursor or waiting briefly for changes. Continue from the returned `next_cursor` to avoid duplicate output. Retention is bounded, so sufficiently old output can be discarded; poll results report when a requested cursor predates the retained range. `stop` requires a process ID and terminates the managed process tree.

Rho owns these processes only within the running instance. It cleans them up when that instance shuts down, and process records do not persist across restarts. The tool does not support stdin writes, process listing, pseudo-terminals, persistent sessions, or pane and session orchestration. Use a dedicated multiplexer such as tmux or Herdr when you need interactive terminals or persistent, orchestrated sessions.

Managed processes use standard output and error pipes, with standard input closed. Commands that require interactive input or terminal emulation will not behave as they do in a foreground terminal. The tool executes shell commands with the same user permissions as Rho. Rho's [permission modes](/configuration#permission-modes) can deny or request approval before process execution, but they do not add operating-system sandboxing.
