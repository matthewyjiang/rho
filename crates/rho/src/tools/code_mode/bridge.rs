//! Nested ToolHost execution with inherited authorization and parent event relay.

use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Instant,
};

use futures_util::{stream, StreamExt};
use rho_sdk::tool::{ToolContext, ToolOutput, ToolProgress};
use rho_sdk::{Error as SdkError, ToolHost, ToolHostCall, ToolHostEvent, ToolHostRun};
use serde_json::Value;
use thiserror::Error;

use super::call_log::{NestedCallRecord, NestedCallStatus};
use super::exposure::{search_entries, CodeModeSurface, ToolCatalogEntry};

pub(crate) const CODEMODE_TOOL_NAME: &str = "codemode";
/// Existing runaway-loop tripwire; failures name the limit and requested count.
const MAX_NESTED_CALLS: usize = 64;

#[derive(Debug, Error)]
pub(super) enum BridgeError {
    #[error("codemode: parent call cancelled")]
    Cancelled,
    #[error("codemode: nested call budget exceeded (limit {max}, requested {requested})")]
    CallLimit { max: usize, requested: usize },
    #[error("codemode: ToolHost error: {0}")]
    Host(#[from] SdkError),
}

/// Every nested call uses a child host inheriting policy, approvals, hooks and
/// run identity. The evaluator lives on a blocking thread, so awaited progress
/// cannot starve the parent task which drains events and answers host questions.
pub(super) struct ToolHostBridge {
    host: ToolHost,
    parent: ToolContext,
    catalog: Vec<ToolCatalogEntry>,
    calls: AtomicUsize,
    /// Held while a rendered snapshot is sent, so concurrent calls cannot
    /// deliver an older snapshot after a newer one.
    log: tokio::sync::Mutex<Vec<NestedCallRecord>>,
}

impl ToolHostBridge {
    pub(super) fn new(
        surface: Arc<CodeModeSurface>,
        parent: ToolContext,
    ) -> Result<Self, rho_sdk::tool::ToolError> {
        let (host, catalog) = surface.snapshot(&parent)?;
        Ok(Self {
            host,
            parent,
            catalog,
            calls: AtomicUsize::new(0),
            log: tokio::sync::Mutex::default(),
        })
    }

    pub(super) fn search(&self, query: &str, limit: usize) -> Vec<ToolCatalogEntry> {
        search_entries(self.catalog.iter(), query, limit)
    }

    pub(super) fn describe(&self, name: &str) -> Option<ToolCatalogEntry> {
        self.catalog
            .iter()
            .find(|entry| entry.name == name)
            .cloned()
    }

    #[cfg(test)]
    pub(super) fn call_count(&self) -> usize {
        self.calls.load(Ordering::Relaxed)
    }

    /// Nested calls that started; unlike `calls`, excludes budget rejections.
    pub(super) async fn started_calls(&self) -> usize {
        self.log.lock().await.len()
    }

    pub(super) async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> Result<ToolOutput, BridgeError> {
        self.reserve(1)?;
        self.run_call(name.to_owned(), arguments).await
    }

    /// Runs independent calls concurrently and returns their results in input
    /// order. The whole batch is checked against the call budget before any
    /// call starts, and a cancelled parent fails the batch.
    pub(super) async fn call_tools(
        &self,
        calls: Vec<(String, Value)>,
    ) -> Result<Vec<Result<ToolOutput, BridgeError>>, BridgeError> {
        self.reserve(calls.len())?;
        let results: Vec<_> = stream::iter(calls)
            .map(|(name, arguments)| self.run_call(name, arguments))
            // Same width as a model-issued parallel tool batch.
            .buffered(crate::app::sdk_config::parallel_tool_limit().get())
            .collect()
            .await;
        if self.parent.cancellation().is_cancelled() {
            return Err(BridgeError::Cancelled);
        }
        Ok(results)
    }

    fn reserve(&self, count: usize) -> Result<(), BridgeError> {
        if self.parent.cancellation().is_cancelled() {
            return Err(BridgeError::Cancelled);
        }
        let requested = self.calls.fetch_add(count, Ordering::Relaxed) + count;
        if requested > MAX_NESTED_CALLS {
            return Err(BridgeError::CallLimit {
                max: MAX_NESTED_CALLS,
                requested,
            });
        }
        Ok(())
    }

    async fn run_call(&self, name: String, arguments: Value) -> Result<ToolOutput, BridgeError> {
        if self.parent.cancellation().is_cancelled() {
            return Err(BridgeError::Cancelled);
        }
        let started = Instant::now();
        let record = NestedCallRecord::running(&name, &arguments);
        let index = {
            let mut log = self.log.lock().await;
            log.push(record);
            log.len() - 1
        };
        // Unknown (including recursive) names are rejected by the child host.
        let result = match self.host.start(ToolHostCall::new(name, arguments)) {
            Ok(run) => {
                self.report(index, |_| {}).await;
                self.run_nested(run, index).await.map_err(BridgeError::from)
            }
            Err(error) => Err(error.into()),
        };
        let cancelled = self.parent.cancellation().is_cancelled();
        self.report(index, |record| {
            record.duration_ms = Some(started.elapsed().as_millis() as u64);
            match &result {
                Ok(output) if !output.is_failure() => {
                    record.status = NestedCallStatus::Ok;
                    record.detail = None;
                }
                Ok(output) => {
                    record.status = NestedCallStatus::Error;
                    record.set_error(output.content());
                }
                Err(_) if cancelled => record.status = NestedCallStatus::Cancelled,
                Err(BridgeError::Host(SdkError::Cancelled)) => {
                    record.status = NestedCallStatus::Cancelled;
                }
                Err(error) => {
                    record.status = NestedCallStatus::Error;
                    record.set_error(&error.to_string());
                }
            }
        })
        .await;
        result
    }

    /// Updates one record, then relays every row as the parent's progress text.
    async fn report(&self, index: usize, update: impl FnOnce(&mut NestedCallRecord)) {
        let mut log = self.log.lock().await;
        update(&mut log[index]);
        let rendered = log
            .iter()
            .map(NestedCallRecord::row)
            .collect::<Vec<_>>()
            .join("\n");
        let _ = self
            .parent
            .progress()
            .send(ToolProgress::message(rendered))
            .await;
    }

    async fn run_nested(&self, mut run: ToolHostRun, index: usize) -> Result<ToolOutput, SdkError> {
        let cancellation = self.parent.cancellation().clone();
        loop {
            tokio::select! {
                biased;
                () = cancellation.cancelled() => {
                    run.cancel();
                    return run.outcome().await;
                }
                event = run.next_event() => match event {
                    Some(ToolHostEvent::Progress(progress)) => {
                        self.report(index, |record| record.set_progress(progress.text())).await;
                    }
                    Some(ToolHostEvent::HostInputRequested(mut pending)) => {
                        if let Ok(response) = self.parent.request_host_input(pending.request().clone()).await {
                            let _ = pending.respond(response);
                        }
                    }
                    Some(_) => {}
                    None => return run.outcome().await,
                },
            }
        }
    }
}
