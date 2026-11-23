//! The poller: one pass over what is due ([`Shared::tick`]), the reads a
//! watch's baseline and a client's show make themselves
//! ([`Shared::read_detail`], [`Shared::read_branch`]), and the one door to
//! the forge ([`Reader`]), which all of them go through, one read at a
//! time.
//!
//! A tick reads each host once for what [`due`] says is due: branch
//! lookups, cheap summaries, and the full detail of those that need it. A
//! watched or shown PR whose summary shows it changed since its last detail
//! gets its detail in a second query. Every watch is then evaluated against
//! what the tick read, and only then are sessions told, so one wake carries
//! all of a session's news from the tick.
//!
//! With no client asking, a PR linked and not watched is kept, and not
//! read.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use rho_sdk::SessionId;
use tokio::sync::Notify;

use super::{
    due::{self, Due},
    gh::{Forge, ReadError},
    gh_query,
    model::{Batch, BranchId, Budget, FetchPlan, Found, PrDetail, PrId, PrSummary, RepoId},
    record::{SessionRecord, NEWS_PER_SESSION},
    service::{Detailed, Shared, State, Trouble},
    state::{CheckTally, News},
    watch::{self, Report},
};

/// How often the poller reads watched pull requests: a cadence, not a cap.
///
/// Receipt (2026-11-22, matthewyjiang/rho, gh 2.99.0): a query of up to 100
/// summaries and 3 details costs 1 point, so a tick a minute is 60 points an
/// hour, 1.2% of GitHub's 5,000. rho's own CI run took 21 min 40 s (rho-ui
/// `pull_requests.rs:32-34`), about 22 ticks. `list_session_pull_requests`
/// shows each PR's read age.
pub(crate) const WATCH_POLL: Duration = Duration::from_secs(60);

/// The rate-limit points rho leaves a host's account: at or below this, it
/// reads nothing from that host until the limit resets.
///
/// Receipt: rho-ui's `BUDGET_FLOOR` (`pull_requests.rs:43-47`), which keeps
/// points for the user's own `gh` and tools. At [`WATCH_POLL`] it holds back
/// 50 quiet ticks, and the dearest measured query (25 full details) costs
/// 10 points, so one tick cannot take the budget below 40. When it is hit,
/// `watch_pull_request` refuses with the remaining points, this floor and
/// the reset time, and the daemon logs one `warn` per pause.
pub(crate) const BUDGET_FLOOR: u32 = 50;

/// The most pull requests one session may watch at once.
///
/// Receipt: the largest batch measured, 25 full details in one query, cost
/// 10 points and took 4.9 s (812 KB), a twelfth of `gh::DEADLINE`; a
/// session's worst tick stays one measured query. A tripwire: a 26th watch
/// is refused at check time with this limit and the asked count.
pub(crate) const WATCHES_PER_SESSION: usize = 25;

/// A query's rate-limit cost above which the poller warns: the dearest
/// query measured, 25 full details, cost 10 points (2026-11-22). A cost
/// above it means the receipts above are wrong.
const DEAREST_MEASURED_COST: u32 = 10;

/// Each host's budget as its last answer told it. [`Reader`] is its one
/// writer; anything may read it without waiting on a read in flight.
#[derive(Debug, Default)]
pub(super) struct Budgets(Mutex<HashMap<String, Budget>>);

impl Budgets {
    /// `host`'s budget, when it is at or below [`BUDGET_FLOOR`] and has not
    /// reset by `now` (unix milliseconds): reads of that host wait.
    pub(super) fn paused(&self, host: &str, now: u64) -> Option<Budget> {
        let budgets = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let budget = budgets.get(host)?;
        let now = i64::try_from(now / 1000).unwrap_or(i64::MAX);
        (budget.remaining <= BUDGET_FLOOR && now < budget.resets_at).then(|| budget.clone())
    }

    fn keep(&self, host: &str, budget: Budget) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(host.to_owned(), budget);
    }
}

/// The only way to the forge. It lives behind the service's async lock, so
/// holding `&mut Reader` is what lets a caller read: reads never overlap,
/// and each passes the host's budget guard.
pub(super) struct Reader {
    forge: Arc<dyn Forge>,
    /// Shared with the service, which reads it for clients.
    budgets: Arc<Budgets>,
}

impl Reader {
    pub(super) fn new(forge: Arc<dyn Forge>, budgets: Arc<Budgets>) -> Self {
        Self { forge, budgets }
    }

    /// `host`'s budget while reads of it wait; see [`Budgets::paused`].
    pub(super) fn paused(&self, host: &str, now: u64) -> Option<Budget> {
        self.budgets.paused(host, now)
    }

    /// Asks `host` what `plan` asks, and keeps the budget it answers with.
    /// Callers check [`Self::paused`] first.
    pub(super) async fn read(&mut self, host: &str, plan: &FetchPlan) -> Result<Batch, ReadError> {
        let read = self.forge.read(host, plan).await;
        if let Ok(Batch {
            budget: Some(budget),
            ..
        }) = &read
        {
            self.keep_budget(host, budget);
        }
        read
    }

    fn keep_budget(&mut self, host: &str, budget: &Budget) {
        let Budget {
            remaining,
            resets_at,
            cost,
        } = *budget;
        match cost {
            Some(cost) if cost > DEAREST_MEASURED_COST => tracing::warn!(
                host,
                cost,
                remaining,
                dearest_measured = DEAREST_MEASURED_COST,
                "a pull request query cost more rate-limit points than rho measured for its dearest"
            ),
            cost => tracing::debug!(host, ?cost, remaining, "read pull requests"),
        }
        // Nothing reads the host again until the reset, so this is the one
        // warning of the pause.
        if remaining <= BUDGET_FLOOR {
            tracing::warn!(
                host,
                remaining,
                floor = BUDGET_FLOOR,
                resets_at,
                "pausing pull request reads on this host until its rate limit resets"
            );
        }
        self.budgets.keep(host, budget.clone());
    }
}

/// What one read learned of one pull request.
enum Read {
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
struct Reads {
    prs: HashMap<PrId, Read>,
    branches: HashMap<BranchId, Result<Found, Trouble>>,
    default_branches: HashMap<RepoId, Option<String>>,
}

impl Reads {
    /// Adds what a later read learned, which replaces what this one says.
    fn extend(&mut self, later: Self) {
        self.prs.extend(later.prs);
        self.branches.extend(later.branches);
        self.default_branches.extend(later.default_branches);
    }
}

/// Why [`Shared::read_detail`] has no detail. A watch refuses to start
/// with it (see `PullRequestInbox::watch`); a show keeps it as trouble.
pub(super) enum DetailError {
    /// The host's budget is at the floor; nothing was read.
    Paused(Budget),
    /// The forge answered without it, in its words.
    Refused(String),
    Failed(ReadError),
}

impl Shared {
    /// One poll pass at `now` (unix milliseconds); see the module docs.
    pub(super) async fn tick(&self, now: u64) {
        let mut reader = self.reader.lock().await;
        let hosts = due::due(&self.state(), now);
        let mut reads = Reads::default();
        for (host, Due { plan, wants_detail }) in hosts {
            if reader.paused(&host, now).is_some() {
                continue;
            }
            read_into(&mut reader, &host, plan, now, &mut reads).await;
            let moved = {
                let state = self.state();
                let mut moved: Vec<PrId> = reads
                    .prs
                    .iter()
                    .filter(|(pr, _)| pr.repo.host == host && wants_detail.contains(*pr))
                    .filter_map(|(pr, read)| match read {
                        Read::Summary(summary)
                            if due::moved_since_detail(state.snapshots.get(pr), summary) =>
                        {
                            Some(pr.clone())
                        }
                        Read::Summary(_) | Read::Detail(_) | Read::Refused(_) | Read::Failed(_) => {
                            None
                        }
                    })
                    .collect();
                moved.sort_by(due::by_number);
                moved
            };
            let plan = FetchPlan {
                details: moved,
                ..FetchPlan::default()
            };
            read_into(&mut reader, &host, plan, now, &mut reads).await;
        }
        drop(reader);
        let told = self.update(|state| self.apply(state, &reads, now));
        self.tell(told);
    }

    /// Reads `pr` in full at `now`, through the same lock and budget guard
    /// as a tick: a watch's baseline, or a client's show. What it reads, or
    /// why it could not, is kept as [`Self::keep_unjudged`] keeps it.
    pub(super) async fn read_detail(&self, pr: &PrId, now: u64) -> Result<PrDetail, DetailError> {
        let mut reader = self.reader.lock().await;
        let host = pr.repo.host.as_str();
        if let Some(budget) = reader.paused(host, now) {
            return Err(DetailError::Paused(budget));
        }
        let plan = FetchPlan {
            details: vec![pr.clone()],
            ..FetchPlan::default()
        };
        let mut reads = Reads::default();
        read_into(&mut reader, host, plan, now, &mut reads).await;
        drop(reader);
        let detail = match reads.prs.get(pr) {
            Some(Read::Detail(detail)) => Ok(PrDetail::clone(detail)),
            Some(Read::Refused(why)) => Err(DetailError::Refused(why.clone())),
            Some(Read::Failed(error)) => Err(DetailError::Failed(error.clone())),
            Some(Read::Summary(_)) | None => Err(DetailError::Failed(ReadError::Failed(format!(
                "GitHub's answer left out {pr}"
            )))),
        };
        self.keep_unjudged(reads, now);
        detail
    }

    /// Looks `branch` up at `now`, with the detail of the PR its last
    /// lookup found, in one query, and the detail of a PR it finds anew in
    /// a second: a client's show of a session's branch. Returns the detail
    /// it read of the PR the lookup found; what it could not read is kept
    /// as trouble, and a paused host reads nothing.
    pub(super) async fn read_branch(&self, branch: &BranchId, now: u64) -> Option<Detailed> {
        let mut reader = self.reader.lock().await;
        let host = branch.repo.host.as_str();
        if reader.paused(host, now).is_some() {
            return None;
        }
        let known = self.state().found(branch).cloned();
        let plan = FetchPlan {
            branches: vec![branch.clone()],
            links: Vec::new(),
            details: known.iter().cloned().collect(),
        };
        let mut reads = Reads::default();
        read_into(&mut reader, host, plan, now, &mut reads).await;
        let found = match reads.branches.get(branch) {
            Some(Ok(Found::Pr(summary))) => Some(summary.id.clone()),
            Some(Ok(Found::None | Found::OnlyForks(_)) | Err(_)) | None => None,
        };
        if let Some(pr) = found.as_ref().filter(|pr| Some(*pr) != known.as_ref()) {
            let plan = FetchPlan {
                details: vec![pr.clone()],
                ..FetchPlan::default()
            };
            read_into(&mut reader, host, plan, now, &mut reads).await;
        }
        drop(reader);
        let detail = found.and_then(|pr| match reads.prs.get(&pr) {
            Some(Read::Detail(detail)) => Some(Detailed {
                detail: PrDetail::clone(detail),
                at: now,
            }),
            Some(Read::Summary(_) | Read::Refused(_) | Read::Failed(_)) | None => None,
        });
        self.keep_unjudged(reads, now);
        detail
    }

    /// Keeps what a read outside a tick learned. A tick tells a watch's
    /// news by comparing the summary it reads with the last detail; a
    /// detail read here was never evaluated against the PR's watches, and
    /// kept, it would hide what changed since. So of a watched PR only the
    /// summary is kept, and it is not counted as asked: the next tick due
    /// for the watch still reads the detail and evaluates it, however
    /// often clients read it fresh.
    fn keep_unjudged(&self, mut reads: Reads, now: u64) {
        self.update(|state| {
            let unjudged: Vec<PrId> = reads
                .prs
                .iter()
                .filter(|(pr, read)| match read {
                    Read::Detail(_) => state.watched(pr),
                    Read::Summary(_) | Read::Refused(_) | Read::Failed(_) => false,
                })
                .map(|(pr, _)| pr.clone())
                .collect();
            for pr in unjudged {
                if let Some(Read::Detail(detail)) = reads.prs.remove(&pr) {
                    let snapshot = state.snapshots.entry(pr).or_default();
                    snapshot.saw_summary(detail.summary, now);
                }
            }
            keep(state, &reads, now);
        });
    }

    /// Keeps what the tick read in `state`, evaluates every watch against
    /// it, and queues and saves each session's news. Returns the sessions
    /// to tell.
    fn apply(&self, state: &mut State, reads: &Reads, now: u64) -> Vec<(SessionId, Arc<Notify>)> {
        keep(state, reads, now);
        let shown = due::shown(state, now);
        let State {
            sessions,
            snapshots,
            interest,
            branches,
            default_branches: _,
        } = state;
        let mut told = Vec::new();
        for (session, entry) in sessions.iter_mut() {
            let (changed, news) = evaluate(&mut entry.record, &reads.prs, now);
            let has_news = !news.is_empty();
            for item in news {
                entry
                    .record
                    .queue_news(item, NEWS_PER_SESSION, &entry.in_flight);
            }
            if changed || has_news {
                // A failed save is logged; the news stays queued in memory.
                let _ = self.save(session, &entry.record);
            }
            if has_news {
                told.push((session.clone(), Arc::clone(&entry.notify)));
            }
        }
        // Forget the sessions that only ever asked for news, the branches
        // no session has checked out, what no session links and no lookup
        // found, and the details no session watches and no client shows.
        sessions.retain(|_, entry| !entry.disposable());
        branches.retain(|branch, _| {
            interest
                .sessions()
                .any(|(_, looked)| looked.checkout().branch() == Some(branch))
        });
        let found: HashSet<&PrId> = branches
            .values()
            .filter_map(|read| match &read.found {
                Some(Found::Pr(summary)) => Some(&summary.id),
                Some(Found::None | Found::OnlyForks(_)) | None => None,
            })
            .collect();
        snapshots.retain(|pr, snapshot| {
            let links = sessions.values().flat_map(|entry| entry.record.links());
            let (linked, watched) = links
                .filter(|link| link.pr == *pr)
                .fold((false, false), |(_, watched), link| {
                    (true, watched || link.watch.is_some())
                });
            if !linked && !found.contains(pr) {
                return false;
            }
            if !watched && !shown.contains(pr) {
                snapshot.detail = None;
            }
            true
        });
        told
    }
}

/// Keeps what `reads` learned at `now` in `state`.
fn keep(state: &mut State, reads: &Reads, now: u64) {
    for (repo, default) in &reads.default_branches {
        state.default_branches.insert(repo.clone(), default.clone());
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

/// Evaluates each watch of `record` that this tick read, and ends those
/// that ended: their link stays. Returns whether any watch changed, and
/// the news to queue, in link order.
fn evaluate(
    record: &mut SessionRecord,
    reads: &HashMap<PrId, Read>,
    now: u64,
) -> (bool, Vec<News>) {
    let watched: Vec<PrId> = record.watching().map(|(link, _)| link.pr.clone()).collect();
    let mut changed = false;
    let mut news = Vec::new();
    for pr in watched {
        let Some(read) = reads.get(&pr) else {
            continue;
        };
        let Some(link) = record.link_mut(&pr) else {
            continue;
        };
        let Some(watch) = link.watch.as_ref() else {
            continue;
        };
        let Report { changes, next, end } = match read {
            // Nothing changed since its last detail.
            Read::Summary(_) => continue,
            Read::Detail(detail) => watch::evaluate(watch, detail, now),
            Read::Refused(why) => watch::refused_read(watch, why),
            Read::Failed(error) => watch::failed_read(watch, error),
        };
        changed |= end.is_some() || link.watch.as_ref() != Some(&next);
        let head = next.head_oid.clone();
        link.watch = if end.is_some() { None } else { Some(next) };
        if !changes.is_empty() || end.is_some() {
            let pushed = link
                .told_head
                .as_ref()
                .is_some_and(|told| Some(told) != head.as_ref());
            link.told_head.clone_from(&head);
            let checks = match read {
                Read::Detail(detail) => Some(CheckTally::of(detail)),
                Read::Summary(_) | Read::Refused(_) | Read::Failed(_) => None,
            };
            news.push(News {
                // Set when queued.
                seq: 0,
                pr,
                url: link.url.clone(),
                head,
                changes,
                end,
                at: now,
                pushed,
                checks,
            });
        }
    }
    (changed, news)
}

/// Reads `plan` from `host` into `reads`, in as many queries as
/// [`gh_query::split`] makes of it, while the host's budget allows.
async fn read_into(reader: &mut Reader, host: &str, plan: FetchPlan, now: u64, reads: &mut Reads) {
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
#[path = "poller_tests.rs"]
mod tests;
