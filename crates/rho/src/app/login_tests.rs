use super::*;
use rho_providers::auth::login_dispatch::AuthenticationMethod;

#[test]
fn ollama_cloud_device_login_target_resolves_interactive_method() {
    let target = catalog::login_target_for_provider("ollama-cloud-device")
        .expect("ollama-cloud-device login target");
    assert_eq!(target.auth, "ollama-cloud-device");
    assert_eq!(
        ProviderAuthentication::method(&target.auth).unwrap(),
        AuthenticationMethod::Interactive {
            provider_label: "Ollama Cloud",
        }
    );
    assert!(ProviderAuthentication::supports_device_login(&target.auth));
}
