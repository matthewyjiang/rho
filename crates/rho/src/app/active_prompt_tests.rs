use super::*;
use crate::prompt::PromptSourceKind;
use pretty_assertions::assert_eq;
use rho_sdk::{model::ModelIdentity, CompactionState, Revision, SessionId};

// Covers: host suffixes must remain in source byte accounting after persistence
// and recovery, rather than saving the pre-suffix diagnostics.
// Owner: host prompt state and provenance boundary
#[test]
fn retained_suffix_keeps_durable_source_accounting_aligned() {
    let mut prompt = ActivePrompt::new(
        SystemPrompt::Custom("base".into()),
        /*loaded*/ None,
        vec![PromptSource {
            kind: PromptSourceKind::Base,
            path: None,
            bytes: 4,
        }],
    );
    prompt.append_retained(" suffix");
    let text = "base suffix";
    assert_eq!(prompt.system, SystemPrompt::Custom(text.into()));
    assert_eq!(
        prompt
            .sources
            .iter()
            .map(|source| source.bytes)
            .sum::<usize>(),
        text.len()
    );

    let snapshot = prompt.decorate(SessionSnapshot::new(
        SessionId::new(),
        Revision::from_u64(1),
        vec![Message::System(text.into())],
        ModelIdentity::new("test", "test", "model"),
        CompactionState::default(),
    ));
    assert_eq!(ActivePrompt::from_snapshot(&snapshot).unwrap(), prompt);
}
