use pretty_assertions::assert_eq;

use super::*;

fn check(id: DoctorCheckId, status: DoctorStatus) -> DoctorCheck {
    DoctorCheck::new(id, "label", status, status.word())
}

// Covers: the report tallies failures, warnings, and pending probes, and is
// still checking only while a probe is pending.
// Owner: pure unit
#[test]
fn summary_counts_issues_and_pending_probes() {
    use DoctorStatus::{Checking, Fail, Info, Ok, Warn};
    let cases: [(&[DoctorStatus], DoctorSummary); 4] = [
        (
            &[Ok, Info],
            DoctorSummary {
                ok: 1,
                info: 1,
                ..DoctorSummary::default()
            },
        ),
        (
            &[Ok, Checking, Checking],
            DoctorSummary {
                ok: 1,
                checking: 2,
                ..DoctorSummary::default()
            },
        ),
        (
            &[Fail, Warn, Warn],
            DoctorSummary {
                fail: 1,
                warn: 2,
                ..DoctorSummary::default()
            },
        ),
        (
            &[Fail, Checking],
            DoctorSummary {
                fail: 1,
                checking: 1,
                ..DoctorSummary::default()
            },
        ),
    ];
    for (statuses, expected) in cases {
        let report = DoctorReport::from_checks(
            statuses
                .iter()
                .map(|status| check(DoctorCheckId::SelectedModel, *status))
                .collect(),
        );
        assert_eq!(
            (report.summary(), report.is_checking()),
            (expected, expected.checking > 0),
            "statuses={statuses:?}"
        );
    }
}

// Covers: sections appear in canonical order regardless of insertion order,
// and sections without checks are absent.
// Owner: pure unit
#[test]
fn sections_follow_canonical_order_and_skip_empty() {
    let report = DoctorReport::from_checks(vec![
        check(DoctorCheckId::Mcp, DoctorStatus::Ok),
        check(
            DoctorCheckId::ProviderAuth {
                auth_mode: "api-key".into(),
            },
            DoctorStatus::Ok,
        ),
        check(DoctorCheckId::Rtk, DoctorStatus::Ok),
    ]);

    assert_eq!(
        report
            .sections
            .iter()
            .map(|section| section.id)
            .collect::<Vec<_>>(),
        vec![
            DoctorSectionId::Authentication,
            DoctorSectionId::Runtimes,
            DoctorSectionId::Extensions,
        ]
    );
}

// Covers: a finished probe replaces its placeholders in place and appends rows
// with ids the report has not seen, leaving other sections still checking.
// Owner: pure unit
#[test]
fn replace_checks_swaps_placeholders_by_id_and_appends_new_ids() {
    let mut report = DoctorReport::from_checks(vec![
        check(
            DoctorCheckId::ProviderEndpoint {
                provider: "ollama".into(),
            },
            DoctorStatus::Checking,
        ),
        check(DoctorCheckId::ClaudeAuth, DoctorStatus::Checking),
        check(DoctorCheckId::ClaudeBinary, DoctorStatus::Checking),
    ]);
    assert_eq!(report.summary().checking, 3);

    let signed_in = DoctorCheck::new(
        DoctorCheckId::ClaudeAuth,
        "Claude Code authentication",
        DoctorStatus::Ok,
        "signed in",
    );
    let binary = DoctorCheck::new(
        DoctorCheckId::ClaudeBinary,
        "Claude Code binary",
        DoctorStatus::Ok,
        "2.0.0",
    );
    let rtk = DoctorCheck::new(DoctorCheckId::Rtk, "rtk", DoctorStatus::Ok, "available");
    report.replace_checks(vec![signed_in.clone(), binary.clone(), rtk.clone()]);

    assert_eq!(
        report.sections[1],
        DoctorSection {
            id: DoctorSectionId::Runtimes,
            checks: vec![signed_in, binary, rtk],
        }
    );
    assert_eq!(
        report.summary(),
        DoctorSummary {
            ok: 3,
            checking: 1,
            ..DoctorSummary::default()
        }
    );
}
