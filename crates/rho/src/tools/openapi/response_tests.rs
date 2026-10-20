use pretty_assertions::assert_eq;
use reqwest::header::{HeaderMap, HeaderValue, LINK};
use url::Url;

use super::*;

// Covers: the `rel="next"` target is found among several links, in any
// parameter order or rel list, and resolved against the request URL.
// Owner: OpenAPI pagination (Link header parsing)
#[test]
fn next_link_finds_the_next_target() {
    let base = Url::parse("https://api.example.com/v3/items?page=1").unwrap();
    let cases: [(&[&str], Option<&str>); 8] = [
        (
            &[
                r#"<https://api.example.com/v3/items?page=2>; rel="next", <https://api.example.com/v3/items?page=9>; rel="last""#,
            ],
            Some("https://api.example.com/v3/items?page=2"),
        ),
        (
            &[
                r#"<https://api.example.com/v3/items?page=1>; rel="prev", <https://api.example.com/v3/items?page=3>; rel="next""#,
            ],
            Some("https://api.example.com/v3/items?page=3"),
        ),
        (
            &[r#"</v3/items?page=2>; title="more"; rel="next""#],
            Some("https://api.example.com/v3/items?page=2"),
        ),
        (
            &[r#"<?page=1>; title="back; rel=next", <?page=2>; rel="next""#],
            Some("https://api.example.com/v3/items?page=2"),
        ),
        (
            &[r#"<?page=2>; title="a\"b"; rel="next""#],
            Some("https://api.example.com/v3/items?page=2"),
        ),
        (
            &[r#"<?page=2>; rel="nofollow next""#],
            Some("https://api.example.com/v3/items?page=2"),
        ),
        (
            &[r#"<?page=0>; rel="first""#, r#"<?page=2>; REL=next"#],
            Some("https://api.example.com/v3/items?page=2"),
        ),
        (
            &[r#"<https://api.example.com/v3/items?page=9>; rel="last""#],
            None,
        ),
    ];
    for (values, expected) in cases {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append(LINK, HeaderValue::from_str(value).unwrap());
        }
        assert_eq!(
            next_link(&headers, &base).map(String::from),
            expected.map(str::to_owned),
            "{values:?}"
        );
    }
}
