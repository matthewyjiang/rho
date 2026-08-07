# Theme

Parent: [Interactive TUI](/interactive-tui).

Rho has one theme today: it matches the terminal that hosts it. Text, accents,
status colors, muted chrome, and soft panels take their colors from that
terminal's palette.

## How it works

At startup, Rho samples the terminal background and standard ANSI color slots
when the host supports that query. Rho uses the terminal palette for the UI roles below:

| Role | Typical use |
| --- | --- |
| Default text | Body copy and ordinary labels |
| Dim | Muted chrome, secondary labels, separators |
| Accent | Brand marks, input prompt, focus highlights |
| Success / warning / error | Status, confirmations, and failures |
| Soft panels | User message and neutral tool backgrounds |

Soft panels blend a little of the palette into the terminal background so
blocks sit on the same surface as the rest of the TUI.

There is no theme picker and no theme setting in config. Change colors in your
terminal emulator, then restart Rho. Rho does not reload the palette while a
session is open.

## When sampling is unavailable

Some hosts cannot report exact RGB values. In that case Rho falls back to
standard ANSI colors and terminal defaults. The match is coarser than a full
palette sample.

## Future themes

Rho may support more themes in the future. Until then, the terminal-matched
look is the only supported theme.

## Related

- [Transcript display](/interactive-tui/transcript) - scroll, copy, and Markdown
- [Interactive TUI](/interactive-tui) - shortcuts and commands
