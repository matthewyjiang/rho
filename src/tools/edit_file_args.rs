use crate::tool::ToolError;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
pub(super) struct Args {
    edits: Option<Vec<EditArgs>>,
    path: Option<String>,
    old_string: Option<String>,
    new_string: Option<String>,
    replace_all: Option<bool>,
    expected_match_count: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditArgs {
    path: String,
    old_string: String,
    new_string: String,
    #[serde(default = "default_expected_match_count")]
    expected_match_count: usize,
}

pub(super) struct Edit {
    pub(super) path: String,
    pub(super) old_string: String,
    pub(super) new_string: String,
    match_count: MatchCount,
}

enum MatchCount {
    Exact(usize),
    All,
}

pub(super) fn input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "edits": {
                "type": "array",
                "description": "Atomic replacements to apply in order. No files are changed if any edit fails validation.",
                "minItems": 1,
                "items": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "old_string": {"type": "string"},
                        "new_string": {"type": "string"},
                        "expected_match_count": {
                            "type": "integer",
                            "minimum": 1,
                            "default": 1,
                            "description": "Exact number of occurrences that must match and will be replaced."
                        }
                    },
                    "required": ["path", "old_string", "new_string"],
                    "additionalProperties": false
                }
            },
            "path": {"type": "string", "description": "Path for a legacy single edit."},
            "old_string": {"type": "string", "description": "Text to replace in a legacy single edit."},
            "new_string": {"type": "string", "description": "Replacement text for a legacy single edit."},
            "replace_all": {"type": "boolean", "description": "Replace every match in a legacy single edit."},
            "expected_match_count": {
                "type": "integer",
                "minimum": 1,
                "description": "Exact match count for a legacy single edit. Cannot be combined with replace_all=true."
            }
        },
        "anyOf": [
            {"required": ["edits"]},
            {"required": ["path", "old_string", "new_string"]}
        ]
    })
}

impl Args {
    pub(super) fn into_edits(self) -> Result<Vec<Edit>, ToolError> {
        if let Some(edits) = self.edits {
            if self.path.is_some()
                || self.old_string.is_some()
                || self.new_string.is_some()
                || self.replace_all.is_some()
                || self.expected_match_count.is_some()
            {
                return Err(ToolError::Message(
                    "edits cannot be combined with single-edit arguments".into(),
                ));
            }
            if edits.is_empty() {
                return Err(ToolError::Message("edits must not be empty".into()));
            }
            return Ok(edits
                .into_iter()
                .map(|edit| Edit {
                    path: edit.path,
                    old_string: edit.old_string,
                    new_string: edit.new_string,
                    match_count: MatchCount::Exact(edit.expected_match_count),
                })
                .collect());
        }

        let path = self
            .path
            .ok_or_else(|| ToolError::Message("path is required".into()))?;
        let old_string = self
            .old_string
            .ok_or_else(|| ToolError::Message("old_string is required".into()))?;
        let new_string = self
            .new_string
            .ok_or_else(|| ToolError::Message("new_string is required".into()))?;
        if self.replace_all == Some(true) && self.expected_match_count.is_some() {
            return Err(ToolError::Message(
                "replace_all=true cannot be combined with expected_match_count".into(),
            ));
        }
        let match_count = if self.replace_all == Some(true) {
            MatchCount::All
        } else {
            MatchCount::Exact(self.expected_match_count.unwrap_or(1))
        };
        Ok(vec![Edit {
            path,
            old_string,
            new_string,
            match_count,
        }])
    }
}

impl Edit {
    pub(super) fn validate(&self, index: usize) -> Result<(), ToolError> {
        if self.old_string.is_empty() {
            return Err(edit_error(
                index,
                &self.path,
                "old_string must not be empty",
            ));
        }
        if self.old_string == self.new_string {
            return Err(edit_error(
                index,
                &self.path,
                "old_string and new_string are identical; nothing to change",
            ));
        }
        if matches!(self.match_count, MatchCount::Exact(0)) {
            return Err(edit_error(
                index,
                &self.path,
                "expected_match_count must be at least 1",
            ));
        }
        Ok(())
    }

    pub(super) fn validate_match_count(
        &self,
        index: usize,
        actual: usize,
    ) -> Result<(), ToolError> {
        let expected = match self.match_count {
            MatchCount::All if actual > 0 => return Ok(()),
            MatchCount::All => 1,
            MatchCount::Exact(expected) if actual == expected => return Ok(()),
            MatchCount::Exact(expected) => expected,
        };
        let reason = if actual < expected {
            "missing match"
        } else {
            "ambiguous match"
        };
        Err(edit_error(
            index,
            &self.path,
            format!("{reason}: found {actual} occurrence(s), expected {expected}"),
        ))
    }
}

pub(super) fn edit_error(index: usize, path: &str, message: impl std::fmt::Display) -> ToolError {
    ToolError::Message(format!("edit {} ({path}) failed: {message}", index + 1))
}

fn default_expected_match_count() -> usize {
    1
}
