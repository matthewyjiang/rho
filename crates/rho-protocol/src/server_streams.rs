//! Requests that answer and then keep sending: an attach or observe, whose
//! session's events follow its response, and a login, whose prompt and end
//! do. [`crate::server`] hands each its responder and the connection's
//! forwarders.

use std::sync::Arc;

use crate::{
    forward::Forwarders,
    methods::{
        AuthLogin, AuthLoginParams, SessionAttach, SessionAttachParams, SessionObserveParams,
    },
    server::{encode_result, Responder, Service},
};

/// A request that subscribes the connection to a session's events.
pub(crate) enum AttachRequest {
    Attach(SessionAttachParams),
    Observe(SessionObserveParams),
}

/// Answers an attach or observe through `responder`, then forwards the
/// session's events through `forwarders`.
pub(crate) async fn attach<Svc: Service>(
    service: &Svc,
    request: AttachRequest,
    responder: Responder,
    forwarders: &Arc<Forwarders>,
) {
    let (session, attached) = match request {
        AttachRequest::Attach(params) => {
            (params.session.clone(), service.attach_session(params).await)
        }
        AttachRequest::Observe(params) => (
            params.session.clone(),
            service.observe_session(params).await,
        ),
    };
    // Both answer with an attach's response.
    let attached = attached.and_then(|(response, subscription)| {
        Ok((encode_result::<SessionAttach>(&response)?, subscription))
    });
    match attached {
        Ok((result, subscription)) => {
            let outbox = responder.outbox.clone();
            forwarders.attach(session, subscription, outbox, move || {
                responder.send(Ok(result));
            });
        }
        Err(error) => responder.send(Err(error)),
    }
}

/// Answers a login through `responder`, then forwards the login's prompt
/// and end through `forwarders`.
pub(crate) async fn login<Svc: Service>(
    service: &Svc,
    params: AuthLoginParams,
    responder: Responder,
    forwarders: &Arc<Forwarders>,
) {
    let started = service.login(params).await.and_then(|(response, stream)| {
        Ok((
            response.login.clone(),
            encode_result::<AuthLogin>(&response)?,
            stream,
        ))
    });
    match started {
        Ok((login, result, stream)) => {
            let outbox = responder.outbox.clone();
            forwarders.relay_login(login, stream, outbox, move || {
                responder.send(Ok(result));
            });
        }
        Err(error) => responder.send(Err(error)),
    }
}
