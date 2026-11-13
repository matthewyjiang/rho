use super::target;

// Covers: `rho login meta` names both auth modes instead of calling the provider unsupported
// Owner: CLI login
#[test]
fn login_meta_names_both_auth_modes() {
    pretty_assertions::assert_eq!(
        target("meta").err().map(|error| error.to_string()),
        Some(
            "provider 'meta' has multiple auth modes; specify one of: meta-api-key, meta-muse"
                .to_owned()
        )
    );
}
