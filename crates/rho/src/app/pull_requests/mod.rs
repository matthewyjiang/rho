//! GitHub pull requests the daemon links to sessions and watches for them.
//!
//! - [`model`]: what a PR is, and what one batched read asks and answers.
//! - [`gh_query`]: the GraphQL query for a read, and its parser.
//! - [`gh`]: the [`gh::Forge`] seam and `GhCli`, which runs `gh` for it.
//! - [`watch`]: the pure evaluator of one watch against one read.
//! - [`news`]: news as the model reads it and as clients show it.
//! - [`record`]: what the daemon keeps for one session: links, watches, news.
//! - [`store`]: the per-session record files, replaced atomically.
//! - [`service`]: [`PullRequests`], the daemon-wide service.
//! - [`checkout`]: what a session's workspace has checked out.
//! - [`interest`]: the leases clients take on what they list and show.
//! - [`due`]: what a poll pass reads, from the state and the leases.
//! - [`poller`]: one poll pass, the reads a show or a watch makes itself,
//!   and the one door to the forge.
//! - [`reads`]: what reads of the forge learned, and how it is kept.
//! - [`inbox`]: [`PullRequestInbox`], one session's side of the service.
//! - [`wire`]: a session's pull requests as `rho-protocol` sends them.
//!
//! Branch lookups and the parts of [`model::PrDetail`] no watch reads serve
//! clients, which list and show a session's pull requests.

// The daemon resolves each session's checkout as a client asks.
pub(crate) mod checkout;
mod due;
mod gh;
mod gh_query;
mod inbox;
mod interest;
mod model;
mod news;
mod poller;
mod reads;
mod record;
mod service;
mod state;
mod store;
#[cfg(test)]
mod test_forge;
mod watch;
mod wire;

// The surface the daemon's wiring and the PR tools consume.
pub(crate) use self::{
    gh::{Forge, GhCli},
    inbox::{
        resolve_pull_request, CheckCounts, LastRead, Linked, ListedPullRequest, NewsBatch,
        PullRequestError, PullRequestInbox, Unlinked, Unwatched, Watched,
    },
    model::{Mergeable, PrState, Rollup},
    poller::WATCH_POLL,
    record::LinkOrigin,
    service::{unix_ms, AllMerged, PullRequests, ShowRead, ShowTarget},
    state::Gate,
};
#[cfg(test)]
pub(crate) use test_forge::ScriptedForge;
