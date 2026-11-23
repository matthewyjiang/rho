//! What reads of the forge learned ([`Reads`]): of each pull request, of
//! each branch, and of each repository's default branch. [`read_into`]
//! sends a plan through the poller's [`Reader`] in as many queries as it
//! takes, and [`keep`] keeps what came back in the service's state.

use std::collections::{HashMap, HashSet};

use super::{
    gh::ReadError,
    gh_query,
    model::{Batch, BranchId, FetchPlan, Found, PrDetail, PrId, PrSummary, RepoId},
    poller::Reader,
    service::{DefaultBranch, State, Trouble},
};

/// What one read learned of one pull request.
pub(super) enum Read {
    /// Its summary only: it did not change, its detail could not follow,
    /// or a branch lookup found it.
    Summary(PrSummary),
    /// Boxed: a detail is several times the size of the other reads.
    Detail(Box<PrDetail>),
    /// The forge answered without it: no such PR, or one the account can't
    /// see, in the forge's words.
    Refused(String),
    Failed(ReadError),
}

/// What reads learned: of each pull request, of each branch, and of each
/// repository's default branch.
#[derive(Default)]
pub(super) struct Reads {
    pub(super) prs: HashMap<PrId, Read>,
    pub(super) branches: HashMap<BranchId, Result<Found, Trouble>>,
    pub(super) default_branches: HashMap<RepoId, Option<String>>,
}

impl Reads {
    /// Adds what a later read learned, which replaces what this one says,
    /// except that a later summary of a PR never replaces its earlier
    /// detail: on a host split into several queries, a branch lookup in a
    /// later query can find a PR whose detail an earlier one read, and the
    /// tick evaluates watches only against a detail.
    fn extend(&mut self, later: Self) {
        for (pr, read) in later.prs {
            let earlier_detail = matches!(self.prs.get(&pr), Some(Read::Detail(_)));
            let replaces = match read {
                Read::Summary(_) => !earlier_detail,
                Read::Detail(_) | Read::Refused(_) | Read::Failed(_) => true,
            };
            if replaces {
                self.prs.insert(pr, read);
            }
        }
        self.branches.extend(later.branches);
        self.default_branches.extend(later.default_branches);
    }
}

/// Reads `plan` from `host` into `reads`, in as many queries as
/// [`gh_query::split`] makes of it, while the host's budget allows.
pub(super) async fn read_into(
    reader: &mut Reader,
    host: &str,
    plan: FetchPlan,
    now: u64,
    reads: &mut Reads,
) {
    let plans = gh_query::split(plan);
    if plans.len() > 1 {
        tracing::info!(
            host,
            queries = plans.len(),
            "reading this host's pull requests takes more than one query"
        );
    }
    for plan in plans {
        if reader.paused(host, now).is_some() {
            return;
        }
        match reader.read(host, &plan).await {
            Ok(batch) => reads.extend(answer(&plan, batch)),
            Err(error) => {
                tracing::warn!(host, %error, "could not read pull requests");
                for branch in &plan.branches {
                    let failed = Err(Trouble::Failed(error.clone()));
                    reads.branches.insert(branch.clone(), failed);
                }
                for pr in plan.links.iter().chain(&plan.details) {
                    reads.prs.insert(pr.clone(), Read::Failed(error.clone()));
                }
            }
        }
    }
}

/// Keeps what `reads` learned at `now` in `state`, each PR and branch
/// counted as asked at `now`.
pub(super) fn keep(state: &mut State, reads: &Reads, now: u64) {
    for (repo, name) in &reads.default_branches {
        let observed = DefaultBranch {
            name: name.clone(),
            at: now,
        };
        state.default_branches.insert(repo.clone(), observed);
    }
    for (branch, answered) in &reads.branches {
        let read = state.branches.entry(branch.clone()).or_default();
        read.asked_at = Some(now);
        match answered {
            Ok(found) => {
                read.found = Some(found.clone());
                read.trouble = None;
            }
            Err(trouble) => read.trouble = Some(trouble.clone()),
        }
    }
    for (pr, read) in &reads.prs {
        let snapshot = state.snapshots.entry(pr.clone()).or_default();
        snapshot.asked_at = Some(now);
        match read {
            Read::Summary(summary) => snapshot.saw_summary(summary.clone(), now),
            Read::Detail(detail) => snapshot.saw_detail(PrDetail::clone(detail), now),
            Read::Refused(why) => snapshot.trouble = Some(Trouble::Refused(why.clone())),
            Read::Failed(error) => snapshot.trouble = Some(Trouble::Failed(error.clone())),
        }
    }
}

/// What `batch` says of each branch and PR `plan` asked for. A branch's PR
/// reads as that PR's summary too. A branch or PR the answer left out
/// entirely reads as a failed read, which keeps a watch's refusal count
/// where it is.
fn answer(plan: &FetchPlan, batch: Batch) -> Reads {
    let Batch {
        budget: _,
        default_branches,
        branches,
        links,
        details,
        missing,
    } = batch;
    let refused = |repo: &RepoId| {
        missing
            .iter()
            .find(|(missing, _)| missing == repo)
            .map(|(_, why)| why.clone())
    };
    let mut reads = Reads {
        default_branches: default_branches.into_iter().collect(),
        ..Reads::default()
    };
    let mut found: HashMap<BranchId, Found> = branches.into_iter().collect();
    for branch in &plan.branches {
        let answered = found
            .remove(branch)
            .ok_or_else(|| match refused(&branch.repo) {
                Some(why) => Trouble::Refused(why),
                None => Trouble::Failed(ReadError::Failed(format!(
                    "GitHub's answer left out the branch {}",
                    branch.branch
                ))),
            });
        if let Ok(Found::Pr(summary)) = &answered {
            reads
                .prs
                .insert(summary.id.clone(), Read::Summary(summary.clone()));
        }
        reads.branches.insert(branch.clone(), answered);
    }
    let mut answered: HashMap<PrId, Read> = HashMap::new();
    for (pr, found) in links {
        let read = match found {
            Found::Pr(summary) => Read::Summary(summary),
            Found::None | Found::OnlyForks(_) => Read::Refused(no_such(&pr)),
        };
        answered.insert(pr, read);
    }
    for (pr, detail) in details {
        let read = detail.map_or_else(
            || Read::Refused(no_such(&pr)),
            |detail| Read::Detail(Box::new(detail)),
        );
        answered.insert(pr, read);
    }
    let mut asked = HashSet::new();
    for pr in plan.links.iter().chain(&plan.details) {
        if !asked.insert(pr) {
            continue;
        }
        let read = answered
            .remove(pr)
            .unwrap_or_else(|| match refused(&pr.repo) {
                Some(why) => Read::Refused(why),
                None => Read::Failed(ReadError::Failed(format!("GitHub's answer left out {pr}"))),
            });
        reads.prs.insert(pr.clone(), read);
    }
    reads
}

fn no_such(pr: &PrId) -> String {
    format!("GitHub shows this account no pull request {pr}")
}

#[cfg(test)]
#[path = "reads_tests.rs"]
mod tests;
