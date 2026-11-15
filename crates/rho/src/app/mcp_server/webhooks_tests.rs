use pretty_assertions::assert_eq;

use super::*;

// Covers: a webhook URL reaches the network only over TLS, plain http only
// to a loopback host, and never with userinfo, which `list` would show.
// Owner: MCP server webhook targets.
#[test]
fn webhook_urls_need_tls_off_loopback_and_no_userinfo() {
    let cases = [
        ("https://api.x.ai/routines/abc", true),
        ("http://127.0.0.1:9000/hook", true),
        ("http://localhost/hook", true),
        ("http://[::1]:9000/hook", true),
        ("http://example.com/hook", false),
        ("http://10.0.0.1/hook", false),
        ("https://user:pass@example.com/hook", false),
        ("https://user@example.com/hook", false),
        ("ftp://example.com/hook", false),
        ("not a url", false),
    ];
    let accepted: Vec<(&str, bool)> = cases
        .iter()
        .map(|(url, _)| (*url, validate_url(url).is_ok()))
        .collect();
    assert_eq!(accepted, cases.to_vec());
}

// Covers: a token may use only the targets bound to it, and a refusal names
// the ones it may use, the same whether the target exists or not, so one
// token learns nothing of another's targets.
// Owner: MCP server webhook targets.
#[test]
fn a_token_uses_only_the_webhooks_bound_to_it() {
    let mut webhooks = WebhookSet::default();
    let tokens = |names: &[&str]| {
        names
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>()
    };
    webhooks
        .upsert(
            "grok",
            "https://example.com/a",
            "s1",
            &tokens(&["bot", "ci"]),
        )
        .unwrap();
    webhooks
        .upsert("pager", "https://example.com/b", "s2", &tokens(&["ops"]))
        .unwrap();
    let names = |found: Result<&WebhookTarget, Unbound>| found.map(|target| target.name.clone());

    assert_eq!(
        [
            names(webhooks.for_token("bot", "grok")),
            names(webhooks.for_token("bot", "pager")),
            names(webhooks.for_token("bot", "missing")),
            names(webhooks.for_token("guest", "grok")),
        ],
        [
            Ok("grok".to_owned()),
            Err(Unbound {
                token: "bot".into(),
                webhook: "pager".into(),
                usable: vec!["grok".into()],
            }),
            Err(Unbound {
                token: "bot".into(),
                webhook: "missing".into(),
                usable: vec!["grok".into()],
            }),
            Err(Unbound {
                token: "guest".into(),
                webhook: "grok".into(),
                usable: Vec::new(),
            }),
        ]
    );
}
