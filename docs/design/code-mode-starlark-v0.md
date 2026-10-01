# Starlark Code Mode v0 — Design Note

Status: live wiring (registry, nested authorization, TUI progress, `/codemode on|only`)
Worktree: `worktree-silver-meadow-a56f`  
Date: 2026-10-01 (America/New_York)  
PR: https://github.com/matthewyjiang/rho/pull/1365

## Sources (verified only)

Primary Pi extension study:

- [boozedog/pi-codemode](https://github.com/boozedog/pi-codemode) README (`master`), `docs/architecture.md`, `src/execute-tool.ts`, `src/index.ts`

Related patterns (lighter weight):

- [pi-codemode-extension](https://github.com/Hor1zonZzz/pi-codeMode) / npm `pi-codemode-extension` — `exec` + `/codeMode` toggle
- [@nghyane/pi-codemode](https://www.npmjs.com/package/@nghyane/pi-codemode) — `event-bridge` for per-sub-tool TUI events

Pi 0.99 / Earendil (MCP + composition):

- [“You Said No MCP!”](https://earendil.com/posts/you-said-no-mcp/) (2026-09-29)

Rho grounding:

- `crates/rho-sdk/src/tool_host.rs` — policy + approvals seam (`ToolHost::child_builder` for nested hosts)
- `crates/rho/src/workflow/starlark*.rs` — existing Starlark evaluator
- Workflow command nodes via ToolHost — closest in-tree pattern for tool-calling-tools

Do **not** treat this note as inventing Pi behavior beyond those sources.

---

## Locked product decisions (2026-10-01)

Matt locked the following; the scaffold and this note must match.

### 1. Ship bar includes tool search / deferred

Composition alone is **not** “done.” Shipping code-mode means:

| Ship checklist item | Role |
|---------------------|------|
| `codemode` tool | Composition surface (Starlark script) |
| ToolHost bridge | Native **and MCP** tools via the same `call_tool` path |
| **Tool search / deferred** | MCP schemas must **not** all dump into model context |
| Nested approvals | Pause whole script on gated nested calls (see below) |
| Registry wiring | Opt the tool into real sessions |

**Pi-aligned exposure (locked):** MCP defaults to **`codemode`** (not declared as normal LLM tools). Core natives stay **`direct`**. Hot MCP (e.g. computer-use) *may* be `direct` via per-server/per-tool override. **`tool_search`** promotes `deferred` into the active direct set. Script-side **`search_tools` / `list_tools`** discover MCP names for Starlark. Short **mcp_servers** catalog in the prompt (one line per server), not full schemas.

### 2. Sequential `call_tool` for v0

No host `parallel([...])` in v0. Nested calls run one after another inside the script.

- Latency win from parallel is **later**, when the conflict planner can see nested batches.
- Fewer LLM turns come from **composition itself**, not from fan-out.
- Document parallel as a follow-on once planner-aware nested batches exist.

### 3. Tool name: `codemode`

The model-facing tool / spec name is **`codemode`** (not `code_mode` / `execute_starlark`). Aligns with the `/codemode on|only` command naming. Rust module paths may stay `tools::code_mode` if renaming directories is noisy.

### 4. Nested approvals: pause the whole script

When a nested `call_tool` needs approval:

1. **Pause** the Starlark evaluation thread on that call (outer `codemode` stays in-flight — one agent turn).
2. Present the **same** session approval UI as a **direct** call of that tool, under the current `/permissions` mode (bypass / auto / allow_edits / plan / supervised, plus `AllowForSession` memory).
3. **No second approval system** and **no double-prompt** (“approve codemode” + “approve write”).
4. On **deny** → error into the script (`call_tool` fails with a loud policy error).
5. On **approve** → continue the script.
6. **Fuel / timeout:** do not hang forever waiting on approval — honor ToolHost / run cancellation and evaluator fuel.

### 4a. `codemode.mode = on | only` — Pi built-in semantics (locked 2026-10-01)

Rho matches built-in Pi (`earendil-works/pi`, `codemode.mode`), **not** the boozedog write-lock package:

| | `on` (default) | `only` |
|---|---|---|
| `codemode` tool | declared | declared |
| `tool_search` | declared | declared |
| Policy-`direct` natives (`read_file`, `write`, `edit`, `bash`, …) | declared — model may use either | **not declared**; reached via `call_tool` |
| Promoted `deferred` tools | declared | declared (exposure is `deferred`, not `direct`, as in Pi) |
| MCP (`codemode` exposure) | script-only | script-only |
| `hidden` | unreachable | unreachable |

- `codemode` is **always registered**. There is no mode that removes it (Pi has none).
- `on` adds a soft nudge in the `codemode` description: prefer it for multi-step work, MCP tools, and filtering large output; call a declared tool directly for a single step.
- No write/edit/bash strip in `on`; `only` hides **all** policy-direct tools (Pi hides active direct tools), not a mutation-classified subset.
- Config: `[codemode] mode = "on" | "only"` (Pi's `codemode.mode`). `/codemode on|only` sets it and saves; bare `/codemode` reports the mode.
- Enforcement is the SDK `ToolVisibility` hook, resolved per model request, so a mode change needs no runtime rebuild. A direct model call to a tool hidden by `only` resolves unavailable.
- **No permission ladder.** Neither mode is a permission level and there is no `yolo`: nested calls inherit the session permission mode (`/permissions bypass|auto|allow_edits|plan|supervised`) through `ToolHost::child_builder`.
- MCP exposure (`direct`/`codemode`/`deferred`/`hidden`, section 5) is a separate axis from the mode.

### 5. Tool exposure modes (Pi-aligned)

Enforcement: the SDK asks a `ToolVisibility` (the app's `ExposureController`) before **every** provider request, so this table is what the provider actually receives. A `tool_search` promotion is advertised on the next request of the same run. A model call to an unadvertised tool resolves as unavailable; nested `call_tool` still reaches any non-hidden registered tool.

| Mode | Model-facing (`specs`) | Script `call_tool` | Notes |
|------|------------------------|--------------------|-------|
| `direct` | yes | yes | Core natives default here |
| `codemode` | no | yes | **MCP default** |
| `deferred` | only after `tool_search` promote | yes | Keyword/substring v0 (no embeddings) |
| `hidden` | no | no | Unreachable |

**Overrides:** exact tool name > pattern (`mcp__computer__*`). Do not force computer-use through scripts by default — support the override API so hot MCP *can* be `direct`.

**Prompt:** short `# MCP servers` catalog (one line per server). Do **not** dump full server/tool lists into `codemode` or `tool_search` descriptions (keep prompt stable).

**Dual direct+codemode by default** was a temporary prototype crutch and is **not** the ship model.



Implementation seam: each `codemode` call builds a child host with `ToolHost::child_builder(&context)`, which inherits the parent call's workspace, workspace policy, hook gate/observer, session id, and approval session (handler, exact-request memory, audit). Nested calls go through `ToolHost::invoke` on that child. That is the reuse path — not a new nested-only approver, and no policy snapshot that could drift from the running mode.

---

## Key lessons from Pi (boozedog + peers)

### Tool vs mode toggle

- One orchestration tool (`codemode` in current execute-tool.ts; README historically `execute_tools`).
- Session mode: built-in Pi uses `codemode.mode = on|only` (the boozedog package's `on|yolo|off` write lock is not the model). Rho adopts `on|only`; permissions stay with `/permissions`.

### Sandbox / host authority

- Guest is untrusted; host dispatcher is authority.
- No shell-string API in guest; typed allowlisted host ops only (Pi `cli.*`).
- Type/schema check before exec where applicable; fail closed.

### Write-locking (boozedog package — not adopted)

The boozedog `pi-codemode` package strips native write/edit/bash and adds a `yolo` escape. Built-in Pi does not, and neither does Rho: `only` hides every direct tool for composition, and authorization stays with `/permissions`.

### TUI / distillate

- Nested results stay out of LLM context; only script distillate returns.
- Nested ToolHost progress is forwarded onto the parent `codemode` call (nghyane event-bridge lesson): one status line per nested call (`name: running|<progress>|done|failed`) updates the `codemode` card. Nested host-input requests relay through the parent call, and parent cancellation cancels the in-flight nested call.

### What NOT to copy into Rho v0

- TypeScript + QuickJS; guest shell; full typed `cli.*` / npm-script / jev matrices.
- Reimplementing builtin tool **copies** outside ToolHost.
- Host `parallel([...])` before planner-aware nested batches.
- A separate nested-only approval system.

---

## Lessons from Pi 0.99 / Earendil

1. **MCP = capability** (wire protocol; prefer structured returns + discovery).
2. **Codemode = composition** (harness-side sandbox orchestrating tool calls).
3. MCP’s weakness is **composability**; codemode is the harness-side fix.
4. Tools need exposure metadata: direct / **codemode-only** / **deferred+searchable**.
5. Pi loads Codemode with MCP because composition is the point — not dumping every MCP tool into every prompt.

---

## Recommended Starlark v0 shape (Rho)

### Architecture

```text
Model
  └─ codemode(script) ──────────────────────────────┐
                                                    ▼
                                         Starlark engine (sequential)
                                                    │
                                         call_tool(name, args)
                                                    ▼
                                         ToolHost (shared session approvals)
                              ┌─────────────────────┴─────────────────────┐
                              ▼                                           ▼
                         Native tools                              MCP-backed tools
```

Optional later: **tool search / deferred** feeds which names/schemas the model (or script) may discover without stuffing every MCP schema into the prompt.

### v0 guest API

- `call_tool(name, args_dict) -> value` — **required**; ToolHost-generic (native + MCP). Resolves like Pi: the tool's structured content when it declares `Tool::output_schema` (shell: `{stdout, stderr, exit_code, truncated, wall_time_ms}`; MCP: `structuredContent`), otherwise `{"content": <text>}`. A completed `Execution` failure with structured content (nonzero exit, MCP `isError` + `structuredContent`) resolves to that value so scripts can branch; denials, cancellations, and text-only failures raise.
- `search_tools` / `list_tools` entries carry `returns` (the output schema, or `null` for the `{"content"}` fallback).
- `print(...)` / assign `result = ...` — distilled outer tool result.
- Loud errors for unknown tools, allowlist denials, recursion, policy denials, timeouts.
- **Sequential only** — no `parallel([...])` in v0.

### Allowlist / limits

- Optional name allowlist for gradual rollout; mechanism remains ToolHost-generic.
- Caps: wall time, Starlark ticks/heap, max nested calls, max distillate bytes.
- Deny recursive `codemode` / self-invocation.

### Nested approval (v0 contract)

```text
call_tool("write", ...) 
  → ToolHost::invoke (child host: same policy, hooks, ApprovalSession as parent)
  → if gated: block Starlark thread until Allow*/Deny (or cancel/timeout)
  → Deny → BridgeError into script
  → Allow → ToolOutput back into script
```

Prototype may stub the “block Starlark until…” glue if the current `block_in_place` + `host.invoke` path already waits on the shared handler; comments must state the pause-whole-script model explicitly.

### Tool search / deferred (ship requirement; scaffold outline)

- **Goal:** model does not receive every MCP tool schema up front.
- **Shapes to explore:** search tool that returns names/short descriptions; deferred load of full schema on demand; or codemode-only exposure until searched.
- **v0 code:** types + TODO module hooks (`tools::code_mode::search`) — full implementation can follow once registry exposure metadata exists.
- Dual direct+codemode remains a **temporary** prototype crutch only.

---

## Prototype “done” vs ship “done”

**This PR / prototype:**

- Design note with locked decisions + ship checklist (including search).
- Engine + ToolHost bridge + tests (native + MCP-named stubs).
- Tool spec name `codemode`.
- Approval model documented; nested path uses shared ToolHost approvals (stub comments if pause glue incomplete).
- Search/deferred outlined (stub module OK).

**Ship (later PRs):**

- [x] Default registry wiring (`codemode` + `tool_search` in `AppToolSet`)
- [x] Nested calls inherit parent authorization via `ToolHost::child_builder` (policy, hooks, approval session, live history) — no double prompt
- [x] Nested progress forwarded onto the parent `codemode` card; host input relayed; parent cancel cancels nested call
- [x] Typed nested-deny classification (`ToolErrorKind::PolicyDenied` / `Error::PolicyDenied`, no string matching)
- [x] `/codemode on|only` (Pi `codemode.mode`) persisted to `[codemode] mode`; `codemode` always registered; no yolo, no write strip
- [x] Exposure enforced at the **provider request** boundary via SDK `ToolVisibility` (per request; promotions apply mid-run; unadvertised model calls resolve unavailable)
- [x] Structured tool output (SDK `Tool::output_schema` + structured content on `ToolOutput`/`ToolError`); shell and MCP produce it; scripts receive it
- [ ] Pi's per-tool "Codemode: `call_tool(...)` returns …" description line in `on` (now meaningful with output schemas)
- [ ] Per-server exposure overrides from config (`ExposurePolicy::override_exact/pattern` exists, not yet wired)
- [ ] Nested pause UX verified end-to-end in a PTY scenario under supervised/auto/bypass
- [ ] Distinct TUI child cards per nested call (today: status lines inside the parent card)
- [ ] Planner-aware parallel (optional, after sequential proves out); embeddings search out of scope

---

## Resolved questions (was open)

1. **Exposure:** dual OK as prototype; **ship** = search + deferred + composition.
2. **Parallel:** sequential `call_tool` for v0.
3. **Name:** `codemode`.
4. **Approvals:** pause whole script; reuse session knobs; no double-prompt.
