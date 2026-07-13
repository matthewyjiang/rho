---
name: rho-diagnostics
description: Diagnose the running Rho harness, including runtime identity, context, prompt sources, tools, and sanitized configuration.
---

# Rho diagnostics

Use the read-only `rho` tool when troubleshooting Rho itself or developing Rho harness behavior.

Available actions:

- `info`: Rho version, provider, model, and reasoning level.
- `context`: Latest known token usage, context window, and whether usage was estimated, provider-reported, or unknown after compaction. A null result means no turn has reported context yet.
- `prompt_sources`: Prompt source kinds, paths, and rendered byte contributions to the exact system prompt. It never returns prompt or instruction contents.
- `tools`: Names of tools available to the current or most recent model request.
- `config`: Sanitized live operational configuration. Restart-only settings continue to show the values used at startup. It excludes credentials, authentication values, model favorites, keybindings, prompt templates, and other user content.

Request only the action needed. Do not collect all diagnostics by default. Treat values as a live snapshot that may change after model switches, applicable configuration updates, or turns.
