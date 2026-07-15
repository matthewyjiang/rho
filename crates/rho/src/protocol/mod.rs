//! Wire protocols implemented by model providers.
//!
//! Protocol modules translate Rho's canonical model request and response types
//! to API-specific wire shapes. They do not own credentials, endpoint selection,
//! retry policy, or provider registration.

pub(crate) mod anthropic_messages;
pub(crate) mod gemini_generate_content;
pub(crate) mod openai_chat;
pub(crate) mod openai_responses;
mod openai_shared;
