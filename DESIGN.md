---
name: Rho Docs
description: Print-specimen docs for a lightweight Rust agent harness — newsprint, rich black, one synthetic red.
colors:
  newsprint: "#f2efe6"
  ink: "#121212"
  ink-soft: "#3a3834"
  accent: "#e63228"
  accent-deep: "#c41f1a"
  accent-bright: "#ff4d42"
  ink-plate: "#121212"
  ink-plate-fg: "#f2efe6"
  code-block-light: "#f3f0e6"
  tip: "#0b6e4f"
  tip-dark: "#3ecf8e"
  warning: "#9a6700"
  dark-newsprint: "#121110"
  dark-ink: "#f0ebe1"
  dark-ink-soft: "#c2bbb0"
  dark-accent: "#ff5a4f"
  dark-accent-deep: "#e63228"
  dark-ink-plate: "#050505"
  dark-code-block: "#0a0908"
  proof-plate: "#0d1117"
  proof-plate-light: "#ffffff"
  white: "#ffffff"
  white-short: "#fff"
typography:
  display:
    fontFamily: "Public Sans, ui-sans-serif, system-ui, sans-serif"
    fontWeight: 700
    letterSpacing: "-0.025em"
    lineHeight: 1.2
  body:
    fontFamily: "Public Sans, ui-sans-serif, system-ui, sans-serif"
    fontSize: "1.02rem"
    fontWeight: 400
    lineHeight: 1.7
  mono:
    fontFamily: "Source Code Pro, ui-monospace, monospace"
    fontWeight: 500
  glyph:
    fontFamily: "ui-sans-serif, system-ui, Segoe UI, Helvetica Neue, Arial, sans-serif"
    fontWeight: 700
  home-lede:
    fontFamily: "Public Sans, ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(1.45rem, 2.6vw, 2.05rem)"
    fontWeight: 700
    letterSpacing: "-0.025em"
    lineHeight: 1.18
  doc-h1:
    fontFamily: "Public Sans, ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(1.85rem, 3vw, 2.35rem)"
    fontWeight: 700
    letterSpacing: "-0.025em"
    lineHeight: 1.2
  doc-h2:
    fontFamily: "Public Sans, ui-sans-serif, system-ui, sans-serif"
    fontSize: "1.35rem"
    fontWeight: 700
  doc-h3:
    fontFamily: "Public Sans, ui-sans-serif, system-ui, sans-serif"
    fontSize: "1.12rem"
    fontWeight: 700
  home-h2:
    fontFamily: "Public Sans, ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(1.25rem, 2vw, 1.5rem)"
    fontWeight: 700
  wordmark-sm:
    fontFamily: "Public Sans, ui-sans-serif, system-ui, sans-serif"
    fontSize: "1.05rem"
    fontWeight: 700
  wordmark-md:
    fontFamily: "Public Sans, ui-sans-serif, system-ui, sans-serif"
    fontSize: "1.35rem"
    fontWeight: 700
  wordmark-lg:
    fontFamily: "Public Sans, ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(2.4rem, 6vw, 4rem)"
    fontWeight: 700
  wordmark-xl:
    fontFamily: "Public Sans, ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(2.75rem, 7vw, 5rem)"
    fontWeight: 700
  label:
    fontFamily: "Public Sans, ui-sans-serif, system-ui, sans-serif"
    fontSize: "0.8rem"
    fontWeight: 700
    letterSpacing: "0.04em"
  label-menu:
    fontFamily: "Public Sans, ui-sans-serif, system-ui, sans-serif"
    fontSize: "0.72rem"
    fontWeight: 700
    letterSpacing: "0.04em"
  nav:
    fontFamily: "Public Sans, ui-sans-serif, system-ui, sans-serif"
    fontSize: "0.92rem"
    fontWeight: 600
  nav-screen:
    fontFamily: "Public Sans, ui-sans-serif, system-ui, sans-serif"
    fontSize: "1rem"
    fontWeight: 600
  button:
    fontFamily: "Public Sans, ui-sans-serif, system-ui, sans-serif"
    fontSize: "0.95rem"
    fontWeight: 600
  search:
    fontFamily: "Public Sans, ui-sans-serif, system-ui, sans-serif"
    fontSize: "0.88rem"
    fontWeight: 600
  caption:
    fontFamily: "Source Code Pro, ui-monospace, monospace"
    fontSize: "0.78rem"
    fontWeight: 500
  kbd:
    fontFamily: "Source Code Pro, ui-monospace, monospace"
    fontSize: "0.72rem"
    fontWeight: 600
  path-detail:
    fontFamily: "Public Sans, ui-sans-serif, system-ui, sans-serif"
    fontSize: "0.92rem"
    fontWeight: 400
  menu-link:
    fontFamily: "Public Sans, ui-sans-serif, system-ui, sans-serif"
    fontSize: "0.9rem"
    fontWeight: 500
  code-inline:
    fontFamily: "Source Code Pro, ui-monospace, monospace"
    fontSize: "0.9em"
    fontWeight: 500
  code-block:
    fontFamily: "Source Code Pro, ui-monospace, monospace"
    fontSize: "0.88em"
    fontWeight: 400
  install-code:
    fontFamily: "Source Code Pro, ui-monospace, monospace"
    fontSize: "clamp(0.72rem, 1.65vw, 0.86rem)"
    fontWeight: 500
rounded:
  none: "0px"
spacing:
  rule: "1px"
  home-max: "1120px"
  layout-max: "1480px"
  section-y: "2.25rem"
  path-pad: "1rem"
components:
  button-primary:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.white}"
    rounded: "{rounded.none}"
    padding: "0.65rem 1.15rem"
    height: "2.75rem"
  button-primary-hover:
    backgroundColor: "{colors.accent-deep}"
    textColor: "{colors.white}"
    rounded: "{rounded.none}"
  button-alt:
    backgroundColor: "transparent"
    textColor: "{colors.ink}"
    rounded: "{rounded.none}"
    padding: "0.65rem 1.15rem"
    height: "2.75rem"
  button-alt-hover:
    backgroundColor: "{colors.ink}"
    textColor: "{colors.newsprint}"
    rounded: "{rounded.none}"
  code-fence:
    backgroundColor: "{colors.code-block-light}"
    textColor: "{colors.ink}"
    rounded: "{rounded.none}"
  proof-plate:
    backgroundColor: "{colors.proof-plate}"
    rounded: "{rounded.none}"
  path-row:
    textColor: "{colors.ink}"
    rounded: "{rounded.none}"
    padding: "1rem 0"
---

# Design System: Rho Docs

## Overview

**Creative North Star: "Print Specimen"**

Rho’s documentation site is a type specimen for a native binary — not a SaaS marketing page. Surfaces read like newsprint laid out with hairline rules, rich black ink, and a single synthetic red used sparingly for status and action. The home page is a first viewport of proof: large wordmark, job line, install command as a code plate, dual actions, and a terminal TUI capture. Interior pages stay calm enough to read for a long time while still belonging to the same sheet.

Personality is direct and technical. Density is measured: tight within a group, generous between sections, no soft glass, no gradient theater. Light and dark are both first-class; dark is charcoal newsprint with lifted red, not a pure OLED invert.

**Key Characteristics:**
- Newsprint ground + rich black ink + one synthetic red
- Square corners everywhere (radius 0)
- Hairline rules and ink underlines instead of shadows
- Public Sans for UI/body; Source Code Pro for code
- Proof lives in theme-matched terminal plates (dark + light SVG captures), not lifestyle imagery
- VitePress default theme extended — identity in tokens and chrome, not a from-scratch app shell

## Colors

A warm paper field with near-black ink and a single signal red. Semantic greens/ambers exist only for tip/warning callouts.

### Primary
- **Specimen Red** (`#e63228` / dark `#ff5a4f`): Primary actions, active nav, focus rings, wordmark ρ glyph. Rarity is the point.
- **Specimen Red Deep** (`#c41f1a` / dark `#e63228`): Hover/active deepen of the accent.

### Neutral
- **Newsprint** (`#f2efe6` / dark `#121110`): Page background.
- **Ink** (`#121212` / dark `#f0ebe1`): Primary text, heavy rules, alt-button hover fill.
- **Ink Soft** (`#3a3834` / dark `#c2bbb0`): Secondary text, captions, sidebar section labels.
- **Ink Plate** (`#121212` fg `#f2efe6`): Reserved for inverted specimen blocks when needed.
- **Code Block Light** (`#f3f0e6`): Light-mode fence plate (keeps Shiki light tokens contrasted).
- **Code Block Dark** (`#0a0908`): Dark-mode fence plate.
- **Proof Plate** (`#0d1117` dark / `#ffffff` light): Terminal capture frame behind the TUI SVG.

### Named Rules
**The One Red Rule.** Synthetic red is the only brand chroma on chrome and actions. Do not introduce a second brand hue for links, charts, or decoration.

**The Paper Rule.** Backgrounds stay newsprint (or charcoal newsprint). Do not drift to pure white, pure black, or cool gray SaaS surfaces.

## Typography

**Display Font:** Public Sans (ui-sans-serif, system-ui)
**Body Font:** Public Sans (ui-sans-serif, system-ui)
**Mono Font:** Source Code Pro (ui-monospace, monospace)

**Character:** Industrial grotesque clarity — self-hosted, no Inter, no system-display costume. Open features `ss01` and `cv11` stay on for body UI. The ρ glyph in the wordmark uses a system UI sans because Public Sans is Latin-only.

### Hierarchy
- **Home wordmark** (700, clamp 2.75rem–5rem, tight tracking): ρ + Rho lockup.
- **Home lede** (700, clamp 1.45rem–2.05rem, ~16em max, balanced wrap): Job line under the mark.
- **Doc H1** (700, clamp 1.85rem–2.35rem, ink underline rule beneath): Page title.
- **Doc H2** (700, 1.35rem, hairline rule above): Section starts.
- **Body** (400, 1.02rem / 1.7, measure guided toward ~72ch): Reading copy.
- **Label** (700, ~0.8rem, +0.04em, uppercase): Sidebar section heads, menu group titles.
- **Mono** (500–600, Source Code Pro): Fences, inline code, proof captions, kbd keys.

### Named Rules
**The No-Costume Mono Rule.** Monospace is for code, data, and measurement captions — never for marketing headlines.

**The Tracking Floor.** Letter-spacing does not go tighter than about -0.03em on UI type; display stays near -0.025em.

## Layout

VitePress shell with a custom full-bleed home (`layout: page`). Content max for home bands is **1120px**; global VP layout max is **1480px**. Home hero becomes two columns from **900px** (copy | proof). Guide paths become two columns from **720px**. Docs sidebar and outline follow VP breakpoints (~960px+).

Spacing rhythm: section padding ~2.25–3.25rem vertical; path rows 1rem vertical with hairline separators; groups tight, bands separated by full-width rules. Body paragraphs prefer a readable measure; tables and fences may span the content column.

## Elevation & Depth

**Flat by default.** Depth is ink, not shadow: 1px hairline borders, solid nav (never translucent), tonal soft fills for code chips and search selection. No drop shadows on cards, menus, or fences.

### Named Rules
**The No-Glass Rule.** No backdrop blur, no frosted nav over scrolling content. Nav is opaque newsprint/charcoal always.

**The Hairline Rule.** Separation is a 1px rule at ~12–16% ink — not a soft gray card edge and not a colored left bar.

## Shapes

**Radius is zero** across buttons, fences, search, menus, images, pagers, and appearance strips. The only intentional round control is the small appearance *switch* thumb (native toggle affordance). Borders are 1px; proof and terminal imagery sit in hard rectangular plates.

## Components

### Buttons
- **Shape:** Square (`border-radius: 0`), min-height 2.75rem, display face weight 600.
- **Primary:** Specimen red fill, white label; hover deepens to accent-deep.
- **Alt / secondary:** Transparent with ink border; hover inverts to ink fill + newsprint text.
- **Focus:** 2px specimen-red outline, 2px offset (global `:focus-visible`).

### Code fences
- Square plate, 1px ink-tinted border, theme-matched block background.
- Copy control is square and icon-only on success — no expanding “Copied!” pill.
- Home install fence reuses VP fence markup so copy behavior stays native.

### Proof plate
- Hard ink border; dark well `#0d1117` or light well `#ffffff` from the matching SVG; linked to Interactive TUI docs.
- Mono caption under the frame (“Interactive TUI”).

### Path list (home Guides)
- Rule-separated rows, not cards: title + detail + red arrow.
- Hover shifts title to specimen red; no fill flash, no shadow.

### Navigation
- Solid newsprint bar, hairline bottom rule, wordmark mark + SR title.
- Active items and outline links use specimen red.
- Sidebar level-0 labels are uppercase specimen labels; level-1+ are sentence case.
- Desktop flyouts and mobile nav screen: square plates, hairline borders, no soft shadow.
- Search trigger: square bordered control with mono ⌘K keys.

### Wordmark
- ρ (accent) + “Rho” (ink). Sizes: sm (nav), md, lg, xl (home).
- `decorative` mode for nav (parent owns the accessible name); labeled mode for home `<h1>`.

### Doc chrome
- H1 sits on a heavy ink underline; H2 opens with a hairline rule above.
- Inline code: square chip, hairline border, soft ink wash.
- Tables: hairline grid, slightly inked header row, subtle zebra.
- Pagers: square bordered links; title in display face.

## Do's and Don'ts

### Do:
- **Do** keep corners at 0px and separate regions with hairline rules.
- **Do** use specimen red only for primary action, active state, focus, and the ρ glyph.
- **Do** ship light and dark with the same geometry; only ink/paper/accent shift.
- **Do** put product proof in theme-matched terminal plates (SVG/PNG captures), not stock illustration.
- **Do** prefer VitePress structural components restyled with tokens over one-off card grids.

### Don't:
- **Don't** introduce rounded SaaS chrome, glass blur, gradient text, or soft multi-shadow cards.
- **Don't** add a second brand color or rainbow provider badges as identity.
- **Don't** use monospace for marketing headlines or section titles.
- **Don't** invent metrics, customers, or competitive claims in the UI.
- **Don't** lock the site to dark-only or light-only.
- **Don't** replace factual doc copy during visual work without an explicit content edit.
