use std::{
    num::NonZeroUsize,
    sync::{Arc, Mutex},
};

use pretty_assertions::assert_eq;
use serde_json::json;

use crate::{
    model::ToolSpec,
    tool::{
        tool_progress_channel, PreparedToolInvocation, Tool, ToolContext, ToolError,
        ToolInvocation, ToolMetadata, ToolOutput, ToolPreparationContext, ToolPrepareFuture,
    },
    ToolHost, ToolHostCall,
};

struct ScheduledTool {
    name: &'static str,
    policy: crate::tool::ToolExecutionPolicy,
    entered: Arc<Mutex<Vec<&'static str>>>,
    release: Arc<tokio::sync::Semaphore>,
}

impl ScheduledTool {
    async fn execute(&self) -> Result<ToolOutput, ToolError> {
        self.entered.lock().unwrap().push(self.name);
        self.release.acquire().await.unwrap().forget();
        Ok(ToolOutput::text(self.name))
    }
}

impl Tool for ScheduledTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.into(),
            description: "controlled scheduler execution".into(),
            input_schema: json!({"type": "object"}),
        }
    }

    fn prepare<'a>(
        &'a self,
        _invocation: ToolInvocation,
        _context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        Box::pin(async move {
            Ok(match &self.policy {
                crate::tool::ToolExecutionPolicy::Exclusive => {
                    PreparedToolInvocation::exclusive(ToolMetadata::new(), move |_context| {
                        Box::pin(self.execute())
                    })
                }
                crate::tool::ToolExecutionPolicy::ResourceAware { accesses } => {
                    PreparedToolInvocation::resource_aware(
                        accesses.clone(),
                        [],
                        ToolMetadata::new(),
                        move |_context| Box::pin(self.execute()),
                    )
                }
            })
        })
    }
}

// Drive the real host worker without spawning, so one explicit poll proves
// admission or blocking rather than relying on a task getting CPU time.
fn scheduled_worker(host: &ToolHost, name: &str) -> crate::tool_host::ToolHostFuture<'static> {
    let call = ToolHostCall::new(name, json!({}));
    let cancellation = crate::CancellationToken::new();
    let (events, _receiver) = tokio::sync::mpsc::channel(host.core.event_capacity.get());
    let (progress, progress_receiver) = tool_progress_channel(host.core.event_capacity);
    let (host_input, host_input_receiver) =
        crate::host_input::channel(host.core.event_capacity.get(), cancellation.clone());
    let context = ToolContext::with_security(
        host.core.workspace.clone(),
        Arc::new(host.core.authorization.for_call()),
        cancellation.clone(),
        progress,
    )
    .with_call_id(call.call_id().clone())
    .with_invocation_source(host.core.invocation_source)
    .with_host_input(host_input);
    Box::pin(
        crate::tool::ToolHostWorker {
            core: Arc::clone(&host.core),
            tool: host.core.tools.get(name).unwrap(),
            call,
            context,
            cancellation,
            events,
            progress: progress_receiver,
            host_input: host_input_receiver,
        }
        .run(),
    )
}

// Covers: child-host calls honor exclusive barriers and conflicting accesses,
// while compatible shared reads overlap. Explicit polling and permits avoid sleeps.
// Owner: SDK ToolHost execution scheduling; planner tests own overlap geometry.
#[tokio::test]
async fn child_host_schedules_exclusive_calls_conflicting_writes_and_shared_reads() {
    use crate::tool::{ToolExecutionPolicy, ToolResource, ToolResourceAccess};
    use std::{future::poll_fn, task::Poll};

    let read = ToolExecutionPolicy::resource_aware([ToolResourceAccess::shared(
        ToolResource::workspace_path("/workspace/file"),
    )]);
    let write = ToolExecutionPolicy::resource_aware([ToolResourceAccess::exclusive(
        ToolResource::workspace_path("/workspace/file"),
    )]);
    for (name, policies, overlap) in [
        (
            "exclusive barrier",
            [read.clone(), ToolExecutionPolicy::Exclusive, read.clone()],
            false,
        ),
        (
            "conflicting writes",
            [write.clone(), write.clone(), write],
            false,
        ),
        ("shared reads", [read.clone(), read.clone(), read], true),
    ] {
        let entered = Arc::new(Mutex::new(Vec::new()));
        let releases =
            ["first", "second", "third"].map(|_| Arc::new(tokio::sync::Semaphore::new(0)));
        let (progress, _receiver) = tool_progress_channel(NonZeroUsize::MIN);
        let context = ToolContext::new(None, crate::CancellationToken::new(), progress);
        let mut builder = ToolHost::child_builder(&context);
        for ((tool_name, policy), release) in ["first", "second", "third"]
            .into_iter()
            .zip(policies)
            .zip(&releases)
        {
            builder = builder.tool(ScheduledTool {
                name: tool_name,
                policy,
                entered: Arc::clone(&entered),
                release: Arc::clone(release),
            });
        }
        let host = builder.build().unwrap();
        let mut workers = ["first", "second", "third"].map(|name| scheduled_worker(&host, name));
        for worker in &mut workers {
            assert!(
                poll_fn(|cx| Poll::Ready(worker.as_mut().poll(cx)))
                    .await
                    .is_pending(),
                "{name}"
            );
        }
        assert_eq!(
            *entered.lock().unwrap(),
            if overlap {
                vec!["first", "second", "third"]
            } else {
                vec!["first"]
            },
            "{name}"
        );
        releases[0].add_permits(1);
        assert_eq!(
            workers[0].as_mut().await.unwrap().content(),
            "first",
            "{name}"
        );
        for worker in &mut workers[1..] {
            assert!(
                poll_fn(|cx| Poll::Ready(worker.as_mut().poll(cx)))
                    .await
                    .is_pending(),
                "{name}"
            );
        }
        assert_eq!(
            *entered.lock().unwrap(),
            if overlap {
                vec!["first", "second", "third"]
            } else {
                vec!["first", "second"]
            },
            "{name}"
        );
        releases[1].add_permits(1);
        assert_eq!(
            workers[1].as_mut().await.unwrap().content(),
            "second",
            "{name}"
        );
        assert!(
            poll_fn(|cx| Poll::Ready(workers[2].as_mut().poll(cx)))
                .await
                .is_pending(),
            "{name}"
        );
        assert_eq!(
            *entered.lock().unwrap(),
            vec!["first", "second", "third"],
            "{name}"
        );
        releases[2].add_permits(1);
        assert_eq!(
            workers[2].as_mut().await.unwrap().content(),
            "third",
            "{name}"
        );
    }
}

// Covers: cancelling a queued exclusive barrier cannot strand compatible calls.
// Owner: SDK execution-arbiter queue lifetime.
#[tokio::test]
async fn cancelled_execution_waiter_releases_its_barrier() {
    use crate::tool::{ExecutionArbiter, ToolExecutionPolicy, ToolResource, ToolResourceAccess};
    use std::{
        future::{poll_fn, Future},
        task::Poll,
    };

    let arbiter = Arc::new(ExecutionArbiter::default());
    let read = ToolExecutionPolicy::resource_aware([ToolResourceAccess::shared(
        ToolResource::session_state(),
    )]);
    let exclusive = ToolExecutionPolicy::Exclusive;
    let first = arbiter.acquire(&read).await;
    let mut barrier = Box::pin(arbiter.acquire(&exclusive));
    let mut later_read = Box::pin(arbiter.acquire(&read));
    assert!(poll_fn(|cx| Poll::Ready(barrier.as_mut().poll(cx)))
        .await
        .is_pending());
    assert!(poll_fn(|cx| Poll::Ready(later_read.as_mut().poll(cx)))
        .await
        .is_pending());

    drop(barrier);

    let Poll::Ready(later) = poll_fn(|cx| Poll::Ready(later_read.as_mut().poll(cx))).await else {
        panic!("a cancelled barrier must release later compatible calls");
    };
    drop(first);
    drop(later);
    let mut final_exclusive = Box::pin(arbiter.acquire(&exclusive));
    assert!(poll_fn(|cx| Poll::Ready(final_exclusive.as_mut().poll(cx)))
        .await
        .is_ready());
}
