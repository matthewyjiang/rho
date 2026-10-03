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
| Plan | `plan` | no | File writes and process execution are denied. Reads outside the workspace are denied except `~/.rho/AGENTS.md`, skill trees, and agent definitions. |
| Supervised | `supervised` | no | Writes, process execution, other outside reads, and unknown capabilities need approval. |

In Auto, Allow edits, and Supervised, gitignored and untracked paths still ask, as do writes outside the workspace, including writes to global `AGENTS.md`, skill trees, and agent definitions. Plan denies those writes.

These reads are allowed in every checked mode, including Plan: workspace-scoped reads, `~/.rho/AGENTS.md`, skill trees (`~/.rho/skills`, `~/.agents/skills`), and agent definitions (`~/.rho/agents`, `~/.agents/agents`). Auto, Allow edits, and Supervised do not prompt for them. Network access, skills, and instruction discovery do not prompt either. There is no attach-directory command. Switch mode to read any other path outside the workspace.

The status line shows **Bypass** in warning style. The other modes appear dim.

## Auto

Auto uses the Allow edits gate. A classifier model reviews what that gate does not allow, instead of opening the approval UI.

The classifier has two stages. A fast low-reasoning screen answers `allow` or `escalate` without explaining itself. Only an escalation pays for a second review at the configured classifier reasoning level. The review picks one fixed option: allow, or deny because the action was not requested, expands scope, could destroy or expose data, or has unclear intent. The stages share a transcript cache breakpoint, so raising that reasoning keeps the screen cheap and skips a message-cache hit on the review.

A denial returns a tool error with the chosen option's fixed reason, so the classifier model cannot pass its own text to the agent, and the run continues. After three consecutive denials, or twenty total, Rho opens the human approval prompt in the TUI, or fails closed in a headless run. A human decision clears both counts.

Auto needs a classifier model. Rho does not pick one. `/config` and interactive startup open the picker when none is set. Headless `rho run` fails at startup without one. Escaping the startup picker falls back to Supervised.

The classifier sees completed questionnaire answers next to the questions. Approving an action there covers that action, not unrelated ones. You do not need to repeat it in chat. Unanswered questions and defaults are not consent. Ordinary tool output stays out of the classifier transcript.

Earlier tool calls appear in the transcript with each string argument cut to 500 characters. The call being classified stays whole. When the model catalog knows the classifier model's context window and the transcript still does not fit, the oldest earlier tool calls are left out first. User messages, questionnaire answers, and the pending request are never left out. If they alone do not fit, the classifier denies the request and reports the estimated tokens and the limit. When the served limit is smaller than the catalog window, such as a local Ollama `num_ctx`, set `usable_context_window` in [local model metadata](/configuration#local-model-metadata).

Set the model under **Agent behavior** in `/config`, or as `[internal_agents.permission-classifier]`.

### Screen model

By default the classifier model answers the screen too, at low reasoning, sharing its prompt cache with the review. Another model can answer it instead: a decision model, which is faster, or another text model. Set it under **Agent behavior > Permission screen model** in `/config`, or as `[internal_agents.permission-classifier-screen]`. Turning on Auto from `/config` or `/permissions auto` offers the same picker after the classifier pick; Esc keeps the current screen model.

The picker lists three groups:

- **Same as classifier** clears the entry, so the screen follows the classifier model when it changes.
- **Decision models** discovered on the hosts below.
- **Text models** from the chat model catalog. A text screen asks the same question as the classifier's screen, at low reasoning, over a transcript fitted to that model's own context window.

Ollama serves a model at the server's `num_ctx`, often far below the window the model advertises, and silently drops the front of a longer prompt. A screen could then allow a request it never read whole. So a text screen on Ollama needs `usable_context_window` set for its model in [local model metadata](/configuration#local-model-metadata), measured against the server. Without it, the screen escalates every request to the review, and the `/config` row and `/doctor` warn. Hosted providers reject an oversize prompt, which also escalates.

The picker saves `kind = "decision"` or `kind = "text"` with the entry, because Ollama serves both. Without `kind`, an entry on Ollama or TypeSafe is a decision model and an entry on any other provider is a text model.

```toml
[internal_agents.permission-classifier-screen]
provider = "ollama"
model = "clef"
kind = "decision"
```

#### Decision models

A decision model such as Cloudflare's Clef or TypeSafe's Jev answers a fixed question with a probability for each option rather than writing a review. Rho asks it over the System One API, on one of two hosts:

- **Ollama**, at `/v1/systemone` on the configured `[providers.ollama]` server. Pull the model there first, for example `ollama pull clef`. Ollama lists it with the `decision` capability.
- **[TypeSafe](/providers/typesafe)**, which hosts Jev. Sign in with `/login typesafe`.

Rho discovers decision models after login and from **Refresh model lists** in `/config`, and caches them with the chat model lists. Cloudflare Workers AI also serves Clef, but it truncates every state to its first 2,048 tokens, which would cut off the pending request, so Rho does not use it.

The `/config` row and `/doctor` warn when an entry's kind does not fit: a decision entry on a provider other than Ollama or TypeSafe, a decision entry on a model its host did not list, or a text entry on a listed decision model that is not also listed as a chat model. Rho does not warn about a host that listed no decision models, since it has nothing to judge by. The picker lists decision models only on hosts with usable credentials, and a text entry without credentials for its auth stops headless `rho run` like any unusable entry.

The screen allows only when the model picks `allow` with probability at least its allow threshold, 95% by default. Anything else goes to the classifier model's review as before, including errors, so the decision model can skip a review but never deny. Both hosts reject input over the model's context instead of truncating it: 16,384 tokens for Clef at Ollama's default `num_ctx`, and about 32,500 on TypeSafe. So the screen reads its own transcript, fitted to about 10,500 estimated tokens on Ollama or 20,500 on TypeSafe: the oldest earlier tool calls are left out first. A screen allow on that shorter transcript skips the review; user messages, questionnaire answers, and the pending request, which carry the user's intent, are never left out. When the screen escalates, the review reads the left-out calls, subject to its own transcript budget. If the user messages, questionnaire answers, and pending request alone do not fit, or the request body would pass Ollama's 64 KiB limit, the screen is skipped and the review decides. Run with `RHO_LOG=rho=warn` to log why.

#### Allow threshold

Set the threshold under **Agent behavior > Screen allow threshold** in `/config`, which appears when the screen is a decision model, or as `allow_threshold_percent`, a whole percent from 50 to 100. The screen asks two options, so a chosen `allow` is already at least 50%. A lower threshold skips more reviews; a higher one sends more requests to the review. A text screen reports no probabilities and ignores the setting. Changing the screen model keeps the threshold. `/doctor` reports the threshold in effect.

```toml
[internal_agents.permission-classifier-screen]
provider = "typesafe"
model = "jev-latest"
kind = "decision"
allow_threshold_percent = 95
```

The 95% default comes from an eval of 49 labeled cases and 40 calls replayed from real sessions. No case labeled deny reached 95% on Clef, Clef-flash, or Jev; the highest were 0.851 on Clef-flash, 0.257 on Clef, and 0.29 to 0.37 on Jev across two runs. Lowering the threshold from 97% let each model allow more cases without review: Jev 18 instead of 15, Clef 43 instead of 33, Clef-flash 19 instead of 12. One replayed call that the review denied as out of scope scored 0.963 on Clef in one run, so at 95% the screen would have allowed it without review. Raise the threshold to 97% if that tradeoff matters to you. Jev reports probabilities rounded to two decimals, so 95% admits any true probability of about 0.945 or more.

#### Screen entry rules

`[internal_agents.permission-classifier-screen]` takes the provider's auth: `none` or `ollama-api-key` on Ollama, `typesafe-api-key` on TypeSafe, and the chat provider's auth for a text model. The review still needs `[internal_agents.permission-classifier]`. An unusable screen entry, including an allow threshold outside 50 to 100 on a decision model, stops headless `rho run` at startup. In the TUI, every classified request is denied with that error until the entry is fixed.

## Change the mode

`/permissions` shows the current mode. `/permissions bypass|auto|allow_edits|plan|supervised` changes it and saves the choice. `/permissions auto` asks for a classifier model if none is set. Cancelling keeps the previous mode. The command is unavailable during a model turn.

`--permission-mode` overrides one invocation and is not saved.

An interactive change applies before the next turn. The session ID and history stay. Every remembered **Allow for session** approval is cleared. Path grants stay bound to the approver that allowed them, so a classifier grant does not skip the human gate after a switch to Allow edits. A human grant may. A reset or a different resumed session starts with no inherited path grants.

## Approval prompt

When a checked mode needs a person, the composer opens an approval prompt. It leads with the path or command and focuses **Deny**. Choose **Allow once**, **Allow for session**, or **Deny**, then press Enter. You can also click a choice to focus it, or double-click it to confirm.

**Allow for session** remembers only that exact capability request for the current session. Page Up and Page Down scroll long details without hiding the choices. **Deny** rejects that operation and lets the run continue. Escape denies it and cancels the run.

## Not a sandbox

Permission modes are application policy checks. Rho and its tools still run as the current user. The policy only covers tools that declare and authorize their capabilities. Unrecognized capability classes fail closed: Plan denies them, Supervised and Allow edits ask, and Auto sends them to the classifier.
