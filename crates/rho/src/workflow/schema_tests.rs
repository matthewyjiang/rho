use std::collections::{BTreeMap, BTreeSet};

use super::*;

// Covers: invalid structured output must not satisfy a typed output contract.
// Owner: workflow schema core.
#[test]
fn validates_closed_object_shape_and_required_fields() {
    let schema = OutputSchema::Object {
        fields: BTreeMap::from([
            (
                "required".to_owned(),
                ObjectFieldSchema {
                    schema: OutputSchema::Integer,
                    required: true,
                },
            ),
            (
                "optional".to_owned(),
                ObjectFieldSchema {
                    schema: OutputSchema::String,
                    required: false,
                },
            ),
        ]),
    };
    let cases = [
        (
            WorkflowValue::Object(BTreeMap::from([(
                "required".to_owned(),
                WorkflowValue::Integer(1),
            )])),
            true,
        ),
        (WorkflowValue::Object(BTreeMap::new()), false),
        (
            WorkflowValue::Object(BTreeMap::from([
                ("required".to_owned(), WorkflowValue::Integer(1)),
                ("extra".to_owned(), WorkflowValue::Integer(1)),
            ])),
            false,
        ),
    ];
    for (value, valid) in cases {
        assert_eq!(schema.validate_value(&value).is_ok(), valid, "{value:?}");
    }
}

// Covers: composite enum members could bypass scalar-only branch guarantees.
// Owner: workflow schema core.
#[test]
fn rejects_non_scalar_enum_members() {
    let schema = OutputSchema::Enum {
        members: BTreeSet::from([WorkflowValue::List(vec![WorkflowValue::Bool(true)])]),
    };
    assert!(matches!(
        schema.validate_definition(),
        Err(WorkflowError::Schema { .. })
    ));
}

// Covers: a stored graph must not bypass the planner's schema-depth budget.
// Owner: workflow schema core.
#[test]
fn rejects_stored_schema_over_depth_budget() {
    let mut schema = OutputSchema::String;
    for _ in 0..OUTPUT_SCHEMA_DEPTH_LIMIT {
        schema = OutputSchema::List {
            item: Box::new(schema),
        };
    }
    let stored = serde_json::to_value(schema).unwrap();
    let loaded: OutputSchema = serde_json::from_value(stored).unwrap();

    assert!(matches!(
        loaded.validate_definition(),
        Err(WorkflowError::BudgetExceeded {
            budget,
            limit,
            actual,
        }) if budget == "output schema depth"
            && limit == OUTPUT_SCHEMA_DEPTH_LIMIT as u64
            && actual == OUTPUT_SCHEMA_DEPTH_LIMIT as u64 + 1
    ));
}
