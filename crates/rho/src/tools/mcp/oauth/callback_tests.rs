use pretty_assertions::assert_eq;

use super::*;

// Failure mode: the loopback listener accepts any request that reaches it, so
// a stray browser probe or a crafted link without the matching CSRF state ends
// the login or is turned into a redirect somewhere else.
// Owner layer: the OAuth callback listener's request filter.
#[test]
fn only_the_callback_path_with_matching_state_is_accepted() {
    let expected = "xyz";
    let cases = [
        (
            "GET /oauth/callback?code=abc&state=xyz HTTP/1.1\r\nhost: x\r\n\r\n",
            Some("/oauth/callback?code=abc&state=xyz"),
        ),
        (
            "GET /oauth/callback?state=xyz&code=abc HTTP/1.1\r\n\r\n",
            Some("/oauth/callback?state=xyz&code=abc"),
        ),
        (
            "GET /oauth/callback?error=access_denied&state=xyz HTTP/1.1\r\n\r\n",
            Some("/oauth/callback?error=access_denied&state=xyz"),
        ),
        // Form-urlencoded equivalent of the expected CSRF state (rmcp decodes).
        (
            "GET /oauth/callback?code=abc&state=xy%7A HTTP/1.1\r\n\r\n",
            Some("/oauth/callback?code=abc&state=xy%7A"),
        ),
        // Duplicate state: last wins, same as rmcp AuthorizationCallback.
        (
            "GET /oauth/callback?code=abc&state=incorrect&state=xyz HTTP/1.1\r\n\r\n",
            Some("/oauth/callback?code=abc&state=incorrect&state=xyz"),
        ),
        (
            "GET /oauth/callback?code=abc&state=xyz&state=incorrect HTTP/1.1\r\n\r\n",
            None,
        ),
        // Present but wrong CSRF state must not finish the login.
        (
            "GET /oauth/callback?code=abc&state=incorrect HTTP/1.1\r\n\r\n",
            None,
        ),
        // No CSRF state: nothing ties this to the round Rho started.
        ("GET /oauth/callback?code=abc HTTP/1.1\r\n\r\n", None),
        // Another path on the same port must not finish the login.
        ("GET /?code=abc&state=xyz HTTP/1.1\r\n\r\n", None),
        ("GET /favicon.ico HTTP/1.1\r\n\r\n", None),
        // Only the browser redirect is a GET.
        (
            "POST /oauth/callback?code=a&state=xyz HTTP/1.1\r\n\r\n",
            None,
        ),
        ("", None),
    ];

    let accepted = cases
        .iter()
        .map(|(request, _)| callback_target(request, expected).ok())
        .collect::<Vec<_>>();
    let expected_targets = cases
        .iter()
        .map(|(_, expected)| *expected)
        .collect::<Vec<_>>();
    assert_eq!(accepted, expected_targets);
}

// Failure mode: the loopback filter and rmcp disagree on which `state` value
// the redirect carries (first vs last, or raw vs form-decoded), so the listener
// accepts a callback that handle_callback_url later rejects - or rejects the
// one that would succeed.
// Owner layer: callback query parsing aligned with rmcp AuthorizationCallback.
#[test]
fn callback_state_matches_rmcp_query_pair_rules() {
    // Percent-encoded and `+` form-urlencoded equivalents of the same value.
    let expected = "token with spaces/+";
    assert_eq!(
        callback_target(
            "GET /oauth/callback?code=abc&state=token%20with%20spaces%2F%2B HTTP/1.1\r\n\r\n",
            expected,
        )
        .ok(),
        Some("/oauth/callback?code=abc&state=token%20with%20spaces%2F%2B")
    );
    assert_eq!(
        callback_target(
            "GET /oauth/callback?code=abc&state=token+with+spaces%2F%2B HTTP/1.1\r\n\r\n",
            expected,
        )
        .ok(),
        Some("/oauth/callback?code=abc&state=token+with+spaces%2F%2B")
    );

    // Last duplicate state is the one rmcp keeps.
    assert_eq!(
        callback_target(
            "GET /oauth/callback?state=first&code=abc&state=second HTTP/1.1\r\n\r\n",
            "second",
        )
        .ok(),
        Some("/oauth/callback?state=first&code=abc&state=second")
    );
    assert!(matches!(
        callback_target(
            "GET /oauth/callback?state=first&code=abc&state=second HTTP/1.1\r\n\r\n",
            "first",
        ),
        Err(CallbackReject::WrongState)
    ));
}

// Failure mode: the authorization URL does not expose the CSRF state Rho needs
// to validate the loopback callback before stopping the listener.
// Owner layer: parsing the state from rmcp's authorization URL.
#[test]
fn state_is_read_from_the_authorization_url() {
    assert_eq!(
        state_from_authorization_url(
            "https://auth.example/authorize?client_id=c&state=csrf-token&code_challenge=x"
        )
        .unwrap(),
        "csrf-token"
    );
    assert!(state_from_authorization_url("https://auth.example/authorize?client_id=c").is_err());
}

// Failure mode: the redirect URI registered with the authorization server does
// not match the socket Rho is listening on, so the browser lands nowhere.
// Owner layer: the OAuth callback listener's bind step.
#[tokio::test]
async fn the_redirect_uri_names_the_bound_loopback_socket() {
    let redirect = LoopbackRedirect::bind().await.unwrap();
    let address = redirect.listener.local_addr().unwrap();

    assert_eq!(
        redirect.redirect_uri(),
        format!("http://127.0.0.1:{}/oauth/callback", address.port())
    );
    assert!(address.ip().is_loopback());
    assert_ne!(address.port(), 0, "an ephemeral port must be assigned");
}

// Failure mode: a request that is not the callback, or that carries the wrong
// state, ends the wait, so the real redirect is never read and the login fails
// for no reason the user can see.
// Owner layer: the OAuth callback listener's accept loop.
#[tokio::test]
async fn the_listener_waits_past_noise_and_wrong_state() {
    use tokio::io::AsyncWriteExt;

    let redirect = LoopbackRedirect::bind().await.unwrap();
    let address = redirect.listener.local_addr().unwrap();
    let expected_state = "nonce";
    let requests = tokio::spawn(async move {
        for line in [
            "GET /favicon.ico HTTP/1.1\r\nhost: x\r\n\r\n",
            "GET /oauth/callback?code=stolen&state=incorrect HTTP/1.1\r\nhost: x\r\n\r\n",
            "GET /oauth/callback?code=granted&state=nonce HTTP/1.1\r\nhost: x\r\n\r\n",
        ] {
            let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
            stream.write_all(line.as_bytes()).await.unwrap();
            stream.flush().await.unwrap();
            // The listener answers before the next request is sent, so the
            // ordering is established by the response, not by a sleep.
            let mut response = Vec::new();
            tokio::io::AsyncReadExt::read_to_end(&mut stream, &mut response)
                .await
                .unwrap();
        }
    });

    let redirected_to = redirect.wait_for_redirect(expected_state).await.unwrap();
    requests.await.unwrap();

    assert_eq!(
        redirected_to,
        format!(
            "http://127.0.0.1:{}/oauth/callback?code=granted&state=nonce",
            address.port()
        )
    );
}
