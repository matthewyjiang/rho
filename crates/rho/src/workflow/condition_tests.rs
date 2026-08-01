use std::collections::BTreeMap;

use super::*;
use crate::workflow::OutputPath;

// Covers: condition guard order must not change unavailable branch decisions.
// Owner: workflow condition core.
#[test]
fn three_valued_all_and_any_are_order_independent() {
    let unavailable = Condition::Output {
        node: NodeId::new("missing").unwrap(),
        path: OutputPath(vec!["value".to_owned()]),
        predicate: ValuePredicate::Equals(WorkflowValue::Bool(true)),
    };
    let true_condition = Condition::NodeStatus {
        node: NodeId::new("done").unwrap(),
        matches: [NodeTerminalState::Success].into_iter().collect(),
    };
    let statuses = BTreeMap::from([(NodeId::new("done").unwrap(), NodeTerminalState::Success)]);
    let exits = BTreeMap::new();
    let outputs = BTreeMap::new();
    let context = ConditionContext {
        statuses: &statuses,
        command_exits: &exits,
        outputs: &outputs,
    };
    for conditions in [
        vec![unavailable.clone(), true_condition.clone()],
        vec![true_condition.clone(), unavailable.clone()],
    ] {
        assert_eq!(
            evaluate_condition(
                &Condition::All {
                    conditions: conditions.clone()
                },
                &context
            ),
            TruthValue::Unavailable
        );
        assert_eq!(
            evaluate_condition(&Condition::Any { conditions }, &context),
            TruthValue::True
        );
    }
}
