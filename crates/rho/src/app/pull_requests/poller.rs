//! The poller: one pass over what is due ([`Shared::tick`]), the reads a
//! watch's baseline and a client's show make themselves
//! ([`Shared::read_detail`], [`Shared::read_branch`]), and the one door to
//! the forge ([`Reader`]), which all of them go through, one read at a
//! time.
//!
//! A tick reads each host once for what [`due`] says is due: branch
//! lookups, cheap summaries, and the full detail of those that need it. A
//! watched or shown PR whose summary shows it changed since its last detail,
//! the PR a shown branch's lookup found included, gets its detail in a
//! second query. Every watch is then evaluated against what the tick read,
//! and only then are sessions told, so one wake carries all of a session's
//! news from the tick.
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
    model::{Batch, BranchId, Budget, FetchPlan, Found, PrDetail, PrId},
    reads::{keep, read_into, Read, Reads},
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
        for (host, due) in hosts {
            let Due {
                plan,
                mut wants_detail,
                shown_branches,
            } = due;
            if reader.paused(&host, now).is_some() {
                continue;
            }
            read_into(&mut reader, &host, plan, now, &mut reads).await;
            // The PR a shown branch's lookup found wants its detail as a
            // shown PR does, though the planner may not have known it.
            wants_detail.extend(shown_branches.iter().filter_map(|branch| {
                match reads.branches.get(branch)? {
                    Ok(Found::Pr(summary)) => Some(summary.id.clone()),
                    Ok(Found::None | Found::OnlyForks(_)) | Err(_) => None,
                }
            }));
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
        // Applied before `reader` is released, so a show's read that waited
        // on it lands after, and this tick's older reads never overwrite it.
        let told = self.update(|state| self.apply(state, &reads, now));
        drop(reader);
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
        let detail = match reads.prs.get(pr) {
            Some(Read::Detail(detail)) => Ok(PrDetail::clone(detail)),
            Some(Read::Refused(why)) => Err(DetailError::Refused(why.clone())),
            Some(Read::Failed(error)) => Err(DetailError::Failed(error.clone())),
            Some(Read::Summary(_)) | None => Err(DetailError::Failed(ReadError::Failed(format!(
                "GitHub's answer left out {pr}"
            )))),
        };
        self.keep_unjudged(reads, now);
        drop(reader);
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
        let detail = found.and_then(|pr| match reads.prs.get(&pr) {
            Some(Read::Detail(detail)) => Some(Detailed {
                detail: PrDetail::clone(detail),
                at: now,
            }),
            Some(Read::Summary(_) | Read::Refused(_) | Read::Failed(_)) | None => None,
        });
        self.keep_unjudged(reads, now);
        drop(reader);
        detail
    }

    /// Keeps what a read outside a tick learned. A tick tells a watch's
    /// news by comparing the summary it reads with the last detail; a
    /// detail read here was never evaluated against the PR's watches, and
    /// kept as that detail, it would hide what changed since. So a watched
    /// PR's detail is kept apart, for shows to answer with (see
    /// [`super::service::Snapshot::newest_detail`]), and no read of a
    /// watched PR here is counted as asked, a failed one included: the next
    /// tick due for the watch still reads the PR and evaluates it, however
    /// often clients read it. A failure is kept as trouble for the client,
    /// with when it failed apart from the tick's ask: a cached show retries
    /// it only once the shown cadence allows since then (see
    /// [`due::show_reads`]), so at most once per show a client sends at its
    /// cadence. Call it holding `reader`, so a tick cannot read in between.
    fn keep_unjudged(&self, mut reads: Reads, now: u64) {
        self.update(|state| {
            let watched: Vec<PrId> = reads
                .prs
                .keys()
                .filter(|pr| state.watched(pr))
                .cloned()
                .collect();
            for pr in watched {
                let Some(read) = reads.prs.remove(&pr) else {
                    continue;
                };
                let snapshot = state.snapshots.entry(pr).or_default();
                match read {
                    Read::Summary(summary) => snapshot.saw_summary(summary, now),
                    Read::Detail(detail) => snapshot.saw_unjudged(*detail, now),
                    Read::Refused(why) => snapshot.show_failed(Trouble::Refused(why), now),
                    Read::Failed(error) => snapshot.show_failed(Trouble::Failed(error), now),
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
        // Each linked PR, and whether any session watches it.
        let mut linked: HashMap<&PrId, bool> = HashMap::new();
        for link in sessions.values().flat_map(|entry| entry.record.links()) {
            *linked.entry(&link.pr).or_default() |= link.watch.is_some();
        }
        snapshots.retain(|pr, snapshot| {
            let watched = linked.get(pr).copied();
            if watched.is_none() && !found.contains(pr) {
                return false;
            }
            if watched != Some(true) && !shown.contains(pr) {
                snapshot.forget_details();
            }
            true
        });
        told
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

#[cfg(test)]
#[path = "poller_tests.rs"]
mod tests;
