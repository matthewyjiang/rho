//! Browser vs device-code choice for dual-grant OAuth.

use ratatui::DefaultTerminal;
use rho_providers::{
    auth::{
        browser::BrowserAvailability,
        login_dispatch::{InteractiveLoginMode, ProviderAuthentication},
    },
    model::catalog::LoginTarget,
};

use super::{
    App, ComposerMode, InlineChoice, InlineChoiceModal, InlineChoiceOption, InlineChoicePending,
    InteractiveRuntime, UiPicker,
};

const BROWSER_VALUE: &str = "browser";
const DEVICE_VALUE: &str = "device";

/// Dual-grant profiles are those [`ProviderAuthentication::preferred_mode`]
/// sends to device-code when headless.
pub(super) fn offers_browser_and_device_login(auth: &str) -> bool {
    ProviderAuthentication::preferred_mode(auth, BrowserAvailability::Headless)
        == InteractiveLoginMode::Device
}

pub(super) fn login_flow_choice(
    provider_label: &str,
    preferred: InteractiveLoginMode,
) -> InlineChoice {
    let preferred = match preferred {
        InteractiveLoginMode::Browser => BROWSER_VALUE,
        InteractiveLoginMode::Device => DEVICE_VALUE,
    };
    InlineChoice::new(
        format!("Select {provider_label} login flow"),
        "Open a local callback, or enter a code on any device.",
        vec![
            InlineChoiceOption::available(
                BROWSER_VALUE,
                '1',
                "Browser",
                "Open a local callback in this environment.",
            )
            .with_alternate_shortcut('b'),
            InlineChoiceOption::available(
                DEVICE_VALUE,
                '2',
                "Device code",
                "Enter a code on any device.",
            )
            .with_alternate_shortcut('d'),
        ],
    )
    .expect("login flow choice has available options")
    .with_selected_value(preferred)
}

fn parse_login_flow_mode(value: &str) -> Option<InteractiveLoginMode> {
    match value {
        BROWSER_VALUE => Some(InteractiveLoginMode::Browser),
        DEVICE_VALUE => Some(InteractiveLoginMode::Device),
        _ => None,
    }
}

impl App {
    pub(super) fn open_login_flow_choice(
        &mut self,
        target: LoginTarget,
        provider_label: &'static str,
    ) {
        let availability = BrowserAvailability::from_process();
        let preferred = ProviderAuthentication::preferred_mode(&target.auth, availability);
        let choice = login_flow_choice(provider_label, preferred);
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
                pending: InlineChoicePending::LoginFlow {
                    target,
                    provider_label,
                },
                parent_picker,
            }));
        self.set_status(format!("select {provider_label} login flow"));
    }

    pub(super) async fn submit_login_flow_choice(
        &mut self,
        value: &str,
        target: LoginTarget,
        provider_label: &'static str,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let Some(mode) = parse_login_flow_mode(value) else {
            self.set_status(self.busy_status_label());
            return Ok(());
        };
        self.start_interactive_login_flow(target, provider_label, mode, terminal, agent)
            .await
    }

    pub(super) fn restore_login_flow_parent(&mut self, parent: Option<Box<UiPicker>>) {
        let Some(parent) = parent else {
            self.set_status(self.busy_status_label());
            return;
        };
        self.set_status_quiet(parent.title.clone());
        self.input_ui.set_composer(ComposerMode::Picker(*parent));
    }
}

#[cfg(test)]
#[path = "login_flow_tests.rs"]
mod tests;
