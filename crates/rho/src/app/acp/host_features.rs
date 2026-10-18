//! Optional host features the host opted into at `initialize`.
//!
//! ACP lets an agent send `compaction_update` and `notice` only to clients
//! that advertise `clientCapabilities.session.compaction` and `.notices`,
//! and `elicitation/create` in form mode only to clients that advertise
//! `clientCapabilities.elicitation.form`. Other hosts keep the baseline:
//! `/compact` replies with an agent message, automatic compaction shows only
//! in `usage_update`, notices are agent messages, and Rho offers no
//! questionnaire tool and declines MCP elicitation.

use std::sync::OnceLock;

use agent_client_protocol::schema::v1::InitializeRequest;

/// Features beyond the ACP baseline that the connected host accepts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) struct HostFeatures {
    /// Compaction lifecycle cards (`compaction_update`).
    pub(super) compaction: bool,
    /// Live, dismissible user notices (`notice`).
    pub(super) notices: bool,
    /// Form-mode `elicitation/create`: questions put to the user.
    pub(super) forms: Forms,
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
        let advertises_forms = capabilities
            .elicitation
            .as_ref()
            .is_some_and(|elicitation| elicitation.form.is_some());
        let client = request.client_info.as_ref().map(|info| info.name.as_str());
        let forms = match (advertises_forms, client) {
            (false, _) => Forms::Unavailable,
            (true, Some("t3-code")) => Forms::Available(FormDialect::T3Code),
            (true, _) => Forms::Available(FormDialect::Standard),
        };
        Self {
            compaction: session.is_some_and(|session| session.compaction.is_some()),
            notices: session.is_some_and(|session| session.notices.is_some()),
            forms,
        }
    }
}

/// The host's choice, recorded by `initialize` and read when the first
/// session starts. A host that skips `initialize` gets the baseline.
#[derive(Default)]
pub(super) struct NegotiatedFeatures(OnceLock<HostFeatures>);

impl NegotiatedFeatures {
    /// Keeps the first `initialize`; ACP negotiates capabilities once per
    /// connection.
    pub(super) fn record(&self, request: &InitializeRequest) {
        let _ = self.0.set(HostFeatures::from_initialize(request));
    }

    pub(super) fn get(&self) -> HostFeatures {
        self.0.get().copied().unwrap_or_default()
    }
}
