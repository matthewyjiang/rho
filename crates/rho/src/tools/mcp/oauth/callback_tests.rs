use pretty_assertions::assert_eq;

use super::*;

// Failure mode: the loopback listener accepts any request that reaches it, so
// a stray browser probe or a crafted link without CSRF state ends the login or
// is turned into a redirect somewhere else.
// Owner layer: the OAuth callback listener's request filter.
#[test]
fn only_the_callback_path_with_state_is_accepted() {
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
        // No CSRF state: nothing ties this to the round Rho started.
        ("GET /oauth/callback?code=abc HTTP/1.1\r\n\r\n", None),
        // Another path on the same port must not finish the login.
        ("GET /?code=abc&state=xyz HTTP/1.1\r\n\r\n", None),
        ("GET /favicon.ico HTTP/1.1\r\n\r\n", None),
        // Only the browser redirect is a GET.
        ("POST /oauth/callback?code=a&state=b HTTP/1.1\r\n\r\n", None),
        ("", None),
    ];

    let accepted = cases
        .iter()
        .map(|(request, _)| callback_target(request).ok())
        .collect::<Vec<_>>();
    let expected = cases
        .iter()
        .map(|(_, expected)| *expected)
        .collect::<Vec<_>>();
    assert_eq!(accepted, expected);
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

// Failure mode: a request that is not the callback ends the wait, so the real
// redirect is never read and the login fails for no reason the user can see.
// Owner layer: the OAuth callback listener's accept loop.
#[tokio::test]
async fn the_listener_waits_past_a_request_that_is_not_the_callback() {
    use tokio::io::AsyncWriteExt;

    let redirect = LoopbackRedirect::bind().await.unwrap();
    let address = redirect.listener.local_addr().unwrap();
    let requests = tokio::spawn(async move {
        for line in [
            "GET /favicon.ico HTTP/1.1\r\nhost: x\r\n\r\n",
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

    let redirected_to = redirect.wait_for_redirect().await.unwrap();
    requests.await.unwrap();

    assert_eq!(
        redirected_to,
        format!(
            "http://127.0.0.1:{}/oauth/callback?code=granted&state=nonce",
            address.port()
        )
    );
}
