# Permission Bypass and Auto classifier

Closes product direction for [#709](https://github.com/matthewyjiang/rho/issues/709).

Today the default mode is named `Auto` and means no permission checks. That name fits a classifier-backed mode elsewhere (Claude Code). This design splits the concepts: **Bypass** for no checks, **Auto** for a model classifier that reviews the calls Supervised would ask a human about.

## Goals

- Rename no-checks mode to Bypass; keep it the default for new sessions.
- Add Auto as a middle mode: Supervised capability gate, classifier approver, deny-and-continue.
- Let the user pick the classifier model (and optional reasoning), same control model as advisor. Rho does not pick a default classifier model.
- Keep Bypass warning-styled in the statusline. Auto uses ambient styling like Plan and Supervised when idle.

## Non-goals (v1)

- Claude Code tiered allowlists (in-project edits free, shell always classified, and so on).
- Prompt-injection probe on tool outputs before they enter the main agent.
- Supervised approval copy polish ("exact request" wording).
- Two-stage classifier (fast single-token filter then CoT). v1 uses one classifier call per gated request.

## Modes

| Mode | Config / CLI string | Default? | Behavior |
|------|---------------------|----------|----------|
| Bypass | `bypass` | yes (new installs / unset) | No policy checks. Every capability allowed. |
| Auto | `auto` | no | Same require-approval set as Supervised. Classifier decides allow/deny. |
| Plan | `plan` | no | Unchanged. |
| Supervised | `supervised` | no | Unchanged. Human approval for writes, processes, unknown capabilities. |

Picker copy (short):

- Bypass: "No permission checks."
- Auto: "Classifier reviews writes and processes."
- Plan / Supervised: keep current wording.

### Config migration (hard cut)

Persisted `permission_mode: "auto"` means the **new** classifier Auto. Rho does not rewrite old `"auto"` to Bypass.

Implications:

- Users who previously saved `"auto"` for no-checks will enter classifier Auto after upgrade.
- Entering Auto requires a configured classifier model. If none is set, open the model picker (TUI) or fail startup (headless). Canceling the picker leaves the previous mode unchanged.
- `PermissionMode::default()` becomes Bypass so users with no persisted mode keep no-checks behavior.

## Classifier design

### When it runs

Auto reuses Supervised's `decision_for` gate:

- Free: read, network, skill, instruction discovery.
- Gated (classifier): write, process, unknown capability kinds.

When policy would `RequireApproval`, Auto calls the classifier instead of opening the approval UI (unless escalation applies).

### Agent shape

- Built-in internal agent (sibling of advisor), sole task: allow or deny the pending gated request.
- Model config stored like advisor (`InternalAgentModelConfig` under a dedicated agent id). Optional reasoning setting mirrors advisor.
- No assumed default model. User must select one to enable Auto.
- Classifier cost folds into the existing non-main session cost total (with subagents / advisor).

### Context the classifier sees

Stripped transcript, aligned with Claude Code's reasoning-blind classifier:

- Include: user messages; tool calls (name + arguments / executable payload); the pending capability request.
- Exclude: assistant prose; reasoning traces; tool outputs.

Rationale: the agent must not lobby the classifier with rationalizations, and injected tool results must not poison the allow/deny decision. User messages keep the intent anchor across follow-ups.

### Decision outcomes

| Result | Behavior |
|--------|----------|
| Allow | Execute the tool. Reset consecutive-deny counter. |
| Deny | Return a tool error that tells the agent to find a safer path and not route around the block. Main agent continues (deny-and-continue). Increment consecutive-deny counter. |
| Classifier failure (timeout, API error, malformed verdict) | Fail closed: treat as deny for that call, still deny-and-continue. |

### Escalation

After **3 consecutive** classifier denials (v1 constant), fall back to the human approval prompt for that call (TUI). Reset the consecutive counter on allow or after a human decision.

Headless / non-interactive: after 3 consecutive denials, fail the run. Do not hang waiting for a human.

No separate "total denials" cap in v1.

### Concurrency

Serialize classifier decisions for gated calls within a session turn. One in-flight classification at a time keeps status and counters simple.

## UX and CLI

### Config picker

- Permission mode row: Bypass, Auto, Plan, Supervised.
- Choosing Auto with no classifier model opens the internal-agent model picker (advisor-style origins for command vs config row).
- Config rows for classifier model and optional reasoning (editable once a model exists; required before Auto activates).

### Statusline

- Bypass: warning style (current Auto emphasis).
- Auto: dim/ambient like Plan and Supervised while idle.
- Optional short "classifying…" / reviewing state while a decision is in flight, if it fits existing advisor-status patterns without new chrome sprawl.

### Mode changes

Keep current rule: permission mode cannot change until the current turn finishes.

### CLI

- `--permission-mode bypass|auto|plan|supervised`.
- Headless Auto without a classifier model: hard error at startup with a clear fix message. No silent fallback to Bypass.

## Architecture

Ownership split:

1. **`PermissionMode`** - rename today's `Auto` to `Bypass`; add new `Auto`. Auto's `decision_for` matches Supervised's require-approval set. `workspace_policy()` returns a mode policy for Auto (not `None`). Host resolves Auto's `RequireApproval` via classifier before the approval UI.
2. **Internal agent id** - classifier model config beside advisor.
3. **Classifier module** - prompt template, transcript stripping, verdict parse, consecutive-deny counter. Feature policy stays out of generic transport/rendering.
4. **TUI** - mode picker, model picker wiring, statusline, escalation to existing approval UI.
5. **Approval UI** - remains Supervised's path and Auto's escalation backstop.

Suggested flow:

```text
capability request
  -> PermissionMode::decision_for
  -> if Allow/Deny: done
  -> if RequireApproval:
       Bypass: (unreachable; Bypass never requires approval)
       Supervised: approval UI
       Auto: classifier
         allow -> execute
         deny -> tool error, continue
         3 consecutive denies -> approval UI (TUI) or fail (headless)
```

## Tests

Focus on failure modes at the owner layer:

- Parse/serialize: default Bypass; `"auto"` is classifier Auto; `"bypass"` is no-checks; unknown rejected.
- Auto cannot activate without a classifier model (TUI picker gate; headless startup error).
- Transcript stripping: user messages + tool calls present; assistant / reasoning / tool outputs absent.
- Allow path executes; deny path returns continue-friendly error; 3 consecutive denials escalate (TUI) or fail (headless).
- Classifier transport/parse failure fails closed (deny).
- Statusline: Bypass uses warning style; Auto does not inherit Bypass warning by default.
- Prefer unit/behavior tests. Add PTY only if mode-entry UX needs a product-gate scenario the unit layer cannot cover.

## Open constants

| Constant | v1 value | Notes |
|----------|----------|-------|
| Consecutive deny escalation | 3 | Same order of magnitude as Claude Code's consecutive backstop. |
| Total deny cap | none | Add later if deny-and-continue loops show up in practice. |

## Follow-ups (not this issue)

- Claude-style safe tiers and environment trust slots for the classifier prompt.
- Input-layer prompt-injection warnings on tool results.
- Supervised prompt copy (risk differentiation, "exact request" scope).
- Two-stage classifier for latency/cost.
- Optional first-run notice for Bypass risk (if statusline warning proves insufficient).
