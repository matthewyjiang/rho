# Mermaid diagrams

Parent: [Interactive TUI](/interactive-tui). Related: [Transcript display](/interactive-tui/transcript).

Closed fenced code blocks whose first info token is `mermaid` render as terminal-native Unicode diagrams. The match is case-insensitive and extra info tokens are allowed. During streaming, an open fence remains a normal source code block and changes to diagram art only when its closing fence arrives. The diagram is laid out again when the terminal width changes.

The published docs site also renders fenced `mermaid` blocks with Mermaid.js, so the same diagram source works in both the TUI and the guide pages.

Rho uses `mermaid-rs-renderer` 0.3.1 as its Mermaid parser and semantic model. The terminal painter provides quality-first support for core subsets of flowcharts and graphs, state diagrams, sequence diagrams, class diagrams, and entity-relationship diagrams. Other diagram families and constructs the painter cannot represent losslessly remain raw code blocks, as do unsupported syntax and malformed input. This is not full Mermaid.js syntax or visual parity.

Flowcharts and state diagrams keep the direction you asked for. When their normal layout is wider than the pane, Rho wraps node labels more tightly and lays the diagram out again, down to a readable limit. Compaction never shortens or truncates label text.

Diagrams Rho cannot draw stay readable as source, and the panel border says why. A diagram that needs a wider pane reads `MERMAID · PANE TOO NARROW`, so you can widen the pane or the terminal to see the art. Everything else Rho declines to draw, such as unsupported, malformed, unsafe, or oversized input, reads `MERMAID · NOT RENDERED`. Very narrow panels drop the label so the `COPY` action keeps its place. Resizing moves a diagram between art and source in both directions.

Rendering does not execute links or scripts, requires no external executable or network access, and does not trust Mermaid-provided terminal styles. The panel's `COPY` action copies the original Mermaid source rather than the rendered box art, for both diagrams and source fallbacks.
