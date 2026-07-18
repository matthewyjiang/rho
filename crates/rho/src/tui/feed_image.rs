use std::{cell::RefCell, fmt, path::Path, rc::Rc};

use image::ImageReader;
use ratatui::{layout::Rect, text::Line, Frame};
use ratatui_image::{
    picker::{Picker, ProtocolType},
    protocol::StatefulProtocol,
    Resize, StatefulImage,
};

pub(super) const IMAGE_HEIGHT: u16 = 12;
pub(super) const IMAGE_MARKER_PREFIX: &str = "\u{0}rho-image:";

#[derive(Clone)]
pub(super) struct FeedImage {
    id: u64,
    state: Rc<RefCell<StatefulProtocol>>,
}

impl fmt::Debug for FeedImage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FeedImage")
            .field("id", &self.id)
            .finish()
    }
}

impl FeedImage {
    pub(super) fn load(id: u64, path: &Path, picker: &Picker) -> image::ImageResult<Self> {
        let image = ImageReader::open(path)?.with_guessed_format()?.decode()?;
        Ok(Self {
            id,
            state: Rc::new(RefCell::new(picker.new_resize_protocol(image))),
        })
    }

    pub(super) fn marker(&self) -> String {
        format!("{IMAGE_MARKER_PREFIX}{}", self.id)
    }

    pub(super) fn id(&self) -> u64 {
        self.id
    }

    pub(super) fn push_placeholder_lines(&self, lines: &mut Vec<Line<'static>>) {
        lines.push(Line::raw(self.marker()));
        lines.extend((1..IMAGE_HEIGHT).map(|_| Line::raw("")));
    }

    pub(super) fn render(&self, frame: &mut Frame<'_>, area: Rect) {
        frame.render_stateful_widget(
            StatefulImage::default().resize(Resize::Fit(None)),
            area,
            &mut *self.state.borrow_mut(),
        );
    }
}

pub(super) fn take_visible_image_rows(lines: &mut [Line<'static>]) -> Vec<(u64, u16)> {
    lines
        .iter_mut()
        .enumerate()
        .filter_map(|(row, line)| {
            let id = line
                .to_string()
                .strip_prefix(IMAGE_MARKER_PREFIX)?
                .parse()
                .ok()?;
            *line = Line::raw("");
            Some((id, row as u16))
        })
        .collect()
}

impl super::App {
    pub(super) fn load_feed_image(&mut self, path: &Path) -> image::ImageResult<Option<FeedImage>> {
        let Some(picker) = &self.image_picker else {
            return Ok(None);
        };
        let image = FeedImage::load(self.next_feed_image_id, path, picker)?;
        self.next_feed_image_id = self.next_feed_image_id.saturating_add(1);
        Ok(Some(image))
    }

    pub(super) fn render_feed_images(
        &self,
        frame: &mut Frame<'_>,
        history_area: Rect,
        visible_images: &[(u64, u16)],
    ) {
        for &(id, row) in visible_images {
            let Some(image) = self.feed_image(id) else {
                continue;
            };
            let image_y = history_area.y.saturating_add(row);
            let available_height = history_area.bottom().saturating_sub(image_y);
            image.render(
                frame,
                Rect::new(
                    history_area.x,
                    image_y,
                    history_area.width,
                    IMAGE_HEIGHT.min(available_height),
                ),
            );
        }
    }

    fn feed_image(&self, id: u64) -> Option<&FeedImage> {
        self.transcript
            .iter()
            .filter_map(|entry| match entry {
                super::Entry::Tool(tool) => tool.image.as_ref(),
                _ => None,
            })
            .chain(
                self.pending_tool_call
                    .as_ref()
                    .and_then(|tool| tool.image.as_ref()),
            )
            .find(|image| image.id() == id)
    }
}

pub(super) fn kitty_picker_from_environment() -> Option<Picker> {
    kitty_graphics_environment(
        std::env::var_os("KITTY_WINDOW_ID").is_some(),
        std::env::var_os("GHOSTTY_RESOURCES_DIR").is_some(),
        std::env::var("TERM_PROGRAM").ok().as_deref(),
        std::env::var("TERM").ok().as_deref(),
    )
    .then(kitty_picker)
}

fn kitty_picker() -> Picker {
    let mut picker = Picker::halfblocks();
    picker.set_protocol_type(ProtocolType::Kitty);
    picker
}

fn kitty_graphics_environment(
    has_kitty_window_id: bool,
    has_ghostty_resources: bool,
    term_program: Option<&str>,
    term: Option<&str>,
) -> bool {
    has_kitty_window_id
        || has_ghostty_resources
        || term_program.is_some_and(|program| {
            matches!(program.to_ascii_lowercase().as_str(), "kitty" | "ghostty")
        })
        || term.is_some_and(|term| {
            let term = term.to_ascii_lowercase();
            term.contains("kitty") || term.contains("ghostty")
        })
}

#[cfg(test)]
#[path = "feed_image_tests.rs"]
mod tests;
