use std::sync::Arc;

use pretty_assertions::assert_eq;

use super::{read_into, Read, Reads};
use crate::app::pull_requests::{
    gh_query::SUMMARIES_PER_QUERY,
    model::{BranchId, FetchPlan, PrId},
    poller::{Budgets, Reader},
    test_forge::ScriptedForge,
};

// Covers: on a host split into two queries, a branch lookup in the later
// query that finds a PR whose detail the earlier query read keeps that
// detail. Otherwise the tick skips the PR's watches, which it evaluates
// only against a detail, though the detail may show a failed check.
// Owner: PR poller
#[tokio::test]
async fn a_later_lookup_keeps_a_detail_an_earlier_query_read() {
    let forge = ScriptedForge::new();
    forge.set_open("acme/widgets#12", &[], &[]);
    let pr = PrId::parse("acme/widgets#12", None).expect("an owner/name#N target");
    let branch = |n: usize| BranchId {
        repo: pr.repo.clone(),
        branch: format!("b{n}"),
    };
    // The first query takes SUMMARIES_PER_QUERY lookups and the detail; the
    // last lookup, which finds the PR, goes in the second.
    forge.set_branch("acme/widgets#12", &format!("b{SUMMARIES_PER_QUERY}"));
    let plan = FetchPlan {
        branches: (0..=SUMMARIES_PER_QUERY).map(branch).collect(),
        links: Vec::new(),
        details: vec![pr.clone()],
    };
    let mut reader = Reader::new(forge.clone(), Arc::new(Budgets::default()));
    let mut reads = Reads::default();

    read_into(&mut reader, "github.com", plan, 0, &mut reads).await;

    let kept = match reads.prs.get(&pr) {
        Some(Read::Detail(_)) => "detail",
        Some(Read::Summary(_)) => "summary",
        Some(Read::Refused(_)) => "refused",
        Some(Read::Failed(_)) => "failed",
        None => "nothing",
    };
    assert_eq!((forge.take_asked().len(), kept), (2, "detail"));
}
