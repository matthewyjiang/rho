//! The poller: one pass over every watched pull request ([`Shared::tick`]),
//! and the one door to the forge ([`Reader`]), which a tick and a watch's
//! baseline read both go through, one read at a time.
//!
//! A tick reads each host once for the cheap summaries of its watched PRs,
//! plus the full detail of those a watch needs now (see [`detail_due`]).
//! A PR whose summary shows it changed since its last detail gets its
//! detail in a second query. Every watch is then evaluated against what
//! the tick read, and only then are sessions told, so one wake carries all
//! of a session's news from the tick.
//!
//! Step 1 reads watched PRs only: a PR linked and not watched is kept, and
//! not read.

use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
    time::Duration,
};

use rho_sdk::SessionId;
use tokio::sync::Notify;

use super::{
    gh::{Forge, ReadError},
    gh_query,
    inbox::PullRequestError,
    model::{Batch, Budget, FetchPlan, Found, PrDetail, PrId, PrSummary, Rollup},
    record::{SessionRecord, NEWS_PER_SESSION},
    service::{Shared, Snapshot, State},
    state::{Baseline, CheckTally, News},
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

/// How old a watched PR's last full read may get before a tick reads it in
/// full again although its summary did not change.
///
/// Receipt: t3's `QUIET_REREAD_MS` (`PullRequestWatchReactor.ts:53`). The
/// summary's change key misses a base-branch push that makes the PR
/// conflict, and an edit to branch protection; this bounds how late such a
/// wake can be. 6 details an hour per quiet watch; 25 watches are 150
/// details, about 60 points an hour.
pub(crate) const QUIET_REREAD: Duration = Duration::from_secs(10 * 60);

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

/// The only way to the forge. It lives behind the service's async lock, so
/// holding `&mut Reader` is what lets a caller read: reads never overlap,
/// and each passes the host's budget guard.
pub(super) struct Reader {
    forge: Arc<dyn Forge>,
    /// Each host's budget as its last answer told it.
    budgets: HashMap<String, Budget>,
}

impl Reader {
    pub(super) fn new(forge: Arc<dyn Forge>) -> Self {
        Self {
            forge,
            budgets: HashMap::new(),
        }
    }

    /// `host`'s budget, when it is at or below [`BUDGET_FLOOR`] and has not
    /// reset by `now` (unix milliseconds): reads of that host wait.
    pub(super) fn paused(&self, host: &str, now: u64) -> Option<&Budget> {
        let budget = self.budgets.get(host)?;
        let now = i64::try_from(now / 1000).unwrap_or(i64::MAX);
        (budget.remaining <= BUDGET_FLOOR && now < budget.resets_at).then_some(budget)
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
        self.budgets.insert(host.to_owned(), budget.clone());
    }
}

/// What one tick learned of one pull request.
enum Read {
    /// Its summary only: it did not change, or its detail could not follow.
    Summary(PrSummary),
    /// Boxed: a detail is several times the size of the other reads.
    Detail(Box<PrDetail>),
    /// The forge answered without it: no such PR, or one the account can't
    /// see, in the forge's words.
    Refused(String),
    Failed(ReadError),
}

/// A watched pull request a tick reads.
struct Tracked {
    pr: PrId,
    /// Whether to read its detail without waiting for its summary.
    detail_due: bool,
}

impl Shared {
    /// One poll pass at `now` (unix milliseconds); see the module docs.
    pub(super) async fn tick(&self, now: u64) {
        let mut reader = self.reader.lock().await;
        let tracked = tracked(&self.state(), now);
        let mut reads = HashMap::new();
        for (host, prs) in tracked {
            if reader.paused(&host, now).is_some() {
                continue;
            }
            let (details, links): (Vec<Tracked>, Vec<Tracked>) =
                prs.into_iter().partition(|tracked| tracked.detail_due);
            let plan = FetchPlan {
                branches: Vec::new(),
                links: links.into_iter().map(|tracked| tracked.pr).collect(),
                details: details.into_iter().map(|tracked| tracked.pr).collect(),
            };
            read_into(&mut reader, &host, plan, now, &mut reads).await;
            let moved = {
                let state = self.state();
                let mut moved: Vec<PrId> = reads
                    .iter()
                    .filter(|(pr, _)| pr.repo.host == host)
                    .filter_map(|(pr, read)| match read {
                        Read::Summary(summary)
                            if moved_since_detail(state.snapshots.get(pr), summary) =>
                        {
                            Some(pr.clone())
                        }
                        Read::Summary(_) | Read::Detail(_) | Read::Refused(_) | Read::Failed(_) => {
                            None
                        }
                    })
                    .collect();
                moved.sort_by(by_number);
                moved
            };
            let plan = FetchPlan {
                details: moved,
                ..FetchPlan::default()
            };
            read_into(&mut reader, &host, plan, now, &mut reads).await;
        }
        drop(reader);
        let told = self.apply(&reads, now);
        self.tell(told);
    }

    /// The baseline read of `pr` for a new watch at `now`, through the
    /// same lock and budget guard as a tick.
    pub(super) async fn read_baseline(
        &self,
        pr: &PrId,
        now: u64,
    ) -> Result<PrDetail, PullRequestError> {
        let mut reader = self.reader.lock().await;
        let host = pr.repo.host.as_str();
        if let Some(budget) = reader.paused(host, now) {
            return Err(PullRequestError::Budget {
                remaining: budget.remaining,
                floor: BUDGET_FLOOR,
                resets_at: budget.resets_at,
            });
        }
        let plan = FetchPlan {
            details: vec![pr.clone()],
            ..FetchPlan::default()
        };
        let batch = reader
            .read(host, &plan)
            .await
            .map_err(|error| match error {
                ReadError::NotInstalled(_) => PullRequestError::NotInstalled,
                ReadError::SignedOut(_) => PullRequestError::SignedOut {
                    host: host.to_owned(),
                },
                ReadError::Failed(why) => PullRequestError::Read(why),
            })?;
        drop(reader);
        let detail = match answer(&plan, batch).pop() {
            Some((_, Read::Detail(detail))) => *detail,
            Some((_, Read::Refused(why))) => return Err(PullRequestError::NotFound(why)),
            Some((_, Read::Failed(error))) => {
                return Err(PullRequestError::Read(error.to_string()))
            }
            Some((_, Read::Summary(_))) | None => {
                return Err(PullRequestError::Read(format!(
                    "GitHub's answer left out {pr}"
                )))
            }
        };
        let mut state = self.state();
        let watched = state
            .sessions
            .values()
            .any(|entry| entry.record.watching().any(|(link, _)| link.pr == *pr));
        let snapshot = state.snapshots.entry(pr.clone()).or_default();
        if watched {
            // A tick tells a watch's news by comparing the summary it reads
            // with the last detail. This detail was never evaluated against
            // that watch: kept, it would hide what changed since.
            snapshot.saw_summary(detail.summary.clone(), now);
        } else {
            snapshot.saw_detail(detail.clone(), now);
        }
        Ok(detail)
    }

    /// Keeps what the tick read, evaluates every watch against it, and
    /// queues and saves each session's news. Returns the sessions to tell.
    fn apply(&self, reads: &HashMap<PrId, Read>, now: u64) -> Vec<(SessionId, Arc<Notify>)> {
        let mut state = self.state();
        let State {
            sessions,
            snapshots,
        } = &mut *state;
        for (pr, read) in reads {
            let snapshot = snapshots.entry(pr.clone()).or_default();
            match read {
                Read::Summary(summary) => snapshot.saw_summary(summary.clone(), now),
                Read::Detail(detail) => snapshot.saw_detail(PrDetail::clone(detail), now),
                Read::Refused(why) => snapshot.trouble = Some(why.clone()),
                Read::Failed(error) => snapshot.trouble = Some(error.to_string()),
            }
        }
        let mut told = Vec::new();
        for (session, entry) in sessions.iter_mut() {
            let (changed, news) = evaluate(&mut entry.record, reads, now);
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
        // Forget the sessions that only ever asked for news, and what no
        // session links any more.
        sessions.retain(|_, entry| !entry.disposable());
        snapshots.retain(|pr, _| {
            sessions
                .values()
                .any(|entry| entry.record.links().iter().any(|link| link.pr == *pr))
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

/// Every watched pull request by host, each once however many sessions
/// watch it, in a stable order.
fn tracked(state: &State, now: u64) -> BTreeMap<String, Vec<Tracked>> {
    let mut pending: HashMap<&PrId, bool> = HashMap::new();
    for entry in state.sessions.values() {
        for (link, watch) in entry.record.watching() {
            *pending.entry(&link.pr).or_default() |= watch.baseline == Baseline::Pending;
        }
    }
    let mut hosts: BTreeMap<String, Vec<Tracked>> = BTreeMap::new();
    for (pr, pending) in pending {
        hosts
            .entry(pr.repo.host.clone())
            .or_default()
            .push(Tracked {
                pr: pr.clone(),
                detail_due: pending || detail_due(state.snapshots.get(pr), now),
            });
    }
    for prs in hosts.values_mut() {
        prs.sort_by(|a, b| by_number(&a.pr, &b.pr));
    }
    hosts
}

/// Whether a watched PR needs its detail at `now` whatever its summary
/// says: it has none yet, its checks were running at the last read, or
/// its last detail is [`QUIET_REREAD`] old.
///
/// Running checks come from the last detail: GitHub's rollup reads failing
/// as soon as one check fails, while others still run, and checks past
/// those the detail lists may be running too. A pending rollup in a newer
/// summary counts as well.
fn detail_due(snapshot: Option<&Snapshot>, now: u64) -> bool {
    let Some(snapshot) = snapshot else {
        return true;
    };
    let Some(detailed) = &snapshot.detail else {
        return true;
    };
    let detail = &detailed.detail;
    let running = detail.unread_checks() > 0
        || detail
            .checks
            .iter()
            .any(|check| watch::running(check.status))
        || snapshot
            .summary
            .as_ref()
            .is_some_and(|summary| summary.change.rollup == Rollup::Pending);
    running || Duration::from_millis(now.saturating_sub(detailed.at)) >= QUIET_REREAD
}

/// Whether `summary` differs from the PR's last detail, so its detail
/// would read differently now.
fn moved_since_detail(snapshot: Option<&Snapshot>, summary: &PrSummary) -> bool {
    snapshot
        .and_then(|snapshot| snapshot.detail.as_ref())
        .is_none_or(|detailed| {
            let last = &detailed.detail.summary;
            last.change != summary.change || last.state != summary.state
        })
}

/// Reads `plan` from `host` into `reads`, in as many queries as
/// [`gh_query::split`] makes of it, while the host's budget allows.
async fn read_into(
    reader: &mut Reader,
    host: &str,
    plan: FetchPlan,
    now: u64,
    reads: &mut HashMap<PrId, Read>,
) {
    let plans = gh_query::split(plan);
    if plans.len() > 1 {
        tracing::info!(
            host,
            queries = plans.len(),
            "reading this host's watched pull requests takes more than one query"
        );
    }
    for plan in plans {
        if reader.paused(host, now).is_some() {
            return;
        }
        match reader.read(host, &plan).await {
            Ok(batch) => reads.extend(answer(&plan, batch)),
            Err(error) => {
                tracing::warn!(host, %error, "could not read watched pull requests");
                let asked = plan.links.iter().chain(&plan.details);
                reads.extend(asked.map(|pr| (pr.clone(), Read::Failed(error.clone()))));
            }
        }
    }
}

/// What `batch` says of each PR `plan` asked for, in the plan's order. A
/// PR the answer left out entirely reads as a failed read, which keeps
/// the watch's refusal count where it is.
fn answer(plan: &FetchPlan, batch: Batch) -> Vec<(PrId, Read)> {
    let Batch {
        links,
        details,
        missing,
        ..
    } = batch;
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
    plan.links
        .iter()
        .chain(&plan.details)
        .map(|pr| {
            let read = answered.remove(pr).unwrap_or_else(|| {
                match missing.iter().find(|(repo, _)| *repo == pr.repo) {
                    Some((_, why)) => Read::Refused(why.clone()),
                    None => {
                        Read::Failed(ReadError::Failed(format!("GitHub's answer left out {pr}")))
                    }
                }
            });
            (pr.clone(), read)
        })
        .collect()
}

fn no_such(pr: &PrId) -> String {
    format!("GitHub shows this account no pull request {pr}")
}

/// Orders PRs by repository, then number.
fn by_number(a: &PrId, b: &PrId) -> std::cmp::Ordering {
    (&a.repo.owner, &a.repo.name, a.number).cmp(&(&b.repo.owner, &b.repo.name, b.number))
}

#[cfg(test)]
#[path = "poller_tests.rs"]
mod tests;
