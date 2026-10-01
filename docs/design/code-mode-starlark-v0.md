# Starlark Code Mode v0 — Design Note

Status: early prototype / design + scaffold  
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

- `crates/rho-sdk/src/tool_host.rs` — policy + approvals seam (`ApprovalHandler` / `approval_session`)
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

**Prototype crutch:** dual direct+codemode exposure of MCP tools is OK until search exists.  
**Ship target:** search + deferred discovery + `codemode` as composition (Pi 0.99 lesson: MCP = capability, codemode = composition, search = discovery).

### 2. Sequential `call_tool` for v0

No host `parallel([...])` in v0. Nested calls run one after another inside the script.

- Latency win from parallel is **later**, when the conflict planner can see nested batches.
- Fewer LLM turns come from **composition itself**, not from fan-out.
- Document parallel as a follow-on once planner-aware nested batches exist.

### 3. Tool name: `codemode`

The model-facing tool / spec name is **`codemode`** (not `code_mode` / `execute_starlark`). Aligns with future `/codemode on|yolo|off` toggle naming. Rust module paths may stay `tools::code_mode` if renaming directories is noisy.

### 4. Nested approvals: pause the whole script

When a nested `call_tool` needs approval:

1. **Pause** the Starlark evaluation thread on that call (outer `codemode` stays in-flight — one agent turn).
2. Present the **same** session approval UI / knobs as a **direct** call of that tool (yolo / auto-approve / bypass / trusted allowlist / `AllowForSession`).
3. **No second approval system** and **no double-prompt** (“approve codemode” + “approve write”).
4. On **deny** → error into the script (`call_tool` fails with a loud policy error).
5. On **approve** → continue the script.
6. **Fuel / timeout:** do not hang forever waiting on approval — honor ToolHost / run cancellation and evaluator fuel.

Mode mapping (future `/codemode` toggle):

- `/codemode on` — composition with **normal** write gating (or write-locked per matrix below).
- `/codemode yolo` — widens **only as far as session policy allows**; config can lock so yolo cannot widen past policy.
- `/codemode off` — restore normal tools without the composition tool (or hide it).

Implementation seam: nested calls go through `ToolHost::invoke` on a host that **shares** the session `ApprovalHandler` / `ApprovalSession` with the parent run (`ToolHostBuilder::approval_session` / `approval_handler_shared`). That is the reuse path — not a new nested-only approver.

---

## Key lessons from Pi (boozedog + peers)

### Tool vs mode toggle

- One orchestration tool (`codemode` in current execute-tool.ts; README historically `execute_tools`).
- Session mode: `/codemode on|yolo|off` (bare toggles `off ↔ on`); config can lock/non-widen.

### Sandbox / host authority

- Guest is untrusted; host dispatcher is authority.
- No shell-string API in guest; typed allowlisted host ops only (Pi `cli.*`).
- Type/schema check before exec where applicable; fail closed.

### Write-locking (Pi reference matrix)

| Door | `on` | `yolo` |
|------|------|--------|
| Guest mutation helpers | denied / read-only | denied / read-only |
| Patch tools | root-scoped | unrestricted |
| Native write/edit | DENY | DENY |
| Native bash | DENY | ALLOW (escape) |

### TUI / distillate

- Nested results stay out of LLM context; only script distillate returns.
- Prefer nested ToolHost-visible progress (nghyane event-bridge lesson).

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

- `call_tool(name, args_dict) -> value` — **required**; ToolHost-generic (native + MCP).
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
  → ToolHost::invoke (same ApprovalSession as parent)
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

- [ ] Tool search / deferred wired so MCP catalogs do not dump into context
- [ ] Nested pause UX verified end-to-end with session yolo/auto/bypass
- [ ] `/codemode on|yolo|off` toggle + config lock
- [ ] Default registry wiring + TUI nested cards under parent `codemode`
- [ ] Planner-aware parallel (optional, after sequential proves out)

---

## Resolved questions (was open)

1. **Exposure:** dual OK as prototype; **ship** = search + deferred + composition.
2. **Parallel:** sequential `call_tool` for v0.
3. **Name:** `codemode`.
4. **Approvals:** pause whole script; reuse session knobs; no double-prompt.
