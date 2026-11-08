use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use crate::methods::{AuthKind, AuthListResponse, AuthStatus, AuthSummary};

/// Asserts `value` encodes as `wire` and `wire` decodes back to `value`.
fn round_trips<T>(value: T, wire: serde_json::Value)
where
    T: Serialize + serde::de::DeserializeOwned + PartialEq + fmt::Debug,
{
    assert_eq!(serde_json::to_value(&value).unwrap(), wire);
    assert_eq!(serde_json::from_value::<T>(wire).unwrap(), value);
}

// Covers: the auth list, a login's params, its device-code and browser
// prompts, and every way it ends travel in their wire form, with absent
// optionals left out and a login that names no flow reading as the browser
// flow (#1662).
// Owner: protocol wire contract.
#[test]
fn login_messages_travel_in_their_wire_form() {
    round_trips(
        AuthListResponse {
            auths: vec![AuthSummary {
                auth: "openai-codex".into(),
                provider: "openai-codex".into(),
                label: "ChatGPT Plus/Pro (Codex)".into(),
                kind: AuthKind::Login,
                status: AuthStatus::SignedOut,
            }],
        },
        json!({"auths": [{
            "auth": "openai-codex",
            "provider": "openai-codex",
            "label": "ChatGPT Plus/Pro (Codex)",
            "kind": "login",
            "status": "signed_out",
        }]}),
    );
    round_trips(
        AuthLoginParams {
            auth: "xai-oauth".into(),
            flow: LoginFlow::DeviceCode,
        },
        json!({"auth": "xai-oauth", "flow": "device_code"}),
    );
    assert_eq!(
        serde_json::from_value::<AuthLoginParams>(json!({"auth": "github-copilot"})).unwrap(),
        AuthLoginParams {
            auth: "github-copilot".into(),
            flow: LoginFlow::Browser,
        }
    );
    round_trips(
        AuthLoginPromptParams {
            login: LoginId::new("l1"),
            prompt: LoginPrompt {
                url: "https://github.com/login/device".into(),
                user_code: Some("ABCD-1234".into()),
                url_with_code: Some("https://github.com/login/device?code=ABCD-1234".into()),
                instruction: "Visit this URL and enter the code.".into(),
            },
        },
        json!({"login": "l1", "prompt": {
            "url": "https://github.com/login/device",
            "user_code": "ABCD-1234",
            "url_with_code": "https://github.com/login/device?code=ABCD-1234",
            "instruction": "Visit this URL and enter the code.",
        }}),
    );
    round_trips(
        LoginPrompt {
            url: "https://auth.openai.com/authorize".into(),
            user_code: None,
            url_with_code: None,
            instruction: "Open this URL to finish Codex login.".into(),
        },
        json!({
            "url": "https://auth.openai.com/authorize",
            "instruction": "Open this URL to finish Codex login.",
        }),
    );
    for (outcome, wire) in [
        (LoginOutcome::Succeeded, json!({"type": "succeeded"})),
        (
            LoginOutcome::Unconfirmed {
                instruction: "Approve the device.".into(),
            },
            json!({"type": "unconfirmed", "instruction": "Approve the device."}),
        ),
        (
            LoginOutcome::Failed {
                message: "the code expired".into(),
            },
            json!({"type": "failed", "message": "the code expired"}),
        ),
        (LoginOutcome::Cancelled, json!({"type": "cancelled"})),
    ] {
        round_trips(
            AuthLoginFinishedParams {
                login: LoginId::new("l1"),
                auth: "xai-oauth".into(),
                outcome,
            },
            json!({"login": "l1", "auth": "xai-oauth", "outcome": wire}),
        );
    }
}

// Covers: a kind, status, or outcome a newer daemon adds reads as
// `Unknown` instead of failing the whole list or notification (#1662).
// Owner: protocol forward compatibility.
#[test]
fn values_a_newer_daemon_adds_read_as_unknown() {
    assert_eq!(
        (
            serde_json::from_value::<AuthKind>(json!("passkey")).unwrap(),
            serde_json::from_value::<AuthStatus>(json!("expired")).unwrap(),
            serde_json::from_value::<LoginOutcome>(json!({"type": "deferred", "until": 3}))
                .unwrap(),
        ),
        (
            AuthKind::Unknown,
            AuthStatus::Unknown,
            LoginOutcome::Unknown
        )
    );
}

// Covers: a prompt's device code, which signs in whoever enters it first,
// never shows in its debug form, so a client that logs notifications does
// not log it (#1662).
// Owner: protocol redaction.
#[test]
fn a_prompts_debug_form_hides_its_device_code() {
    let prompt = LoginPrompt {
        url: "https://github.com/login/device".into(),
        user_code: Some("ABCD-1234".into()),
        url_with_code: Some("https://github.com/login/device?code=ABCD-1234".into()),
        instruction: "Visit this URL and enter the code.".into(),
    };
    assert_eq!(
        format!("{prompt:?}"),
        "LoginPrompt { url: \"https://github.com/login/device\", user_code: Some(\"[redacted]\"), \
url_with_code: Some(\"[redacted]\"), instruction: \"Visit this URL and enter the code.\" }"
    );
}
