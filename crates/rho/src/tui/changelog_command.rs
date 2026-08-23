use ratatui::{text::Line, DefaultTerminal};

use super::{command_block::CommandBlock, App, Entry};
use crate::{
    changelog::{
        bundled_current_display, fetch_latest_display, parse_request, ChangelogDisplay,
        ChangelogRequest, ChangelogSource,
    },
    commands::CommandInvocation,
};

pub(super) type ChangelogFetchResult = anyhow::Result<ChangelogDisplay>;

impl App {
    pub(super) fn execute_changelog_command(
        &mut self,
        invocation: &CommandInvocation,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let request = match parse_request(&invocation.args) {
            Ok(request) => request,
            Err(error) => {
                self.insert_entry(&Entry::Error(error.to_string()));
                self.set_status("changelog usage");
                return Ok(());
            }
        };

        match request {
            ChangelogRequest::Current => match bundled_current_display(env!("CARGO_PKG_VERSION")) {
                Ok(display) => self.show_changelog(display),
                Err(message) => {
                    self.insert_entry(&Entry::Error(message));
                    self.set_status("changelog unavailable");
                }
            },
            ChangelogRequest::Latest => {
                if self.start_latest_changelog_command() {
                    terminal.draw(|frame| self.draw(frame))?;
                }
            }
        }
        Ok(())
    }

    /// Starts a background fetch for `/changelog latest`. Returns whether a new
    /// task was spawned so callers can redraw the "fetching" status at once.
    pub(super) fn start_latest_changelog_command(&mut self) -> bool {
        if self.pending_changelog.is_some() {
            self.set_status("a latest changelog fetch is already in progress");
            return false;
        }

        self.pending_changelog = Some(tokio::spawn(async move { fetch_latest_display().await }));
        self.set_status("fetching latest changelog");
        true
    }

    pub(super) async fn cancel_changelog_command(&mut self) {
        if let Some(handle) = self.pending_changelog.take() {
            handle.abort();
            let _ = handle.await;
        }
    }

    pub(super) async fn poll_changelog_command(&mut self) -> anyhow::Result<bool> {
        if !self
            .pending_changelog
            .as_ref()
            .is_some_and(|handle| handle.is_finished())
        {
            return Ok(false);
        }
        self.finish_changelog_command().await?;
        Ok(true)
    }

    async fn finish_changelog_command(&mut self) -> anyhow::Result<()> {
        let Some(handle) = self.pending_changelog.take() else {
            return Ok(());
        };
        match handle.await {
            Ok(Ok(display)) => self.show_changelog(display),
            Ok(Err(error)) => {
                self.insert_entry(&Entry::Error(format!(
                    "unable to fetch latest changelog: {error}"
                )));
                self.set_status("changelog fetch failed");
            }
            Err(_) => {
                self.insert_entry(&Entry::Error(
                    "background task failed: changelog fetch".into(),
                ));
                self.set_status("changelog fetch failed");
            }
        }
        Ok(())
    }

    fn show_changelog(&mut self, display: ChangelogDisplay) {
        let version = display.section.version.clone();
        self.insert_entry(&Entry::Changelog(Box::new(display)));
        self.set_status(format!("changelog v{version}"));
    }
}

pub(super) fn changelog_lines(display: &ChangelogDisplay, width: usize) -> Vec<Line<'static>> {
    let mut block = CommandBlock::new(width);
    let detail = match &display.section.date {
        Some(date) => format!("v{} - {date}", display.section.version),
        None => format!("v{}", display.section.version),
    };
    block.push_header("changelog", &detail);

    match display.source {
        ChangelogSource::Bundled => {
            block.push_note(
                display
                    .note
                    .as_deref()
                    .unwrap_or("notes for this installed version"),
            );
        }
        ChangelogSource::LatestRelease => {
            block.push_note(
                display
                    .note
                    .as_deref()
                    .unwrap_or("notes for the latest published release"),
            );
        }
    }

    for group in &display.section.groups {
        block.push_section(&group.title);
        for item in &group.items {
            block.push_note(&format!("• {item}"));
        }
    }

    block.finish()
}
