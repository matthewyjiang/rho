---
name: rho-agent-creator
description: Create a new Rho agent through a guided questionnaire. Use when the user wants to define a custom agent, subagent, delegated role, or reusable specialist.
---

# Rho agent creator

Guide the user through creating one valid agent definition. Do not jump directly to a file. Collect decisions step by step with the `questionnaire` tool, draft the definition, confirm it, write it safely, and verify it.

Agent definitions are Markdown files with YAML frontmatter and a prompt body. Valid discovery locations are:

- `~/.agents/agents/<id>.md` for the shared agents home
- `~/.rho/agents/<id>.md` for Rho-specific global agents
- `<project-root>/.agents/agents/<id>.md` for a project agent

Project agents are loaded only when the project is trusted, currently by starting Rho with `RHO_TRUST_PROJECT_AGENTS=1`.

## 1. Scope and identity

Use one questionnaire with these choices:

1. Save location: shared global (`~/.agents/agents`), Rho global (`~/.rho/agents`), or current project (`<project-root>/.agents/agents`).
2. Agent ID: offer an Other response so the user can enter it.
3. Description: offer an Other response. Explain that this is the short delegation metadata other agents use to decide when this agent is appropriate.

Validate the ID before continuing. It must contain 1-64 lowercase ASCII letters, digits, or single hyphens, with no leading hyphen, trailing hyphen, or double hyphen. The destination filename must be `<id>.md`. The description must contain 1-1024 characters.

## 2. Role and behavior

Ask what the agent should accomplish and how it should behave. Use an Other response for the role/instructions. Ask follow-up choices when useful, such as whether it may modify files, what it should avoid, what its final response should contain, and when it should ask the user rather than proceed. Keep this conversational and do not ask for information the user already supplied.

## 3. Capabilities

First ask whether the agent should receive all tools or a focused allowlist. If the user chooses an allowlist, use a multi-select questionnaire containing only these valid tool names:

`agent`, `agents`, `bash`, `edit_file`, `fetch_content`, `get_search_content`, `list_dir`, `powershell`, `process`, `questionnaire`, `read_file`, `rho`, `shell`, `skill`, `web_search`, `write_file`

Then ask for reasoning level with these choices: inherit/default, off, minimal, low, medium, high, xhigh, max. Omitting `reasoning` means the selected model's normal default.

## 4. Model policy

Ask for one model policy: `inherit`, `prefer`, `require`, or `select`. Explain that `inherit` keeps the parent agent's provider and model, while every other policy names a model selection. Do not invent finer behavioral differences between the non-inherit policies.

If the answer is not `inherit`, ask for the model ID and optional provider. Both values must be non-empty and contain no whitespace when present. A model is required for `prefer`, `require`, and `select`. Do not emit `model` or `provider` for `inherit`.

## 5. Prompt policy

Ask whether the body should:

- `extend` the standard Rho system prompt
- `replace` the system prompt completely

Use a choice questionnaire with `default: "extend"` and `default_selection: "focused"` so extend is recommended without being pre-selected. Explain that `replace` needs a non-empty, self-contained body. Draft a concise body from the user's answers. It should state the role first, then give concrete operating rules, boundaries, and completion expectations. Do not repeat metadata merely to make the body longer.

## 6. Draft and confirm

Construct valid content in this shape, omitting optional fields that were not selected:

```markdown
---
id: example-agent
description: Use for ... Not for ...
prompt: extend
model-policy: inherit
reasoning: medium
tools: [read_file, list_dir]
---

You are ...

- ...
```

`prompt` must be `extend` or `replace`. `model-policy` must be `inherit`, `prefer`, `require`, or `select`. `tools` must be `all` or a YAML list of valid names. Present the exact destination path and complete proposed file to the user, then ask for confirmation with a confirm questionnaire. Revise and reconfirm if requested.

## 7. Write safely and verify

Before writing, inspect the destination. If `<id>.md` already exists, read it and ask for explicit overwrite confirmation. Never overwrite based only on the earlier draft confirmation. Create the destination directory if needed using the available platform shell, then use `write_file` for the definition.

After writing:

1. Read the file back.
2. Check that the frontmatter delimiters, ID, description, policies, tools, and non-empty body are present and match the confirmed draft.
3. Correct only clear serialization mistakes. For any semantic change, ask first.
4. Tell the user the final path.
5. Ask the user to run `/agents`. Opening `/agents` reloads definitions from disk and shows the new agent and its metadata.

Mention that project-scoped agents need project trust. Do not claim that an already initialized delegation tool schema has changed merely because the file was written. The new agent is guaranteed to be available after starting a new Rho session that loads it.
