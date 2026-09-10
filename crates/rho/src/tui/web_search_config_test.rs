//! Explicit test-query consent and background task lifecycle.
use super::*;

impl App {
    pub(in crate::tui) fn prompt_web_search_test(&mut self) -> anyhow::Result<()> {
        if self.pending_web_search_test.is_some() {
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
        if self.pending_web_search_test.is_some() {
            self.set_status("a web search test is already in progress");
            return Ok(());
        }
        self.pending_web_search_test = Some(tokio::spawn(async move {
            crate::tools::web::test_search_backend(&config).await
        }));
        self.set_status("testing web search connection");
        Ok(())
    }

    pub(in crate::tui) async fn poll_web_search_test(&mut self) -> anyhow::Result<bool> {
        if !self
            .pending_web_search_test
            .as_ref()
            .is_some_and(|handle| handle.is_finished())
        {
            return Ok(false);
        }
        let Some(handle) = self.pending_web_search_test.take() else {
            return Ok(false);
        };
        match handle.await {
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
        Ok(true)
    }

    pub(in crate::tui) async fn cancel_web_search_test(&mut self) {
        if let Some(handle) = self.pending_web_search_test.take() {
            handle.abort();
            let _ = handle.await;
        }
    }
}
