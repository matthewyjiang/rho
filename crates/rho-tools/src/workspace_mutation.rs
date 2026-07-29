use std::{future::Future, path::Path, pin::Pin};

/// A workspace effect that a native mutation observer cannot restore.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UntrackedWorkspaceEffect {
    ShellCommand,
    MutatingTool,
}

/// Host-owned boundary for tracking native workspace mutations.
///
/// Implementors must capture each path's state before `before_mutation`
/// succeeds. Tools call `after_mutation` after the write attempt so the host can
/// record the state that restoration must later expect.
pub trait WorkspaceMutationObserver: Send + Sync {
    fn before_mutation<'a>(
        &'a self,
        paths: &'a [&'a Path],
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;

    fn after_mutation<'a>(
        &'a self,
        paths: &'a [&'a Path],
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;

    /// Records a tool effect whose workspace changes cannot be tracked.
    fn mark_untracked_effect(&self, kind: UntrackedWorkspaceEffect, source: &str);
}

impl WorkspaceMutationObserver for () {
    fn before_mutation<'a>(
        &'a self,
        _paths: &'a [&'a Path],
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    fn after_mutation<'a>(
        &'a self,
        _paths: &'a [&'a Path],
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    fn mark_untracked_effect(&self, _kind: UntrackedWorkspaceEffect, _source: &str) {}
}
