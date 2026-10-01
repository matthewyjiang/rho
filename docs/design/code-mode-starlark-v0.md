# Starlark Code Mode v0 — Design Note

Status: early prototype / design + scaffold  
Worktree: `worktree-silver-meadow-a56f`  
Date: 2026-10-01 (America/New_York)

## Sources (verified only)

Primary Pi extension study:

- [boozedog/pi-codemode](https://github.com/boozedog/pi-codemode) README (`master`), `docs/architecture.md`, `src/execute-tool.ts`, `src/index.ts`

Related patterns (lighter weight):

- [pi-codemode-extension](https://github.com/Hor1zonZzz/pi-codeMode) / npm `pi-codemode-extension` — `exec` + `/codeMode` toggle; re-registers tool descriptions to match orchestrable set; runs *copies* of builtins
- [@nghyane/pi-codemode](https://www.npmjs.com/package/@nghyane/pi-codemode) — `createCodeTool`; `event-bridge` for per-sub-tool TUI events

Pi 0.99 / Earendil (MCP + composition):

- [“You Said No MCP!”](https://earendil.com/posts/you-said-no-mcp/) (2026-09-29) — rationale for core MCP + Codemode in Pi 0.99

Rho grounding (this repo):

- `crates/rho-sdk/src/tool_host.rs` — policy + approvals seam for nested tool calls
- `crates/rho/src/workflow/starlark*.rs` — existing Starlark evaluator
- Workflow command nodes via ToolHost (`WorkflowCommandHosts`) — closest in-tree pattern for tool-calling-tools

Do **not** treat this note as inventing Pi behavior beyond those sources.

---

## Key lessons from Pi (boozedog + peers)

### Tool vs mode toggle

- One orchestration tool is the composition surface (tool `name` is `codemode` in current `execute-tool.ts`; README historically says `execute_tools`).
- Session mode is separate: `/codemode on|yolo|off` (bare `/codemode` toggles `off ↔ on`), plus config `mode`.
- **`on`**: orchestration tool + normal non-bash tools; **write-locked**.
- **`yolo`**: same as `on` plus native `bash` as an explicit escape hatch when available (graceful fallback + notify if missing).
- **`off`**: restore normal Pi tools including write/edit/bash.
- Global config can **lock** / non-widen `mode` and `cli` so a project overlay cannot escalate permissions.

### `execute_tools` / `codemode` surface

- Model supplies a **code body** (not necessarily a full function), optional `strings` injected as `π.key`, optional result formatting.
- **Type-check before execute**; type errors ⇒ no side effects.
- Return value + `print` / `console.log` are captured into the tool result.
- Current execute-tool description: guest **mutation helpers intentionally unavailable**; top-level patch tools handle writes so diffs stay visible.

### Sandbox model

- Default executor: **QuickJS** (no Node fs/env/net/process globals). Deno optional behind the same interface; **Node VM skipped**.
- Host dispatcher is the authority; guest only sees injected globals (`read`, `codemode.*`, `mcp.*`, `cli.*`, `print`, `π`, optional `jev.ask`).
- **No shell-string API in guest** — typed allowlisted `cli.*` host ops only.

### Write-locking

| Door | `on` | `yolo` |
|------|------|--------|
| Guest mutation helpers | read-only / denied | read-only / denied |
| Patch tools | root-scoped | unrestricted |
| Native write/edit | DENY | DENY |
| Native bash | DENY | ALLOW (escape) |
| `cli.*` | allowlisted | allowlisted |

Also: deny writing policy under `.pi/` and project-root `.mcp.json` while write-locked.

### Typed stubs

- Generate TypeScript declarations from schemas / MCP metadata; fail closed on type errors.
- Cloudflare `@cloudflare/codemode` helpers used for JSON Schema → TS (not the Workers executor).

### Parallel calls

- Model uses `Promise.all` for independent host-bridged calls; executor must map reject/cancel to the correct guest promises.

### TUI visibility of nested tools

- Large codemode payloads collapse the middle; **Ctrl+O** expands (boozedog).
- `@nghyane/pi-codemode` advertises **per-sub-tool TUI events** via `event-bridge` — better match for Rho’s desire that nested ToolHost calls remain visible.
- Pi 0.99 demos show many nested MCP ticks under one codemode call, with only the distilled return entering model context.

### Approvals / escape hatches

- `yolo` bash is **outside** the guest sandbox as an operator escape hatch.
- Generated code remains untrusted even in `yolo`.
- Config non-widening prevents model-written project config from escalating `on` → `yolo`.

### What NOT to copy into Rho v0

- TypeScript + QuickJS stack (Rho chooses **Starlark**).
- Full typed `cli.*` matrix / npm-script decomposition / `jev.ask`.
- Free-form shell inside the guest.
- **Reimplementing copies of builtins outside ToolHost** (pi-codemode-extension pattern) — Rho should call **ToolHost** so policy, hooks, and approvals stay authoritative.
- Shipping mode-toggle UX before the engine + bridge work.

---

## Lessons from Pi 0.99 / Earendil (“You Said No MCP!”)

Verified from the Earendil post:

1. **MCP = capability / wire protocol.** Think closer to OpenAPI: discoverable tools, structured returns. Bloated “dump tools into context” servers remain a problem, but that is often server/harness pattern debt—not a reason to refuse MCP as a capability surface.
2. **Codemode = composition.** A sandbox that runs **on the harness side** so the model can orchestrate tool calls (order, fan-out, filter) with a small language. Trust model differs from bash-in-sandbox: codemode coordinates harness-level tools.
3. MCP’s long-standing weakness is **composability**; codemode is the harness-side fix. Without composition, MCP tools fight token budgets.
4. In a codemode world, each tool needs **exposure metadata**: available to the LLM directly, **codemode-only**, or **deferred / searchable**. Extensions without that metadata cannot place MCP tools correctly.
5. Pi 0.99 loads Codemode automatically when MCP is configured because **composition is the point** of bringing MCP into a small harness—not dumping every MCP tool into every prompt.
6. Nested results should stay **out of the LLM context**; only the script’s distilled return comes back (post shows hundreds of nested MCP/Jev calls under one codemode turn).

**Matt’s product call (2026-10-01):** Rho Starlark code mode **must** support MCP tool calls on the **same path as native tools** (via ToolHost). Distilled script output only returns to the LLM.

---

## Recommended Starlark v0 shape (Rho)

### Goal

One tool (working name: `code_mode`) that evaluates a Starlark script. The script composes **any ToolHost-registered tool** (native **or MCP-backed**) via a single bridge. Nested tool I/O stays on the ToolHost/TUI path; the model sees the script’s return/print distillate.

### Architecture

```text
Model
  └─ code_mode(script) ─────────────────────────────┐
                                                    ▼
                                         Starlark engine
                                         (no fs/net/process)
                                                    │
                                         call_tool(name, args)
                                                    ▼
                                         ToolHost (policy, approvals, hooks)
                                                    │
                              ┌─────────────────────┴─────────────────────┐
                              ▼                                           ▼
                         Native tools                              MCP-backed tools
                         (read, search, …)                         (same Tool trait)
```

### v0 API (Starlark guest)

- `call_tool(name, args_dict) -> value` — **required**. Routes through ToolHost. Names are whatever Rho already registers (including MCP tool names). No separate MCP client in-guest.
- `print(...)` / return value — captured for the outer tool result.
- Loud errors for unknown tools, policy denials, timeouts, arg shape failures.
- Optional later: thin helpers (`read_file`, …) that are sugar over `call_tool`.

### Allowlist / limits (loud)

- v0 may start with an **allowlist of tool name patterns** for safety while wiring tests, but the **mechanism must be ToolHost-generic** (native + MCP). Do not ship “native read-only forever” as the product shape.
- Cap: script wall time, Starlark ticks/heap (reuse workflow evaluator limits where possible), max nested calls, max result bytes returned to the model.
- Deny recursive `code_mode` / self-invocation.

### TODOs after v0 scaffold

1. **Nested approvals** — propagate ToolHost approval prompts for nested MCP/native calls; decide batch vs per-call UX.
2. **Mode toggle** — Pi-like on / write-locked / yolo escape; optional deferred vs direct exposure for large MCP sets.
3. **Planner-aware parallel** — Starlark has no `Promise.all`; explore explicit `parallel([...])` host helper or sequential-only v0 with a clear note.
4. **TUI cards** — surface nested ToolHost events under the parent `code_mode` card (nghyane-style event bridge).
5. **Tool search / deferred loading** — when MCP catalogs are large, expose discovery to the script or planner without stuffing every schema into the model prompt.

### What “done” means for this prototype

- Design note (this file).
- Engine + ToolHost bridge scaffolding with tests for: script runs; `call_tool` hits ToolHost; deny unknown/disallowed; MCP-backed tools use the **same** `call_tool` path (can be stubbed ToolHost in unit tests).
- Not required for v0: full mode toggle, production allowlist policy, or end-to-end MCP integration test against a live server.

---

## Open questions for Matt

1. Default exposure: should MCP tools be **codemode-only** by default (Pi 0.99 style) or remain dual (direct + codemode) until we have deferred/search?
2. Starlark parallel: is sequential `call_tool` enough for v0, or do we need a host `parallel([...])` in the first PR?
3. Naming: `code_mode` vs `execute_starlark` vs Pi’s `codemode`?
4. Approval UX for nested MCP writes: pause the whole script, or pre-approve a set for one `code_mode` turn?
