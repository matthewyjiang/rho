# TUI rendering refactor plan

## Goal

Move Rho from an inline, terminal-scrollback-backed UI to a fullscreen, app-owned Ratatui UI so Rho can:

- know whether the user is viewing the bottom of the transcript
- support app-owned scrolling
- show a scroll-to-bottom affordance only when needed
- support a keybind and click target for jumping to the bottom
- stop relying on terminal scrollback as the transcript viewport

The key design change is:

```text
current:
  finalized transcript is inserted into terminal scrollback
  Ratatui only owns the live bottom viewport

target:
  transcript stays in App state
  Ratatui renders the whole screen from state every frame
  App owns history scroll position
```

## Current model

Today the TUI starts Ratatui with an inline viewport:

```rust
ratatui::init_with_options(TerminalOptions {
    viewport: Viewport::Inline(INLINE_VIEWPORT_HEIGHT),
})
```

Finalized messages are pushed into the real terminal scrollback through `insert_history_lines(...)`, which calls `terminal.insert_before(...)`. The live Ratatui frame owns only the composer, statusline, command suggestions, pending tool call, spinner, and live stream preview.

This model preserves visible terminal history after Rho exits, but it means the terminal emulator owns scroll position. Rho cannot reliably know whether the user is scrolled away from the bottom.

## Target model

Use fullscreen alternate-screen Ratatui:

```rust
let mut terminal = ratatui::init();
```

Then render the whole app every frame from `App` state.

A target screen layout:

```text
┌────────────────────────────────────────┐
│ transcript viewport                    │
│                                        │
│ assistant / tool / user history        │
│ live stream preview                    │
│                                        │
├────────────────────────────────────────┤
│ ↓ jump to bottom  ctrl+g               │  only when scrolled up
├────────────────────────────────────────┤
│ composer input                         │
├────────────────────────────────────────┤
│ statusline                             │
│ command palette suggestions            │
└────────────────────────────────────────┘
```

## Phase 1: introduce app-owned history rendering

Add a pure rendering path that can produce all transcript/history lines without writing them into the terminal.

Suggested helper:

```rust
fn history_lines(&self, width: usize, now: Instant) -> Vec<Line<'static>>;
```

This should include:

- session header
- recorded transcript entries
- pending tool call, if any
- live stream preview, if any
- loading spinner, if active

Important: keep existing behavior mostly intact while adding this pure rendering path. This makes the diff easier to review.

Affected areas:

- `src/tui.rs`
- existing helpers:
  - `session_header_lines`
  - `entry_lines`
  - `render_stream_preview_lines`
  - `transcript_lines`

## Phase 2: add fullscreen layout rendering

Replace `active_frame_at_for_height` with a layout-driven draw path that splits the full frame into regions:

```text
history viewport
optional jump-to-bottom button
divider
composer
divider
statusline
command suggestions
```

Likely internal type:

```rust
struct ScreenLayout {
    history: Rect,
    jump_to_bottom: Option<Rect>,
    top_divider: Rect,
    composer: Rect,
    bottom_divider: Rect,
    statusline: Rect,
    commands: Rect,
    composer_start: usize,
}
```

The layout must preserve current behavior for:

- long multiline input
- command palette
- picker UI
- questionnaire UI
- secret/config input UI
- statusline visibility
- cursor visibility

Composer handling should continue to use the existing `visible_composer_start(...)` logic or a close replacement.

## Phase 3: switch terminal initialization to fullscreen alternate screen

Change startup from inline viewport initialization to fullscreen alternate-screen initialization.

Then remove or stop using inline viewport machinery:

- `resize_inline_viewport_if_needed`
- `desired_inline_viewport_height`
- `reflow_history`
- `replay_history`
- `clear_terminal_for_history_reflow`
- `insert_history_lines`
- `inline_viewport_height`
- `inline_viewport_width`

`insert_entry(...)` should become state-only:

```rust
fn insert_entry(&mut self, entry: Entry) {
    self.record_inserted_entry(entry);
}
```

To keep the first mechanical change smaller, it is acceptable to temporarily keep the existing `terminal` parameter at call sites and remove it in a cleanup pass.

## Phase 4: add history scroll state

Use explicit app-owned scroll state. Prefer a top-line anchor over a raw offset-from-bottom because it handles new streamed content predictably.

Suggested state:

```rust
enum HistoryScroll {
    Bottom,
    Manual { top_line: usize },
}
```

Behavior:

- `Bottom` means always follow new output.
- `Manual { top_line }` means keep showing the same top line while new content arrives.
- The jump button appears when the computed visible start is above the bottom.
- Jumping to bottom sets state back to `Bottom`.

Helpers:

```rust
fn scroll_history_to_bottom(&mut self);
fn scroll_history_page_up(&mut self, width: usize, height: usize);
fn scroll_history_page_down(&mut self, width: usize, height: usize);
fn scroll_history_lines(&mut self, width: usize, height: usize, delta: isize);
fn history_at_bottom(&self, width: usize, height: usize) -> bool;
```

## Phase 5: add keyboard controls

Add key handling for history scrolling:

| Key | Action |
| --- | --- |
| `pageup` | scroll history up |
| `pagedown` | scroll history down |
| `ctrl+g` | jump to bottom |

Avoid stealing `End` because it currently means move input cursor to end, which is expected composer behavior.

The visible button should show:

```text
↓ jump to bottom  ctrl+g
```

Use a compact variant on narrow terminals.

## Phase 6: add mouse support

Once fullscreen rendering is in place, enable mouse capture:

- `EnableMouseCapture` on startup
- `DisableMouseCapture` on shutdown

Then handle:

- mouse wheel up: scroll history up
- mouse wheel down: scroll history down
- click on the jump button row: jump to bottom

Implementation detail: avoid storing click rectangles from `draw(&self)` if possible. Instead, use the same layout helper in both draw and event handling so hit testing is deterministic.

## Phase 7: revise exit behavior

Since the alternate screen restores the shell view on exit, stop calling `print_exit_lines(...)` to dump the full transcript.

Replace that with either:

1. print nothing, or
2. print a short summary, for example:

```text
rho session saved: <session-id>
```

Recommendation: print the short summary when a session id exists. Otherwise print nothing or a minimal `rho exited`.

This avoids clearing the terminal after restore, which `print_exit_lines(...)` currently does.

## Phase 8: cleanup old inline assumptions

After the fullscreen path is working:

- remove `ActiveFrame` if no longer needed
- remove inline viewport fields from `App`
- remove unused imports:
  - `TerminalOptions`
  - `Viewport`
  - possibly `CrosstermBackend`
  - possibly `Terminal`
- rename or replace `INLINE_VIEWPORT_HEIGHT` if it is only being used for picker sizing
- update docs to mention:
  - `pageup` / `pagedown`
  - `ctrl+g`
  - scroll-to-bottom button
  - transcript no longer remains printed in terminal after exit


## Implementation status

Implemented in this branch:

- Rho initializes Ratatui in fullscreen alternate-screen mode with mouse capture.
- Transcript rendering is state-owned through `history_lines(width, now)`.
- The frame is split through `ScreenLayout` into history, optional jump button, composer, statusline, and command regions.
- `HistoryScroll` supports bottom-follow and manual top-line anchoring.
- `pageup`, `pagedown`, `ctrl+g`, mouse wheel, and jump-button click are wired.
- Finalized entries no longer write into terminal scrollback.
- Exit no longer dumps the full transcript; it prints `rho session saved: <session-id>` when a session exists.

Still pending manual validation:

- Full interactive smoke test for alternate-screen startup/restore, wheel/click behavior, and text-selection tradeoff.
- `INLINE_VIEWPORT_HEIGHT` was renamed to `DEFAULT_TUI_HEIGHT` for picker/test sizing.

## Test plan

### Unit and buffer tests

Add or update tests for:

- history rendering includes session header and transcript entries
- fullscreen draw shows latest transcript at bottom when in `Bottom` mode
- `pageup` enters manual scroll mode
- `pagedown` moves toward bottom
- `ctrl+g` returns to bottom
- jump button appears only when not at bottom
- jump button line is rendered directly above the composer area
- clicking jump button returns to bottom
- mouse wheel scrolls history
- new streamed content follows when at bottom
- new streamed content does not yank the viewport when manually scrolled up
- long composer input still keeps cursor and statusline visible
- command palette still renders below composer
- picker and questionnaire rendering still work

### Existing regressions to protect

Preserve behavior covered by existing tests around:

- command palette
- input cursor positioning
- long input visibility
- picker rendering
- streaming preview
- tool output expansion/collapse
- questionnaire cursor behavior

### Manual smoke test after implementation

Run from source and verify:

- startup uses full screen
- old terminal content returns after exit
- transcript scrolls with PageUp/PageDown
- wheel scroll works
- button appears when scrolled up
- click button jumps to bottom
- `ctrl+g` jumps to bottom
- running model output follows bottom unless manually scrolled up
- exit prints only the chosen small summary

## Proposed first implementation checkpoint

The first checkpoint should be:

> Rho launches fullscreen and renders the transcript from app state, but without mouse support yet.

That gives us a solid base before adding scroll state and click handling.
