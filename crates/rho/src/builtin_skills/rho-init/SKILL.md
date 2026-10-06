---
name: rho-init
description: Survey the repository and create or update its AGENTS.md project instructions.
disable-model-invocation: true
---

# Project instruction onboarding

Survey the repository, then write concise instructions to the target `AGENTS.md` path supplied by `/init`. Work from the target's parent directory (the git root, or the current directory outside a repository), even when Rho was started in a nested directory. `AGENTS.md` is Rho's only instruction filename; do not create `CLAUDE.md` or other instruction files.

## Survey

Read the README, package/build manifests, CI configuration, and lint/test configuration. Inspect enough of the layout and representative code to identify conventions that are actually used. Read an existing target before making any changes. Also consult `.cursor/rules`, `.github/copilot-instructions.md`, and `CLAUDE.md` when present as source material only; Rho does not load them as instructions. Do not blindly copy conflicting or obsolete guidance.

## Write or update

Focus on what an agent cannot readily guess:

- Exact build, test, and lint commands supported by the manifests and CI.
- The important layout and ownership boundaries.
- Non-obvious coding conventions and constraints.
- Commit and pull-request rules from the repository's contribution guidance.

Keep it short, concrete, and repository-specific. Omit generic advice, obvious language conventions, exhaustive file inventories, and invented commands. Survey with read-only tools; do not run builds or tests merely to discover their commands. Do not copy secrets into instructions.

If the target does not exist, create it with `write`. If it already exists, preserve its structure, voice, and unrelated guidance. Make only targeted additions or corrections using the live file-edit tool; never rewrite an existing file wholesale. Recheck the target before writing so a newly created file is not overwritten.

## Finish

Tell the user the final `AGENTS.md` path and briefly summarize what changed. Instructions are re-read from disk when they start or switch to a different session (`/new`, `/resume`, or cross-session tree selection), or restart Rho. The current session keeps its cached instructions, including on model switches and same-session tree navigation.
