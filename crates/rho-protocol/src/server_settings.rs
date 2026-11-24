//! The settings part of the daemon's protocol: `rho/config/*` (#1720), and
//! `rho/model/*`, the models a model setting may take. The
//! [`SettingsService`] methods every [`crate::Service`] answers, and
//! forwarding a namespace's changes, or the models', once a client reads
//! them.

use std::{future::Future, sync::Arc};

use crate::{
    forward::{ConfigWatch, Forwarders, ModelWatch},
    methods::{
        ConfigGet, ConfigGetParams, ConfigRegisterParams, ConfigRegisterResponse, ConfigSchema,
        ConfigSetParams, ConfigSetResponse, ModelListMethod, ModelListParams, ModelRefreshParams,
        ModelRefreshResponse, CONFIG_GET, CONFIG_REGISTER, CONFIG_SCHEMA, CONFIG_SET, MODEL_LIST,
        MODEL_REFRESH,
    },
    server::Responder,
    server_codec::{encode_result, unsupported},
    RpcError,
};

/// The daemon's answers to `rho/config/*` and `rho/model/*`, which
/// [`crate::Service`] requires. Calls run as [`crate::Service`]'s do: each
/// in its own task, overlapping within a connection and across
/// connections. Each method answers [`crate::ErrorCode::MethodNotFound`]
/// unless overridden, so a daemon without settings implements none of
/// them.
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

    /// Handles `rho/model/list`: the models the client may pick, current
    /// when this resolves. The connection answers with them, then sends
    /// each later list that differs as `rho/model/changed` until the client
    /// lists again or the connection ends.
    fn list_models(
        &self,
        params: ModelListParams,
    ) -> impl Future<Output = Result<ModelWatch, RpcError>> + Send {
        let _ = params;
        unsupported(MODEL_LIST)
    }

    /// Handles `rho/model/refresh`: fetch the provider model lists and the
    /// models.dev catalog again, then publish the new lists to every
    /// [`ModelWatch`].
    fn refresh_models(
        &self,
        params: ModelRefreshParams,
    ) -> impl Future<Output = Result<ModelRefreshResponse, RpcError>> + Send {
        let _ = params;
        unsupported(MODEL_REFRESH)
    }
}

/// A read that subscribes its connection to the state it read.
pub(crate) enum WatchRequest {
    Config(ConfigGetParams),
    Models(ModelListParams),
}

/// Answers a watching read through `responder`, then forwards the state's
/// changes through `forwarders`.
pub(crate) async fn watch<Svc: SettingsService>(
    service: &Svc,
    request: WatchRequest,
    responder: Responder,
    forwarders: &Arc<Forwarders>,
) {
    let outbox = responder.outbox.clone();
    match request {
        WatchRequest::Config(params) => {
            let namespace = params.namespace.clone();
            match service.get_config(params).await {
                Ok(watch) => forwarders.watch_config(namespace, watch, outbox, move |state| {
                    responder.send(encode_result::<ConfigGet>(state));
                }),
                Err(error) => responder.send(Err(error)),
            }
        }
        WatchRequest::Models(params) => match service.list_models(params).await {
            Ok(watch) => forwarders.watch_models(watch, outbox, move |list| {
                responder.send(encode_result::<ModelListMethod>(list));
            }),
            Err(error) => responder.send(Err(error)),
        },
    }
}
