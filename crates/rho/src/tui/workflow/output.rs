//! Load and format durable node outputs for the workflow details pane.

use std::{
    borrow::Cow,
    io::Read,
    path::{Component, Path, PathBuf},
};

use ratatui::text::Line;

use super::event_adapter::{ArtifactReference, WorkflowNodeSnapshot};
use crate::workflow::{ArtifactKind, ArtifactObservation, Digest};

/// Preferred artifact kinds for the details body, most useful first.
const PREFERRED_KINDS: [ArtifactKind; 4] = [
    ArtifactKind::AgentAnswer,
    ArtifactKind::StructuredOutput,
    ArtifactKind::Stdout,
    ArtifactKind::Stderr,
];

/// Soft cap so a pathological artifact cannot freeze the TUI.
const MAX_DISPLAY_BYTES: usize = 512 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct NodeOutputBody {
    pub(super) node_id: crate::workflow::NodeId,
    pub(super) digest: Digest,
    pub(super) kind: ArtifactKind,
    pub(super) relative_path: String,
    pub(super) text: String,
    pub(super) notice: Option<String>,
}

pub(super) fn primary_artifact(node: &WorkflowNodeSnapshot) -> Option<&ArtifactReference> {
    for kind in PREFERRED_KINDS {
        if let Some(artifact) = node.artifacts.iter().find(|item| item.kind == kind) {
            return Some(artifact);
        }
    }
    node.artifacts.first()
}

/// Load the primary durable output for a finished node, when present.
pub(super) fn load_finished_output(
    run_directory: &Path,
    node: &WorkflowNodeSnapshot,
) -> Option<NodeOutputBody> {
    if node.state.terminal().is_none() {
        return None;
    }
    let reference = primary_artifact(node)?;
    let relative = reference.artifact.relative_path.as_str();
    let (text, read_notices) = match read_artifact_text(run_directory, relative) {
        Ok(decoded) => decoded,
        Err(error) => {
            return Some(NodeOutputBody {
                node_id: node.id.clone(),
                digest: reference.artifact.digest.clone(),
                kind: reference.kind,
                relative_path: relative.to_owned(),
                text: String::new(),
                notice: Some(error),
            });
        }
    };
    let mut notices = Vec::new();
    if let Some(notice) = observation_notice(
        &reference.artifact.observed,
        reference.artifact.retained_bytes,
    ) {
        notices.push(notice);
    }
    notices.extend(read_notices);
    Some(NodeOutputBody {
        node_id: node.id.clone(),
        digest: reference.artifact.digest.clone(),
        kind: reference.kind,
        relative_path: relative.to_owned(),
        text,
        notice: join_notices(notices),
    })
}

pub(super) fn body_matches_node(body: &NodeOutputBody, node: &WorkflowNodeSnapshot) -> bool {
    primary_artifact(node).is_some_and(|reference| {
        body.node_id == node.id
            && body.digest == reference.artifact.digest
            && body.kind == reference.kind
    })
}

pub(super) fn kind_label(kind: ArtifactKind) -> &'static str {
    match kind {
        ArtifactKind::AgentAnswer => "answer",
        ArtifactKind::StructuredOutput => "structured output",
        ArtifactKind::Stdout => "stdout",
        ArtifactKind::Stderr => "stderr",
        ArtifactKind::CommandOutcome => "command outcome",
    }
}

/// Render loaded output text for the current pane width.
pub(super) fn render_body_lines(body: &NodeOutputBody, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut lines = Vec::new();
    if let Some(notice) = &body.notice {
        lines.push(Line::from(format!("({notice})")));
        lines.push(Line::from(""));
    }
    if body.text.is_empty() {
        if body.notice.is_none() {
            lines.push(Line::from("(empty)"));
        }
        return lines;
    }

    match body.kind {
        ArtifactKind::AgentAnswer => {
            let mut in_code_block = false;
            lines.extend(super::super::markdown::markdown_lines(
                &body.text,
                width,
                &mut in_code_block,
            ));
        }
        ArtifactKind::StructuredOutput | ArtifactKind::CommandOutcome => {
            lines.extend(render_structured(&body.text, width));
        }
        ArtifactKind::Stdout | ArtifactKind::Stderr => {
            lines.extend(render_plain(&body.text, width));
        }
    }
    lines
}

fn render_structured(text: &str, width: usize) -> Vec<Line<'static>> {
    let pretty = pretty_json(text).unwrap_or_else(|| text.to_owned());
    let fenced = format!("```json\n{}\n```", pretty.trim_end());
    let mut in_code_block = false;
    super::super::markdown::markdown_lines(&fenced, width, &mut in_code_block)
}

fn render_plain(text: &str, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    super::super::render::push_wrapped_text(
        &mut lines,
        text,
        width,
        super::super::theme::Theme::text(),
        super::super::render::LineFill::Natural,
    );
    if lines.is_empty() {
        lines.push(Line::from(""));
    }
    lines
}

fn pretty_json(text: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(text.trim()).ok()?;
    serde_json::to_string_pretty(&value).ok()
}

fn observation_notice(observed: &ArtifactObservation, retained_bytes: u64) -> Option<String> {
    match observed {
        ArtifactObservation::Complete { .. } => None,
        ArtifactObservation::Truncated {
            observed_bytes_at_least,
        } => Some(format!(
            "truncated · showing {retained_bytes} of at least {observed_bytes_at_least} bytes"
        )),
        ArtifactObservation::Incomplete { observed_bytes } => Some(format!(
            "incomplete · retained {retained_bytes} bytes (observed {observed_bytes})"
        )),
    }
}

fn read_artifact_text(
    run_directory: &Path,
    relative: &str,
) -> Result<(String, Vec<String>), String> {
    let relative_path = validated_relative(relative)
        .ok_or_else(|| "output path is not safe to read from the run directory".to_owned())?;
    let mut file = crate::workflow::open_private_file_beneath(
        run_directory,
        &relative_path,
        /*writable*/ false,
    )
    .map_err(|error| format!("could not open output: {error}"))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| format!("could not read output: {error}"))?;
    Ok(decode_artifact_bytes(&bytes))
}

fn decode_artifact_bytes(bytes: &[u8]) -> (String, Vec<String>) {
    let mut notices = Vec::new();
    let decoded = String::from_utf8_lossy(bytes);
    // Owned Cow means replacements were inserted for invalid UTF-8 sequences.
    if matches!(decoded, Cow::Owned(_)) {
        notices.push("output is not valid UTF-8; showing lossy text".into());
    }
    let text = if decoded.len() > MAX_DISPLAY_BYTES {
        let mut end = MAX_DISPLAY_BYTES;
        while end > 0 && !decoded.is_char_boundary(end) {
            end -= 1;
        }
        notices.push(format!(
            "display capped at {MAX_DISPLAY_BYTES} bytes for the TUI"
        ));
        decoded[..end].to_owned()
    } else {
        decoded.into_owned()
    };
    (text, notices)
}

fn join_notices(notices: Vec<String>) -> Option<String> {
    if notices.is_empty() {
        None
    } else {
        Some(notices.join("; "))
    }
}

fn validated_relative(relative: &str) -> Option<PathBuf> {
    if relative.is_empty() {
        return None;
    }
    let path = Path::new(relative);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return None;
    }
    Some(path.to_path_buf())
}

#[cfg(test)]
#[path = "output_tests.rs"]
mod tests;
