//! The [`AcpClientPort`] of an ACP v2 connection: Rho's session hosts speak
//! v1, and this port rewrites what they send into v2.

use std::{collections::HashSet, future::Future, pin::Pin, sync::Mutex};

use agent_client_protocol::schema::{v1, v2};
use agent_client_protocol::{Client, Error as AcpError, V2ConnectionTo};

use super::{bridge, requests, updates};
use crate::app::acp::AcpClientPort;

pub(super) struct V2Port {
    cx: V2ConnectionTo<Client>,
    /// On a port replaying history, the messages it has reset so far.
    replayed: Option<Mutex<HashSet<v2::MessageId>>>,
}

impl V2Port {
    pub(super) fn new(cx: V2ConnectionTo<Client>) -> Self {
        Self { cx, replayed: None }
    }

    /// A port for a replay. v2 hosts append chunks to a message they already
    /// hold, so before a replayed message's first chunk this port sends the
    /// message with no content, which resets it; see [`updates::reset`].
    pub(super) fn replaying(cx: V2ConnectionTo<Client>) -> Self {
        Self {
            cx,
            replayed: Some(Mutex::default()),
        }
    }

    /// Sends a v2-only update, such as a `state_update`.
    pub(super) fn send_update(
        &self,
        session_id: &v2::SessionId,
        update: v2::SessionUpdate,
    ) -> Result<(), AcpError> {
        self.cx
            .send_notification(v2::UpdateSessionNotification::new(
                session_id.clone(),
                update,
            ))
    }
}

impl AcpClientPort for V2Port {
    fn send_session_notification(
        &self,
        notification: v1::SessionNotification,
    ) -> Pin<Box<dyn Future<Output = Result<(), AcpError>> + Send + '_>> {
        Box::pin(async move {
            match updates::notification(notification) {
                Ok(Some(notification)) => {
                    if let Some(replayed) = &self.replayed {
                        let reset = updates::reset(&notification).filter(|(message, _)| {
                            replayed
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .insert(message.clone())
                        });
                        if let Some((_, reset)) = reset {
                            self.cx.send_notification(reset)?;
                        }
                    }
                    self.cx.send_notification(notification)
                }
                Ok(None) => Ok(()),
                // A card the host cannot draw must not fail the turn that
                // produced it, so it is dropped and logged instead.
                Err(error) => {
                    tracing::warn!(%error, "could not send session update over ACP v2");
                    Ok(())
                }
            }
        })
    }

    fn request_permission(
        &self,
        request: v1::RequestPermissionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<v1::RequestPermissionResponse, AcpError>> + Send + '_>>
    {
        Box::pin(async move {
            let request = requests::permission_request(request)?;
            let response = self.cx.send_request(request).block_task().await?;
            Ok(requests::permission_response(response))
        })
    }

    fn create_elicitation(
        &self,
        request: v1::CreateElicitationRequest,
    ) -> Pin<Box<dyn Future<Output = Result<v1::CreateElicitationResponse, AcpError>> + Send + '_>>
    {
        Box::pin(async move {
            let request: v2::CreateElicitationRequest = bridge(&request)?;
            let response = self.cx.send_request(request).block_task().await?;
            bridge(&response)
        })
    }

    fn complete_elicitation(
        &self,
        notification: v1::CompleteElicitationNotification,
    ) -> Pin<Box<dyn Future<Output = Result<(), AcpError>> + Send + '_>> {
        Box::pin(async move {
            let notification: v2::CompleteElicitationNotification = bridge(&notification)?;
            self.cx.send_notification(notification)
        })
    }

    // `fs/*` and `terminal/*` keep the port's defaults: v2 removed them, and
    // `requests::initialize_request` never negotiates them.
}
