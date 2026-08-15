//! Optional process-local tracing subscriber for startup and runtime spans.
//!
//! Off by default. Set `RHO_LOG` to an env-filter (for example `rho=info` or
//! `rho=debug,rho::mcp=trace`) to print spans and events to stderr.

/// Install a fmt subscriber when `RHO_LOG` is set and non-empty.
///
/// Safe to call more than once: later installs are ignored. Tests should pass
/// the filter explicitly rather than mutating process environment.
pub(crate) fn install_from_env() {
    install(std::env::var("RHO_LOG").ok().as_deref());
}

fn install(filter: Option<&str>) {
    let Some(filter) = filter.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    let env_filter = tracing_subscriber::EnvFilter::try_new(filter)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(std::io::stderr)
        .try_init();
}

#[cfg(test)]
#[path = "logging_tests.rs"]
mod tests;
