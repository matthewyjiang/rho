# Theme

Parent: [Interactive TUI](/interactive-tui).

Rho can match the host terminal or use a named color theme. Open the picker with
`/theme`, or from `/config` → **Models & reasoning** → **Theme**.

## Default: terminal

The default theme is `terminal`. Rho samples the host background and standard
ANSI color slots at startup (when the host supports that query) and maps them
into UI roles:

| Role | Typical use |
| --- | --- |
| Default text | Body copy and ordinary labels |
| Dim | Muted chrome, secondary labels, separators |
| Accent | Brand marks, input prompt, focus highlights |
| Success / warning / error | Status, confirmations, and failures |
| Soft panels | User message and neutral tool backgrounds |

Soft panels blend a little of the palette into the background so blocks sit on
the same surface as the rest of the TUI.

## Built-in themes

These ship with Rho and work offline:

| Id | Name |
| --- | --- |
| `terminal` | Match the host terminal (default) |
| `one-half-dark` | One Half Dark |
| `one-half-light` | One Half Light |
| `monochrome-dark` | Monochrome Dark |
| `monochrome-light` | Monochrome Light |

Named themes paint Rho's own surface and RGB accents so light and dark schemes
stay readable even when the host terminal uses the opposite mode.

## Custom themes

Drop Windows Terminal color-scheme JSON files into:

```text
~/.rho/themes/
```

or `$RHO_HOME/themes/` when `RHO_HOME` is set. The file stem is the theme id
(`dracula.json` → `dracula`). Each file should include `background`,
`foreground`, the eight normal colors (`black` … `white`), the eight bright
colors (`brightBlack` … `brightWhite`), and may include `name`, `cursorColor`,
and `selectionBackground`.

That shape matches common catalogs such as
[iTerm2-Color-Schemes](https://github.com/mbadolato/iTerm2-Color-Schemes)
(`windowsterminal/*.json`) and [windowsterminalthemes.dev](https://windowsterminalthemes.dev/).

Built-in ids are reserved. A custom file named `one-half-dark.json` is ignored so
it cannot hide the built-in scheme.

## Picker and preview

The theme picker lists terminal, built-in, and custom schemes together, sorted
by name. Moving the selection previews colors live on the whole UI. Enter saves
the choice to config and keeps it. Escape restores the previous theme.

## Config

```toml
[display]
theme = "terminal" # or one-half-dark, monochrome-light, a custom file stem, ...
```

Changes from `/theme` or `/config` apply immediately and persist in
`config.toml`.

## When sampling is unavailable

Some hosts cannot report exact RGB values. With `theme = "terminal"`, Rho falls
back to standard ANSI colors and terminal defaults. The match is coarser than a
full palette sample. Named themes do not need host sampling.

## Related

- [Transcript display](/interactive-tui/transcript) - scroll, copy, and Markdown
- [Configuration](/configuration) - `display` group settings
- [Interactive TUI](/interactive-tui) - shortcuts and commands
