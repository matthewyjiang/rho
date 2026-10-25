//! Optional host features the host opted into at `initialize`.
//!
//! ACP lets an agent send `compaction_update` and `notice` only to clients
//! that advertise `clientCapabilities.session.compaction` and `.notices`,
//! `type: "boolean"` config options only to clients that advertise
//! `clientCapabilities.session.configOptions.boolean`, and
//! `elicitation/create` in form mode only to clients that advertise
//! `clientCapabilities.elicitation.form`, and in URL mode only to clients that
//! advertise `.url`. Other hosts keep the baseline: `/compact` replies with an
//! agent message, automatic compaction shows only in `usage_update`, notices
//! are agent messages, on/off options are two-value selects, Rho offers no
//! questionnaire tool and declines MCP elicitation, and OAuth sign-in runs in
//! a terminal. A host advertising both `clientCapabilities.fs.readTextFile`
//! and `.writeTextFile` serves file text through `fs/*` requests, and one
//! advertising `clientCapabilities.terminal` runs shell commands through
//! `terminal/*`; others keep Rho on disk and local processes.

use std::sync::{Arc, OnceLock};

use agent_client_protocol::schema::v1::InitializeRequest;

use super::AcpClientPort;

/// Features beyond the ACP baseline that the connected host accepts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) struct HostFeatures {
    /// Compaction lifecycle cards (`compaction_update`).
    pub(super) compaction: bool,
    /// Live, dismissible user notices (`notice`).
    pub(super) notices: bool,
    /// How on/off session config options are drawn.
    pub(super) toggles: Toggles,
    /// Form-mode `elicitation/create`: questions put to the user.
    pub(super) forms: Forms,
    /// URL-mode `elicitation/create`: links the user opens, such as an OAuth
    /// sign-in page.
    pub(super) urls: bool,
    /// `fs/read_text_file` and `fs/write_text_file`. Rho needs both: writing
    /// without reading would overwrite unsaved edits it never saw.
    pub(super) files: bool,
    /// `terminal/*`: the host runs shell commands in terminals it shows.
    pub(super) terminals: bool,
}

/// The shape of an on/off session config option, such as `fast_mode`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Toggles {
    /// The ACP baseline: a two-value `select`.
    #[default]
    Select,
    /// `type: "boolean"`, which the host draws as a switch.
    Boolean,
}

/// Whether, and how, the host draws form-mode elicitations.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Forms {
    /// The host advertised no form support.
    #[default]
    Unavailable,
    Available(FormDialect),
}

/// How a form-capable host draws the schema, which decides how choice
/// questions are shaped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FormDialect {
    /// A host that draws the ACP form schema as specified.
    Standard,
    /// T3 Code (`clientInfo.name` "t3-code"). It turns each property into
    /// one question listing its top-level `enum` values that also accepts
    /// free text, requires every property, and answers each with one string.
    T3Code,
}

impl HostFeatures {
    pub(super) fn from_initialize(request: &InitializeRequest) -> Self {
        let capabilities = &request.client_capabilities;
        let session = capabilities.session.as_ref();
        let toggles = match session
            .and_then(|session| session.config_options.as_ref())
            .and_then(|options| options.boolean.as_ref())
        {
            Some(_) => Toggles::Boolean,
            None => Toggles::Select,
        };
        let elicitation = capabilities.elicitation.as_ref();
        let advertises_forms = elicitation.is_some_and(|elicitation| elicitation.form.is_some());
        let client = request.client_info.as_ref().map(|info| info.name.as_str());
        let forms = match (advertises_forms, client) {
            (false, _) => Forms::Unavailable,
            (true, Some("t3-code")) => Forms::Available(FormDialect::T3Code),
            (true, _) => Forms::Available(FormDialect::Standard),
        };
        Self {
            compaction: session.is_some_and(|session| session.compaction.is_some()),
            notices: session.is_some_and(|session| session.notices.is_some()),
            toggles,
            forms,
            urls: elicitation.is_some_and(|elicitation| elicitation.url.is_some()),
            files: capabilities.fs.read_text_file && capabilities.fs.write_text_file,
            terminals: capabilities.terminal,
        }
    }
}

/// The host's choice and connection, recorded by `initialize` and read when
/// the first session starts. Every connection begins with `initialize`: the
/// protocol router that picks the version rejects anything else first.
#[derive(Default)]
pub(super) struct NegotiatedFeatures(OnceLock<(HostFeatures, Arc<dyn AcpClientPort>)>);

impl NegotiatedFeatures {
    /// Keeps the first `initialize`; ACP negotiates capabilities once per
    /// connection.
    pub(super) fn record(&self, request: &InitializeRequest, client: Arc<dyn AcpClientPort>) {
        let _ = self.0.set((HostFeatures::from_initialize(request), client));
    }

    pub(super) fn get(&self) -> HostFeatures {
        self.0
            .get()
            .map(|(features, _)| *features)
            .unwrap_or_default()
    }

    pub(super) fn client(&self) -> Option<Arc<dyn AcpClientPort>> {
        self.0.get().map(|(_, client)| Arc::clone(client))
    }
}
