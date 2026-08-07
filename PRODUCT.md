# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

Primary readers of the docs site are developers evaluating Rho for the first time (install, auth, first run), then daily Rho users looking up configuration, TUI behavior, tools, and workflows, then Rust embedders integrating `rho-sdk`. The homepage lands evaluators; interior pages serve all three.

## Product Purpose

Rho is a lightweight coding-agent harness inspired by Pi, built in Rust. It gives developers a terminal-native agent UI, one-shot automation CLI, deterministic workflows, and an embeddable headless SDK without a plugin store.

Success for the docs site: a visitor understands what Rho is, can install and run it quickly, and can find the right guide (TUI, CLI, workflows, config, SDK) without marketing noise.

## Positioning

Rho is small on purpose: a native Rust binary with coding tools, RTK, and Herdr integration built in. Differentiator: low process overhead, bring-your-own provider, and an embeddable SDK with explicit capabilities - not a heavyweight multi-surface AI product suite.

## Operating Context

- Developers work in terminals, editors, CI, and local git worktrees.
- Primary product surfaces: interactive TUI (`rho`), automation (`rho run`), workflows, configuration under `~/.rho`, and Rust host embedding via `rho-sdk`.
- Docs ship as a VitePress site (`docs/`, base `/rho/`) with local search, Mermaid diagrams, and install scripts served from the site root.
- Real product imagery is terminal-dark (GitHub-adjacent charcoal, cyan/green status accents).

## Capabilities and Constraints

- Stack is fixed: VitePress default theme extended via `docs/.vitepress/theme`.
- Site must remain useful in both light and dark color schemes.
- Do not invent customers, benchmarks, pricing, or unverified claims. Existing proof assets may be used and restyled.
- Navigation may be rewritten; product facts in markdown pages remain authoritative unless deliberately edited.
- Install paths, GitHub links, and provider lists are product truth.

## Brand Commitments

- Name: **Rho**
- Voice: direct, technical, concise. No hype, no playful mascot tone.
- Explicit anti-goals for visual work: generic AI-startup marketing look; heavy chrome that slows reading; bright playful branding; locking to dark-only or light-only.

## Evidence on Hand

- `docs/assets/rho-ui-demo.svg` - terminal UI session demo
- `docs/assets/cli-overhead.svg` - CLI startup/memory comparison chart
- `docs/assets/subagent-panel.png` - subagent panel screenshot
- README and full guide corpus under `docs/`
- Live site: https://matthewyjiang.github.io/rho/
- Repo: https://github.com/matthewyjiang/rho

Do not fabricate testimonials, adoption metrics, or competitor claims beyond what existing assets already show.

## Product Principles

1. **Clarity over theater** - reading and install paths beat spectacle.
2. **Prove, don't pitch** - show the TUI and overhead evidence instead of slogans.
3. **Fast path to first run** - homepage and start docs optimize for install → auth → `rho`.
4. **Serve three depths** - evaluator, operator, embedder without burying any of them.
5. **Respect both light and dark** - terminal identity without abandoning light-mode readers.
