//! ACP `compaction_update` cards, for hosts that advertise
//! `clientCapabilities.session.compaction`.
//!
//! A card opens `in_progress` and ends with exactly one terminal update.
//! The card's ID is the daemon's compaction ID, which is random per
//! compaction: hosts such as T3 Code keep their transcript across
//! reconnects, so an ID that restarted per process would patch an earlier
//! card in place.
//!
//! Cards carry no `summary`: ACP reserves it for retained summary content,
//! not status text such as the before and after counts, which hosts get
//! from `usage_update`.

use agent_client_protocol::schema::v1::{
    CompactionId, CompactionStatus, CompactionUpdate, SessionUpdate,
};
use rho_protocol as wire;

/// The card update for compaction `id` reaching `status`; nothing for a
/// status a newer daemon sends.
pub(super) fn update(
    id: &wire::CompactionId,
    status: &wire::CompactionStatus,
) -> Option<SessionUpdate> {
    let id = CompactionId::new(id.as_str());
    let update = match status {
        wire::CompactionStatus::Started => {
            CompactionUpdate::new(id, CompactionStatus::InProgress)
        }
        wire::CompactionStatus::Completed => CompactionUpdate::new(id, CompactionStatus::Completed),
        wire::CompactionStatus::Cancelled => CompactionUpdate::new(id, CompactionStatus::Cancelled),
        wire::CompactionStatus::Failed { error } => {
            CompactionUpdate::new(id, CompactionStatus::Failed).error(error.clone())
        }
        // `Unknown`, or a status a newer daemon sends.
        _ => return None,
    };
    Some(SessionUpdate::CompactionUpdate(update))
}
