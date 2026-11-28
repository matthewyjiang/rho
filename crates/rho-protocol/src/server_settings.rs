//! The settings part of the daemon's protocol: `rho/config/*` (#1720),
//! `rho/model/*`, the models a model setting may take, `rho/mcp/list`
//! and `rho/hooks/list`, the MCP servers and hooks the configuration sets
//! up, `rho/hooks/file/*`, the user's hooks file as text, and
//! `rho/plugins/*`, the agent plugins and managing them. The
//! [`SettingsService`] methods every [`crate::Service`] answers, and
//! forwarding a namespace's changes, or the models', once a client reads
//! them.

use std::{future::Future, sync::Arc};

use crate::{
    forward::{ConfigWatch, Forwarders, ModelWatch},
    methods::{
        ConfigGet, ConfigGetParams, ConfigRegisterParams, ConfigRegisterResponse, ConfigSchema,
        ConfigSetParams, ConfigSetResponse, ConfigTableGetParams, ConfigTableGetResponse,
        ConfigTableSetParams, ConfigTableSetResponse, HooksFileGetParams, HooksFileGetResponse,
        HooksFileSetParams, HooksFileSetResponse, HooksListParams, HooksListResponse,
        McpListParams, McpListResponse, ModelListMethod, ModelListParams, ModelRefreshParams,
        ModelRefreshResponse, CONFIG_GET, CONFIG_REGISTER, CONFIG_SCHEMA, CONFIG_SET,
        CONFIG_TABLE_GET, CONFIG_TABLE_SET, HOOKS_FILE_GET, HOOKS_FILE_SET, HOOKS_LIST, MCP_LIST,
        MODEL_LIST, MODEL_REFRESH,
    },
    methods::{
        PluginsInstallParams, PluginsInstallResponse, PluginsListParams, PluginsListResponse,
        PluginsRemoveParams, PluginsRemoveResponse, PluginsSetEnabledParams,
        PluginsSetEnabledResponse, PluginsUpdateParams, PluginsUpdateResponse, PLUGINS_INSTALL,
        PLUGINS_LIST, PLUGINS_REMOVE, PLUGINS_SET_ENABLED, PLUGINS_UPDATE,
    },
    server_codec::{encode_result, unsupported},
    server_outbox::Responder,
    RpcError,
};

/// The daemon's answers to `rho/config/*`, `rho/model/*`, `rho/mcp/list`,
/// `rho/hooks/list`, `rho/hooks/file/*`, and `rho/plugins/*`, which
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

    /// Answers `rho/config/table/get`: one structured table's entries, with
    /// its revision.
    fn get_config_table(
        &self,
        params: ConfigTableGetParams,
    ) -> impl Future<Output = Result<ConfigTableGetResponse, RpcError>> + Send {
        let _ = params;
        unsupported(CONFIG_TABLE_GET)
    }

    /// Handles `rho/config/table/set`: put or remove every entry, or none,
    /// and publish the agent namespace's new state when a setting's value
    /// moved.
    fn set_config_table(
        &self,
        params: ConfigTableSetParams,
    ) -> impl Future<Output = Result<ConfigTableSetResponse, RpcError>> + Send {
        let _ = params;
        unsupported(CONFIG_TABLE_SET)
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

    /// Answers `rho/mcp/list`: the MCP servers a session would configure,
    /// read without a session or starting a server.
    fn list_mcp_servers(
        &self,
        params: McpListParams,
    ) -> impl Future<Output = Result<McpListResponse, RpcError>> + Send {
        let _ = params;
        unsupported(MCP_LIST)
    }

    /// Answers `rho/hooks/list`: the hooks files a session would load and
    /// the hooks they define, read without a session or running a hook.
    fn list_hooks(
        &self,
        params: HooksListParams,
    ) -> impl Future<Output = Result<HooksListResponse, RpcError>> + Send {
        let _ = params;
        unsupported(HOOKS_LIST)
    }

    /// Answers `rho/hooks/file/get`: the user's hooks file as text, with its
    /// revision.
    fn read_hooks_file(
        &self,
        params: HooksFileGetParams,
    ) -> impl Future<Output = Result<HooksFileGetResponse, RpcError>> + Send {
        let _ = params;
        unsupported(HOOKS_FILE_GET)
    }

    /// Handles `rho/hooks/file/set`: replace or delete the user's hooks file
    /// when the params' revision is current and the text parses.
    fn write_hooks_file(
        &self,
        params: HooksFileSetParams,
    ) -> impl Future<Output = Result<HooksFileSetResponse, RpcError>> + Send {
        let _ = params;
        unsupported(HOOKS_FILE_SET)
    }

    /// Answers `rho/plugins/list`: the agent plugins a session would find.
    fn list_plugins(
        &self,
        params: PluginsListParams,
    ) -> impl Future<Output = Result<PluginsListResponse, RpcError>> + Send {
        let _ = params;
        unsupported(PLUGINS_LIST)
    }

    /// Handles `rho/plugins/install`: copy, link, or fetch a plugin package
    /// into a plugins root. A git install resolves once `git` finishes, so
    /// an implementation runs it off the async workers.
    fn install_plugin(
        &self,
        params: PluginsInstallParams,
    ) -> impl Future<Output = Result<PluginsInstallResponse, RpcError>> + Send {
        let _ = params;
        unsupported(PLUGINS_INSTALL)
    }

    /// Handles `rho/plugins/update`: fetch git-installed plugins again,
    /// resolving once every fetch finishes.
    fn update_plugins(
        &self,
        params: PluginsUpdateParams,
    ) -> impl Future<Output = Result<PluginsUpdateResponse, RpcError>> + Send {
        let _ = params;
        unsupported(PLUGINS_UPDATE)
    }

    /// Handles `rho/plugins/set_enabled`: save a plugin's enable state.
    fn set_plugin_enabled(
        &self,
        params: PluginsSetEnabledParams,
    ) -> impl Future<Output = Result<PluginsSetEnabledResponse, RpcError>> + Send {
        let _ = params;
        unsupported(PLUGINS_SET_ENABLED)
    }

    /// Handles `rho/plugins/remove`: delete a plugin's package.
    fn remove_plugin(
        &self,
        params: PluginsRemoveParams,
    ) -> impl Future<Output = Result<PluginsRemoveResponse, RpcError>> + Send {
        let _ = params;
        unsupported(PLUGINS_REMOVE)
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
