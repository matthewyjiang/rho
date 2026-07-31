use ratatui::text::Line;

use super::{command_block::CommandBlock, App, Entry};
use crate::{
    changelog::{
        bundled_current_display, fetch_latest_display, parse_request, ChangelogDisplay,
        ChangelogRequest, ChangelogSource,
    },
    commands::CommandInvocation,
};

impl App {
    pub(super) async fn execute_changelog_command(
        &mut self,
        invocation: &CommandInvocation,
    ) -> anyhow::Result<()> {
        let request = match parse_request(&invocation.args) {
            Ok(request) => request,
            Err(error) => {
                self.insert_entry(&Entry::Error(error.to_string()));
                self.status = "changelog usage".into();
                return Ok(());
            }
        };

        match request {
            ChangelogRequest::Current => match bundled_current_display(env!("CARGO_PKG_VERSION")) {
                Ok(display) => self.show_changelog(display),
                Err(message) => {
                    self.insert_entry(&Entry::Error(message));
                    self.status = "changelog unavailable".into();
                }
            },
            ChangelogRequest::Latest => {
                self.status = "fetching latest changelog".into();
                match fetch_latest_display().await {
                    Ok(display) => self.show_changelog(display),
                    Err(error) => {
                        self.insert_entry(&Entry::Error(format!(
                            "unable to fetch latest changelog: {error}"
                        )));
                        self.status = "changelog fetch failed".into();
                    }
                }
            }
        }
        Ok(())
    }

    fn show_changelog(&mut self, display: ChangelogDisplay) {
        let version = display.section.version.clone();
        self.insert_entry(&Entry::Changelog(Box::new(display)));
        self.status = format!("changelog v{version}");
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
            block.push_note("notes for this installed version");
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
