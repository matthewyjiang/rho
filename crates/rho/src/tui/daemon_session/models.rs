//! Having the daemon fetch the provider model lists for the TUI
//! (`rho/model/refresh`): the daemon owns them, so every client reads the
//! lists it fetched.

use rho_protocol::{
    methods::ModelRefresh, ClientEnvironment, ModelRefreshParams, ModelRefreshResponse,
    ModelRefreshTarget,
};

use super::DaemonSession;

impl DaemonSession {
    /// Has the daemon fetch the model lists with this process's
    /// credentials: `only` one provider's, or every signed-in provider's
    /// and the models.dev catalog. Answers once every fetch ended.
    pub(in crate::tui) async fn refresh_models(
        &self,
        only: Option<ModelRefreshTarget>,
    ) -> anyhow::Result<ModelRefreshResponse> {
        let params = ModelRefreshParams {
            environment: ClientEnvironment::of_this_process(),
            only,
        };
        let link = self.connected_link().await?;
        Ok(link.request::<ModelRefresh>(&params).await?)
    }
}
