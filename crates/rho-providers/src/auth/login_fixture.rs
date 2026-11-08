//! An offline interactive login for host tests that relay logins, such as
//! the daemon's `rho/auth/login` (#1662). Debug builds only, like the
//! automation fixture provider.
//!
//! With `RHO_AUTOMATION_TEST_LOGIN` set, a login to `xai-oauth` serves a
//! loopback page instead of calling SpaceXAI, and shows it as a device-code
//! prompt. A GET of the prompt's URL, standing in for the user approving
//! the code, finishes the login with placeholder SpaceXAI tokens; until then
//! it waits as a real login waits for the user, and dropping it closes the
//! port, as a real login's callback port closes.

use std::time::Duration;

use super::{
    device_prompt, flow_error, AuthenticationError, CompletedAuthentication,
    InteractiveLoginCompletion, LoginCredentials, StartedLogin,
};
use crate::{auth::loopback, credentials::XaiTokens};

const LOGIN_ENV: &str = "RHO_AUTOMATION_TEST_LOGIN";
/// The one auth profile the fixture signs in to.
const FIXTURE_AUTH: &str = "xai-oauth";
/// The device code the fixture's prompt shows.
const FIXTURE_CODE: &str = "FIXTURE-CODE";
/// The access token the fixture saves, which tests read back.
const FIXTURE_TOKEN: &str = "automation-test-login-token";
/// How long the page waits for the approving request's bytes once it
/// connected: the provider flows' callback read timeout
/// (`xai_oauth::CALLBACK_READ_TIMEOUT`). The fixture approves on any
/// request, read or not.
const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(2);

/// The fixture's login when `auth` is the fixture's and the fixture is on;
/// `None` otherwise, so the provider's own login runs.
pub(super) async fn start(auth: &str) -> Option<Result<StartedLogin, AuthenticationError>> {
    if auth != FIXTURE_AUTH || std::env::var_os(LOGIN_ENV).is_none() {
        return None;
    }
    Some(serve().await)
}

async fn serve() -> Result<StartedLogin, AuthenticationError> {
    let listener = loopback::bind_ipv4(0).await.map_err(flow_error)?;
    let url = loopback::callback_url(&listener, "/approve").map_err(flow_error)?;
    Ok(StartedLogin {
        provider_label: "SpaceXAI",
        prompt: device_prompt(url, FIXTURE_CODE, None),
        completion: InteractiveLoginCompletion::Confirm(Box::pin(async move {
            let (mut stream, _request) = loopback::accept_request(&listener, REQUEST_READ_TIMEOUT)
                .await
                .map_err(flow_error)?;
            // The page is a courtesy; the approval already happened.
            let _ = loopback::write_http_response(&mut stream, "200 OK", "approved").await;
            Ok(CompletedAuthentication {
                credentials: LoginCredentials::Xai(XaiTokens {
                    access_token: FIXTURE_TOKEN.into(),
                    refresh_token: None,
                    expires_at_unix: None,
                    id_token: None,
                }),
            })
        })),
    })
}
