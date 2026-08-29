use std::sync::{Arc, OnceLock};

use crate::CapabilityRequest;

/// First capability a tool call passed to [`super::ToolContext::authorize`].
///
/// Shared across the context clone used for prepared-capability authorization
/// and the copy that outlives `execute`, so `after_tool_use` can report it.
#[derive(Clone, Debug, Default)]
pub(crate) struct FirstCapability {
    inner: Arc<OnceLock<CapabilityRequest>>,
}

impl FirstCapability {
    pub(crate) fn record(&self, request: &CapabilityRequest) {
        let _ = self.inner.set(request.clone());
    }

    pub(crate) fn get(&self) -> Option<&CapabilityRequest> {
        self.inner.get()
    }
}
