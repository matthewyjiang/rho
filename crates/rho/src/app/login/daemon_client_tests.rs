use pretty_assertions::assert_eq;

use super::refuse_shell_override;

// Covers: `rho login` refuses before reaching the daemon when its shell
// sets the auth profile's credential variable, which would override what
// the daemon saves; a blank or unset variable does not count (#1662).
// Owner: rho login CLI.
#[test]
fn a_credential_variable_in_the_shell_refuses_the_login() {
    let refused = |value: Option<&str>| {
        refuse_shell_override("xai-oauth", |variable| {
            (variable == "XAI_ACCESS_TOKEN")
                .then_some(value)
                .flatten()
                .map(str::to_owned)
        })
        .map_err(|error| error.to_string())
    };
    assert_eq!(
        [refused(Some("token")), refused(Some("  ")), refused(None)],
        [
            Err("XAI_ACCESS_TOKEN is set in this shell and overrides any credential saved for 'xai-oauth', here and in a rho daemon started from here; unset it, then run rho login again".to_owned()),
            Ok(()),
            Ok(()),
        ]
    );
}
