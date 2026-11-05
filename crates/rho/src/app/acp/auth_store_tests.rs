use std::{future::Future, pin::Pin, sync::Mutex};

use agent_client_protocol::schema::v1::{
    CreateElicitationRequest, CreateElicitationResponse, ElicitationAcceptAction,
    ElicitationAction, ElicitationContentValue, RequestPermissionRequest,
    RequestPermissionResponse, SessionNotification,
};
use pretty_assertions::assert_eq;
use rho_providers::credentials::CredentialStoreProbe;
use serde_json::json;

use super::*;
use crate::app::acp::host_features::FormDialect;

/// Answers the store question with `action` and records the form.
struct Host {
    action: ElicitationAction,
    shown: Mutex<Vec<CreateElicitationRequest>>,
}

impl AcpClientPort for Host {
    fn send_session_notification(
        &self,
        _notification: SessionNotification,
    ) -> Pin<Box<dyn Future<Output = Result<(), AcpError>> + Send + '_>> {
        unreachable!("the store question sends no session updates")
    }

    fn request_permission(
        &self,
        _request: RequestPermissionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RequestPermissionResponse, AcpError>> + Send + '_>>
    {
        unreachable!("the store question asks no permission")
    }

    fn create_elicitation(
        &self,
        request: CreateElicitationRequest,
    ) -> Pin<Box<dyn Future<Output = Result<CreateElicitationResponse, AcpError>> + Send + '_>>
    {
        self.shown.lock().unwrap().push(request);
        let response = CreateElicitationResponse::new(self.action.clone());
        Box::pin(async move { Ok(response) })
    }
}

fn choices(os: bool, file: bool) -> StoreChoiceRequest {
    let probe = |backend, available| CredentialStoreProbe {
        backend,
        available,
        detail: format!("{} probed", backend.as_str()),
    };
    StoreChoiceRequest {
        os: probe(CredentialStoreBackend::Os, os),
        file: probe(CredentialStoreBackend::File, file),
    }
}

fn answer(value: &str) -> ElicitationAction {
    let mut accept = ElicitationAcceptAction::new();
    accept.content = Some(
        [(
            QUESTION.to_owned(),
            ElicitationContentValue::from(value.to_owned()),
        )]
        .into(),
    );
    ElicitationAction::Accept(accept)
}

// Covers: the store question is a form scoped to `authenticate` offering
// only stores that can be opened, answered by value on standard hosts and
// by label on T3 Code; declining abandons the sign-in; a host without forms
// or a machine with no usable store fails without asking.
// Owner: ACP credential store question.
#[tokio::test]
async fn the_store_question_offers_usable_stores_and_returns_the_choice() {
    const FILE_LABEL: &str = "Local file under ~/.rho/credentials (not encrypted at rest)";
    let standard = Forms::Available(FormDialect::Standard);
    let t3 = Forms::Available(FormDialect::T3Code);
    let unset = AcpError::internal_error()
        .data("credential store is unset; choose one with `rho credential-store set os|file`");
    let none = AcpError::internal_error()
        .data("no credential store backend is available (os: os probed; file: file probed)");
    let cases = [
        // (os, file available), forms, answer -> chosen, options offered
        (
            (true, true),
            standard,
            answer("file"),
            Ok(CredentialStoreBackend::File),
            Some(json!(["os", "file"])),
        ),
        (
            (false, true),
            t3,
            answer(FILE_LABEL),
            Ok(CredentialStoreBackend::File),
            Some(json!([FILE_LABEL])),
        ),
        (
            (true, true),
            standard,
            ElicitationAction::Decline,
            Err(AcpError::request_cancelled()),
            Some(json!(["os", "file"])),
        ),
        (
            (true, true),
            Forms::Unavailable,
            answer("os"),
            Err(unset),
            None,
        ),
        ((false, false), standard, answer("os"), Err(none), None),
    ];
    for ((os, file), forms, action, expected, offered) in cases {
        let host = Host {
            action,
            shown: Mutex::default(),
        };
        let chosen = ask(&choices(os, file), forms, &RequestId::Number(7), &host).await;
        assert_eq!(chosen, expected, "os={os} file={file} {forms:?}");
        let shown = host
            .shown
            .into_inner()
            .unwrap()
            .iter()
            .map(|form| {
                let form = serde_json::to_value(form).unwrap();
                assert_eq!(form["mode"], "form");
                assert_eq!(form["requestId"], 7, "scoped to authenticate");
                form["requestedSchema"]["properties"][QUESTION]["enum"].clone()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            shown,
            Vec::from_iter(offered),
            "os={os} file={file} {forms:?}"
        );
    }
}
