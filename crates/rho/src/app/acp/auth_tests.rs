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

// Covers: OAuth and API-key logins run in this process only for hosts that
// can show a URL, and, until a credential store is chosen, only for hosts
// that can also ask the store question as a form; API keys stay in the
// terminal when the user's browser is elsewhere (SSH, containers), since it
// cannot reach the local key page; `authenticate` accepts exactly the logins
// advertised as agent methods; a host gets no method it cannot complete;
// device-key setup is never offered.
// Owner: ACP auth advertisement and the authenticate gate.
#[test]
fn each_login_is_offered_where_the_host_can_finish_it() {
    use rho_providers::auth::browser::BrowserAvailability::{Graphical, Headless};
    use CredentialStoreChoice::{Made, Pending};
    let initialize = |terminal: bool, urls: bool, forms: bool| {
        let mut elicitation = serde_json::Map::new();
        if urls {
            elicitation.insert("url".into(), serde_json::json!({}));
        }
        if forms {
            elicitation.insert("form".into(), serde_json::json!({}));
        }
        serde_json::from_value::<InitializeRequest>(serde_json::json!({
            "protocolVersion": 1,
            "clientCapabilities": {
                "auth": {"terminal": terminal},
                "elicitation": elicitation,
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
    let (agent, terminal) = (Some("agent"), Some("terminal"));
    let cases = [
        // (terminal, url elicitation, form elicitation, store, browser) -> (OAuth, API key)
        ((true, true, false, Made, Graphical), (agent, agent)),
        (
            (true, true, false, Pending, Graphical),
            (terminal, terminal),
        ),
        ((true, true, true, Pending, Graphical), (agent, agent)),
        ((true, false, true, Made, Graphical), (terminal, terminal)),
        ((true, true, true, Made, Headless), (agent, terminal)),
        ((false, true, false, Made, Graphical), (agent, agent)),
        ((false, true, false, Pending, Graphical), (None, None)),
        ((false, true, true, Pending, Graphical), (agent, agent)),
        ((false, false, true, Made, Graphical), (None, None)),
        ((false, true, true, Made, Headless), (agent, None)),
    ];
    for ((terminal, urls, forms, store, browser), expected) in cases {
        let request = initialize(terminal, urls, forms);
        let sign_in = sign_in(HostFeatures::from_initialize(&request), store, browser);
        let methods = methods(&request, sign_in);
        let context =
            format!("terminal={terminal} urls={urls} forms={forms} store={store:?} {browser:?}");
        assert_eq!(
            (kind(&methods, "xai-oauth"), kind(&methods, "api-key")),
            expected,
            "{context}"
        );
        assert_eq!(kind(&methods, "ollama-cloud-device"), None);
        let accepted =
            ["xai-oauth", "api-key", "ollama-cloud-device"].map(|auth| agent_login(auth, sign_in));
        let advertised = |kind: Option<&str>, login| (kind == agent).then_some(login);
        assert_eq!(
            accepted,
            [
                advertised(expected.0, AgentLogin::OAuth),
                advertised(expected.1, AgentLogin::ApiKey),
                None
            ],
            "authenticate accepts what was advertised as agent methods: {context}"
        );
    }
}
