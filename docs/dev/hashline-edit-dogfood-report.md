# Hashline `edit` dogfood report

**Context:** While applying the thermo-nuclear review fixes on
`feat/hashline-edit-tool`, the agent used Rho's own hashline `edit` tool (the
tool under review) for multi-step refactors across `crates/rho-tools/src/hashline/`.
After several failures mid-session, the agent abandoned `edit` for ad-hoc
`python3` rewrites. That is a dogfood failure and should be treated as product
signal, not operator weakness alone.

**Date:** 2026-03-26  
**Branch:** `feat/hashline-edit-tool`  
**Reporter:** agent session applying review fixes

---

## Summary

| # | Severity | Issue | Agent impact |
| --- | --- | --- | --- |
| 1 | **High (UX)** | `PUT N.:` is a silent syntax footgun; error looks like a bad line number | Repeated failed single-line edits; agent concluded "edit tool is broken" |
| 2 | **High** | Large replace + immediate follow-up CUT mangled file structure | Required manual repair; trust loss |
| 3 | **Medium** | Error messages are easy to misread (trailing `.` in locator vs sentence) | Slow recovery from #1 |
| 4 | **Medium** | Multi-step refactors push agents off `edit` onto shell/Python | Defeats the point of dogfooding |
| 5 | **Low** | Post-edit previews are focus-windowed; hard to plan the next structural CUT | Encourages guessing anchors |

---

## Issue 1 — `PUT N.:` footgun (reproduced)

### What the agent typed

Single-line replace was written as:

```text
PUT 205.:
+    let anchors: HashSet<usize> = collect_anchor_lines(ops, AnchorMode::AllLines)
+        .into_iter()
+        .collect();
```

### Why

The documented range form is `N.=M`. After internalizing that, the agent
"completed" a single-line locator as `N.:` (dot before colon) instead of the
actual single-line shorthand `N:`.

Valid forms:

| Form | Meaning |
| --- | --- |
| `PUT 205:` | replace line 205 |
| `PUT 205.=205:` | same |
| `PUT 205.:` | **invalid** — locator parses as `205.` |

### Actual error

```text
line must be a positive integer, got: 205.
```

### Why this is a product bug, not just PEBKAC

1. **The invalid token is taught by the valid range syntax.** `.=` trains a
   trailing-dot muscle memory. Single-line `N:` does not.
2. **The error does not say the locator is malformed.** It says the number is
   not a positive integer, which reads as "line 205 is wrong" rather than
   "you included a stray `.`".
3. **Display is ambiguous.** `got: 205.` looks like `got: 205` plus a sentence
   period. The trailing dot from the locator is invisible as a distinct
   character unless the reader inspects carefully or sees a repr.
4. **Harness surface reported the same string**, so the agent blamed the tool
   stack ("line must be a positive integer, got: 205") and switched strategies
   instead of fixing the document.

### Suggested fixes (parser / errors) — landed direction

1. **Reject trailing `.` on bare locators with an explicit message** (no alias):
   quote the token and point at `PUT N:` / `PUT N.=M:`.
2. **Do not accept `N.:` as an alias** — wrong spellings stay wrong; diagnostics
   and instructions must make the right form obvious.
3. **Quote the raw locator in all line-number errors** (`got "205."`).
4. **Parser unit test** for `PUT 3.:`, `PUT 3.=:`, and `CUT 3.`.
5. **Tool description + system prompt + docs** lead with `PUT N:` and call out
   `PUT N.:` as invalid.

### Minimal repro

```text
[any.rs#TAG]
PUT 3.:
+only
```

Expect: explicit truncated-range diagnostic naming `"3."` and showing `PUT 3:`.
Do not expect silent acceptance.

---

## Issue 2 — Structural mangling after large PUT + CUT

### What happened

1. Agent replaced a large region of `format.rs` (`PUT 37.=236:`) with a unified
   snapshot renderer, accidentally leaving an unused `SnapshotFooter` enum in
   the new body.
2. Agent attempted to remove that enum with `CUT 85.=97` on the post-edit tag.
3. Resulting file was **not** a clean enum deletion. A later read showed a
   severed function signature:

```text
85:/// Footer notice after a bounded numbered body.
86:    text: &str,
87:    offset: Option<usize>,
```

   i.e. a doc comment stranded above orphaned parameters — the start of
   `format_hashline_view` had been partially destroyed.

### Why this hurts dogfooding

- Fail-closed tag checks do not protect against **successful applies that
  delete the wrong span** when anchors are slightly wrong or the agent
  mis-counts through a focused preview.
- Recovery from a mangled mid-function state is harder than a stale-tag error:
  the file compiles into a red blob and the agent reaches for `write` or
  shell.
- The session then generalized: "edit is unreliable for refactors" → Python.

### Contributing factors

1. **Focused post-edit previews** show ~40 lines around focus. After a 200-line
   replace, the next CUT targets may sit outside the preview; the agent either
   re-reads (good) or guesses (bad). This session mixed both.
2. **No syntax/structure check** after apply (expected for a text tool), so a
   bad CUT is a successful tool result.
3. **High churn files** (`format.rs` mid-rewrite) make original-line documents
   brittle across steps if the agent does not wait for a full fresh read.

### Suggested fixes (product / prompt / UX)

1. **Prompt:** after any replace that rewrites a whole region, require
   `read_file` (or rely only on the returned preview lines) before a follow-up
   CUT on the same path. Already partially stated; strengthen for "do not stack
   structural cleanup on a focused preview alone."
2. **Preview:** when a single op touches >N lines or the selected body is
   capped, return a stronger notice: `structural edit; re-read before further
   ops on this path`.
3. **Eval suite:** add a multi-step case: large PUT introducing dead code, then
   CUT cleanup, gold = clean file. Measure whether models corrupt the neighbor
   function.
4. **Tool result:** on successful apply, include `ops_applied` summary
   (`CUT 85.=97 (13 lines removed)`) so the agent can sanity-check span size
   before continuing.

---

## Issue 3 — Error readability

Related to #1. Concrete message upgrades worth doing together:

| Today | Better |
| --- | --- |
| `line must be a positive integer, got: 205.` | `invalid line locator "205.": trailing '.' (use PUT 205: or PUT 205.=205:)` |
| `unrecognized hashline line ...` | Keep, but point at the first unexpected character with a caret when cheap |
| Tag mismatch wall of text | Already includes live snapshot; good — keep |

Use `{raw:?}` / quoted forms so trailing spaces and dots stay visible.

---

## Issue 4 — Escape hatch to shell/Python under pressure

### What the agent did

After #1 and #2, subsequent call-site updates (`sdk_adapter.rs`, `mod_tests.rs`,
docs) were applied with `python3 - <<'PY' ... Path.write_text ...`.

### Why that is a signal

- The hashline tool's job is exactly this class of multi-file, multi-hunk work.
- When the agent of record abandons it mid-PR on its own implementation, the
  loop is telling you the happy path is too fragile under realistic refactor
  load.
- Shell/Python bypasses also skip snapshot provenance, which is the safety
  story this PR is selling.

### Suggested fixes

1. Land #1 and #2 mitigations first (highest leverage).
2. In agent prompt: **do not use shell/Python to rewrite files that `edit` can
   express**, and treat edit failures as retry-with-diagnostics, not
   format-switch.
3. Track a metric: `edit` failures followed by `bash`/`write` on the same
   path within one turn window (dogfood dashboards / eval).

---

## Issue 5 — Preview window vs next-op planning

Post-edit and chain snapshots cap body lines (~40) and collapse gaps with `…`.
That is correct for context cost. It is insufficient as the **sole** anchor
source for a follow-up structural CUT outside the focus window.

Already documented ("re-read for other lines"). Dogfood gap: agents still try.

### Suggested fixes

1. Prefer fail-closed live snapshots and re-read over session remap / unseen-line
   soft paths. This branch deletes `SnapshotStore`, recovery remap, and seen-line
   provenance so the product stays a direct tagged-read → apply → live-snapshot loop.
2. Keep post-edit previews focused; force re-read when the next op needs anchors
   outside that window (prompt + tool description).

---

## What worked

- Multi-hunk `PUT` with correct `N.=M:` / `N:` syntax and a fresh tag applied
  cleanly for large intentional rewrites (`apply.rs`, full-file `write` for
  `mod.rs`).
- Fail-closed stale tags and live snapshot on error are the right recovery UX
  when the agent uses them.
- Module split under 1k lines made targeted reads feasible.
- `write` for full-file rewrites is the correct escape when the agent is
  replacing most of a file — better than a 400-line PUT when the old text is
  irrelevant.

---

## Recommended action order

1. **Parser/error:** quote locators; special-case trailing `.`; unit test (landed).
2. **Delete shadow memory:** no session store, remap, or seen-line guardrails (landed).
3. **Eval:** multi-step large-PUT-then-CUT cleanup case; `PUT N.:` confusion case.
4. **Prompt:** prefer `edit` over shell/Python rewrites; re-read after structural edits when anchors leave the preview (landed direction).

---

## Appendix — session timeline (abbreviated)

1. `write` `apply.rs` / `apply_tests.rs` (full rewrite) — OK  
2. Large `edit` PUT on `format.rs` introducing unified renderer + accidental `SnapshotFooter` — OK apply  
3. `edit` CUT to remove `SnapshotFooter` — **file mangled**  
4. Repair via `edit` PUT restoring `format_hashline_view` header — OK  
5. `write` full `mod.rs` — OK  
6. Several single-line `edit` attempts with `PUT N.:` — **hard fail, misread error**  
7. Agent switched to `python3` path rewrites for recovery call sites, tests, docs  
8. User asked why Python; this report filed  

This report should be linked from `docs/dev/hashline-edit-eval.md` when the
eval harness grows multi-step / confusion cases.
