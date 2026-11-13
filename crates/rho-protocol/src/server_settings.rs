//! The settings part of the daemon's protocol (`rho/config/*`, #1720):
//! the [`SettingsService`] methods every [`crate::Service`] answers, and
//! forwarding a namespace's changes once a client reads it.

use std::{future::Future, sync::Arc};

use crate::{
    forward::{ConfigWatch, Forwarders},
    methods::{
        ConfigGet, ConfigGetParams, ConfigRegisterParams, ConfigRegisterResponse, ConfigSchema,
        ConfigSetParams, ConfigSetResponse, CONFIG_GET, CONFIG_REGISTER, CONFIG_SCHEMA, CONFIG_SET,
    },
    server::{encode_result, unsupported, Responder},
    RpcError,
};

/// The daemon's answers to `rho/config/*`, which [`crate::Service`]
/// requires. Calls run as [`crate::Service`]'s do: each in its own task,
/// overlapping within a connection and across connections. Each method
/// answers [`crate::ErrorCode::MethodNotFound`] unless overridden, so a
/// daemon without settings implements none of them.
pub trait SettingsService: Send + Sync {
    /// Answers `rho/config/schema`: every agent setting.
    fn config_schema(&self) -> impl Future<Output = Result<ConfigSchema, RpcError>> + Send {
        unsupported(CONFIG_SCHEMA)
    }

    /// Handles `rho/config/register`: replace the client's settings schema.
    fn register_config(
        &self,
        params: ConfigRegisterParams,
    ) -> impl Future<Output = Result<ConfigRegisterResponse, RpcError>> + Send {
        let _ = params;
        unsupported(CONFIG_REGISTER)
    }

    /// Handles `rho/config/get`: the namespace's state, current when this
    /// resolves. The connection answers with it, then sends each later
    /// state's changes as `rho/config/changed` until the client reads the
    /// namespace again or the connection ends.
    fn get_config(
        &self,
        params: ConfigGetParams,
    ) -> impl Future<Output = Result<ConfigWatch, RpcError>> + Send {
        let _ = params;
        unsupported(CONFIG_GET)
    }

    /// Handles `rho/config/set`: write every change, or none, and publish
    /// the new state to the namespace's [`ConfigWatch`]es.
    fn set_config(
        &self,
        params: ConfigSetParams,
    ) -> impl Future<Output = Result<ConfigSetResponse, RpcError>> + Send {
        let _ = params;
        unsupported(CONFIG_SET)
    }
}

/// Answers a `rho/config/get` through `responder`, then forwards the
/// namespace's changes through `forwarders`.
pub(crate) async fn watch_config<Svc: SettingsService>(
    service: &Svc,
    params: ConfigGetParams,
    responder: Responder,
    forwarders: &Arc<Forwarders>,
) {
    let namespace = params.namespace.clone();
    match service.get_config(params).await {
        Ok(watch) => {
            let outbox = responder.outbox.clone();
            forwarders.watch_config(namespace, watch, outbox, move |state| {
                responder.send(encode_result::<ConfigGet>(state));
            });
        }
        Err(error) => responder.send(Err(error)),
    }
}
