//! The UI-free session runtime: one per thread, owning a built session, its
//! file, and the state between prompts.
//!
//! A host drives it and shows what it reports. ACP (`app::acp`) is the host
//! today; the daemon and `rho run` host it next. Nothing here knows a wire
//! protocol: hosts parse their own requests and map the runtime's results and
//! errors back to theirs.

mod prompt_gate;

pub(crate) use prompt_gate::{Closing, PromptGate, PromptGuard};
