use super::*;
use pretty_assertions::assert_eq;

// Covers: which typed failures ask the host to sign in, wherever they sit in
// the cause chain, before (startup) or after (prompt) the provider layer
// converts them for the SDK, and that the reply carries no credential or
// provider diagnostic. Other provider errors stay internal.
// Owner: ACP auth classification; hosts render only code and message.
#[test]
fn credential_failures_map_to_auth_required_without_diagnostics() {
    const SECRET: &str = "Authorization: Bearer sk-leaked";
    let refused = |status: u16| ModelError::HttpStatus {
        status: http::StatusCode::from_u16(status).unwrap(),
        body: SECRET.into(),
        retry_after: None,
    };
    let provider = |kind| {
        rho_sdk::Error::Provider(
            rho_sdk::ProviderError::new(kind, "HTTP 401", rho_sdk::Retryability::Permanent)
                .with_diagnostic(SECRET),
        )
    };
    let converted = |error| {
        rho_sdk::Error::Provider(
            rho_providers::providers::sdk_contract::provider_error_from_model_error(error),
        )
    };
    let missing = "missing OpenAI API key; run /login openai";
    let cases: Vec<(anyhow::Error, Option<CredentialFailure>)> = vec![
        (
            anyhow::Error::from(ModelError::MissingCredentials(missing))
                .context("could not start session"),
            Some(CredentialFailure::Missing(missing)),
        ),
        (refused(401).into(), Some(CredentialFailure::Refused)),
        (refused(403).into(), Some(CredentialFailure::Refused)),
        (refused(500).into(), None),
        (
            provider(rho_sdk::ProviderErrorKind::Authentication).into(),
            Some(CredentialFailure::Refused),
        ),
        (
            rho_sdk::Error::Authentication {
                message: SECRET.into(),
            }
            .into(),
            Some(CredentialFailure::Refused),
        ),
        (provider(rho_sdk::ProviderErrorKind::RateLimit).into(), None),
        (
            ModelError::credentials("keyring locked").into(),
            Some(CredentialFailure::Unreadable),
        ),
        // Prompt-time failures arrive already converted.
        (
            converted(ModelError::credentials("keyring locked")).into(),
            Some(CredentialFailure::Refused),
        ),
        (
            converted(refused(401)).into(),
            Some(CredentialFailure::Refused),
        ),
        (converted(refused(429)).into(), None),
        // Wording alone never classifies: only typed causes count.
        (anyhow::anyhow!("missing OpenAI API key; HTTP 401"), None),
    ];
    for (error, expected) in cases {
        let found = CredentialFailure::find(&error);
        assert_eq!(found, expected, "{error:#}");
        if let Some(failure) = found {
            let reply = serde_json::to_value(failure.into_acp()).unwrap();
            assert_eq!(reply["code"], -32000);
            assert_eq!(reply.get("data"), None);
            assert!(!reply.to_string().contains("sk-leaked"), "{reply}");
        }
    }
}

// Covers: OAuth runs in this process only for hosts that can show its URL and
// once a credential store is chosen (only the terminal login asks), and
// `authenticate` accepts exactly the logins advertised as agent methods; API
// keys always need the terminal's hidden entry; a host gets no method it
// cannot complete; device-key setup is never offered.
// Owner: ACP auth advertisement and the authenticate gate.
#[test]
fn each_login_is_offered_where_the_host_can_finish_it() {
    use CredentialStoreChoice::{Made, Pending};
    let initialize = |terminal: bool, urls: bool| {
        serde_json::from_value::<InitializeRequest>(serde_json::json!({
            "protocolVersion": 1,
            "clientCapabilities": {
                "auth": {"terminal": terminal},
                "elicitation": if urls { serde_json::json!({"url": {}}) } else { serde_json::json!({"form": {}}) },
            },
        }))
        .unwrap()
    };
    let kind = |methods: &[AuthMethod], id: &str| {
        methods
            .iter()
            .find(|method| method.id().0.as_ref() == id)
            .map(|method| match method {
                AuthMethod::Agent(_) => "agent",
                AuthMethod::Terminal(_) => "terminal",
                _ => "other",
            })
    };
    let cases = [
        // (terminal, url elicitation, store) -> (OAuth, API key)
        ((true, true, Made), (Some("agent"), Some("terminal"))),
        ((true, true, Pending), (Some("terminal"), Some("terminal"))),
        ((true, false, Made), (Some("terminal"), Some("terminal"))),
        ((false, true, Made), (Some("agent"), None)),
        ((false, true, Pending), (None, None)),
        ((false, false, Made), (None, None)),
    ];
    for ((terminal, urls, store), expected) in cases {
        let request = initialize(terminal, urls);
        let sign_in = oauth_sign_in(HostFeatures::from_initialize(&request), store);
        let methods = methods(&request, sign_in);
        assert_eq!(
            (kind(&methods, "xai-oauth"), kind(&methods, "api-key")),
            expected,
            "terminal={terminal} urls={urls} store={store:?}"
        );
        assert_eq!(kind(&methods, "ollama-cloud-device"), None);
        let accepted = ["xai-oauth", "api-key", "ollama-cloud-device"]
            .map(|auth| runs_in_process(auth, sign_in).then_some("agent"));
        assert_eq!(
            accepted,
            [expected.0.filter(|kind| *kind == "agent"), None, None],
            "authenticate accepts what was advertised as agent methods"
        );
    }
}
