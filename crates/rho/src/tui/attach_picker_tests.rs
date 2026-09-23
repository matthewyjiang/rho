use pretty_assertions::assert_eq;

use super::{
    candidate_agent_id, candidate_detail, candidate_item, merge_live_candidates, output_blocks,
    picker, retire_departed_live_runs, visible_candidates, AttachCandidate, WorkspaceRunFilter,
    LATEST_EXCERPT_ROWS,
};
use crate::{
    subagent::{RunState, RunStatus},
    tui::{
        picker::{DetailBlock, DetailField, DetailTone, ExcerptAnchor},
        PickerBadge, PickerBadgeTone,
    },
};

fn candidate(run_id: &str, state: RunState) -> AttachCandidate {
    AttachCandidate {
        run_id: run_id.into(),
        agent_id: "worker".into(),
        elapsed_seconds: 1,
        status: RunStatus {
            state,
            title: Some(run_id.into()),
            ..RunStatus::default()
        },
        prompt: None,
    }
}

fn heading(label: &str) -> DetailBlock {
    DetailBlock::Heading {
        label: label.into(),
        status: String::new(),
    }
}

fn facts(candidate: &AttachCandidate, now: u64) -> Vec<DetailField> {
    candidate_detail(candidate, now)
        .blocks
        .into_iter()
        .find_map(|block| match block {
            DetailBlock::Fields(fields) => Some(fields),
            _ => None,
        })
        .expect("run card has a facts block")
}

// Covers: attach rows name the run by role and title, and the nav badge
// shows live activity or the outcome in its own tone, never the run id.
// Owner: attach picker
#[test]
fn attach_row_shows_role_title_and_outcome_badge() {
    let mut running = candidate("abc123", RunState::Running);
    running.status.title = Some("Review the auth path".into());
    running.status.last_activity = Some("tool: read".into());
    let mut starting = candidate("def456", RunState::Starting);
    starting.agent_id = "explorer".into();
    starting.status.title = None;

    let cases = [
        (
            running,
            "worker",
            "Review the auth path",
            "read",
            PickerBadgeTone::Internal,
        ),
        (
            starting,
            "explorer",
            "untitled",
            "starting",
            PickerBadgeTone::Muted,
        ),
        (
            candidate("ok0001", RunState::Ok),
            "worker",
            "ok0001",
            "✓ done",
            PickerBadgeTone::Healthy,
        ),
        (
            candidate("err001", RunState::Error),
            "worker",
            "err001",
            "✗ error",
            PickerBadgeTone::Error,
        ),
    ];

    for (candidate, role, label, badge, tone) in cases {
        let item = candidate_item(&candidate);
        assert_eq!(
            (
                item.section.as_deref(),
                item.label.as_str(),
                item.value.as_str(),
                item.badge
            ),
            (
                Some(role),
                label,
                candidate.run_id.as_str(),
                Some(PickerBadge {
                    text: badge.into(),
                    tone
                }),
            )
        );
    }
}

// Covers: the card's closing section follows the run's outcome, so a live run
// shows its newest output, a finished one its answer, and a failure its error.
// Owner: attach picker run card
#[test]
fn run_card_output_section_follows_state() {
    let status = |state| RunStatus {
        state,
        last_text: Some("streaming tail".into()),
        result: Some("final answer".into()),
        error: Some("provider exploded".into()),
        ..RunStatus::default()
    };
    let cases = [
        (
            status(RunState::Running),
            vec![
                heading("LATEST"),
                DetailBlock::Excerpt {
                    text: "streaming tail".into(),
                    rows: LATEST_EXCERPT_ROWS,
                    anchor: ExcerptAnchor::End,
                },
            ],
        ),
        (
            status(RunState::Ok),
            vec![
                heading("RESULT"),
                DetailBlock::Paragraph("final answer".into()),
            ],
        ),
        (
            status(RunState::Error),
            vec![
                heading("ERROR"),
                DetailBlock::Error("provider exploded".into()),
            ],
        ),
        (
            RunStatus {
                state: RunState::Running,
                ..RunStatus::default()
            },
            vec![
                heading("LATEST"),
                DetailBlock::Muted("(no output yet)".into()),
            ],
        ),
    ];
    for (status, expected) in cases {
        assert_eq!(output_blocks(&status), expected, "{:?}", status.state);
    }
}

// Covers: facts prefer what the runtime actually bound and recede when the
// run never reported them, instead of printing blanks or zeros.
// Owner: attach picker run card
#[test]
fn run_card_facts_use_reported_values_and_mute_unknowns() {
    let mut finished = candidate("aaaaaa", RunState::Ok);
    finished.elapsed_seconds = 75;
    finished.status = RunStatus {
        state: RunState::Ok,
        model: Some("opus".into()),
        claude_model: Some("claude-opus-5-5".into()),
        reasoning: Some(rho_sdk::ReasoningLevel::High),
        runtime: Some(crate::agent::AgentRuntime::ClaudeCli),
        finished_at: Some(1_000),
        input_tokens: Some(48_200),
        output_tokens: Some(3_100),
        total_cost_usd: Some(0.14),
        ..RunStatus::default()
    };
    let unknown = candidate("bbbbbb", RunState::Running);

    assert_eq!(
        facts(&finished, 1_120),
        vec![
            DetailField::new("Model", "claude-opus-5-5 · high", DetailTone::Normal)
                .with_note("claude-cli"),
            DetailField::new("Elapsed", "1m 15s", DetailTone::Normal).with_note("finished 2m ago"),
            DetailField::new("Tokens", "48.2K in · 3.1K out", DetailTone::Normal)
                .with_note("$0.14"),
        ]
    );
    assert_eq!(
        facts(&unknown, 0),
        vec![
            DetailField::new("Activity", "working", DetailTone::Normal),
            DetailField::new("Model", "not reported", DetailTone::Muted),
            DetailField::new("Elapsed", "1s", DetailTone::Normal),
            DetailField::new("Tokens", "not reported", DetailTone::Muted),
        ]
    );
}

// Covers: a live refresh must not drop a prompt already read from disk, and a
// newly listed live run reads its prompt once.
// Owner: attach picker
#[test]
fn live_merge_keeps_known_prompts_and_reads_new_ones() {
    let mut disk = candidate("aaaaaa", RunState::Running);
    disk.prompt = Some("known task".into());
    let live = vec![
        candidate("aaaaaa", RunState::Running),
        candidate("bbbbbb", RunState::Starting),
    ];

    let merged = merge_live_candidates(vec![disk], live, |run_id| Some(format!("read {run_id}")));

    assert_eq!(
        merged
            .iter()
            .map(|run| (run.run_id.as_str(), run.prompt.as_deref()))
            .collect::<Vec<_>>(),
        [
            ("bbbbbb", Some("read bbbbbb")),
            ("aaaaaa", Some("known task"))
        ]
    );
}

// Covers: running-only must hide finished transcripts until the user toggles.
// Owner: attach picker
#[test]
fn running_only_hides_terminal_runs() {
    let candidates = [
        candidate("aaaaaa", RunState::Running),
        candidate("bbbbbb", RunState::Ok),
        candidate("cccccc", RunState::Error),
    ];

    let running_ids = visible_candidates(&candidates, WorkspaceRunFilter::RunningOnly)
        .into_iter()
        .map(|run| run.run_id.as_str())
        .collect::<Vec<_>>();
    let all_ids = visible_candidates(&candidates, WorkspaceRunFilter::All)
        .into_iter()
        .map(|run| run.run_id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(running_ids, ["aaaaaa"]);
    assert_eq!(all_ids, ["aaaaaa", "bbbbbb", "cccccc"]);
}

// Covers: an empty inventory must still build an attach overlay.
// Owner: attach picker
#[test]
fn empty_inventory_still_builds_a_picker() {
    let empty = picker(&[], WorkspaceRunFilter::RunningOnly);
    assert!(empty.items.is_empty());
    assert_eq!(empty.selected_item().map(|item| item.value.as_str()), None);
}

// Covers: live panel rows overlay matching disk rows and keep panel order for new lives.
// Owner: attach picker
#[test]
fn live_candidates_replace_matching_disk_rows() {
    let disk = vec![
        candidate("aaaaaa", RunState::Ok),
        candidate("bbbbbb", RunState::Running),
    ];
    let mut live = candidate("bbbbbb", RunState::Running);
    live.status.title = Some("updated".into());
    live.elapsed_seconds = 9;
    let first_missing = candidate("cccccc", RunState::Starting);
    let second_missing = candidate("dddddd", RunState::Starting);

    let merged = merge_live_candidates(disk, vec![live, first_missing, second_missing], |_| None);

    assert_eq!(
        merged
            .iter()
            .map(|run| (
                run.run_id.as_str(),
                run.status.title.as_deref(),
                run.elapsed_seconds
            ))
            .collect::<Vec<_>>(),
        [
            ("cccccc", Some("cccccc"), 1),
            ("dddddd", Some("dddddd"), 1),
            ("aaaaaa", Some("aaaaaa"), 1),
            ("bbbbbb", Some("updated"), 9),
        ]
    );
}

// Covers: finished transcripts keep their role after the picker closes.
// Owner: attach picker
#[test]
fn finished_run_agent_id_comes_from_candidates() {
    let mut finished = candidate("aaaaaa", RunState::Ok);
    finished.agent_id = "explorer".into();

    assert_eq!(candidate_agent_id(&[finished], "aaaaaa"), Some("explorer"));
    assert_eq!(candidate_agent_id(&[], "aaaaaa"), None);
}

// Covers: a run that leaves the live panel must not stay listed as running.
// Owner: attach picker
#[test]
fn departed_live_run_uses_real_terminal_state() {
    let mut candidates = vec![
        candidate("aaaaaa", RunState::Running),
        candidate("bbbbbb", RunState::Running),
        candidate("cccccc", RunState::Running),
        candidate("dddddd", RunState::Running),
    ];
    let previously_live = [
        "aaaaaa".into(),
        "bbbbbb".into(),
        "cccccc".into(),
        "dddddd".into(),
    ]
    .into();
    let live_ids = ["dddddd".into()].into();

    retire_departed_live_runs(
        &mut candidates,
        &live_ids,
        &previously_live,
        |run_id| match run_id {
            "aaaaaa" => RunState::Ok,
            "bbbbbb" => RunState::Error,
            _ => RunState::Stopped,
        },
    );

    assert_eq!(
        candidates
            .iter()
            .map(|run| (run.run_id.as_str(), run.state()))
            .collect::<Vec<_>>(),
        [
            ("aaaaaa", RunState::Ok),
            ("bbbbbb", RunState::Error),
            ("cccccc", RunState::Stopped),
            ("dddddd", RunState::Running),
        ]
    );
}
