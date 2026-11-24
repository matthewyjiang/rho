//! The daemon's catalog of saved sessions, as the core reads it: where a
//! session no job writes works, and the titles of sessions the daemon does
//! not have loaded, which its job list carries none for.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use rho_protocol::{methods::SessionList, SessionId, SessionListParams, SessionSummary};

use super::{ControlError, Daemon};

/// The catalog page a walk asks for: the daemon's largest
/// (`rho/session/list` refuses more than 1000).
const PAGE: u32 = 1000;

/// Walks the sessions saved in workspace `cwd`, or in every workspace
/// without one, newest first, until `visit` answers with a value; `None`
/// when it never did. Each page is one request, for which the daemon reads
/// its index of those sessions again.
pub(super) async fn scan<T>(
    daemon: &Daemon,
    cwd: Option<&Path>,
    mut visit: impl FnMut(SessionSummary) -> Option<T>,
) -> Result<Option<T>, ControlError> {
    let mut cursor = None;
    loop {
        let page = daemon
            .request::<SessionList>(&SessionListParams {
                cwd: cwd.map(Path::to_path_buf),
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

/// The saved titles of the `wanted` sessions that have one, each named
/// with the workspace it is saved in. Asks each workspace's catalog, so a
/// title never comes from another workspace's session of the same id, and
/// a session the catalog does not list, as one with nothing saved is not,
/// costs a walk of its own workspace's sessions only.
//
// One request a workspace, unless one holds over `PAGE` sessions. Measured
// on a workstation's store of 505 sessions in 55 workspaces: `rho sessions
// list --json`, which reads the same index in its own process, lists a
// workspace's 51 sessions in under 10 ms, and all 505 in 40 ms.
pub(super) async fn titles<'a>(
    daemon: &Daemon,
    wanted: impl IntoIterator<Item = (&'a SessionId, &'a Path)>,
) -> Result<HashMap<SessionId, String>, ControlError> {
    let mut workspaces: HashMap<&Path, HashSet<&SessionId>> = HashMap::new();
    for (session, workspace) in wanted {
        workspaces.entry(workspace).or_default().insert(session);
    }
    let mut titles = HashMap::new();
    for (workspace, mut wanted) in workspaces {
        scan(daemon, Some(workspace), |saved| {
            if wanted.remove(&saved.session) {
                if let Some(title) = saved.title {
                    titles.insert(saved.session, title);
                }
            }
            wanted.is_empty().then_some(())
        })
        .await?;
    }
    Ok(titles)
}
