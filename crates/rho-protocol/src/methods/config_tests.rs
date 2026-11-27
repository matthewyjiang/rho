use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

fn spec(per_machine: bool) -> SettingSpec {
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
    }
}

// Covers: `per_machine` travels only when true, so a setting that does not
// belong to one machine encodes as it did before the field, and a spec from
// a daemon that predates the field reads as not per-machine.
// Owner: protocol wire contract.
#[test]
fn per_machine_travels_only_when_true() {
    let wire = |per_machine: Option<bool>| {
        let mut wire = json!({
            "key": "behavior.agent_concurrency",
            "kind": {"kind": "bool"},
            "category": "agent",
            "label": "Concurrent agents",
            "help": "Help.",
            "applies": "immediately",
            "overridable_per_project": false,
        });
        if let Some(per_machine) = per_machine {
            wire["per_machine"] = json!(per_machine);
        }
        wire
    };
    for (value, encoded) in [(spec(true), wire(Some(true))), (spec(false), wire(None))] {
        assert_eq!(serde_json::to_value(&value).unwrap(), encoded);
        assert_eq!(
            serde_json::from_value::<SettingSpec>(encoded).unwrap(),
            value
        );
    }
}
