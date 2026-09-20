use rho_providers::auth::login_dispatch::InteractiveLoginMode;

use super::{login_flow_choice, BROWSER_VALUE, DEVICE_VALUE};

// Covers: dual-grant choice lists both modes and preselects preferred_mode
// Owner: login flow choice
#[test]
fn login_flow_choice_lists_both_modes_and_preselects_preferred() {
    for preferred in [InteractiveLoginMode::Browser, InteractiveLoginMode::Device] {
        let choice = login_flow_choice("Codex", preferred);
        let values = choice
            .options
            .iter()
            .map(|option| option.value.as_str())
            .collect::<Vec<_>>();
        pretty_assertions::assert_eq!(values, vec![BROWSER_VALUE, DEVICE_VALUE], "{preferred:?}");
        let expected = match preferred {
            InteractiveLoginMode::Browser => BROWSER_VALUE,
            InteractiveLoginMode::Device => DEVICE_VALUE,
        };
        pretty_assertions::assert_eq!(choice.selected_value(), expected, "{preferred:?}");
    }
}
