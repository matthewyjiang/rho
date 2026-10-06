use pretty_assertions::assert_eq;

use super::*;

// Covers: the replay endpoint comes from the printed link's encoded
// `redirect_uri`; lines without the link, or a non-loopback redirect, give
// nothing to replay to.
// Owner: Antigravity login link parsing.
#[test]
fn printed_link_yields_the_loopback_redirect() {
    let link = |redirect: &str| {
        format!(
            "{LINK_PREFIX}https://accounts.google.com/o/oauth2/v2/auth?client_id=c&redirect_uri={redirect}&state=s"
        )
    };
    for (line, expected) in [
        (
            link("http%3A%2F%2F127.0.0.1%3A40123%2F"),
            Some(Some("http://127.0.0.1:40123/")),
        ),
        (link("https%3A%2F%2Fexample.com%2Fcb"), Some(None)),
        ("I0000 server started".into(), None),
    ] {
        assert_eq!(
            printed_link(&line).map(|link| loopback_redirect(&link).map(String::from)),
            expected.map(|redirect| redirect.map(String::from)),
            "{line}"
        );
    }
}

// Covers: only the server's own loopback redirect (same port and path, no
// userinfo, carrying an OAuth answer) is replayed, rebuilt on the advertised
// endpoint; anything else is refused, so pasted text cannot aim a request
// elsewhere.
// Owner: Antigravity login paste validation.
#[test]
fn pasted_redirect_must_be_the_loopback_answer() {
    let redirect = Url::parse("http://127.0.0.1:40123/").unwrap();
    let refused = Err(PasteError::NotTheRedirect {
        expected: redirect.clone(),
    });
    for (pasted, expected) in [
        (
            " http://127.0.0.1:40123/?state=s&code=abc ",
            Ok("http://127.0.0.1:40123/?state=s&code=abc"),
        ),
        (
            "http://localhost:40123/?error=access_denied",
            Ok("http://127.0.0.1:40123/?error=access_denied"),
        ),
        ("http://127.0.0.1:40124/?code=abc", refused.clone()),
        ("https://127.0.0.1:40123/?code=abc", refused.clone()),
        ("http://example.com:40123/?code=abc", refused.clone()),
        ("http://127.0.0.1:40123/ping", refused.clone()),
        ("http://127.0.0.1:40123/other?code=abc", refused.clone()),
        ("http://user@127.0.0.1:40123/?code=abc", refused),
        ("code=abc", Err(PasteError::NotAUrl)),
    ] {
        assert_eq!(
            local_redirect(pasted, &redirect)
                .as_ref()
                .map(Url::as_str)
                .map_err(Clone::clone),
            expected,
            "{pasted}"
        );
    }
}
