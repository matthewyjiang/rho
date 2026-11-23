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
//! - [`poller`]: one poll pass, and the one door to the forge.
//! - [`inbox`]: [`PullRequestInbox`], one session's side of the service.
//!
//! Branch lookups and the parts of [`model::PrDetail`] no watch reads serve
//! the coming rho-ui protocol step, which shows linked PRs to clients.

mod gh;
mod gh_query;
mod inbox;
mod model;
mod news;
mod poller;
mod record;
mod service;
mod state;
mod store;
#[cfg(test)]
mod test_forge;
mod watch;

// The surface the daemon's wiring and the PR tools consume.
pub(crate) use self::{
    gh::{Forge, GhCli},
    inbox::{
        CheckCounts, LastRead, Linked, ListedPullRequest, NewsBatch, PullRequestError,
        PullRequestInbox, Unlinked, Unwatched, Watched,
    },
    model::{Mergeable, PrState, Rollup},
    record::LinkOrigin,
    service::{unix_ms, PullRequests},
    state::Gate,
};
#[cfg(test)]
pub(crate) use test_forge::ScriptedForge;
