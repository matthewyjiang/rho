use std::{future::Future, pin::Pin, sync::Mutex};

use agent_client_protocol::schema::v1::{
    CreateElicitationResponse, ElicitationAcceptAction, ElicitationMode, RequestPermissionRequest,
    RequestPermissionResponse, SessionNotification,
};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

/// How the fake host answers the sign-in elicitation.
#[derive(Clone, Copy, Debug)]
enum HostAnswer {
    Accept,
    Decline,
    Dismiss,
    /// The user has not chosen yet.
    Pending,
    /// The host cannot show it.
    Refuses,
}

/// What the provider does with the login.
#[derive(Clone, Copy, Debug)]
enum Provider {
    Approves,
    Fails,
    /// Still waiting for the user.
    Waits,
}

/// Records the elicitation it is shown and the completions it is sent.
struct FakeHost {
    answer: HostAnswer,
    shown: Mutex<Vec<CreateElicitationRequest>>,
    completed: Mutex<Vec<ElicitationId>>,
}

impl AcpClientPort for FakeHost {
    fn send_session_notification(
        &self,
        _notification: SessionNotification,
    ) -> Pin<Box<dyn Future<Output = Result<(), AcpError>> + Send + '_>> {
        unreachable!("sign-in sends no session updates")
    }

    fn request_permission(
        &self,
        _request: RequestPermissionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RequestPermissionResponse, AcpError>> + Send + '_>>
    {
        unreachable!("sign-in asks no permission")
    }

    fn create_elicitation(
        &self,
        request: CreateElicitationRequest,
    ) -> Pin<Box<dyn Future<Output = Result<CreateElicitationResponse, AcpError>> + Send + '_>>
    {
        self.shown.lock().unwrap().push(request);
        let action = match self.answer {
            HostAnswer::Accept => ElicitationAction::Accept(ElicitationAcceptAction::new()),
            HostAnswer::Decline => ElicitationAction::Decline,
            HostAnswer::Dismiss => ElicitationAction::Cancel,
            HostAnswer::Pending => return Box::pin(std::future::pending()),
            HostAnswer::Refuses => return Box::pin(async { Err(AcpError::method_not_found()) }),
        };
        Box::pin(async move { Ok(CreateElicitationResponse::new(action)) })
    }

    fn complete_elicitation(
        &self,
        notification: CompleteElicitationNotification,
    ) -> Pin<Box<dyn Future<Output = Result<(), AcpError>> + Send + '_>> {
        self.completed
            .lock()
            .unwrap()
            .push(notification.elicitation_id);
        Box::pin(async { Ok(()) })
    }
}

// Covers: the host shows the login's URL (the one with the device code filled
// in) tied to the authenticate request; authenticate succeeds only once the
// provider confirms, including when the user approves before the host
// answers; declining or dismissing abandons a login that is still waiting; a
// host that cannot show the link points at the terminal login; and only a
// finished sign-in is marked complete.
// Owner: ACP authenticate presentation; provider flows are faked.
#[tokio::test]
async fn sign_in_waits_for_the_provider_and_ends_with_the_host() {
    let failed = || AcpError::internal_error().data("could not sign in to SpaceXAI: denied");
    let cases = [
        (HostAnswer::Accept, Provider::Approves, Ok(()), true),
        (HostAnswer::Pending, Provider::Approves, Ok(()), true),
        (HostAnswer::Accept, Provider::Fails, Err(failed()), false),
        (
            HostAnswer::Decline,
            Provider::Waits,
            Err(AcpError::request_cancelled()),
            false,
        ),
        (
            HostAnswer::Dismiss,
            Provider::Waits,
            Err(AcpError::request_cancelled()),
            false,
        ),
        (
            HostAnswer::Refuses,
            Provider::Waits,
            Err(AcpError::internal_error().data(format!(
                "could not show the SpaceXAI sign-in link: {}; run `rho login xai-oauth` in a terminal instead",
                AcpError::method_not_found()
            ))),
            false,
        ),
    ];
    let prompt = LoginPrompt::device_code(
        "https://accounts.x.ai/device",
        "ABCD-1234",
        Some("https://accounts.x.ai/device?code=ABCD-1234".into()),
        "Visit this URL and enter the code.",
    );
    for (answer, provider, expected, completes) in cases {
        let host = FakeHost {
            answer,
            shown: Mutex::default(),
            completed: Mutex::default(),
        };
        let signed_in = async move {
            match provider {
                Provider::Approves => Ok(()),
                Provider::Fails => Err(failed()),
                Provider::Waits => std::future::pending().await,
            }
        };
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            show_until_signed_in(
                &host,
                RequestId::Number(7),
                ShownLogin {
                    auth: "xai-oauth",
                    provider_label: "SpaceXAI",
                    prompt: &prompt,
                },
                signed_in,
            ),
        )
        .await
        .expect("sign-in ends without waiting on a login nobody finishes");
        assert_eq!(result, expected, "{answer:?}, {provider:?}");

        let shown = host.shown.into_inner().unwrap();
        let [request] = shown.as_slice() else {
            panic!("one elicitation per sign-in: {shown:?}");
        };
        let ElicitationMode::Url(mode) = &request.mode else {
            panic!("sign-in is a URL elicitation: {request:?}");
        };
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            json!({
                "mode": "url",
                "requestId": 7,
                "elicitationId": mode.elicitation_id.0.as_ref(),
                "url": "https://accounts.x.ai/device?code=ABCD-1234",
                "message": request.message,
            })
        );
        assert!(
            request.message.contains("ABCD-1234"),
            "the code to confirm is shown: {}",
            request.message
        );
        let completed = host.completed.into_inner().unwrap();
        let expected_completed = if completes {
            vec![mode.elicitation_id.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(completed, expected_completed, "{answer:?}, {provider:?}");
    }
}

/// Sets its flag when dropped, as a login releases its callback port.
struct Releases(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Drop for Releases {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

// Covers: a new authenticate supersedes a login the host abandoned without
// `$/cancel_request`: the old one ends cancelled, and the new one starts only
// after the old login was dropped, so a fixed callback port is free to bind.
// Owner: ACP authenticate single-flight.
#[tokio::test]
async fn a_new_sign_in_replaces_the_abandoned_one_after_it_lets_go() {
    use std::sync::{atomic::AtomicBool, atomic::Ordering, Arc};
    let flight = LoginFlight::default();
    let released = Arc::new(AtomicBool::new(false));
    let abandoned = {
        let guard = Releases(Arc::clone(&released));
        async move {
            let _port = guard;
            std::future::pending::<Result<&str, AcpError>>().await
        }
    };
    let retry = async { Ok(released.load(Ordering::SeqCst)) };
    let (first, second) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        tokio::join!(flight.run(abandoned), flight.run(retry))
    })
    .await
    .expect("the abandoned login does not block the retry");
    assert_eq!(
        (first, second),
        (Err(AcpError::request_cancelled()), Ok(true)),
        "the retry ran after the abandoned login was dropped"
    );
}
