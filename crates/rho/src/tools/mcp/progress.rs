//! Routes MCP `notifications/progress` into the SDK tool-progress channel.
//!
//! rmcp stamps every outbound request with a progress token, so a server can
//! report progress against any in-flight call. The router maps the token of a
//! live `tools/call` to that invocation's [`ToolProgressSender`], which is what
//! renders the live line on a tool card. Subscriptions are scoped to the call
//! through [`ProgressSubscription`], so a finished or cancelled call always
//! stops receiving.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use rho_sdk::tool::{ToolProgress, ToolProgressSender};
use rmcp::model::{ProgressNotificationParam, ProgressToken};

/// Live progress subscriptions for one MCP session.
#[derive(Clone, Debug, Default)]
pub(crate) struct McpProgressRouter {
    subscribers: Arc<Mutex<HashMap<ProgressToken, ToolProgressSender>>>,
}

impl McpProgressRouter {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Route progress for `token` to `sender` until the guard drops.
    pub(crate) fn subscribe(
        &self,
        token: ProgressToken,
        sender: ToolProgressSender,
    ) -> ProgressSubscription {
        self.lock().insert(token.clone(), sender);
        ProgressSubscription {
            token,
            router: self.clone(),
        }
    }

    pub(crate) async fn dispatch(&self, params: ProgressNotificationParam) {
        let Some(sender) = self.lock().get(&params.progress_token).cloned() else {
            return;
        };
        // The lock is released before the await: `send` applies backpressure and
        // must never hold the map across a suspension point.
        sender.send(tool_progress(params)).await;
    }

    fn unsubscribe(&self, token: &ProgressToken) {
        self.lock().remove(token);
    }

    /// A poisoned lock means a panic while a subscription map was borrowed. The
    /// map stays structurally valid, so recover rather than fail a tool call.
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<ProgressToken, ToolProgressSender>> {
        self.subscribers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }
}

/// Drops the routing entry when one tool call ends.
pub(crate) struct ProgressSubscription {
    token: ProgressToken,
    router: McpProgressRouter,
}

impl Drop for ProgressSubscription {
    fn drop(&mut self) {
        self.router.unsubscribe(&self.token);
    }
}

/// MCP progress is a float pair; the SDK card shows whole units. Counts are
/// only forwarded when the server supplies a total, because a bare growing
/// float has nothing to render against.
fn tool_progress(params: ProgressNotificationParam) -> ToolProgress {
    let message = params
        .message
        .unwrap_or_else(|| "MCP server reported progress".into());
    let progress = ToolProgress::message(message);
    match params.total {
        Some(total) if total.is_finite() && total > 0.0 => {
            progress.units(whole_units(params.progress), whole_units(total))
        }
        _ => progress,
    }
}

fn whole_units(value: f64) -> u64 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    value.round() as u64
}

#[cfg(test)]
#[path = "progress_tests.rs"]
mod tests;
