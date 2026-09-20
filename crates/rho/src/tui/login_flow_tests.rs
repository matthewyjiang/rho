use rho_providers::auth::login_dispatch::InteractiveLoginMode;

use super::{login_flow_picker, parse_login_flow_mode, LoginFlowTarget};

// Covers: the environment's preferred grant remains the default submitted mode.
// Owner: login flow construction policy
#[test]
fn login_flow_picker_preselects_preferred() {
    for preferred in [InteractiveLoginMode::Browser, InteractiveLoginMode::Device] {
        let picker = login_flow_picker(
            LoginFlowTarget {
                target: rho_providers::model::catalog::login_target_for_provider("openai-codex")
                    .unwrap(),
                provider_label: "Codex",
            },
            preferred,
        );
        pretty_assertions::assert_eq!(
            picker
                .selected_item()
                .and_then(|item| parse_login_flow_mode(&item.value)),
            Some(preferred),
            "{preferred:?}"
        );
    }
}
