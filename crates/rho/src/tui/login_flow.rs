//! Browser vs device-code picker for dual-grant OAuth.

use ratatui::DefaultTerminal;
use rho_providers::{
    auth::{
        browser::BrowserAvailability,
        login_dispatch::{InteractiveLoginMode, ProviderAuthentication},
    },
    model::catalog::LoginTarget,
};

use super::{App, ComposerMode, InteractiveRuntime, PickerItem, PickerKeyHints, UiPicker};

const BROWSER_VALUE: &str = "browser";
const DEVICE_VALUE: &str = "device";

/// Authentication context retained while the user chooses a grant flow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LoginFlowTarget {
    pub(super) target: LoginTarget,
    pub(super) provider_label: &'static str,
}

/// Dual-grant profiles are those [`ProviderAuthentication::preferred_mode`]
/// sends to device-code when headless.
pub(super) fn offers_browser_and_device_login(auth: &str) -> bool {
    ProviderAuthentication::preferred_mode(auth, BrowserAvailability::Headless)
        == InteractiveLoginMode::Device
}

fn login_flow_picker(target: LoginFlowTarget, preferred: InteractiveLoginMode) -> UiPicker {
    let items = [
        (
            BROWSER_VALUE,
            "Browser",
            "Open a local callback in this environment.",
        ),
        (DEVICE_VALUE, "Device code", "Enter a code on any device."),
    ]
    .into_iter()
    .map(|(value, label, detail)| PickerItem {
        label: label.into(),
        section: None,
        detail: Some(detail.into()),
        preview: None,
        badge: None,
        value: value.into(),
        selection_verb: None,
        allow_filter_completion: true,
    })
    .collect();
    let mut picker = UiPicker::login_flow(
        format!("Select {} login flow", target.provider_label),
        items,
        target,
    )
    .with_key_hints(PickerKeyHints {
        tab_complete: true,
        ..Default::default()
    });
    let preferred = match preferred {
        InteractiveLoginMode::Browser => BROWSER_VALUE,
        InteractiveLoginMode::Device => DEVICE_VALUE,
    };
    App::restore_picker_position(&mut picker, preferred, String::new());
    picker
}

fn parse_login_flow_mode(value: &str) -> Option<InteractiveLoginMode> {
    match value {
        BROWSER_VALUE => Some(InteractiveLoginMode::Browser),
        DEVICE_VALUE => Some(InteractiveLoginMode::Device),
        _ => None,
    }
}

impl App {
    pub(super) fn open_login_flow_picker(
        &mut self,
        target: LoginTarget,
        provider_label: &'static str,
    ) {
        let availability = BrowserAvailability::from_process();
        let preferred = ProviderAuthentication::preferred_mode(&target.auth, availability);
        let picker = login_flow_picker(
            LoginFlowTarget {
                target,
                provider_label,
            },
            preferred,
        );
        if matches!(self.input_ui.composer(), ComposerMode::Picker(_)) {
            self.open_child_picker(picker);
        } else {
            self.input_ui.set_composer(ComposerMode::Picker(picker));
        }
        self.set_status(format!("select {provider_label} login flow"));
    }

    pub(super) async fn submit_login_flow_selection(
        &mut self,
        value: &str,
        target: LoginFlowTarget,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let Some(mode) = parse_login_flow_mode(value) else {
            self.set_status(self.busy_status_label());
            return Ok(());
        };
        self.start_interactive_login_flow(
            target.target,
            target.provider_label,
            mode,
            terminal,
            agent,
        )
        .await
    }
}

#[cfg(test)]
#[path = "login_flow_tests.rs"]
mod tests;
