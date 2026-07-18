use super::{
    markdown::render_markdown,
    render::{padded_inner_width, RenderedEntry},
    theme::Theme,
};

pub(super) fn render_assistant_content(text: &str, width: usize) -> RenderedEntry {
    let mut in_code_block = false;
    let rendered = render_markdown(text, padded_inner_width(width), &mut in_code_block);
    RenderedEntry {
        lines: rendered.lines,
        code_blocks: rendered.code_blocks,
    }
}

pub(super) fn render_reasoning_content(text: &str, width: usize) -> RenderedEntry {
    let mut rendered = render_assistant_content(text, width);
    Theme::reasoning_output(&mut rendered.lines);
    rendered
}
