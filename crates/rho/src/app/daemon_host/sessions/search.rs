//! `rho/session/search` over the socket: the sessions `rho/session/list`
//! lists whose conversation holds every word of a query, from the session
//! store's search index, the one the agent's `sessions` tool searches
//! (`session::search_index`).
//!
//! A search never waits for a full read of the transcripts. Until the index
//! has read every one, as on a daemon's first search or after an upgrade
//! changed the index's layout, a background thread builds it, committing a
//! file at a time, and each search answers from what it holds so far with
//! `indexing` set. Once it is built, a search first reads the transcripts
//! changed since the last one, on its own blocking thread, and says
//! `indexing` while one waits to be read again, as one that changed while
//! it was read.

use std::{
    num::{NonZeroU32, NonZeroUsize},
    path::PathBuf,
    sync::{Arc, Mutex},
    thread::JoinHandle,
    time::Instant,
};

use rho_protocol::{
    ErrorCode, RpcError, SessionSearchHit, SessionSearchParams, SessionSearchResponse,
};
use rho_sdk::CancellationToken;

use super::{catalog::summary_to_wire, Sessions};
use crate::{
    app::row_locations,
    session::{
        search_index,
        search_page::{self, SearchError, SearchRequest},
    },
};

/// Sessions per page unless a client asks for another size. A hit with two
/// excerpts of ~320 characters is under a kilobyte, so a page is ~15 KB.
const DEFAULT_PAGE_SIZE: NonZeroUsize = NonZeroUsize::new(20).unwrap();

/// The most sessions one page holds: what a client shows at once, ~75 KB.
/// A client that wants more follows `next_cursor`.
const MAX_PAGE_SIZE: usize = 100;

/// The daemon's background build of the search index, so searches start
/// at most one at a time.
#[derive(Clone, Default)]
pub(super) struct IndexBuild(Arc<Mutex<Option<JoinHandle<()>>>>);

impl IndexBuild {
    fn running(&self) -> bool {
        self.lock()
            .as_ref()
            .is_some_and(|build| !build.is_finished())
    }

    /// Builds the index of session root `root` on a thread of its own,
    /// unless a build runs already; `stopping` cancels it between files.
    fn start(&self, root: PathBuf, stopping: CancellationToken) {
        let mut build = self.lock();
        if build.as_ref().is_some_and(|build| !build.is_finished()) {
            return;
        }
        let spawned = std::thread::Builder::new()
            .name("session-search-index".into())
            .spawn(move || {
                let started = Instant::now();
                let built = search_index::open(&root).and_then(|mut connection| {
                    search_index::refresh(
                        &mut connection,
                        &root,
                        /*reconcile*/ false,
                        row_locations,
                        &stopping,
                    )
                });
                match built {
                    Ok(report) => tracing::info!(
                        elapsed = ?started.elapsed(),
                        files = report.files_updated,
                        bytes = report.bytes_read,
                        skipped = report.skipped_files,
                        "built the sessions search index"
                    ),
                    Err(error) => {
                        tracing::warn!(%error, "could not build the sessions search index");
                    }
                }
            });
        match spawned {
            Ok(spawned) => *build = Some(spawned),
            Err(error) => {
                tracing::warn!(%error, "could not start building the sessions search index");
            }
        }
    }

    /// Waits for the build under way, if one is.
    #[cfg(test)]
    fn finish(&self) {
        if let Some(build) = self.lock().take() {
            build.join().unwrap();
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<JoinHandle<()>>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Sessions {
    /// One page of the listed sessions whose conversation matches
    /// `params.query`, newest first.
    pub(in crate::app::daemon_host) async fn search(
        &self,
        params: SessionSearchParams,
    ) -> Result<SessionSearchResponse, RpcError> {
        let limit = page_size(params.limit)?;
        let request = search_page::check(SearchRequest {
            query: &params.query,
            cwd: params.cwd.as_deref(),
            cursor: params.cursor.as_deref(),
            limit,
        })
        .map_err(refused)?;
        let root = crate::session::session_root_in(&self.rho_home);
        let build = self.index_build.clone();
        let stopping = self.stopping.clone();
        let searched = tokio::task::spawn_blocking(move || {
            // Listing reconciles the catalog, which journals transcripts it
            // only now found for the refresh below to read.
            let listed = search_page::listed(&root, params.cwd.as_deref())?;
            let mut connection = search_index::open(&root).map_err(SearchError::Store)?;
            let indexing = build.running()
                || !search_index::complete(&connection)
                    .map_err(|error| SearchError::Store(error.into()))?;
            let indexing = if indexing {
                build.start(root.clone(), stopping);
                true
            } else {
                match search_index::refresh(
                    &mut connection,
                    &root,
                    /*reconcile*/ false,
                    row_locations,
                    &stopping,
                ) {
                    Ok(_) => !search_index::complete(&connection)
                        .map_err(|error| SearchError::Store(error.into()))?,
                    // Another process may hold the index a while; answer
                    // from what it holds, saying it may miss matches.
                    Err(error) => {
                        tracing::warn!(%error, "could not refresh the sessions search index");
                        true
                    }
                }
            };
            let page = search_page::search(&connection, listed, request)?;
            Ok::<_, SearchError>((page, indexing))
        })
        .await
        .map_err(|error| failed(&error.into()))?;
        let (page, indexing) = searched.map_err(refused)?;
        Ok(SessionSearchResponse::new(
            page.hits
                .into_iter()
                .map(|hit| {
                    SessionSearchHit::new(
                        summary_to_wire(hit.summary),
                        hit.matching_messages,
                        hit.excerpts,
                    )
                })
                .collect(),
            page.next_cursor,
            page.total,
            indexing,
        ))
    }
}

/// `limit` as a page size: [`DEFAULT_PAGE_SIZE`] when absent, and refused
/// over [`MAX_PAGE_SIZE`].
fn page_size(limit: Option<NonZeroU32>) -> Result<NonZeroUsize, RpcError> {
    let Some(limit) = limit else {
        return Ok(DEFAULT_PAGE_SIZE);
    };
    NonZeroUsize::try_from(limit)
        .ok()
        .filter(|size| size.get() <= MAX_PAGE_SIZE)
        .ok_or_else(|| {
            RpcError::new(
                ErrorCode::InvalidParams,
                format!(
                    "limit {limit} is over the session search's page limit (MAX_PAGE_SIZE = {MAX_PAGE_SIZE})"
                ),
            )
        })
}

/// The error a search answers for `error`: the request's own mistakes are
/// `InvalidParams`, the store's failures `InternalError`.
fn refused(error: SearchError) -> RpcError {
    match error {
        SearchError::BlankQuery
        | SearchError::LongQuery(_)
        | SearchError::RelativeCwd
        | SearchError::UnknownCursor(_) => {
            RpcError::new(ErrorCode::InvalidParams, error.to_string())
        }
        SearchError::Store(error) => failed(&error),
    }
}

fn failed(error: &anyhow::Error) -> RpcError {
    RpcError::new(
        ErrorCode::InternalError,
        format!("could not search the sessions: {error:#}"),
    )
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
