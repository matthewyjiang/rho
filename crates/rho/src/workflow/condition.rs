use std::collections::BTreeMap;

use super::{
    CommandExit, Condition, ExitCodePredicate, NodeId, NodeTerminalState, TruthValue,
    ValuePredicate, WorkflowValue,
};

pub(crate) struct ConditionContext<'a> {
    pub(crate) statuses: &'a BTreeMap<NodeId, NodeTerminalState>,
    pub(crate) command_exits: &'a BTreeMap<NodeId, CommandExit>,
    pub(crate) outputs: &'a BTreeMap<NodeId, WorkflowValue>,
}

pub(crate) fn evaluate_condition(
    condition: &Condition,
    context: &ConditionContext<'_>,
) -> TruthValue {
    match condition {
        Condition::NodeStatus { node, matches } => context
            .statuses
            .get(node)
            .map_or(TruthValue::Unavailable, |status| {
                truth(matches.contains(status))
            }),
        Condition::CommandExit { node, predicate } => context
            .command_exits
            .get(node)
            .map_or(TruthValue::Unavailable, |exit| {
                truth(exit_matches(exit, predicate))
            }),
        Condition::Output {
            node,
            path,
            predicate,
        } => context
            .outputs
            .get(node)
            .and_then(|value| value.at_path(&path.0))
            .map_or(TruthValue::Unavailable, |value| {
                truth(value_matches(value, predicate))
            }),
        Condition::All { conditions } => combine_all(
            conditions
                .iter()
                .map(|condition| evaluate_condition(condition, context)),
        ),
        Condition::Any { conditions } => combine_any(
            conditions
                .iter()
                .map(|condition| evaluate_condition(condition, context)),
        ),
        Condition::Not { condition } => match evaluate_condition(condition, context) {
            TruthValue::True => TruthValue::False,
            TruthValue::False => TruthValue::True,
            TruthValue::Unavailable => TruthValue::Unavailable,
        },
    }
}

fn truth(value: bool) -> TruthValue {
    if value {
        TruthValue::True
    } else {
        TruthValue::False
    }
}

fn combine_all(values: impl Iterator<Item = TruthValue>) -> TruthValue {
    let mut unavailable = false;
    for value in values {
        match value {
            TruthValue::False => return TruthValue::False,
            TruthValue::Unavailable => unavailable = true,
            TruthValue::True => {}
        }
    }
    if unavailable {
        TruthValue::Unavailable
    } else {
        TruthValue::True
    }
}

fn combine_any(values: impl Iterator<Item = TruthValue>) -> TruthValue {
    let mut unavailable = false;
    for value in values {
        match value {
            TruthValue::True => return TruthValue::True,
            TruthValue::Unavailable => unavailable = true,
            TruthValue::False => {}
        }
    }
    if unavailable {
        TruthValue::Unavailable
    } else {
        TruthValue::False
    }
}

fn exit_matches(exit: &CommandExit, predicate: &ExitCodePredicate) -> bool {
    match (exit, predicate) {
        (CommandExit::Code { code }, ExitCodePredicate::Equals(expected)) => code == expected,
        (CommandExit::Code { code }, ExitCodePredicate::IsOneOf(expected)) => {
            expected.contains(code)
        }
        (CommandExit::Code { code }, ExitCodePredicate::Succeeded) => *code == 0,
        (CommandExit::Code { code }, ExitCodePredicate::Failed) => *code != 0,
        (
            CommandExit::Signal { .. }
            | CommandExit::Timeout
            | CommandExit::Cancellation
            | CommandExit::Abnormal,
            ExitCodePredicate::Failed,
        ) => true,
        (
            CommandExit::Signal { .. }
            | CommandExit::Timeout
            | CommandExit::Cancellation
            | CommandExit::Abnormal,
            ExitCodePredicate::Equals(_)
            | ExitCodePredicate::IsOneOf(_)
            | ExitCodePredicate::Succeeded,
        ) => false,
    }
}

fn value_matches(value: &WorkflowValue, predicate: &ValuePredicate) -> bool {
    match predicate {
        ValuePredicate::Equals(expected) => value == expected,
        ValuePredicate::NotEquals(expected) => value != expected,
        ValuePredicate::IsOneOf(expected) => expected.contains(value),
    }
}

#[cfg(test)]
#[path = "condition_tests.rs"]
mod tests;
