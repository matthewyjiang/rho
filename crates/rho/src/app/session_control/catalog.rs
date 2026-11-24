//! The daemon's catalog of saved sessions, as the core reads it: where a
//! session no job writes works, and the titles of sessions the daemon does
//! not have loaded, which its job list carries none for.

use std::collections::{HashMap, HashSet};

use rho_protocol::{methods::SessionList, SessionId, SessionListParams, SessionSummary};

use super::{ControlError, Daemon};

/// The catalog page a walk asks for: the daemon's largest
/// (`rho/session/list` refuses more than 1000).
const PAGE: u32 = 1000;

/// Walks the saved sessions, newest first, until `visit` answers for one;
/// `None` when it answered for none.
pub(super) async fn find<T>(
    daemon: &Daemon,
    mut visit: impl FnMut(SessionSummary) -> Option<T>,
) -> Result<Option<T>, ControlError> {
    let mut cursor = None;
    loop {
        let page = daemon
            .request::<SessionList>(&SessionListParams {
                cwd: None,
                cursor,
                limit: std::num::NonZeroU32::new(PAGE),
            })
            .await?;
        if let Some(found) = page.sessions.into_iter().find_map(&mut visit) {
            return Ok(Some(found));
        }
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => return Ok(None),
        }
    }
}

/// The saved titles of the `wanted` sessions that have one. Walks the
/// catalog only as far as the last of them, and not at all for none.
//
// One request for a store of up to `PAGE` sessions. Measured on a
// workstation's store: `rho sessions list --all-projects --json`, which
// walks the same catalog, lists its 505 sessions in 40 ms.
pub(super) async fn titles(
    daemon: &Daemon,
    mut wanted: HashSet<&SessionId>,
) -> Result<HashMap<SessionId, String>, ControlError> {
    let mut titles = HashMap::new();
    if wanted.is_empty() {
        return Ok(titles);
    }
    find(daemon, |saved| {
        if wanted.remove(&saved.session) {
            if let Some(title) = saved.title {
                titles.insert(saved.session, title);
            }
        }
        wanted.is_empty().then_some(())
    })
    .await?;
    Ok(titles)
}
