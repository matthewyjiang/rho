use std::{future::Future, pin::Pin, sync::Arc};

use rho_sdk::tool::Tool as SdkTool;

use super::sdk_registry::ToolBundle;

mod display;
mod exact;
mod manager;
mod notify;
mod output;
mod platform;
pub(super) mod sdk_process;
mod supervisor;
mod tools;
mod types;

pub(crate) use exact::{ExactProcessExit, WorkflowCommandTool};
pub use manager::ProcessManager;
pub(crate) use notify::{notification_prompts, ProcessNotification};
pub(crate) use output::decode_header_value;
pub(crate) use platform::{prepare_child_command, ProcessTree};
pub use tools::Process;
pub(super) use tools::ProcessArgs;
pub use types::{Chunk, ProcessLimits, Snapshot, State};
pub(crate) use types::{HostProcessView, LiveProcessSummary, Stream};

pub(super) struct SdkProcessBundle {
    tools: Vec<Arc<dyn SdkTool>>,
    manager: ProcessManager,
}

impl SdkProcessBundle {
    pub(super) fn manager_handle(&self) -> ProcessManager {
        self.manager.clone()
    }
}

impl ToolBundle for SdkProcessBundle {
    fn tools(&self) -> &[Arc<dyn SdkTool>] {
        &self.tools
    }

    fn shutdown(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(self.manager.shutdown())
    }
}

pub(super) fn sdk_bundle(
    max_output_bytes: usize,
    environment: rho_sdk::ProcessEnvironment,
    mutation_observer: Arc<dyn rho_tools::WorkspaceMutationObserver>,
) -> SdkProcessBundle {
    let manager = ProcessManager::with_environment(
        ProcessLimits {
            max_bytes: max_output_bytes,
            ..ProcessLimits::default()
        },
        environment.clone(),
    );
    let tools = vec![sdk_process::tool(
        Process::new(manager.clone()),
        max_output_bytes,
        environment,
        mutation_observer,
    )];
    SdkProcessBundle { tools, manager }
}

#[cfg(test)]
#[path = "process_tests.rs"]
mod tests;
