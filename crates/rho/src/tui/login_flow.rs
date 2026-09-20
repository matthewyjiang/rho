//! Nested browser vs device-code choice for dual-grant OAuth.

use ratatui::DefaultTerminal;
use rho_providers::{
    auth::{
        browser::BrowserAvailability,
        login_dispatch::{AuthenticationMethod, InteractiveLoginMode, ProviderAuthentication},
    },
    model::catalog::{self, LoginTarget},
};

use super::{
    picker::{PickerItem, PickerKeyHints},
    App, ComposerMode, Entry, InteractiveRuntime, UiPicker,
};

pub(super) fn login_flow_picker(
    target: &LoginTarget,
    provider_label: &str,
    preferred: InteractiveLoginMode,
) -> UiPicker {
    let items = vec![
        flow_item(
            &target.auth,
            InteractiveLoginMode::Browser,
            "Browser",
            "Open a local callback in this environment.",
        ),
        flow_item(
            &target.auth,
            InteractiveLoginMode::Device,
            "Device code",
            "Enter a code on any device.",
        ),
    ];
    let selected = match preferred {
        InteractiveLoginMode::Browser => 0,
        InteractiveLoginMode::Device => 1,
    };
    let mut picker = UiPicker::login_flow(format!("Select {provider_label} login flow"), items)
        .with_key_hints(PickerKeyHints {
            tab_complete: true,
            row_delete: false,
            ..Default::default()
        });
    picker.selected = selected;
    picker
}

pub(super) fn parse_login_flow_value(value: &str) -> Option<(String, InteractiveLoginMode)> {
    let (auth, mode) = value.rsplit_once('/')?;
    if auth.is_empty() {
        return None;
    }
    let mode = match mode {
        "browser" => InteractiveLoginMode::Browser,
        "device" => InteractiveLoginMode::Device,
        _ => return None,
    };
    Some((auth.to_string(), mode))
}

fn flow_item(auth: &str, mode: InteractiveLoginMode, label: &str, detail: &str) -> PickerItem {
    PickerItem {
        section: None,
        label: label.into(),
        detail: Some(detail.into()),
        preview: None,
        badge: None,
        value: format!("{auth}/{}", mode.as_str()),
        selection_verb: None,
        allow_filter_completion: true,
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
        let picker = login_flow_picker(&target, provider_label, preferred);
        if matches!(self.input_ui.composer(), ComposerMode::Picker(_)) {
            self.open_child_picker(picker);
            return;
        }
        self.input_ui.set_composer(ComposerMode::Picker(picker));
        self.set_status(format!("select {provider_label} login flow"));
    }

    pub(super) async fn commit_login_flow(
        &mut self,
        value: &str,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let Some((auth, mode)) = parse_login_flow_value(value) else {
            self.insert_entry(&Entry::Error("unsupported login flow".into()));
            self.set_status("login failed");
            return Ok(());
        };
        let Some(target) = catalog::login_target_for_auth(&auth) else {
            self.insert_entry(&Entry::Error(format!(
                "unsupported login provider '{auth}'"
            )));
            self.set_status("login failed");
            return Ok(());
        };
        match ProviderAuthentication::method(&target.auth) {
            Ok(AuthenticationMethod::Interactive { provider_label }) => {
                self.start_interactive_login_flow(target, provider_label, mode, terminal, agent)
                    .await
            }
            Ok(_) => {
                self.insert_entry(&Entry::Error(format!(
                    "provider '{auth}' does not use interactive login"
                )));
                self.set_status("login failed");
                Ok(())
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(err.to_string()));
                self.set_status("login failed");
                Ok(())
            }
        }
    }
}

#[cfg(test)]
#[path = "login_flow_tests.rs"]
mod tests;
