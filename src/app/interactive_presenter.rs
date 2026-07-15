use std::{collections::BTreeMap, path::PathBuf};

use rho_sdk::{
    model::ToolCall,
    tool::{ToolMetadata, ToolProgress},
    ToolCallId, ToolCompletion,
};

use crate::tool::ToolDisplayStyle;

#[path = "interactive_presenter_format.rs"]
mod format;
use format::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ToolPresentation {
    pub(crate) command: Option<String>,
    pub(crate) display_style: ToolDisplayStyle,
    pub(crate) display_lines: Vec<String>,
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
    Bash,
    PowerShell,
    Process,
    ListDir,
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
            "bash" => Self::Bash,
            "powershell" => Self::PowerShell,
            "process" => Self::Process,
            "list_dir" => Self::ListDir,
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

    fn display_style(self, metadata: &ToolMetadata) -> ToolDisplayStyle {
        match self {
            Self::Bash | Self::PowerShell | Self::ListDir | Self::ReadFile => {
                ToolDisplayStyle::file_or_command()
            }
            Self::WriteFile | Self::EditFile => ToolDisplayStyle::file_diff(),
            Self::Skill => ToolDisplayStyle::skill(),
            Self::WebSearch | Self::FetchContent | Self::GetSearchContent => {
                ToolDisplayStyle::web()
            }
            Self::Questionnaire => ToolDisplayStyle::questionnaire(),
            Self::Process | Self::Other => style_from_metadata(metadata),
        }
    }

    fn preview_uses_arguments(self) -> bool {
        !matches!(self, Self::WriteFile | Self::EditFile)
    }
}

#[derive(Clone, Debug, Default)]
struct StreamedPreview {
    name: Option<String>,
    arguments: String,
    next_parse_length: usize,
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

    pub(crate) fn preview(
        &mut self,
        index: usize,
        name: Option<String>,
        arguments_delta: &str,
    ) -> Option<Vec<String>> {
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
        }
        preview.arguments.push_str(arguments_delta);
        let name = preview.name.as_deref()?;
        let kind = ToolKind::from_name(name);
        if !name_changed
            && (!kind.preview_uses_arguments()
                || preview.arguments.len() < preview.next_parse_length)
        {
            return None;
        }
        let arguments = parse_incomplete_json(&preview.arguments);
        let lines = preview_lines(kind, name, arguments.as_ref(), &self.cwd);
        preview.next_parse_length = preview
            .arguments
            .len()
            .saturating_add(preview.arguments.len().max(1));
        Some(lines)
    }

    pub(crate) fn historical(&self, call: &ToolCall, ok: bool, content: &str) -> ToolPresentation {
        let view = ToolView {
            kind: ToolKind::from_name(&call.name),
            name: call.name.clone(),
            arguments: call.arguments.clone(),
            metadata: ToolMetadata::default(),
        };
        let lines = finished_lines(&view, content, ok, &self.cwd);
        presentation(&view, lines)
    }

    pub(crate) fn proposed(&mut self, call: ToolCall) -> Vec<String> {
        let id = call.id.clone();
        let view = ToolView {
            kind: ToolKind::from_name(&call.name),
            name: call.name,
            arguments: call.arguments,
            metadata: ToolMetadata::default(),
        };
        let lines = start_lines(&view, &self.cwd);
        self.calls.insert(id, view);
        lines
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
        presentation(view, start_lines(view, &self.cwd))
    }

    pub(crate) fn updated(&mut self, call_id: &ToolCallId, progress: &ToolProgress) -> Vec<String> {
        let Some(view) = self.calls.get_mut(&call_id.to_string()) else {
            return progress_lines(None, progress);
        };
        if progress.presentation() != &ToolMetadata::default() {
            view.metadata = progress.presentation().clone();
        }
        progress_lines(Some((view, &self.cwd)), progress)
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
        let lines = finished_lines(&view, &content, ok, &self.cwd);
        (ok, presentation(&view, lines))
    }
}

#[cfg(test)]
#[path = "interactive_presenter_tests.rs"]
mod tests;
