# Permission Bypass and Auto classifier Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rename no-checks permission mode to Bypass (still default), and implement Auto as a user-chosen classifier that reviews Supervised-gated calls with deny-and-continue plus human escalation.

**Architecture:** `PermissionMode` gains Bypass + classifier Auto. Auto's policy matches Supervised (`RequireApproval` for write/process/unknown). A classifying `ApprovalHandler` (goal-judge-style one-shot, advisor-style model config) sits in `approval_channel_for` for Auto; after 3 consecutive denials it forwards to the existing human channel (TUI) or fails (headless).

**Tech Stack:** Rust, `rho` host, `rho-sdk` `ApprovalHandler` / `PolicyDecision`, internal agents (`run_one_shot_with_provider`), TUI config/model pickers.

**Spec:** `docs/superpowers/specs/2026-08-11-permission-bypass-auto-classifier-design.md` (local design notes; do not ship in the PR — delete `docs/superpowers/` before opening/updating the PR).

## Global Constraints

- Bypass is default (`PermissionMode::default()` and config default).
- Hard cut: persisted `"auto"` means classifier Auto (no rewrite to Bypass).
- Classifier model is required to enter Auto; no default model.
- Classifier context: user messages + tool calls only (no assistant prose, reasoning, tool outputs).
- Deny-and-continue; escalate after 3 consecutive classifier denials.
- Classifier failures fail closed (deny).
- Claude-cli spawn: Bypass → `bypassPermissions`, Plan → `plan`, Auto and Supervised refuse (Rho classifier cannot cover Claude children).
- Max cargo jobs 12; redirect verbose logs to temp files.
- Conventional Commits; do not edit CHANGELOG.md.
- Before claiming done: run `rho-rust-change-validation` / narrow tests + fmt + architecture check as appropriate.
- Before PR: remove `docs/superpowers/` from the branch (user request).

## File map

| Path | Responsibility |
|------|----------------|
| `crates/rho/src/permission.rs` + `permission_tests.rs` | Mode enum, parse, `decision_for`, `workspace_policy` |
| `crates/rho/src/permission_classifier/` (new) | Prompt, strip transcript, verdict parse, consecutive-deny counter, classify call |
| `crates/rho/src/permission_classifier_handler.rs` (new) | `ApprovalHandler` for Auto: classify or escalate |
| `crates/rho/src/agent/internal.rs` | `PERMISSION_CLASSIFIER_AGENT_ID`, definition, `requires_own_model: true`, `accepts_claude_runtime: false` |
| `crates/rho/src/app/interactive_runtime_startup.rs` | `approval_channel_for` wires classifier + optional human receiver |
| `crates/rho/src/app/agent_binding.rs` | `narrower_permission_mode` ranks |
| `crates/rho/src/claude_runtime/spawn.rs` | `map_permission_mode` |
| `crates/rho/src/config.rs` + load/format | Default Bypass; docs strings |
| `crates/rho/src/cli.rs` + startup wiring | `--permission-mode`; headless Auto model gate |
| `crates/rho/src/tui/config_picker.rs`, `config_actions.rs`, `permission_mode.rs`, `model_actions.rs`, `agent_picker.rs`, `statusline.rs` | UX |
| `docs/configuration.md`, `docs/tools-workspace.md`, `docs/sdk/security.md`, `docs/subagents/claude-cli.md`, `docs/configuration/full-example.md` | User-facing docs |

---

### Task 1: Rename Auto → Bypass; add classifier Auto in `PermissionMode`

**Files:**
- Modify: `crates/rho/src/permission.rs`
- Modify: `crates/rho/src/permission_tests.rs`
- Modify: every exhaustive `match` on `PermissionMode` that still compiles after rename (compiler will list them; at minimum `app/agent_binding.rs`, `claude_runtime/spawn.rs`, `tui/statusline.rs`, `tui/config_picker.rs`, `app/interactive_runtime_startup.rs`, `app/interactive_runtime_workspace_rewind.rs`, `app/workflow_cli/runtime.rs`)

**Interfaces:**
- Produces: `PermissionMode::{Bypass, Auto, Plan, Supervised}` with `Bypass` as `#[default]`
- `as_str`: `bypass` / `auto` / `plan` / `supervised`
- `label`: `Bypass` / `Auto` / `Plan` / `Supervised`
- `decision_for(Bypass)` = always Allow; `decision_for(Auto)` = same as Supervised
- `workspace_policy(Bypass)` = `None`; `workspace_policy(Auto|Plan|Supervised)` = `Some(ModePolicy)`

- [ ] **Step 1: Update failing tests in `permission_tests.rs`**

Replace Auto-as-allow tests with Bypass; add Auto-matches-Supervised tests; update parse error expected string to include `bypass`; assert `"auto".parse()` → `PermissionMode::Auto` (classifier), `"bypass".parse()` → `Bypass`, default is Bypass.

```rust
#[test]
fn decision_for_bypass_allows_everything() {
    for kind in all_capability_kinds() {
        assert_eq!(
            PermissionMode::Bypass.decision_for(kind),
            PolicyDecision::Allow
        );
    }
}

#[test]
fn decision_for_auto_matches_supervised() {
    for kind in all_capability_kinds() {
        assert_eq!(
            PermissionMode::Auto.decision_for(kind),
            PermissionMode::Supervised.decision_for(kind)
        );
    }
}

#[test]
fn parse_auto_is_classifier_mode_and_bypass_is_no_checks() {
    assert_eq!("auto".parse::<PermissionMode>().unwrap(), PermissionMode::Auto);
    assert_eq!("bypass".parse::<PermissionMode>().unwrap(), PermissionMode::Bypass);
    assert_eq!(PermissionMode::default(), PermissionMode::Bypass);
}
```

- [ ] **Step 2: Run tests (expect fail)**

```bash
cargo test -j12 -p rho --lib permission:: -- --nocapture > /tmp/rho-perm-t1.log 2>&1
```

Expected: FAIL (old `Auto` allow-all / missing Bypass).

- [ ] **Step 3: Implement enum + methods in `permission.rs`**

```rust
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PermissionMode {
    #[default]
    Bypass,
    Auto,
    Plan,
    Supervised,
}

impl PermissionMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bypass => "bypass",
            Self::Auto => "auto",
            Self::Plan => "plan",
            Self::Supervised => "supervised",
        }
    }

    pub fn decision_for(self, kind: CapabilityKind) -> PolicyDecision {
        match self {
            Self::Bypass => PolicyDecision::Allow,
            Self::Plan => { /* unchanged Plan arms */ }
            Self::Auto | Self::Supervised => { /* current Supervised arms */ }
        }
    }

    pub fn workspace_policy(self) -> Option<ModePolicy> {
        match self {
            Self::Bypass => None,
            Self::Auto | Self::Plan | Self::Supervised => Some(ModePolicy { mode: self }),
        }
    }
}
```

Update `FromStr` arms and error text: `expected bypass, auto, plan, or supervised`.

- [ ] **Step 4: Fix compile breaks at match sites (temporary stubs OK)**

- `narrower_permission_mode` ranks: Plan=0, Supervised=1, Auto=2, Bypass=3
- `map_permission_mode`: Bypass → `BypassPermissions`; Plan → `Plan`; Auto | Supervised → `SupervisedUnsupported` (or a dedicated Auto error — refuse spawn)
- `approval_channel_for`: for now treat Auto like Supervised (human channel) so interactive still works until Task 4; Plan/Bypass → `(None, None)`
- `permission_style`: Bypass = warning; Auto/Plan/Supervised = dim
- `permission_mode_description`: Bypass = "No permission checks."; Auto = "Classifier reviews writes and processes."; Plan/Supervised unchanged
- Config default field: `permission_mode: PermissionMode::Bypass` in `config.rs`

- [ ] **Step 5: Re-run permission + statusline tests**

```bash
cargo test -j12 -p rho --lib permission:: -- --nocapture > /tmp/rho-perm-t1b.log 2>&1
cargo test -j12 -p rho --lib tui::statusline -- --nocapture > /tmp/rho-perm-t1c.log 2>&1
```

Expected: PASS for updated names (`permission_style_marks_bypass_as_warning`, etc.).

- [ ] **Step 6: Commit**

```bash
git add crates/rho/src/permission.rs crates/rho/src/permission_tests.rs \
  crates/rho/src/config.rs crates/rho/src/app/agent_binding.rs \
  crates/rho/src/claude_runtime/spawn.rs crates/rho/src/tui/statusline.rs \
  crates/rho/src/tui/statusline_tests.rs crates/rho/src/tui/config_picker.rs \
  crates/rho/src/app/interactive_runtime_startup.rs
# plus any other match-site files the compiler required
git commit -m "$(cat <<'EOF'
refactor(permission): rename no-checks Auto to Bypass

Add classifier-shaped Auto that matches Supervised's capability gate.
Hard-cut config string auto means the new mode; Bypass is the default.

EOF
)"
```

---

### Task 2: Classifier transcript strip + verdict parse (pure)

**Files:**
- Create: `crates/rho/src/permission_classifier/mod.rs`
- Create: `crates/rho/src/permission_classifier/transcript.rs`
- Create: `crates/rho/src/permission_classifier/transcript_tests.rs`
- Create: `crates/rho/src/permission_classifier/verdict.rs`
- Create: `crates/rho/src/permission_classifier/verdict_tests.rs`
- Modify: `crates/rho/src/lib.rs` or `main` module tree to `mod permission_classifier;`

**Interfaces:**
- Produces:
  - `pub(crate) fn render_classifier_transcript(history: &[Message], pending: &ApprovalRequest) -> String`
  - `pub(crate) enum ClassifierVerdict { Allow, Deny { reason: String } }`
  - `pub(crate) fn parse_classifier_verdict(text: &str) -> Result<ClassifierVerdict, …>`
  - `pub(crate) const CLASSIFIER_PROMPT: &str` (JSON-only allow/deny; conservative; user-intent anchored)
  - `pub(crate) const CONSECUTIVE_DENY_ESCALATION: u32 = 3`

- [ ] **Step 1: Write transcript strip tests**

Fixture history with User, Assistant (text + reasoning_summary), ToolCall blocks, ToolResult. Assert output contains user text and tool-call name/args; assert it does **not** contain assistant prose, reasoning, or tool-result bodies. Assert pending capability details appear at the end.

- [ ] **Step 2: Run (expect fail) → implement `transcript.rs` → pass**

Walk `Message` variants; for assistants keep only `ContentBlock::ToolCall` payloads; skip `Message::ToolResult` entirely; include `Message::User` text (and images as a short placeholder if present). Append a final section describing `pending.capability()`.

- [ ] **Step 3: Write verdict parse tests**

Accept `{"decision":"allow"}` and `{"decision":"deny","reason":"…"}` (extract JSON object from surrounding prose like `goal.rs` `parse_evaluation`). Reject empty reason on deny; reject unknown decision; treat malformed as Err (caller maps to deny).

- [ ] **Step 4: Implement `verdict.rs` + `CLASSIFIER_PROMPT`**

Prompt must require JSON only, instruct fail-closed on unclear user intent, and state the agent must not be trusted for rationalizations (those are stripped anyway).

- [ ] **Step 5: Commit**

```bash
git commit -m "feat(permission): add classifier transcript strip and verdict parse"
```

---

### Task 3: Internal agent id + one-shot classify function

**Files:**
- Modify: `crates/rho/src/agent/internal.rs`
- Modify: `crates/rho/src/agent/mod.rs` (re-export id if needed)
- Create: `crates/rho/src/permission_classifier/classify.rs`
- Create: `crates/rho/src/permission_classifier/classify_tests.rs` (unit-test parse wiring; mock provider only if an existing test harness makes it cheap — otherwise keep transport tests in Task 4 with a fake `ApprovalHandler` collaborator)

**Interfaces:**
- Produces: `PERMISSION_CLASSIFIER_AGENT_ID = "permission-classifier"`
- `requires_own_model: true`, `accepts_claude_runtime: false`
- `pub(crate) async fn classify_capability_request(…) -> ClassifierVerdict` using `run_one_shot_with_provider` + `parse_classifier_verdict`; on any Err return `Deny { reason: "classifier unavailable: …" }` (fail closed)

- [ ] **Step 1: Register internal agent**

```rust
pub(crate) const PERMISSION_CLASSIFIER_AGENT_ID: &str = "permission-classifier";
// INTERNAL_AGENTS entry:
// prompt: PromptPolicy::Replace(CLASSIFIER_PROMPT.into())
// reasoning default: Low or Medium (pick Low for latency)
// requires_own_model: true
// accepts_claude_runtime: false
```

- [ ] **Step 2: Implement `classify_capability_request`**

Mirror advisor Rho path / goal-judge:

1. Resolve `config.internal_agent_model(PERMISSION_CLASSIFIER_AGENT_ID)` — if missing, return Deny with clear reason (caller should not reach here if UX gated).
2. `build_provider` + `OneShotAgentRequest { definition: internal_definition(PERMISSION_CLASSIFIER_AGENT_ID), usage_purpose: "permission-classifier", reasoning: Some(effective…), input: render_classifier_transcript(...), … }`
3. Parse verdict; map errors to Deny.

- [ ] **Step 3: Commit**

```bash
git commit -m "feat(permission): add permission-classifier internal agent"
```

---

### Task 4: Classifying `ApprovalHandler` + wire `approval_channel_for`

**Files:**
- Create: `crates/rho/src/permission_classifier_handler.rs` (+ tests)
- Modify: `crates/rho/src/app/interactive_runtime_startup.rs` (`approval_channel_for`)
- Modify: interactive/automation runtime construction so Auto gets an `ApprovalSession` with the classifier handler and a bound session/history source
- Modify: headless startup (`app/automation.rs` and any shared bootstrap) to fail if mode is Auto and classifier model missing

**Interfaces:**
- Produces: `ClassifierApprovalHandler` implementing `ApprovalHandler`
  - Holds: model/config resolver, history provider, `Mutex<u32>` consecutive denials, optional inner human `ApprovalHandler` (TUI channel) or `None` (headless)
  - `request`: if consecutive >= 3 and human handler present → forward to human, then reset counter from human outcome; if consecutive >= 3 and no human → `Deny` with escalation reason (fail run)
  - else call classify; Allow → reset counter to 0, return `AllowOnce`; Deny → increment, return `Deny { reason }` (include “find a safer path; do not route around this block”)
- `approval_channel_for(mode, …)` returns for Auto: `(Some(classifier_handler), Some(human_receiver_or_none))`

**Note:** Today `approval_channel_for` only takes `mode`. Extend it (or add a builder used by startup) so Auto can receive config + history + whether interactive. Keep Plan/Bypass as `(None, None)`. Supervised unchanged (human channel only).

- [ ] **Step 1: Handler unit tests with a stub classifier fn / injectable classify future**

Cover: allow resets counter; deny increments; 3rd consecutive deny escalates to inner handler; inner allow resets; classify Err → deny + increment; headless escalation → Deny without calling missing inner.

- [ ] **Step 2: Implement handler + wire startup**

Interactive Auto: classifier wraps `approval_channel` human handler; TUI still polls receiver for escalations only.

Headless Auto: classifier with no human inner; ensure automation does **not** use `approval_session: None` for Auto (that would DenyApprovals without classifier). Build `ApprovalSession::new(classifier)`.

- [ ] **Step 3: Headless model gate**

Before building runtime for `rho run` (and workflow non-interactive paths that honor Auto): if `permission_mode == Auto` && `internal_agent_model(PERMISSION_CLASSIFIER_AGENT_ID).is_none()` → return error: `permission mode auto requires a configured permission-classifier model (set via /config or config.toml [internal_agents.permission-classifier])`.

- [ ] **Step 4: Integration-style runtime test**

Extend `interactive_runtime_tests` mode-switch coverage: switching to Auto installs a non-None approval session; Bypass clears it.

- [ ] **Step 5: Commit**

```bash
git commit -m "feat(permission): classify Auto approvals with deny-and-continue"
```

---

### Task 5: TUI model gate and config rows (advisor pattern)

**Files:**
- Modify: `crates/rho/src/tui/permission_mode.rs` / config actions that set permission mode
- Modify: `crates/rho/src/tui/agent_picker.rs` — add picker origins for classifier (config row / mode select)
- Modify: `crates/rho/src/tui/config_picker.rs` — Auto classifier model (+ reasoning) rows under Agent behavior
- Modify: `crates/rho/src/tui/model_actions.rs` — finish selection for `PERMISSION_CLASSIFIER_AGENT_ID`
- Modify: `crates/rho/src/tui/statusline.rs` + tests (Bypass warning already Task 1)

**Behavior:**
- Selecting Auto without classifier model opens internal-agent model picker; cancel restores previous mode (do not persist Auto).
- Selecting model may then apply Auto (mirror `finish_advisor_model_selection`).
- Config rows always allow editing classifier model once present; enabling Auto still gates on model.

- [ ] **Step 1: Tests for “Auto without model opens picker / cancel keeps prior mode”** (follow `advisor_command_tests` patterns)

- [ ] **Step 2: Implement picker origins + config rows + apply gate**

- [ ] **Step 3: Commit**

```bash
git commit -m "feat(tui): require classifier model before enabling Auto mode"
```

---

### Task 6: CLI `--permission-mode` override

**Files:**
- Modify: `crates/rho/src/cli.rs`
- Modify: wherever `Cli` applies overrides into `Config` for interactive and `Run` (search `cli.reasoning` / `cli.model` apply sites)
- Tests: clap parse / config override test if an existing CLI test module exists; otherwise a focused parse unit test on the value parser

**Behavior:**
- Global or top-level `--permission-mode bypass|auto|plan|supervised` overrides config for the invocation (like `--reasoning`), not persisted unless you already have a `--save` pattern for it — **do not** add `--save` for permission mode unless an existing pattern covers it; session override is enough for v1.
- Combined with Task 4 headless Auto model gate.

- [ ] **Step 1: Add clap arg + wire into config used at startup**

```rust
#[arg(long, value_name = "MODE")]
pub permission_mode: Option<PermissionMode>, // needs ValueParser/FromStr
```

- [ ] **Step 2: Test parse + override**

- [ ] **Step 3: Commit**

```bash
git commit -m "feat(cli): add --permission-mode override"
```

---

### Task 7: User-facing docs

**Files:**
- Modify: `docs/configuration.md` (Permission modes section + mermaid)
- Modify: `docs/configuration/full-example.md` (`permission_mode = "bypass"`)
- Modify: `docs/tools-workspace.md`, `docs/sdk/security.md`
- Modify: `docs/subagents/claude-cli.md` (Plan or Bypass before launch; Auto refuses like Supervised)
- Grep `permission_mode` / `` `auto` `` permission wording under `docs/` and update stale “auto means allow” claims

- [ ] **Step 1: Update docs to match modes table in the spec**

- [ ] **Step 2: Commit**

```bash
git commit -m "docs: document Bypass default and Auto classifier mode"
```

---

### Task 8: Validation + strip local design/plan docs before PR

**Files:**
- Delete from branch: `docs/superpowers/` (entire tree: specs + plans)

- [ ] **Step 1: Run validation**

Follow `.claude/skills/rho-rust-change-validation/SKILL.md`:

```bash
cargo fmt --all
python3 scripts/check_architecture.py
cargo test -j12 -p rho --lib permission:: -- --nocapture > /tmp/rho-perm-final.log 2>&1
# plus targeted classifier / tui / runtime tests touched above
# if change is broad:
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

- [ ] **Step 2: Remove design/plan docs from the branch**

```bash
git rm -r docs/superpowers
git commit -m "chore: drop local permission-mode design notes from branch"
```

- [ ] **Step 3: Open/update PR for #709 without `docs/superpowers/` in the diff**

---

## Spec coverage checklist

| Spec requirement | Task |
|------------------|------|
| Bypass rename + default | 1 |
| Hard-cut `"auto"` | 1 |
| Auto decision_for = Supervised | 1 |
| Stripped transcript | 2 |
| Classifier agent + user model | 3, 5 |
| Deny-and-continue + fail closed | 4 |
| 3 consecutive → human / headless fail | 4 |
| TUI model gate | 5 |
| `--permission-mode` + headless model error | 4, 6 |
| Statusline Bypass warning | 1 |
| User docs | 7 |
| Remove design docs from PR | 8 |

## Placeholder / consistency notes

- Agent id string is exactly `permission-classifier` everywhere (config table key, usage purpose, picker).
- Escalation constant `CONSECUTIVE_DENY_ESCALATION = 3` lives in `permission_classifier` and is used by the handler.
- Claude spawn refuses Auto (not Claude's classifier `auto`).
