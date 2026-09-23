use pretty_assertions::assert_eq;

use super::*;
use crate::doctor::{DoctorCheck, DoctorCheckId, DoctorStatus};

// Covers: the text report aligns the status and label columns, prints hints
// under their row at the label column, and leads with the headline.
// Owner: pure unit
#[test]
fn renders_aligned_sections_with_hints() {
    let report = DoctorReport::from_checks(vec![
        DoctorCheck::new(
            DoctorCheckId::ProviderAuth {
                auth_mode: "api-key".into(),
            },
            "OpenAI API key",
            DoctorStatus::Ok,
            "authenticated",
        ),
        DoctorCheck::new(
            DoctorCheckId::ProviderAuth {
                auth_mode: "anthropic-api-key".into(),
            },
            "Anthropic API key",
            DoctorStatus::Warn,
            "missing",
        )
        .with_hint("run /login anthropic-api-key"),
        DoctorCheck::new(DoctorCheckId::Rtk, "rtk", DoctorStatus::Info, "unavailable"),
    ]);

    assert_eq!(
        render(&report),
        "\
Doctor: 1 warning

Authentication
  ok    OpenAI API key     authenticated
  warn  Anthropic API key  missing
        run /login anthropic-api-key

Runtimes
  info  rtk                unavailable
"
    );
}
