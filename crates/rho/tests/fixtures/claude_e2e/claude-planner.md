---
id: claude-planner
description: Deterministic Claude Code planner for fake-runtime PTY E2E.
runtime: claude-cli
model: claude-opus-demo
tools: [Read, Edit, "Bash(git *)"]
inherit_claude_config: false
prompt: replace
---
Produce a short plan. Prefer reading before editing.
