//! Secret API-key collection for interactive provider login.

use rho_providers::{
    auth::login_dispatch::ProviderAuthentication, credentials::CredentialStore,
    model::catalog::LoginTarget, provider::AuthMode,
};

use super::{
    composer_chrome,
    line_editor::LineEditor,
    line_editor_view::EditorPresentation,
    login::ApiKeySubmission,
    view_composer::{editor_frame, ComposerFrame},
};

/// What Enter on a blank API-key field means.
#[derive(Clone, Copy, Debug)]
pub(super) enum BlankKey {
    /// The target needs a key.
    Reject,
    /// The host also runs keyless: keep a reachable key or run without one.
    LeaveUnset,
    /// Copy the stored key of a profile that shares keys with the target.
    Reuse(AuthMode),
}

#[derive(Clone, Debug)]
pub(super) struct SecretInput {
    pub(super) target: LoginTarget,
    pub(super) editor: LineEditor,
    pub(super) blank: BlankKey,
}

impl SecretInput {
    pub(super) fn new(target: LoginTarget) -> Self {
        Self {
            target,
            editor: LineEditor::new(""),
            blank: BlankKey::Reject,
        }
    }

    /// For hosts that also run keyless, where blank means "do not set a new key".
    pub(super) fn optional(target: LoginTarget) -> Self {
        Self {
            blank: BlankKey::LeaveUnset,
            ..Self::new(target)
        }
    }

    /// The API-key field for a provider login. Blank reuses a sibling
    /// profile's stored key when one exists, so one OpenCode console key logs
    /// in to Zen and Go without pasting it twice.
    pub(super) fn for_provider_login(
        target: LoginTarget,
        keyless_host: bool,
        store: &dyn CredentialStore,
    ) -> Self {
        if keyless_host {
            return Self::optional(target);
        }
        // Reuse is a shortcut. If the store cannot be read, asking for the
        // key still works, and saving it reports the store error.
        let source = ProviderAuthentication::reusable_api_key_source(store, &target.auth)
            .ok()
            .flatten();
        Self {
            blank: source.map_or(BlankKey::Reject, BlankKey::Reuse),
            ..Self::new(target)
        }
    }

    /// What the field asks for, shown above it and in the status bar.
    pub(super) fn prompt(&self) -> String {
        match self.blank {
            BlankKey::Reject => format!("enter {}", self.target.label),
            BlankKey::LeaveUnset => "enter API key (optional)".into(),
            BlankKey::Reuse(source) => format!(
                "enter {} or leave blank to use your {}",
                self.target.label, source.login_label
            ),
        }
    }

    /// What pressing Enter with the current value means.
    pub(super) fn submission(&self) -> ApiKeySubmission {
        let key = self.editor.value.trim().to_string();
        let target = self.target.clone();
        if !key.is_empty() {
            return ApiKeySubmission::Save { target, key };
        }
        match self.blank {
            BlankKey::Reject => ApiKeySubmission::Rejected,
            BlankKey::LeaveUnset => ApiKeySubmission::LeaveUnset { target },
            BlankKey::Reuse(source) => ApiKeySubmission::Reuse {
                target,
                source: source.id,
            },
        }
    }
}

pub(super) fn secret_input_frame(secret: &SecretInput, width: usize) -> ComposerFrame {
    let prompt = format!(
        "{}  {}",
        secret.prompt(),
        composer_chrome::join_footer_parts(["Enter save", "Esc cancel"])
    );
    editor_frame(
        &prompt,
        secret.editor.viewport(EditorPresentation::Masked, width),
        width,
    )
}

#[cfg(test)]
#[path = "login_secret_input_tests.rs"]
mod tests;
