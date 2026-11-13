//! The [`AcpClientPort`] of an ACP v2 connection: Rho's adapter speaks v1,
//! and this port rewrites what it sends into v2.

use std::{collections::HashSet, future::Future, pin::Pin, sync::Mutex};

use agent_client_protocol::{v1::schema as v1, v2::schema as v2};
use agent_client_protocol::{v2::ConnectionTo as V2ConnectionTo, Client, Error as AcpError};

use rho_protocol::BackgroundWork;

use super::{background, bridge, requests, updates};
use crate::app::acp::{AcpClientPort, BackgroundChanged, Foreground, TurnEnd};

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
                    if let (Some(replayed), Some(message)) =
                        (&self.replayed, updates::chunk_message_id(&notification))
                    {
                        let first = replayed
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .insert(message.clone());
                        if first {
                            if let Some(reset) = updates::reset(&notification) {
                                self.cx.send_notification(reset)?;
                            }
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

    /// Closes the `running` that `session/resume` reported with `idle`.
    fn attached_turn_ended(
        &self,
        session_id: &v1::SessionId,
        end: TurnEnd,
        background: &[BackgroundWork],
    ) {
        let sent = bridge::<_, v2::SessionId>(session_id)
            .and_then(|session_id| self.send_update(&session_id, requests::idle(&end, background)));
        if let Err(error) = sent {
            tracing::warn!(%error, "could not report the end of an attached turn over ACP v2");
        }
    }

    /// Reports `requires_action` while the turn waits for a person, and
    /// `running` once it goes on.
    fn foreground_changed(
        &self,
        session_id: &v1::SessionId,
        foreground: Foreground,
        background: &[BackgroundWork],
    ) {
        let sent = bridge::<_, v2::SessionId>(session_id).and_then(|session_id| {
            self.send_update(&session_id, background::state(foreground, background))
        });
        if let Err(error) = sent {
            tracing::warn!(%error, "could not report a turn's state over ACP v2");
        }
    }

    /// Announces the work that started and finalizes the work that
    /// stopped, then repeats the session's state with the new background
    /// list. The list changes exactly when work starts or stops, so with
    /// neither the host hears nothing.
    fn background_changed(&self, session_id: &v1::SessionId, change: BackgroundChanged<'_>) {
        let updates = background::changes(change.previous, change.current);
        if updates.is_empty() {
            return;
        }
        let sent = bridge::<_, v2::SessionId>(session_id).and_then(|session_id| {
            for update in updates {
                self.send_update(&session_id, update)?;
            }
            self.send_update(
                &session_id,
                background::state(change.foreground, change.current),
            )
        });
        if let Err(error) = sent {
            tracing::warn!(%error, "could not report background work over ACP v2");
        }
    }
}
