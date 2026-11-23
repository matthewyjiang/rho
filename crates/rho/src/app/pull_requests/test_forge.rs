//! [`ScriptedForge`], the [`Forge`] the PR service's tests read from.

use std::{
    collections::{HashMap, VecDeque},
    future::{self, Future},
    pin::Pin,
    sync::{Arc, Mutex, PoisonError},
};

use super::{
    gh::{Forge, ReadError},
    model::{
        Batch, BranchId, Budget, ChangeKey, CheckRun, CheckStatus, FetchPlan, Found, MergeMethod,
        Mergeable, PrDetail, PrId, PrState, PrSummary, RepoId, Review, Rollup,
    },
};

/// The default branch of every repository a [`ScriptedForge`] answers for.
pub(crate) const DEFAULT_BRANCH: &str = "main";

/// A forge that answers every read from the pull requests a test put in it
/// ([`Self::set`]) and the branches it gave them ([`Self::set_branch`]),
/// the way GitHub would answer that plan, and records each plan it was
/// asked. Every repository's default branch is [`DEFAULT_BRANCH`]. A
/// failure queued with [`Self::fail_next`] answers the next read instead.
#[derive(Default)]
pub(crate) struct ScriptedForge {
    world: Mutex<World>,
    asked: Mutex<Vec<FetchPlan>>,
}

#[derive(Default)]
struct World {
    prs: HashMap<PrId, PrDetail>,
    /// The PR each branch has.
    branches: HashMap<BranchId, PrId>,
    failures: VecDeque<ReadError>,
    budget: Option<Budget>,
}

impl ScriptedForge {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// The PR now reads as `detail`, its summary included.
    pub(crate) fn set(&self, detail: PrDetail) {
        self.world().prs.insert(detail.summary.id.clone(), detail);
    }

    /// Pull request `target` (`owner/name#N`) now reads open on one head,
    /// merging cleanly, with checks `running` still running and `failed`
    /// failed. For tests outside this module, which cannot name a
    /// [`PrDetail`]. Its change key moves with each failure, as GitHub's
    /// does.
    pub(crate) fn set_open(&self, target: &str, running: &[&str], failed: &[&str]) {
        let id = PrId::parse(target, None).expect("an owner/name#N target");
        let check = |name: &str, status: CheckStatus| CheckRun {
            name: name.to_owned(),
            workflow: None,
            status,
            url: Some(format!("https://ci.example/{name}")),
            required: None,
        };
        let checks: Vec<CheckRun> = failed
            .iter()
            .map(|name| check(name, CheckStatus::Failed))
            .chain(running.iter().map(|name| check(name, CheckStatus::Running)))
            .collect();
        let rollup = match (failed.is_empty(), running.is_empty()) {
            (false, _) => Rollup::Failing,
            (true, false) => Rollup::Pending,
            (true, true) => Rollup::None,
        };
        let url = id.url();
        self.set(PrDetail {
            summary: PrSummary {
                id,
                title: "Widgets".into(),
                url,
                state: PrState::Open,
                change: ChangeKey {
                    updated_at: format!("2026-11-22T00:{:02}:00Z", failed.len()),
                    head_oid: "1a2b3c4d5e6f".into(),
                    rollup,
                },
            },
            author: "me".into(),
            head: "feature".into(),
            base: "main".into(),
            body: String::new(),
            additions: 1,
            deletions: 1,
            changed_files: 1,
            review: Review::None,
            checks_total: u32::try_from(checks.len()).expect("a few checks"),
            checks,
            posts: Vec::new(),
            older_posts: 0,
            threads: Vec::new(),
            older_threads: 0,
            mergeable: Mergeable::Yes,
            merge_methods: vec![MergeMethod::Squash],
            viewer: Some("me".into()),
            merged_by: None,
            merge_commit: None,
        });
    }

    /// Branch `branch` of pull request `target`'s (`owner/name#N`)
    /// repository now has that PR, which reads as [`Self::set`] or
    /// [`Self::set_open`] put it.
    pub(crate) fn set_branch(&self, target: &str, branch: &str) {
        let id = PrId::parse(target, None).expect("an owner/name#N target");
        let branch = BranchId {
            repo: id.repo.clone(),
            branch: branch.to_owned(),
        };
        self.world().branches.insert(branch, id);
    }

    /// The next read fails with `error`.
    pub(crate) fn fail_next(&self, error: ReadError) {
        self.world().failures.push_back(error);
    }

    /// Every answer from now on tells this budget.
    pub(crate) fn budget(&self, budget: Budget) {
        self.world().budget = Some(budget);
    }

    /// The plans asked since the last call, oldest first.
    pub(crate) fn take_asked(&self) -> Vec<FetchPlan> {
        std::mem::take(&mut *self.asked.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn world(&self) -> std::sync::MutexGuard<'_, World> {
        self.world.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn answer(&self, plan: &FetchPlan) -> Result<Batch, ReadError> {
        self.asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(plan.clone());
        let mut world = self.world();
        if let Some(error) = world.failures.pop_front() {
            return Err(error);
        }
        let summary = |pr: &PrId| {
            world
                .prs
                .get(pr)
                .map_or(Found::None, |detail| Found::Pr(detail.summary.clone()))
        };
        let branches = plan
            .branches
            .iter()
            .map(|branch| {
                let found = world.branches.get(branch).map_or(Found::None, summary);
                (branch.clone(), found)
            })
            .collect();
        let mut repos: Vec<&RepoId> = Vec::new();
        let named = plan
            .branches
            .iter()
            .map(|branch| &branch.repo)
            .chain(plan.links.iter().chain(&plan.details).map(|pr| &pr.repo));
        for repo in named {
            if !repos.contains(&repo) {
                repos.push(repo);
            }
        }
        let default_branches = repos
            .into_iter()
            .map(|repo| (repo.clone(), Some(DEFAULT_BRANCH.to_owned())))
            .collect();
        let links = plan
            .links
            .iter()
            .map(|pr| (pr.clone(), summary(pr)))
            .collect();
        let details = plan
            .details
            .iter()
            .map(|pr| (pr.clone(), world.prs.get(pr).cloned()))
            .collect();
        Ok(Batch {
            budget: world.budget.clone(),
            default_branches,
            branches,
            links,
            details,
            missing: Vec::new(),
        })
    }
}

impl Forge for ScriptedForge {
    fn read<'a>(
        &'a self,
        _host: &'a str,
        plan: &'a FetchPlan,
    ) -> Pin<Box<dyn Future<Output = Result<Batch, ReadError>> + Send + 'a>> {
        Box::pin(future::ready(self.answer(plan)))
    }
}
