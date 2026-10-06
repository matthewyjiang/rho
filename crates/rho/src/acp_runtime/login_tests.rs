use super::authenticate;
use crate::acp_runtime::{
    scripted_agent::{Script, ScriptedAgent},
    test_support::TEST_BUDGET,
};
use agent_client_protocol::Channel;
use pretty_assertions::assert_eq;
use serde_json::{json, Value};

// Covers: login sends the caller's method after initialize, and the agent's
// own failure text (for example a sign-in timeout) reaches the user.
// Owner: ACP login. Delegated runs never authenticate (driver tests).
#[tokio::test]
async fn authenticate_sends_the_method_and_keeps_agent_errors() {
    let cases = [
        (None, Ok(())),
        (
            Some("Onboarding failed: Timed out"),
            Err("Onboarding failed: Timed out"),
        ),
    ];
    for (agent_error, expected) in cases {
        let (client, agent) = Channel::duplex();
        let (fake, record, _signals) = ScriptedAgent::new(Script {
            authenticate_error: agent_error.map(str::to_owned),
            ..Script::default()
        });
        let (outcome, _) = tokio::time::timeout(TEST_BUDGET, async {
            tokio::join!(
                authenticate(client, "oauth-personal", TEST_BUDGET),
                fake.run(agent)
            )
        })
        .await
        .expect("login settles within the test budget");
        match (outcome, expected) {
            (Ok(()), Ok(())) => {}
            (Err(message), Err(detail)) => assert!(
                message.contains("authenticate") && message.contains(detail),
                "agent error kept with its step: {message}"
            ),
            (actual, expected) => panic!("{actual:?} != {expected:?}"),
        }
        let requests = record.lock().unwrap().requests.clone();
        assert_eq!(
            requests
                .iter()
                .map(|request| (
                    request["method"].clone(),
                    request["params"]["methodId"].clone()
                ))
                .collect::<Vec<_>>(),
            vec![
                (json!("initialize"), Value::Null),
                (json!("authenticate"), json!("oauth-personal")),
            ]
        );
    }
}
