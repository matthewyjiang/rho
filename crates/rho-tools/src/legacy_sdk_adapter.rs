//! Compatibility adapter for application [`Tool`](crate::tool::Tool) implementations.
//!
//! New tools should implement the SDK contract directly. This adapter keeps
//! the fixed set of older tools used by Rho usable while preserving SDK
//! authorization, progress, metadata, cancellation, and error behavior.

use std::{fmt, sync::Arc};

use rho_sdk::tool::{
    OperationKind, Tool as SdkTool, ToolContext as SdkToolContext, ToolError as SdkToolError,
    ToolErrorKind, ToolFuture, ToolInvocation, ToolMetadata, ToolOutput, ToolProgress,
    ToolSecurity,
};

use crate::{
    sdk_security::{authorize_legacy, legacy_security_for},
    sdk_support::{check_cancelled, workspace_root},
    tool::{Tool, ToolContext, ToolError},
};

macro_rules! legacy_adapter {
    ($constructor:ident, $profile:ident) => {
        #[doc = concat!("Adapts Rho's trusted `", stringify!($profile), "` legacy tool.")]
        pub fn $constructor<T>(
            tool: T,
            max_output_bytes: usize,
        ) -> Result<Arc<dyn SdkTool>, AdaptError>
        where
            T: Tool + 'static,
        {
            adapt_with_profile(tool, max_output_bytes, LegacyToolProfile::$profile)
        }
    };
}

legacy_adapter!(agent, Agent);
legacy_adapter!(agents, Agents);
legacy_adapter!(process, Process);
legacy_adapter!(rho, Rho);
legacy_adapter!(web_search, WebSearch);
legacy_adapter!(get_search_content, GetSearchContent);

fn adapt_with_profile<T>(
    tool: T,
    max_output_bytes: usize,
    profile: LegacyToolProfile,
) -> Result<Arc<dyn SdkTool>, AdaptError>
where
    T: Tool + 'static,
{
    let spec = tool.spec();
    if spec.name != profile.name() {
        return Err(AdaptError {
            expected: profile.name(),
            actual: spec.name,
        });
    }
    Ok(Arc::new(LegacySdkTool {
        inner: tool,
        spec,
        profile,
        max_output_bytes,
    }))
}

/// Error returned when a legacy tool does not match its trusted feature profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdaptError {
    expected: &'static str,
    actual: String,
}

impl fmt::Display for AdaptError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "legacy tool profile '{}' does not match wrapped tool '{}'",
            self.expected, self.actual
        )
    }
}

impl std::error::Error for AdaptError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LegacyToolProfile {
    Agent,
    Agents,
    Process,
    Rho,
    WebSearch,
    GetSearchContent,
}

impl LegacyToolProfile {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Agents => "agents",
            Self::Process => "process",
            Self::Rho => "rho",
            Self::WebSearch => "web_search",
            Self::GetSearchContent => "get_search_content",
        }
    }
}

struct LegacySdkTool<T> {
    inner: T,
    spec: rho_sdk::model::ToolSpec,
    profile: LegacyToolProfile,
    max_output_bytes: usize,
}

impl<T> SdkTool for LegacySdkTool<T>
where
    T: Tool + 'static,
{
    fn spec(&self) -> rho_sdk::model::ToolSpec {
        self.spec.clone()
    }

    fn security(&self) -> ToolSecurity {
        legacy_security_for(self.profile)
    }

    fn start_metadata(&self, _arguments: &serde_json::Value) -> ToolMetadata {
        metadata_for(self.profile)
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, context: SdkToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            check_cancelled(&context)?;
            authorize_legacy(
                self.profile,
                invocation.arguments(),
                &context,
                self.max_output_bytes,
            )
            .await?;
            let cwd = workspace_root(&context)?;
            let app_context = ToolContext {
                cwd: cwd.to_path_buf(),
                max_output_bytes: self.max_output_bytes,
            };
            let (update_sender, mut updates) =
                tokio::sync::mpsc::unbounded_channel::<Vec<String>>();
            let mut on_update = move |lines: Vec<String>| {
                let _ = update_sender.send(lines);
            };
            let call = self.inner.call_with_updates_and_cancellation(
                invocation.arguments().clone(),
                app_context,
                invocation.id().to_string(),
                context.cancellation().clone(),
                &mut on_update,
            );
            tokio::pin!(call);
            let mut updates_open = true;
            let result = loop {
                tokio::select! {
                    result = &mut call => break result,
                    update = updates.recv(), if updates_open => {
                        match update {
                            Some(lines) => {
                                let _ = context
                                    .progress()
                                    .send(ToolProgress::message(lines.join("\n")))
                                    .await;
                            }
                            None => updates_open = false,
                        }
                    }
                }
            };
            while let Ok(lines) = updates.try_recv() {
                let _ = context
                    .progress()
                    .send(ToolProgress::message(lines.join("\n")))
                    .await;
            }
            let result = result.map_err(map_app_error)?;
            if !result.ok {
                return Err(SdkToolError::new(ToolErrorKind::Execution, result.content));
            }
            Ok(ToolOutput::text(result.content).metadata(metadata_for(self.profile)))
        })
    }
}

fn metadata_for(profile: LegacyToolProfile) -> ToolMetadata {
    let operation = match profile {
        LegacyToolProfile::Process => OperationKind::Execute,
        LegacyToolProfile::WebSearch => OperationKind::Network,
        LegacyToolProfile::GetSearchContent => OperationKind::Read,
        _ => OperationKind::Other(profile.name().to_string()),
    };
    ToolMetadata::new().operation(operation)
}

fn map_app_error(error: ToolError) -> SdkToolError {
    match &error {
        ToolError::InvalidArguments(_) => {
            SdkToolError::new(ToolErrorKind::InvalidArguments, error.to_string())
        }
        ToolError::Message(message) if message == "tool interrupted" => SdkToolError::cancelled(),
        ToolError::Io(_) | ToolError::Utf8(_) | ToolError::Message(_) => {
            SdkToolError::new(ToolErrorKind::Execution, error.to_string())
        }
    }
}

#[cfg(test)]
#[path = "legacy_sdk_adapter_tests.rs"]
mod tests;
