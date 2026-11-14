//! Prompt context that has not reached a model yet.
//!
//! A prompt whose command runs no model turn, such as `/compact` or a
//! `/workflow:` start, or that ends before its turn runs, leaves its
//! context with the session for its next model turn. T3 Code states its
//! instructions on a thread's first prompt alone, so a thread that opens
//! with a command still gets them. The session holds it in memory only: a
//! session the daemon unloads drops it.

use std::sync::{Arc, PoisonError};

use super::LoadedSession;
use crate::app::prompt_content::PromptContext;

/// A prompt's context until a model turn sends it. Dropped unsent, it goes
/// back to its session for the next prompt.
pub(super) struct UnsentContext {
    session: Arc<LoadedSession>,
    context: Option<PromptContext>,
}

impl UnsentContext {
    /// `context` after the context `session` held, which it takes.
    pub(super) fn take(session: &Arc<LoadedSession>, context: PromptContext) -> Self {
        let held = std::mem::take(&mut *session.lock_held_context());
        Self {
            session: Arc::clone(session),
            context: Some(context.after(held)),
        }
    }

    /// The context, for the model input of the turn that sends it.
    pub(super) fn send(mut self) -> PromptContext {
        self.context.take().unwrap_or_default()
    }
}

impl Drop for UnsentContext {
    fn drop(&mut self) {
        if let Some(context) = self.context.take() {
            *self.session.lock_held_context() = context;
        }
    }
}

impl LoadedSession {
    fn lock_held_context(&self) -> std::sync::MutexGuard<'_, PromptContext> {
        self.held_context
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}
