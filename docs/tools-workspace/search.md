# Search tools

Parent: [Tools and workspace](/tools-workspace).

`grep` searches file contents with a regex. `glob` lists files whose paths match a pattern. Both run in-process and do not need `rg`, `fd`, or [RTK](/integrations/rtk). Prefer these tools over shell search for workspace inspection: `grep` content mode mints chainable `[path#TAG]` snapshots (via the hashline header format) and match line numbers so `edit` can target anchors. Match text is search preview only.

- Patterns: `grep` takes a Rust/`regex` pattern. `glob` takes a path glob; a pattern with no `/` (for example `*.rs`) matches nested paths as `**/*.rs`.
- Defaults: both honor `.gitignore`, skip hidden files, and never follow symlinks. Pass `include_hidden` when you need dotfiles.
- Order: results come back in walk order, sorted by name within each directory, so repeat runs agree and a capped result is the first N paths shown rather than an arbitrary sample.
- Caps: results are bounded (default 200). `grep` also caps matches per file and trims long lines. Every capped, timed-out, or cancelled search says so in its summary, including when it found nothing.
- Output: `grep` groups matches by file. Content mode prefixes each file with a chainable `[path#TAG]` header and `N | text` match previews. Copy TAG and line numbers into `edit`; do not copy preview bodies into PUT rows (previews may be truncated). Use `read_file` when you need exact line text. Set `output_mode` to `files_with_matches` or `count` when you only need paths or tallies; default is `content`.
- Permissions: both request read access only, so they work in every permission mode, including `plan`.
