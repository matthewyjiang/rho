//! Decoding and running `rho/sync/*` calls for [`crate::Service`]. Kept
//! apart from [`crate::server_calls`] so sync's methods sit behind one
//! [`crate::server_calls::ServiceCall`] variant.

use serde_json::Value;

use crate::{
    methods::{
        SyncCompare, SyncCompareParams, SyncDiscard, SyncDiscardParams, SyncExport,
        SyncExportParams, SyncImportKeys, SyncImportKeysParams, SyncRecipient, SyncRecipientParams,
        SyncRevert, SyncRevertParams, SyncStatus, SyncStatusParams, SYNC_COMPARE, SYNC_DISCARD,
        SYNC_EXPORT, SYNC_IMPORT_KEYS, SYNC_RECIPIENT, SYNC_REVERT, SYNC_STATUS,
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
    ImportKeys(SyncImportKeysParams),
    Status(SyncStatusParams),
    Revert(SyncRevertParams),
    Discard(SyncDiscardParams),
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
            SYNC_IMPORT_KEYS => decode_params::<SyncImportKeys>(params).map(Self::ImportKeys),
            SYNC_STATUS => decode_params::<SyncStatus>(params).map(Self::Status),
            SYNC_REVERT => decode_params::<SyncRevert>(params).map(Self::Revert),
            SYNC_DISCARD => decode_params::<SyncDiscard>(params).map(Self::Discard),
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
            Self::ImportKeys(params) => {
                encode_result::<SyncImportKeys>(&service.sync_import_keys(params).await?)
            }
            Self::Status(params) => {
                encode_result::<SyncStatus>(&service.sync_status(params).await?)
            }
            Self::Revert(params) => {
                encode_result::<SyncRevert>(&service.sync_revert(params).await?)
            }
            Self::Discard(params) => {
                encode_result::<SyncDiscard>(&service.sync_discard(params).await?)
            }
        }
    }
}
