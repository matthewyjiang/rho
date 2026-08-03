---
name: rho-next-major-debt
description: Mark and inventory minor-compatible API compromises that must collapse on the next major release. Use when implementing an issue or PR that hits a semver wall, choosing dual variants or dual-emits over the ideal shape, reviewing public SDK/event/error surface changes, or preparing a major cutover. Grep NEXT_MAJOR( to list open debt.
compatibility: Applies to the Rho monorepo, especially rho-sdk public contracts.
---

# Next-major debt markers

Minor releases sometimes must ship a worse API shape to stay compatible (no new
fields on existing struct variants, no type changes on public enums, dual-emits
for 1.x hosts). That debt is easy to lose. **Mark it at the compromise site** so
the next major can clean it up in one pass.

Load this skill when:

- implementing an issue or PR that touches public contracts or would prefer a
  breaking shape
- `cargo-semver-checks` or review blocks the ideal API
- reviewing SDK/event/error diffs for un-marked compromises
- cutting a major release (inventory + land cleanups)

## Gate on every issue / PR

Before finishing implementation, answer:

1. Did this change choose a **worse shape only to stay minor-compatible**?
2. If yes, is there a greppable `NEXT_MAJOR(...)` marker on the compromised API?
3. Are construction/match helpers (if any) documented as the preferred surface
   until major?
4. If hosts must match carefully, is host-facing docs updated?

If (1) is yes and (2) is no, **add the marker before merge**. Do not file a
tracking issue *instead of* a code marker; the marker is the source of truth.

Unrelated ordinary TODOs, nits, and minor-safe cleanups are **not** next-major debt.

## Marker line

Exact token (one search inventories everything):

```text
NEXT_MAJOR(<crate-or-surface>): <imperative cleanup>
```

Examples:

```text
NEXT_MAJOR(rho-sdk): collapse RetryableFailure and RetryableFailureWithRetryAfter into one shape with optional retry_after
NEXT_MAJOR(rho-sdk): remove ProviderActivity and PROVIDER_ACTIVITY_* dual-emits
```

Rules for the cleanup phrase:

- Imperative, specific, and complete enough to act without re-reading the PR
- Name the preferred end state when non-obvious
- Keep one concern per marker (do not bundle unrelated majors)

## Where to put it

1. **On the compromised API (required)**
   - Public items: rustdoc `# Next major` section that includes the
     `NEXT_MAJOR(...)` line, why the compromise exists, and the preferred end
     state.
   - Private implementation: `// NEXT_MAJOR(...): ...` at the split site.
2. **Host-facing docs (preferred)** when external callers match the awkward shape.
3. **Helpers**: if you add construct/match helpers to paper over a split, document
   them as the stable surface until major and keep the marker on the underlying type.

### Public-item template

```rust
/// # Next major
///
/// NEXT_MAJOR(rho-sdk): <imperative cleanup and preferred end state>.
///
/// <One or two sentences: why this minor compromise exists; how to match/construct until major.>
```

### Private-site template

```rust
// NEXT_MAJOR(rho-sdk): <imperative cleanup and preferred end state>.
```

## What counts

Mark when the worse shape exists **only** for minor compatibility, including:

| Pattern | Typical preferred end state |
| --- | --- |
| Parallel enum variants for optional metadata | One variant (or event field) + `Option` |
| Metadata that belongs on an existing struct variant | Field on that variant (major) |
| Deprecated dual-emits / aliases for 1.x hosts | Remove dual path; typed-only |
| Public helpers that only paper over a temporary split | Inline into the collapsed type |

Do **not** mark:

- Cleanups that can ship in a minor
- Speculative refactors without a forced compromise
- Product TODOs unrelated to API shape / semver

## Default choices under a minor constraint

When the ideal shape is major-only:

1. Prefer an additive path: new `#[non_exhaustive]` enum variant, new event at
   the end of `RunEvent`, private struct fields + builders, newtypes.
2. Prefer **one** structured compromise with helpers over string encoding or dual
   events when possible.
3. Always leave `NEXT_MAJOR(...)` at the compromise.
4. Do not grow a second parallel compromise later—extend helpers or wait for major.

## Inventory

```bash
rg 'NEXT_MAJOR\(' -n
```

Unique cleanups (dedupe repeated markers on related items):

```bash
rg -o 'NEXT_MAJOR\([^)]+\):[^\n]+' -N | sort -u
```

## Major cutover

1. Run the inventory commands above.
2. For each marker, land the preferred end state.
3. Delete the markers and any helpers that only existed for the split.
4. Call out breaks in release notes / upgrade guide with `BREAKING CHANGE`.
5. Re-run `rg 'NEXT_MAJOR\('` and confirm only docs/examples (if any) remain, or none.

## Review checklist (agents)

On every issue implementation and PR that touches contracts:

- [ ] Ideal shape considered; if blocked by minor semver, compromise is intentional
- [ ] Every compromise has `NEXT_MAJOR(...)` on the API
- [ ] Helpers cover all arms; docs point hosts at helpers, not partial matches
- [ ] No new stringly or dual-path debt without a marker
- [ ] PR notes the compromise briefly under Breaking changes (none) or a short
      "Next major" bullet if useful for reviewers

## Related

- `AGENTS.md` — short pointer to this skill
- `rho-rust-change-validation` — run this gate while validating Rust changes
- `docs/sdk/compatibility.md` — public contract / major rules
- `docs/sdk/events-and-cancellation.md` — host-visible event behavior
