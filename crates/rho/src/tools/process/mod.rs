use std::{future::Future, pin::Pin, sync::Arc};

use rho_sdk::tool::Tool as SdkTool;

use super::sdk_registry::ToolBundle;

mod display;
mod manager;
mod platform;
pub(super) mod sdk_process;
mod supervisor;
mod tools;
mod types;

pub use manager::ProcessManager;
pub use tools::Process;
pub(super) use tools::ProcessArgs;
pub use types::{Chunk, ProcessLimits, Snapshot, State};

pub(super) struct SdkProcessBundle {
    tools: Vec<Arc<dyn SdkTool>>,
    manager: ProcessManager,
}

impl ToolBundle for SdkProcessBundle {
    fn tools(&self) -> &[Arc<dyn SdkTool>] {
        &self.tools
    }

    fn shutdown(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(self.manager.shutdown())
    }
}

pub(super) fn sdk_bundle(max_output_bytes: usize) -> SdkProcessBundle {
    let manager = ProcessManager::new(ProcessLimits {
        max_bytes: max_output_bytes,
        ..ProcessLimits::default()
    });
    let tools = vec![sdk_process::tool(
        Process::new(manager.clone()),
        max_output_bytes,
    )];
    SdkProcessBundle { tools, manager }
}

#[cfg(test)]
#[path = "process_tests.rs"]
mod tests;
