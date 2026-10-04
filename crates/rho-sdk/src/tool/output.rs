use std::{fmt, path::PathBuf, sync::Arc};

use serde_json::Value;

use super::OperationKind;
use crate::AuthorizationError;

/// Immutable binary data produced by a tool.
///
/// Hosts may interpret assets according to their media type. The SDK does not
/// prescribe how, or whether, they are presented.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolAsset {
    media_type: String,
    bytes: Arc<[u8]>,
}

impl ToolAsset {
    pub fn new(media_type: impl Into<String>, bytes: impl Into<Arc<[u8]>>) -> Self {
        Self {
            media_type: media_type.into(),
            bytes: bytes.into(),
        }
    }

    pub fn media_type(&self) -> &str {
        &self.media_type
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Structured presentation metadata for a tool result or progress update.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolMetadata {
    operation: Option<OperationKind>,
    affected_paths: Vec<PathBuf>,
    command_summary: Option<String>,
    urls: Vec<String>,
    diff: Option<String>,
    assets: Vec<ToolAsset>,
    presentation_notices: Vec<String>,
}

impl ToolMetadata {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn operation(mut self, operation: OperationKind) -> Self {
        self.operation = Some(operation);
        self
    }

    pub fn affected_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.affected_paths.push(path.into());
        self
    }

    pub fn command_summary(mut self, summary: impl Into<String>) -> Self {
        self.command_summary = Some(summary.into());
        self
    }

    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.urls.push(url.into());
        self
    }

    pub fn diff(mut self, diff: impl Into<String>) -> Self {
        self.diff = Some(diff.into());
        self
    }

    /// Attaches immutable binary data produced by the tool.
    pub fn asset(mut self, asset: ToolAsset) -> Self {
        self.assets.push(asset);
        self
    }

    /// Adds a host-facing notice that is not included in model-visible output.
    pub fn presentation_notice(mut self, notice: impl Into<String>) -> Self {
        self.presentation_notices.push(notice.into());
        self
    }

    pub fn operation_kind(&self) -> Option<&OperationKind> {
        self.operation.as_ref()
    }

    pub fn affected_paths(&self) -> &[PathBuf] {
        &self.affected_paths
    }

    pub fn command_summary_text(&self) -> Option<&str> {
        self.command_summary.as_deref()
    }

    pub fn urls(&self) -> &[String] {
        &self.urls
    }

    pub fn unified_diff(&self) -> Option<&str> {
        self.diff.as_deref()
    }

    pub fn assets(&self) -> &[ToolAsset] {
        &self.assets
    }

    pub fn presentation_notices(&self) -> &[String] {
        &self.presentation_notices
    }
}

/// Output of a completed tool call, including calls whose result reports failure.
///
/// Return `Ok(ToolOutput::text(...).failed())` for a completed failed result;
/// reserve [`ToolError`] for calls that could not produce a completed result.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolOutput {
    content: String,
    metadata: ToolMetadata,
    images: Vec<crate::model::ImageContent>,
    failure: bool,
    /// Boxed: rare, and keeps `ToolCompletion` variants close in size.
    extras: Option<Box<OutputExtras>>,
}

/// Rarely present parts of a [`ToolOutput`], kept behind one allocation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct OutputExtras {
    structured: Option<Value>,
    process: Option<ProcessResult>,
}

impl ToolOutput {
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            metadata: ToolMetadata::default(),
            images: Vec::new(),
            failure: false,
            extras: None,
        }
    }

    fn extras_mut(&mut self) -> &mut OutputExtras {
        self.extras.get_or_insert_with(Box::default)
    }

    /// Marks a completed result as a failure for the model and lifecycle hooks.
    ///
    /// The call still returns `Ok` and retains its output for programmatic callers.
    /// Denials, cancellation, invalid arguments, and execution errors without a
    /// completed result must return [`ToolError`] instead.
    pub fn failed(mut self) -> Self {
        self.failure = true;
        self
    }

    /// Whether this completed result should be presented to the model as an error.
    pub fn is_failure(&self) -> bool {
        self.failure
    }

    /// Attaches a machine-readable result for programmatic callers.
    ///
    /// Retained successful content should match [`Tool::output_schema`], when
    /// declared; failed content is not constrained by that schema. Result budget
    /// limits may discard structured content, so callers must handle its absence.
    /// The model still receives [`Self::content`].
    pub fn with_structured_content(mut self, structured: Value) -> Self {
        self.extras_mut().structured = Some(structured);
        self
    }

    /// Machine-readable result, when the tool produced one.
    pub fn structured_content(&self) -> Option<&Value> {
        self.extras.as_ref()?.structured.as_ref()
    }

    pub fn metadata(mut self, metadata: ToolMetadata) -> Self {
        self.metadata = metadata;
        self
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    /// Attaches images to this completed output, replacing any prior images.
    ///
    /// The runtime delivers images as attributed, untrusted supplemental user content
    /// after all outstanding tool calls have paired results. Cancelled batches discard
    /// undelivered images. Hosts can read them from `ToolCompletion::Success` and
    /// `ToolCompletion::CompletedFailure`; failed output retains images but does
    /// not deliver them to the model.
    ///
    /// # Next major
    ///
    /// NEXT_MAJOR(rho-sdk): represent images in structured tool results instead of supplemental user messages.
    /// Adding fields to `ToolResult` or variants to `Message` would break minor compatibility.
    pub fn with_images(mut self, images: Vec<crate::model::ImageContent>) -> Self {
        self.images = images;
        self
    }

    /// Images returned by the tool, in output order.
    pub fn images(&self) -> &[crate::model::ImageContent] {
        &self.images
    }

    pub fn presentation(&self) -> &ToolMetadata {
        &self.metadata
    }

    /// Records how the one process this call ran exited.
    ///
    /// Process-running tools attach this so observers such as lifecycle hooks
    /// read exit status and streams directly instead of parsing
    /// [`Self::content`]. The model still receives only [`Self::content`].
    pub fn with_process_result(mut self, process: ProcessResult) -> Self {
        self.extras_mut().process = Some(process);
        self
    }

    /// Exit status and streams of the process this call ran, when it ran one.
    pub fn process_result(&self) -> Option<&ProcessResult> {
        self.extras.as_ref()?.process.as_ref()
    }
}

/// Exit status and retained output of a finished process.
///
/// `stdout` and `stderr` hold what the tool retained under its own output
/// budget, which may be shorter than what the process wrote.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProcessResult {
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl ProcessResult {
    /// `exit_code` is `None` when the process ended without one, for example
    /// when a signal terminated it.
    pub fn new(
        exit_code: Option<i32>,
        stdout: impl Into<String>,
        stderr: impl Into<String>,
    ) -> Self {
        Self {
            exit_code,
            stdout: stdout.into(),
            stderr: stderr.into(),
        }
    }

    pub fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    pub fn stdout(&self) -> &str {
        &self.stdout
    }

    pub fn stderr(&self) -> &str {
        &self.stderr
    }
}

/// Tool failure category independent of an implementation's internal errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolErrorKind {
    InvalidArguments,
    Execution,
    PolicyDenied,
    Cancelled,
}

/// Sanitized failure returned by a tool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolError {
    kind: ToolErrorKind,
    message: String,
}

impl ToolError {
    pub fn new(kind: ToolErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn kind(&self) -> ToolErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn policy_denied(error: &AuthorizationError) -> Self {
        Self::new(
            ToolErrorKind::PolicyDenied,
            format!(
                "{} capability denied: {}",
                error.capability().label(),
                error.message()
            ),
        )
    }

    pub fn cancelled() -> Self {
        Self::new(ToolErrorKind::Cancelled, "tool call cancelled")
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "tool failed: {}", self.message)
    }
}

impl std::error::Error for ToolError {}
