//! Reading the daemon's settings on a connection, at attach and again
//! after the daemon restarts, which forgets the connection's subscription
//! and the TUI's registration.

use rho_protocol::{
    methods::{ConfigGet, ConfigRegister, ConfigSchemaMethod},
    ConfigChangedParams, ConfigGetParams, ConfigRegisterParams, ConfigSchemaParams,
};
use tokio::sync::mpsc;

use super::{
    daemon_events::DaemonView,
    daemon_settings::{tui_client, DaemonSettings, SettingsNamespace},
};
use crate::app::daemon_client::{Daemon, Link};

/// Routes `rho/config/changed` to the receiver this returns, replacing
/// any earlier route.
pub(super) fn route_config(daemon: &Daemon) -> mpsc::UnboundedReceiver<ConfigChangedParams> {
    let (sender, changes) = mpsc::unbounded_channel();
    daemon.route_config(sender);
    changes
}

/// The view of a settings read. Settings that cannot be read leave
/// `/config` with the session's own options; the session still runs.
pub(super) fn settings_loaded(settings: anyhow::Result<DaemonSettings>) -> DaemonView {
    match settings {
        Ok(settings) => DaemonView::SettingsLoaded(Box::new(settings)),
        Err(error) => DaemonView::Error(format!("could not read the rho settings: {error:#}")),
    }
}

/// Registers the `tui` client's settings, then reads the agent schema
/// and both namespaces' values, which subscribes this connection to
/// their changes.
pub(super) async fn load_settings(link: &Link) -> anyhow::Result<DaemonSettings> {
    let tui_specs = crate::tui::client_settings::tui_setting_specs();
    link.request::<ConfigRegister>(&ConfigRegisterParams {
        client: tui_client(),
        settings: tui_specs.clone(),
    })
    .await?;
    let schema = link
        .request::<ConfigSchemaMethod>(&ConfigSchemaParams {})
        .await?;
    let agent = link
        .request::<ConfigGet>(&ConfigGetParams {
            namespace: SettingsNamespace::Agent.wire(),
        })
        .await?;
    let tui = link
        .request::<ConfigGet>(&ConfigGetParams {
            namespace: SettingsNamespace::Tui.wire(),
        })
        .await?;
    Ok(DaemonSettings::new(
        schema,
        tui_specs,
        agent.into(),
        tui.into(),
    ))
}
