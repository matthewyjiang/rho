//! Host-owned builders for the transcript presentations of
//! [`rho_protocol::display`], which journals, rendering and the daemon share.

use rho_protocol::display::{
    NotificationCard, NotificationDelivery, NotificationPreview, NotificationTone,
    NotificationVisibility,
};

pub(crate) mod checklist;
pub(crate) mod diff;
pub(crate) mod number;
pub(crate) mod workflow;

/// Incoming parent text, with delivery details supplied by the runtime owner.
pub(crate) fn parent_message_card(
    body: String,
    delivery: NotificationDelivery,
    detail: String,
) -> NotificationCard {
    NotificationCard {
        title: "Message from parent".into(),
        sender: "parent".into(),
        recipient: "agent".into(),
        delivery,
        tone: NotificationTone::Neutral,
        preview: NotificationPreview::Full,
        visibility: NotificationVisibility::Conversation,
        reference: None,
        subtitle: None,
        body,
        output: None,
        details: vec![detail],
    }
}
