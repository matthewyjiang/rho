//! Stderr presentation of an interactive [`LoginPrompt`].

use rho_providers::auth::login_prompt::LoginPrompt;

/// Print the authorize URL, optional device code, browser note, and instruction.
pub(crate) fn eprint_login_prompt(provider_label: &str, prompt: &LoginPrompt) {
    eprintln!("{provider_label} login");
    eprintln!("{}", prompt.url);
    if let Some(code) = &prompt.user_code {
        eprintln!("code: {code}");
    }
    if let Some(complete) = &prompt.url_with_code {
        if complete != prompt.url.as_str() {
            eprintln!("{complete}");
        }
    }
    eprintln!("{}", prompt.browser.note());
    eprintln!("{}", prompt.instruction);
}
