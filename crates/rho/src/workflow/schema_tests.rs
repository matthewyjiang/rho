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
