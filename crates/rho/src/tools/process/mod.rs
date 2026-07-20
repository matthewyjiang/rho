use std::{future::Future, pin::Pin, sync::Arc};

use rho_sdk::tool::Tool as SdkTool;

use super::sdk_registry::ToolBundle;

mod display;
mod manager;
mod platform;
mod supervisor;
mod tools;
mod types;

pub use manager::ProcessManager;
pub use tools::Process;
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
    let tools = vec![rho_tools::legacy_sdk_adapter::process(
        Process::new(manager.clone()),
        max_output_bytes,
    )
    .expect("process is a supported legacy tool")];
    SdkProcessBundle { tools, manager }
}

#[cfg(test)]
#[path = "process_tests.rs"]
mod tests;
