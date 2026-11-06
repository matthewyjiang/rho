//! `session/set_mode` for one ACP session: the runtime swaps the permission
//! mode; this maps why it could not.

use agent_client_protocol::Error as AcpError;

use super::{host_apply_error, SessionHost};
use crate::{app::session_runtime::ModeChangeError, config::Config, permission::PermissionMode};

impl SessionHost {
    /// Applies `mode` to later prompts. The caller holds the host slot lock,
    /// so no prompt is in flight. On error the session keeps its old mode.
    pub(in crate::app::acp) async fn set_permission_mode(
        &mut self,
        mode: PermissionMode,
        process_config: &Config,
    ) -> Result<(), AcpError> {
        self.runtime
            .set_permission_mode(mode, process_config)
            .await
            .map_err(|error| match error {
                ModeChangeError::Unsupported(_) => {
                    AcpError::invalid_params().data(error.to_string())
                }
                ModeChangeError::Rebuild(_) => host_apply_error(error),
            })
    }
}

#[cfg(test)]
#[path = "session_host_mode_tests.rs"]
mod tests;
