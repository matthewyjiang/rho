//! Explicit test-query consent and background task lifecycle.
use super::*;
use crate::tui::background_tasks::{TaskId, UiOutput};

impl App {
    pub(in crate::tui) fn prompt_web_search_test(&mut self) -> anyhow::Result<()> {
        if self.web_search_test_running() {
            self.set_status("a web search test is already in progress");
            return Ok(());
        }
        let choice = InlineChoice::new(
            "Test web search",
            "This sends a test query off this machine and may incur a charge.",
            vec![
                InlineChoiceOption::available(
                    TEST_CONFIRM_VALUE,
                    'y',
                    "Send test query",
                    "The query leaves this machine and may be billed",
                )
                .require_full_visibility(),
                InlineChoiceOption::available("cancel", 'n', "Don't send", "Stay on this machine")
                    .with_alternate_shortcut('c'),
            ],
        )?;
        let parent_picker = match self.input_ui.take_composer() {
            ComposerMode::Picker(picker) => Some(Box::new(picker)),
            composer => {
                self.input_ui.set_composer(composer);
                None
            }
        };
        self.input_ui
            .set_composer(ComposerMode::InlineChoice(InlineChoiceModal {
                choice,
                pending: InlineChoicePending::TestWebSearch,
                parent_picker,
            }));
        self.set_status("confirm web search test");
        Ok(())
    }

    pub(in crate::tui) fn submit_web_search_test_choice(
        &mut self,
        value: &str,
        parent_picker: Option<Box<super::UiPicker>>,
    ) -> anyhow::Result<()> {
        if let Some(picker) = parent_picker {
            self.set_status_quiet(picker.title.clone());
            self.input_ui.set_composer(ComposerMode::Picker(*picker));
        }
        if value != TEST_CONFIRM_VALUE {
            return self.refresh_web_search_picker(WEB_SEARCH_TEST_VALUE, None);
        }
        let config = self.info.services.config_repository.load()?;
        self.refresh_web_search_picker(WEB_SEARCH_TEST_VALUE, None)?;
        if self.web_search_test_running() {
            self.set_status("a web search test is already in progress");
            return Ok(());
        }
        self.tasks.spawn(
            TaskId::WebSearchTest,
            async move { crate::tools::web::test_search_backend(&config).await },
            |result| UiOutput::WebSearchTest(result).into(),
        );
        self.set_status("testing web search connection");
        Ok(())
    }

    fn web_search_test_running(&self) -> bool {
        self.tasks.contains(|id| *id == TaskId::WebSearchTest)
    }

    pub(in crate::tui) fn apply_web_search_test(
        &mut self,
        result: Result<Result<usize, rho_tools::tool::ToolError>, tokio::task::JoinError>,
    ) -> bool {
        match result {
            Ok(Ok(count)) => {
                let notice = format!(
                    "web search test: {count} result{}",
                    crate::tui::plural_suffix(count)
                );
                self.insert_entry(&Entry::Notice(notice.clone()));
                self.set_status(notice);
            }
            Ok(Err(error)) => {
                self.insert_entry(&Entry::Error(format!("could not test web search: {error}")));
                self.set_status("web search test failed");
            }
            Err(_) => {
                self.insert_entry(&Entry::Error(
                    "background task failed: web search test".into(),
                ));
                self.set_status("web search test failed");
            }
        }
        true
    }
}
