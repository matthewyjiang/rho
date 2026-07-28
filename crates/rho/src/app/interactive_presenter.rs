use std::{collections::BTreeMap, path::PathBuf};

use rho_sdk::{
    model::ToolCall,
    tool::{ToolAsset, ToolMetadata, ToolProgress},
    ToolCallId, ToolCompletion,
};

use rho_tools::tool_card::ToolCard;

#[path = "interactive_presenter_agent.rs"]
mod agent_format;
#[path = "interactive_presenter_format.rs"]
mod format;
use format::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ToolPresentation {
    pub(crate) card: ToolCard,
    pub(crate) image_asset: Option<ToolAsset>,
}

#[derive(Clone, Debug)]
struct ToolView {
    kind: ToolKind,
    name: String,
    arguments: serde_json::Value,
    metadata: ToolMetadata,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToolKind {
    Agent,
    Agents,
    Bash,
    PowerShell,
    Process,
    ListDir,
    Grep,
    Glob,
    ReadFile,
    WriteFile,
    EditFile,
    Skill,
    WebSearch,
    FetchContent,
    GetSearchContent,
    Questionnaire,
    Other,
}

impl ToolKind {
    fn from_name(name: &str) -> Self {
        match name {
            "agent" => Self::Agent,
            "agents" => Self::Agents,
            "bash" => Self::Bash,
            "powershell" => Self::PowerShell,
            "process" => Self::Process,
            "list_dir" => Self::ListDir,
            "grep" => Self::Grep,
            "glob" => Self::Glob,
            "read_file" => Self::ReadFile,
            "write_file" => Self::WriteFile,
            "edit_file" => Self::EditFile,
            "skill" => Self::Skill,
            "web_search" => Self::WebSearch,
            "fetch_content" => Self::FetchContent,
            "get_search_content" => Self::GetSearchContent,
            "questionnaire" => Self::Questionnaire,
            _ => Self::Other,
        }
    }

    /// How many new argument bytes to wait before re-rendering a live preview.
    ///
    /// Zero re-evaluates on every delta so previews track the model as it
    /// writes; identical renders are still suppressed by `last_card`, so the
    /// stride bounds parse cost rather than update rate. Ordinary tool calls
    /// stay under [`PREVIEW_FULL_PARSE_LIMIT`] and re-render delta for delta.
    /// Oversized buffers, including long agent prompts, fall back to a coarse
    /// stride so parse cost stays linear in argument size.
    fn preview_parse_stride(self, arguments_len: usize) -> usize {
        if arguments_len < PREVIEW_FULL_PARSE_LIMIT {
            0
        } else {
            PREVIEW_LARGE_PARSE_STRIDE
        }
    }
}

/// Argument-buffer size above which live previews stop parsing every delta.
///
/// Ordinary tool calls stay far below this and re-render delta for delta. A
/// long `write_file` body would otherwise re-parse the whole buffer thousands
/// of times, so oversized buffers switch to [`PREVIEW_LARGE_PARSE_STRIDE`].
const PREVIEW_FULL_PARSE_LIMIT: usize = 4096;

/// Argument bytes accumulated between parses past [`PREVIEW_FULL_PARSE_LIMIT`].
const PREVIEW_LARGE_PARSE_STRIDE: usize = 4096;

#[derive(Clone, Debug, Default)]
struct StreamedPreview {
    name: Option<String>,
    arguments: String,
    next_parse_length: usize,
    last_args: Option<serde_json::Value>,
    last_card: Option<ToolCard>,
}

pub(crate) struct InteractiveToolPresenter {
    cwd: PathBuf,
    calls: BTreeMap<String, ToolView>,
    streamed: BTreeMap<usize, StreamedPreview>,
}

impl InteractiveToolPresenter {
    pub(crate) fn new(cwd: PathBuf) -> Self {
        Self {
            cwd,
            calls: BTreeMap::new(),
            streamed: BTreeMap::new(),
        }
    }

    /// Discards streamed previews from the previous model step. Provider
    /// tool-call indexes restart at zero every step, so stale entries would
    /// otherwise absorb the next step's argument deltas.
    pub(crate) fn step_started(&mut self) {
        self.streamed.clear();
    }

    pub(crate) fn preview(
        &mut self,
        index: usize,
        name: Option<String>,
        arguments_delta: &str,
    ) -> Option<ToolPresentation> {
        let preview = self.streamed.entry(index).or_default();
        let name_changed = name
            .as_ref()
            .is_some_and(|name| preview.name.as_ref() != Some(name));
        if let Some(name) = name {
            preview.name = Some(name);
        }
        if name_changed {
            preview.arguments.clear();
            preview.next_parse_length = 0;
            preview.last_args = None;
            preview.last_card = None;
        }
        preview.arguments.push_str(arguments_delta);
        // A provider commonly announces a call's identity before sending any
        // arguments. Keep accumulating that canonical stream state, but do not
        // render a bare card while there is nothing useful to preview.
        if preview.arguments.is_empty() {
            return None;
        }
        let name = preview.name.as_deref()?;
        let kind = ToolKind::from_name(name);
        if !name_changed && preview.arguments.len() < preview.next_parse_length {
            return None;
        }
        if let Some(args) = parse_incomplete_json(&preview.arguments) {
            preview.last_args = Some(args);
        }
        let card = match kind {
            // Keep the last successful parse so a mid-stream incomplete fragment
            // does not wipe a useful card back to a bare header.
            ToolKind::Agent => agent_format::agent_streaming_preview_card(
                preview
                    .last_args
                    .as_ref()
                    .unwrap_or(&serde_json::Value::Object(Default::default())),
            ),
            _ => streaming_preview_card(kind, name, preview.last_args.as_ref(), &self.cwd),
        };
        preview.next_parse_length = preview
            .arguments
            .len()
            .saturating_add(kind.preview_parse_stride(preview.arguments.len()));
        if preview.last_card.as_ref() == Some(&card) {
            return None;
        }
        preview.last_card = Some(card.clone());
        // Streaming previews carry no notices or image assets; skip a second
        // argument parse just to assemble ToolPresentation.
        Some(ToolPresentation {
            card,
            image_asset: None,
        })
    }

    pub(crate) fn interrupted(
        &self,
        name: Option<&str>,
        partial_arguments: &str,
    ) -> ToolPresentation {
        let name = name.unwrap_or("tool call");
        let kind = ToolKind::from_name(name);
        let arguments = parse_incomplete_json(partial_arguments)
            .unwrap_or_else(|| serde_json::Value::Object(Default::default()));
        let view = ToolView {
            kind,
            name: name.into(),
            arguments,
            metadata: ToolMetadata::default(),
        };
        presentation(&view, interrupted_card(&view, partial_arguments, &self.cwd))
    }

    pub(crate) fn historical(&self, call: &ToolCall, ok: bool, content: &str) -> ToolPresentation {
        let view = ToolView {
            kind: ToolKind::from_name(&call.name),
            name: call.name.clone(),
            arguments: call.arguments.clone(),
            metadata: ToolMetadata::default(),
        };
        presentation(&view, finished_card(&view, content, ok, &self.cwd))
    }

    pub(crate) fn proposed(&mut self, call: ToolCall) -> ToolPresentation {
        let id = call.id.clone();
        let view = ToolView {
            kind: ToolKind::from_name(&call.name),
            name: call.name,
            arguments: call.arguments,
            metadata: ToolMetadata::default(),
        };
        let presented = presentation(&view, start_card(&view, &self.cwd));
        self.calls.insert(id, view);
        presented
    }

    pub(crate) fn started(
        &mut self,
        call_id: ToolCallId,
        name: String,
        metadata: ToolMetadata,
    ) -> ToolPresentation {
        let id = call_id.to_string();
        let view = self.calls.entry(id).or_insert_with(|| ToolView {
            kind: ToolKind::from_name(&name),
            name: name.clone(),
            arguments: serde_json::Value::Object(Default::default()),
            metadata: metadata.clone(),
        });
        view.kind = ToolKind::from_name(&name);
        view.name = name;
        view.metadata = metadata;
        presentation(view, start_card(view, &self.cwd))
    }

    pub(crate) fn updated(
        &mut self,
        call_id: &ToolCallId,
        progress: &ToolProgress,
    ) -> ToolPresentation {
        if let Some(view) = self.calls.get_mut(&call_id.to_string()) {
            if progress.presentation() != &ToolMetadata::default() {
                view.metadata = progress.presentation().clone();
            }
            let card = progress_card(Some((view, &self.cwd)), progress);
            return presentation(view, card);
        }
        let card = progress_card(None, progress);
        let view = ToolView {
            kind: ToolKind::Other,
            name: "tool".into(),
            arguments: serde_json::Value::Object(Default::default()),
            metadata: ToolMetadata::default(),
        };
        presentation(&view, card)
    }

    pub(crate) fn finished(
        &mut self,
        call_id: &ToolCallId,
        result: ToolCompletion,
    ) -> (bool, ToolPresentation) {
        let mut view = self
            .calls
            .remove(&call_id.to_string())
            .unwrap_or_else(|| ToolView {
                kind: ToolKind::Other,
                name: "tool".into(),
                arguments: serde_json::Value::Object(Default::default()),
                metadata: ToolMetadata::default(),
            });
        let (ok, content) = match result {
            ToolCompletion::Success(output) => {
                if output.presentation() != &ToolMetadata::default() {
                    view.metadata = output.presentation().clone();
                }
                (true, output.content().to_string())
            }
            ToolCompletion::Failure(error) => (false, error.message().to_string()),
            ToolCompletion::Unavailable => (false, "tool is unavailable".into()),
            _ => (false, "unknown tool result".into()),
        };
        let card = finished_card(&view, &content, ok, &self.cwd);
        (ok, presentation(&view, card))
    }
}

