use super::{ChatMedia, Entry};

pub(super) fn render_user_entry(prompt: &str, media: &[ChatMedia]) -> String {
    let mut parts = Vec::new();
    if !prompt.is_empty() {
        parts.push(prompt.to_string());
    }
    parts.extend(
        media
            .iter()
            .enumerate()
            .map(|(index, media)| media.composer_label(index + 1)),
    );
    parts.join("\n")
}

impl super::App {
    pub(super) fn set_history_entries(&mut self, entries: Vec<Entry>) {
        self.history.set_entries(entries);
        self.history.images_mut().clear();
    }
}
