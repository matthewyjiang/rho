//! Decoding and running `rho/sync/*` calls for [`crate::Service`]. Kept
//! apart from [`crate::server_calls`] so sync's methods sit behind one
//! [`crate::server_calls::ServiceCall`] variant.

use serde_json::Value;

use crate::{
    methods::{
        SyncCompare, SyncCompareParams, SyncExport, SyncExportParams, SyncRecipient,
        SyncRecipientParams, SYNC_COMPARE, SYNC_EXPORT, SYNC_RECIPIENT,
    },
    server::Service,
    server_codec::{decode_params, encode_result},
    ErrorCode, RpcError,
};

/// A `rho/sync/*` request whose params decoded.
pub(crate) enum SyncCall {
    Recipient(SyncRecipientParams),
    Export(SyncExportParams),
    Compare(SyncCompareParams),
}

impl SyncCall {
    /// What every sync method's name starts with.
    pub(crate) const PREFIX: &str = "rho/sync/";

    /// Decodes a request for a method named under [`Self::PREFIX`], or
    /// refuses one the protocol does not define.
    pub(crate) fn decode(method: &str, params: Option<Value>) -> Result<Self, RpcError> {
        match method {
            SYNC_RECIPIENT => decode_params::<SyncRecipient>(params).map(Self::Recipient),
            SYNC_EXPORT => decode_params::<SyncExport>(params).map(Self::Export),
            SYNC_COMPARE => decode_params::<SyncCompare>(params).map(Self::Compare),
            method => Err(RpcError::new(
                ErrorCode::MethodNotFound,
                format!("unknown method {method}"),
            )),
        }
    }

    /// Runs the call, returning its answer.
    pub(crate) async fn run<Svc: Service>(self, service: &Svc) -> Result<Value, RpcError> {
        match self {
            Self::Recipient(params) => {
                encode_result::<SyncRecipient>(&service.sync_recipient(params).await?)
            }
            Self::Export(params) => {
                encode_result::<SyncExport>(&service.sync_export(params).await?)
            }
            Self::Compare(params) => {
                encode_result::<SyncCompare>(&service.sync_compare(params).await?)
            }
        }
    }
}
