# Mermaid diagrams

Parent: [Interactive TUI](/interactive-tui).
Related: [Transcript display](/interactive-tui/transcript).

Closed fenced code blocks whose first info token is `mermaid` render as
terminal-native Unicode diagrams in the transcript. The same source also
renders on the published docs site with Mermaid.js, so one diagram works in
both places.

```mermaid
flowchart TD
    fence[Closed mermaid fence] --> closed{Fence closed?}
    closed -->|no| code[Ordinary code block while streaming]
    closed -->|yes| parse[Parse with mermaid-rs-renderer]
    parse --> kind{Supported kind and safe?}
    kind -->|yes and fits pane| art[Unicode diagram art]
    kind -->|too wide| narrow[Source plus PANE TOO NARROW]
    kind -->|unsupported kind| raw[Source plus UNSUPPORTED]
    kind -->|other decline| other[Source plus INVALID TOO LARGE or NOT RENDERED]
    art --> resize[Relayout on width change]
    narrow --> resize
    raw --> resize
```

## When a fence becomes a diagram

| Rule | Detail |
| --- | --- |
| Fence info | First token is `mermaid` (case-insensitive). Extra tokens after it are allowed |
| Streaming | An open fence stays a normal source block until the closing fence arrives |
| Resize | Art is laid out again when the terminal or pane width changes |
| Copy | Panel `COPY` always copies the original Mermaid source, for art and fallbacks |

Example:

````markdown
```mermaid
flowchart LR
    read[read_file] --> edit[edit]
```
````

## What the terminal painter supports

Rho parses with `mermaid-rs-renderer` **0.3.1**. The terminal painter aims for
lossless, readable art on a core subset:

| Family | Terminal art |
| --- | --- |
| Flowcharts / graphs | Yes (core subset) |
| State diagrams | Yes (core subset) |
| Sequence diagrams | Yes (core subset) |
| Class diagrams | Yes (core subset) |
| Entity-relationship diagrams | Yes (core subset) |
| Pie, gantt, gitGraph, C4, mindmap, journey, timeline, and other kinds | Source fallback |

This is not full Mermaid.js syntax or visual parity. The painter prefers a
readable approximation over a source dump: styles are ignored, common shapes
map onto rectangle / round / diamond, parallel edges share a route with joined
labels, and a too-wide `LR`/`RL` flowchart retries as `TD`. Exotic families and
malformed input stay as source.

### Flow and state layout

Flowcharts and state diagrams keep the direction you asked for (`TD`, `LR`, and
so on) when it fits. When the normal layout is wider than the pane, Rho wraps
node and edge labels more tightly and lays the diagram out again, down to a
readable limit. Compaction never shortens or truncates node label text. If a
horizontal flowchart still cannot fit, Rho retries top-down. If even that
cannot fit, the panel falls back to source with a narrow-pane title.

## Fallback titles

Diagrams Rho cannot draw stay readable as source. The panel border says why:

| Border title | Meaning |
| --- | --- |
| `MERMAID · PANE TOO NARROW` | Needs a wider pane or terminal; resize may produce art |
| `MERMAID · UNSUPPORTED` | Kind or construct the terminal painter will not draw |
| `MERMAID · INVALID` | Source did not parse |
| `MERMAID · TOO LARGE` | Source, model, or painted output exceeded a hard cap |
| `MERMAID · NOT RENDERED` | Blank, unsafe, or other decline |

Very narrow panels drop the title text so the `COPY` action keeps its place.
Resizing can move a diagram between art and source in both directions.

## Safety and limits

Rendering:

- does not execute links or scripts
- needs no external executable or network access
- does not trust Mermaid-provided terminal styles
- strips or rejects unsafe content (for example script-like labels, `javascript:`
  URLs, and ANSI escapes in labels)

Hard caps protect the feed (approximate ceilings):

| Cap | Value |
| --- | --- |
| Source bytes | 64 KiB |
| Source lines | 2,048 |
| Primary entities | 128 |
| Relationships | 512 |
| Rendered lines | 4,096 |
| Rendered cells | 2,000,000 |

Over a cap, the panel keeps the source and shows `MERMAID · TOO LARGE`.

## Writing diagrams for the TUI

Prefer small graphs. Stick to flowchart, state, sequence, class, or ER shapes
when the diagram should paint in the terminal. Common extras such as `[(db)]`
nodes, `classDef`, sequence `alt`/`activate`, and long edge labels now paint
as approximations. Larger or exotic Mermaid families still ship as readable
source for copy-out and for the docs site.

The agent system prompt uses the same guidance for structure-heavy answers:
closed `mermaid` fences, small diagrams, wrap-friendly labels.

## Related

- [Transcript display](/interactive-tui/transcript) - scroll, copy, and Markdown
- [Math rendering](/interactive-tui/math) - terminal math art
- [Interactive TUI](/interactive-tui)
- Docs authoring in this repo also uses fenced `mermaid` blocks (VitePress +
  Mermaid.js on the site)
