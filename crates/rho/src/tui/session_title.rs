#[cfg(test)]
use crate::title::sanitize_title;

use super::App;

pub(crate) use crate::title::SESSION_TITLE_PROMPT;

#[cfg(test)]
pub(super) fn sanitize_session_title(title: &str) -> Option<String> {
    sanitize_title(title)
}

impl App {
    /// Title of the current session, as the daemon last reported it.
    pub(super) fn session_title(&self) -> Option<String> {
        self.daemon_title.clone()
    }
}
