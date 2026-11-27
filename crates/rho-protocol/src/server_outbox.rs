//! The connection's outgoing queue, which its one writer drains, and the
//! duty to answer each request handed to the service through it.

use serde_json::Value;
use tokio::sync::{mpsc, OwnedSemaphorePermit};

use crate::{jsonrpc, ErrorCode, RequestId, Response, RpcError};

/// Messages queued for the connection's writer, written in queue order.
///
/// Responses are unbounded: each answers a request the peer sent, so they
/// grow only while the peer sends requests faster than it reads the answers.
/// The socket is private to the daemon's user, so only a buggy frontend does
/// that, and it costs memory rather than correctness. A request's answer
/// never waits behind events.
///
/// Notifications are bounded: each holds one of the connection's
/// [`crate::forward::NOTIFICATION_QUEUE`] permits until written, so a client
/// that stops reading stalls its forwarders, and their publishers report
/// the lag, instead of growing this queue. One queue for both keeps write
/// order equal to queue order, which the forwarders' ordering rests on.
pub(crate) type Outbox = mpsc::UnboundedSender<Outgoing>;

/// One message for the connection's writer.
pub(crate) enum Outgoing {
    Response(Response),
    /// Holds its queue permit until written.
    Notification(jsonrpc::Notification, OwnedSemaphorePermit),
}

/// The duty to answer one request handed to the service.
///
/// A call that ends without answering, because its handler panicked or the
/// connection cancelled it, answers with an internal error on drop, so the
/// peer never waits on a request that will not finish.
pub(crate) struct Responder {
    /// `None` once answered.
    id: Option<RequestId>,
    pub(crate) outbox: Outbox,
}

impl Responder {
    pub(crate) fn new(id: RequestId, outbox: &Outbox) -> Self {
        Self {
            id: Some(id),
            outbox: outbox.clone(),
        }
    }

    pub(crate) fn send(mut self, outcome: Result<Value, RpcError>) {
        self.answer(outcome);
    }

    fn answer(&mut self, outcome: Result<Value, RpcError>) {
        if let Some(id) = self.id.take() {
            // Fails only once the writer is gone; then nobody can hear it.
            let _ = self.outbox.send(Outgoing::Response(Response {
                id: Some(id),
                outcome,
            }));
        }
    }
}

impl Drop for Responder {
    fn drop(&mut self) {
        self.answer(Err(RpcError::new(
            ErrorCode::InternalError,
            "the daemon failed while handling this request",
        )));
    }
}
