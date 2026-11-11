//! What the host's cache warmer (see [`crate::cache_warm`]) asks of the
//! session it warms. Only the daemon, unix-only so far, warms a session it
//! runs (#1665).

use std::sync::Arc;

use super::SessionRuntime;
use crate::{cache_warm::CacheWarmPing, tools::process::CacheWarmRoute};

impl SessionRuntime {
    /// The session's model route as its host's warmer reads it, when the
    /// host runs one.
    pub(crate) fn cache_warm_route(&self) -> Option<&CacheWarmRoute> {
        self.build.host.cache_warming.route()
    }

    /// A ping that re-reads the prefix the session's next turn sends, on the
    /// model that turn runs on.
    pub(crate) fn cache_warm_ping(&self) -> Result<CacheWarmPing, rho_sdk::Error> {
        CacheWarmPing::for_session(
            Arc::clone(&self.built.provider),
            &self.built.session,
            self.built.runtime.usage_recording(),
        )
    }
}
