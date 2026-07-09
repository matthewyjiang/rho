# TUI rendering refactor todo

## Planning

- [x] Identify current inline rendering mechanics
- [x] Confirm transcript state already exists in `App`
- [x] Decide target architecture: fullscreen app-owned rendering
- [x] Decide primary jump keybind: `ctrl+g`
- [x] Confirm desired exit summary wording before final polish

## Rendering refactor

- [x] Add pure `history_lines(width, now)` helper
- [x] Add fullscreen screen layout helper
- [x] Render history viewport from app-owned lines
- [x] Render composer/status/command regions through layout
- [x] Preserve cursor positioning for all composer modes
- [x] Preserve long input visibility behavior
- [x] Preserve picker/questionnaire/config/secret composer modes

## Terminal lifecycle

- [x] Switch initialization from inline viewport to fullscreen alternate screen
- [x] Remove inline viewport resizing/reflow path
- [x] Stop inserting finalized history into terminal scrollback
- [x] Make `insert_entry` state-only
- [x] Replace full transcript printing on exit with minimal summary behavior

## Scroll behavior

- [x] Add `HistoryScroll` state
- [x] Implement visible history slice calculation
- [x] Implement PageUp/PageDown scrolling
- [x] Implement `ctrl+g` jump to bottom
- [x] Keep bottom-follow behavior during streaming
- [x] Preserve manual viewport while new output arrives
- [x] Clamp scroll position on resize

## Scroll-to-bottom button

- [x] Render button only when history is not at bottom
- [x] Position button directly above composer
- [x] Include keybind text in button
- [x] Add compact/narrow terminal rendering
- [x] Add click hit testing

## Mouse support

- [x] Enable mouse capture in fullscreen mode
- [x] Disable mouse capture on shutdown
- [x] Handle wheel up/down for history scrolling
- [x] Handle jump button click
- [ ] Verify text selection tradeoff is acceptable in alternate screen - pending manual review

## Cleanup

- [x] Remove `insert_history_lines`
- [x] Remove `clear_terminal_for_history_reflow`
- [x] Remove `reflow_history`
- [x] Remove `replay_history`
- [x] Remove `resize_inline_viewport_if_needed`
- [x] Remove inline viewport fields from `App`
- [x] Remove stale imports
- [x] Rename or replace `INLINE_VIEWPORT_HEIGHT`
- [x] Update docs for new controls and exit behavior

## Validation

- [x] Run `cargo fmt`
- [x] Run focused TUI tests (`cargo test tui:: --no-fail-fast`)
- [x] Run broader test suite (`cargo test --no-fail-fast`)
- [ ] Smoke test with `cargo run` - launched fullscreen and verified alternate-screen restore after forced exit; still need interactive key/click validation
- [ ] Verify no full transcript is printed on exit - only verified no-session forced-exit restore, not session summary path
- [ ] Verify scroll-to-bottom key and click behavior manually - pending smoke test
