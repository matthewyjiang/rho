use super::run;

// Covers: `rho login meta` names both auth modes instead of calling the provider unsupported
// Owner: CLI login
#[tokio::test]
async fn login_meta_names_both_auth_modes() {
    let error = run("meta", false).await.unwrap_err();
    pretty_assertions::assert_eq!(
        error.to_string(),
        "provider 'meta' has multiple auth modes; specify one of: meta-api-key, meta-muse"
    );
}
