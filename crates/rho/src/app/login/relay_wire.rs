//! A relayed login's flow and prompt as `rho/auth/login` carries them
//! (#1662): the daemon's end (`daemon_host/logins.rs`) and `rho login`'s
//! (`daemon_client.rs`) map them here, so the two stay inverses.
//!
//! The prompt's mapping is lossy twice. The provider's prompt records
//! whether the process that started the login opened a browser
//! ([`LoginPrompt::browser`]); the daemon opens none, so that note is never
//! sent, and the client opens the URL itself and records its own attempt
//! with [`LoginPrompt::with_browser`]. And a prompt without a device code
//! reads back as a browser flow, which drops a `url_with_code`; only
//! [`LoginPrompt::device_code`] sets one, alongside its code.

use rho_protocol::LoginFlow;
use rho_providers::auth::{login_dispatch::InteractiveLoginMode, login_prompt::LoginPrompt};

/// `mode` as the client asks for it.
pub(in crate::app) fn flow_to_wire(mode: InteractiveLoginMode) -> LoginFlow {
    match mode {
        InteractiveLoginMode::Browser => LoginFlow::Browser,
        InteractiveLoginMode::Device => LoginFlow::DeviceCode,
    }
}

/// The mode the daemon runs for `flow`.
pub(in crate::app) fn flow_from_wire(flow: LoginFlow) -> InteractiveLoginMode {
    match flow {
        LoginFlow::Browser => InteractiveLoginMode::Browser,
        LoginFlow::DeviceCode => InteractiveLoginMode::Device,
    }
}

/// `prompt` as the daemon sends it, without its browser note.
pub(in crate::app) fn prompt_to_wire(prompt: LoginPrompt) -> rho_protocol::LoginPrompt {
    let LoginPrompt {
        url,
        user_code,
        url_with_code,
        browser: _,
        instruction,
    } = prompt;
    rho_protocol::LoginPrompt {
        url,
        user_code,
        url_with_code,
        instruction,
    }
}

/// `prompt` as the client received it, before it opens the browser.
pub(in crate::app) fn prompt_from_wire(prompt: rho_protocol::LoginPrompt) -> LoginPrompt {
    let rho_protocol::LoginPrompt {
        url,
        user_code,
        url_with_code,
        instruction,
    } = prompt;
    match user_code {
        Some(user_code) => LoginPrompt::device_code(url, user_code, url_with_code, instruction),
        None => LoginPrompt::browser_flow(url, instruction),
    }
}
