//! rho's A2UI v1.0 core: the data model, expressions and catalogs, and the
//! renderer side that applies an agent's messages to surfaces.
//!
//! This crate has no GUI code. It is tested against the conformance suites
//! that Google publishes with A2UI, vendored under `vendor/a2ui`.
//!
//! [`Catalog`] loads a v1.0 catalog, the components and functions a surface
//! may use, and [`CatalogValidator`] checks component instances against one.
//!
//! [`MessageProcessor`] is the renderer side: it applies an agent's messages
//! to [`Surface`]s, reporting each [`Change`] for a view to act on, resolves
//! a component's properties with [`MessageProcessor::resolve`], and turns a
//! user's action into a message for the agent with
//! [`MessageProcessor::dispatch_action`].
//!
//! [`Express`] is the agent side of A2UI Express, the compact notation a
//! model writes a surface in: it compiles a block into v1.0 messages,
//! decompiles messages back, and renders the prompt that teaches it.

mod action;
mod basic_catalog;
mod bundle;
mod catalog;
mod catalogs;
mod coerce;
mod component;
mod data_context;
mod data_model;
mod expr;
mod express;
mod functions;
mod ident;
mod message;
mod pointer;
mod processor;
mod prune;
mod regex_dialect;
mod resolve;
mod schema;
mod shape;
mod slot;
mod surface;
mod template;
mod topology;
mod validate;

pub use action::{Action, ActionError, Dispatched, Event, Interaction};
pub use catalog::{
    Callers, Catalog, CatalogError, ComponentDef, Derivation, Entry, EntryKind, FunctionDef,
    ReturnType,
};
pub use catalogs::UnresolvedCatalog;
pub use component::{Accessibility, CheckRule, ChildList, Component, Live, Property};
pub use data_context::{DataContext, UserActivation};
pub use data_model::{DataError, DataModel, WatchId, MAX_LIST_INDEX};
pub use expr::{Call, Expr, ExpressionError};
pub use express::{
    AllowedMessage, ErrorKind, Express, ExpressError, Framing, Position, PromptOptions,
    DEFAULT_SURFACE_ID,
};
pub use functions::{Args, Effect, Function, FunctionError, Functions};
pub use ident::is_identifier;
pub use message::{
    parse_payload, ActionMessage, AgentMessage, CallRendererFunction, CreateSurface, ErrorCode,
    ErrorMessage, ErrorTarget, FunctionFailure, FunctionResponse, Metadata, MetadataError,
    RendererMessage, UpdateComponents, UpdateDataModel, WireComponent, PROTOCOL_VERSION,
};
pub use pointer::{resolve_path, Pointer, PointerError};
pub use processor::{
    Change, InlineCatalogs, MessageProcessor, Processed, Validation, ValidationConfig,
};
pub use regex_dialect::RegexError;
pub use resolve::{
    CheckFailure, Checks, ChildRef, Resolved, ResolvedAccessibility, ResolvedComponent, Scope,
    Severity,
};
pub use schema::{agent_to_renderer, common_types};
pub use surface::Surface;
pub use template::{parse_template, MAX_DEPTH, MAX_TEMPLATE_CHARS, MAX_TEMPLATE_PARTS};
pub use validate::{validate_common_type, CatalogValidator, ValidationError, Violation};
