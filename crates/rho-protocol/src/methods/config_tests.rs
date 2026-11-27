use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

fn spec(per_machine: bool, read_only: bool) -> SettingSpec {
    SettingSpec {
        key: SettingKey::new("behavior.agent_concurrency"),
        kind: SettingKind::Bool,
        default: None,
        category: "agent".into(),
        section: None,
        label: "Concurrent agents".into(),
        help: "Help.".into(),
        applies: Applies::Immediately,
        overridable_per_project: false,
        per_machine,
        read_only,
    }
}

// Covers: `per_machine` and `read_only` each travel only when true, so a
// setting with neither encodes as it did before the fields, and a spec from
// a daemon that predates them reads as neither.
// Owner: protocol wire contract.
#[test]
fn setting_flags_travel_only_when_true() {
    let wire = |flags: &[&str]| {
        let mut wire = json!({
            "key": "behavior.agent_concurrency",
            "kind": {"kind": "bool"},
            "category": "agent",
            "label": "Concurrent agents",
            "help": "Help.",
            "applies": "immediately",
            "overridable_per_project": false,
        });
        for flag in flags {
            wire[*flag] = json!(true);
        }
        wire
    };
    for (value, encoded) in [
        (spec(true, false), wire(&["per_machine"])),
        (spec(false, true), wire(&["read_only"])),
        (spec(false, false), wire(&[])),
    ] {
        assert_eq!(serde_json::to_value(&value).unwrap(), encoded);
        assert_eq!(
            serde_json::from_value::<SettingSpec>(encoded).unwrap(),
            value
        );
    }
}
