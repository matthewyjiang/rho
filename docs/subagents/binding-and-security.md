# Binding and security

Parent: [Agents and delegation](/subagents).

Every invocation goes through the same binder. Binding is runtime-specific:

- `runtime: rho`: resolve model aliases and reasoning against the host config, render prompt policy, and intersect requested Rho tools with host-supplied capabilities. Host policy remains the upper authority boundary.
- `runtime: claude-cli`: copy `model` byte-for-byte (or omit it when inherited), keep the Claude tool list, map optional `reasoning:` to Claude `--effort` (`low`/`medium`/`high`/`xhigh`/`max`), and record `inherit_claude_config`. No Rho model-alias resolution and no mutation of the parent provider/model config. Rho-style `@alias` model values and `reasoning: off` / `reasoning: minimal` are rejected. `runtime: claude-cli` is delegated-only: interactive and automation roots cannot bind it.

Delegated Rho invocations do not receive `agent` or `agents`, so they cannot recursively delegate. Background delegated Rho agents under an interactive parent may use the questionnaire tool. The child pauses on that request, the parent TUI presents the structured form without blocking its active turn or goal loop, and the answer is routed back to the same child run. TUI approvals and questionnaires still use one shared interaction slot, so concurrent requests wait in order. Foreground delegated agents and headless automation omit questionnaire support. Each delegated run owns a fresh run status file, cancellation token, and attachment stream. Rho-runtime delegated runs also own a fresh SDK runtime, session, tool registry, and usage accounting. Claude-cli delegated runs spawn an external `claude` process instead of an in-process SDK loop. Immutable configuration and provider infrastructure may be shared for Rho runs.
