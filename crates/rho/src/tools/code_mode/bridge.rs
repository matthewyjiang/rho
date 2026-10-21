//! Nested ToolHost execution with inherited authorization and parent event relay.

use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, OnceLock,
    },
    time::Instant,
};

use futures_util::future::join_all;
use rho_sdk::tool::{ToolContext, ToolOutput, ToolProgress};
use rho_sdk::{Error as SdkError, ToolHost, ToolHostCall, ToolHostEvent, ToolHostRun};
use serde_json::Value;
use thiserror::Error;

use super::call_log::{NestedCallRecord, NestedCallStatus};
use super::exposure::{CodeModeSurface, ScriptTool, ToolCatalogEntry};
use super::search::search_entries;
use crate::tools::mcp::exported_name::resolve_foreign_spelling;

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
    /// Handles freeze with the child host, while contracts remain live until
    /// the first list/search/describe operation in this script.
    tools: Vec<ScriptTool>,
    catalog: OnceLock<Vec<ToolCatalogEntry>>,
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
        let (host, tools) = surface.snapshot(&parent)?;
        Ok(Self {
            host,
            parent,
            tools,
            catalog: OnceLock::new(),
            calls: AtomicUsize::new(0),
            log: tokio::sync::Mutex::default(),
        })
    }

    pub(super) fn search(&self, query: &str, limit: usize) -> Vec<&ToolCatalogEntry> {
        search_entries(self.catalog(), query, limit)
    }

    pub(super) fn describe(&self, name: &str) -> Option<&ToolCatalogEntry> {
        let name = self.resolve(name);
        let catalog = self.catalog();
        catalog
            .binary_search_by(|entry| entry.name.as_str().cmp(name))
            .ok()
            .map(|index| &catalog[index])
    }

    /// The registered name a script means. Another harness's MCP spelling, such
    /// as T3 Code's `mcp__t3_code__<tool>`, resolves when it names exactly one
    /// tool; anything else, including an exact name, passes through unchanged.
    /// Nested hosts, hooks, and approvals only ever see the registered name.
    /// This is the only name-shaped policy codemode applies, and the MCP
    /// module owns it.
    fn resolve<'a>(&'a self, name: &'a str) -> &'a str {
        resolve_foreign_spelling(name, self.tools.iter().map(ScriptTool::name)).unwrap_or(name)
    }

    fn catalog(&self) -> &[ToolCatalogEntry] {
        self.catalog.get_or_init(|| {
            let mut entries: Vec<_> = self.tools.iter().map(ScriptTool::catalog_entry).collect();
            entries.sort_by(|a, b| a.name.cmp(&b.name));
            entries
        })
    }

    /// The evaluator and native tool waits share the parent cancellation token.
    pub(super) fn cancellation(&self) -> &rho_sdk::CancellationToken {
        self.parent.cancellation()
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
    ///
    /// Every call starts at once, so all of them prepare and await approval
    /// together, as a model-issued batch does. The child host bounds how many
    /// execute at once.
    pub(super) async fn call_tools(
        &self,
        calls: Vec<(String, Value)>,
    ) -> Result<Vec<Result<ToolOutput, BridgeError>>, BridgeError> {
        self.reserve(calls.len())?;
        let results = join_all(
            calls
                .into_iter()
                .map(|(name, arguments)| self.run_call(name, arguments)),
        )
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
        let name = self.resolve(&name).to_owned();
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
