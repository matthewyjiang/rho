//! Shared geometry for the prompted composer input.

use super::render::display_width;

pub(super) const PROMPT_PREFIX: &str = "> ";

pub(super) fn prompt_width() -> usize {
    display_width(PROMPT_PREFIX)
}

pub(super) fn content_width(composer_width: usize) -> usize {
    composer_width.saturating_sub(prompt_width()).max(1)
}
