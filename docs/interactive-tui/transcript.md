# Transcript display

Parent: [Interactive TUI](/interactive-tui).

The interactive UI owns the transcript viewport while it is open, so use the built-in transcript scrolling controls instead of terminal scrollback. When you exit, your previous shell view returns and Rho prints only a short saved-session summary when a session exists.

Markdown ATX headings from `#` through `######` render without their syntax markers, using distinct terminal colors and stronger emphasis for the top three levels. Provider streams that deliver no data for two minutes are treated as stale, so Rho can reset or surface an error instead of remaining in the `working` state indefinitely.

Copied text is sent to the terminal clipboard, and Rho briefly shows how many characters were copied. Code block copy buttons are shown in the top-right border and highlight on hover.

When the transcript is scrolled away from the bottom, Rho overlays a right-aligned `↓ jump to bottom  ctrl+end` button on the last transcript row and obscures only the button's own cells. During generation, the spinner is similarly overlaid on the left. At the live bottom, transcript content stops one row above the spinner; while manually scrolled, the complete last row remains visible wherever neither control is drawn. Press `ctrl-end` or click the button to resume following live output.

For Mermaid rendering details, see [Mermaid diagrams](/interactive-tui/mermaid).
