//! Google Gemini `generateContent` protocol placeholder.
//!
//! This module reserves ownership for conversion between Rho's canonical model
//! contract and Google's `generateContent` and `streamGenerateContent` wire
//! formats. It intentionally contains no provider runtime and is not selectable:
//! authentication, endpoint policy, model discovery, and provider registration
//! belong in a future `providers::google` module.
//!
//! Implement the codec here when Gemini support is added. In particular, keep
//! Gemini-specific `Content`, `Part`, function declaration, function response,
//! usage, safety, grounding, and thought-signature types out of the canonical
//! model unless agent behavior needs the corresponding concept.
