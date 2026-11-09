//! Process-local tracing subscriber for startup and runtime spans.
//!
//! Interactive and CLI runs log only when `RHO_LOG` is set to an env-filter
//! (for example `rho=info` or `rho=debug,rho::mcp=trace`), printing to stderr.
//! `rho daemon run` always logs, to journald or its log file; see
//! `app::daemon_cli`.

use std::fs::File;

/// Where a subscriber writes.
#[cfg_attr(
    not(unix),
    expect(dead_code, reason = "only the Unix daemon logs to journald or a file")
)]
pub(crate) enum LogSink {
    /// stderr with ANSI colors, for a terminal.
    Terminal,
    /// Plain stderr without timestamps: journald adds its own and does not
    /// render colors.
    Journal,
    /// Plain lines appended to a file.
    File(File),
}

/// Install a terminal subscriber when `RHO_LOG` is set and non-empty.
///
/// Safe to call more than once: later installs are ignored. Tests should pass
/// the filter explicitly rather than mutating process environment.
pub(crate) fn install_from_env() {
    let filter = std::env::var("RHO_LOG").ok();
    if let Some(filter) = filter
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        install(filter, LogSink::Terminal);
    }
}

/// Install a subscriber with an env-filter directive, falling back to `warn`
/// when the directive does not parse. Later installs are ignored.
pub(crate) fn install(filter: &str, sink: LogSink) {
    let env_filter = tracing_subscriber::EnvFilter::try_new(filter)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    let builder = tracing_subscriber::fmt();
    let _ = match sink {
        LogSink::Terminal => builder
            .with_env_filter(env_filter)
            .with_writer(std::io::stderr)
            .try_init(),
        LogSink::Journal => journal_format(builder)
            .with_env_filter(env_filter)
            .with_writer(std::io::stderr)
            .try_init(),
        LogSink::File(file) => builder
            .with_env_filter(env_filter)
            .with_ansi(false)
            .with_writer(file)
            .try_init(),
    };
}

/// The journal sink's line format: plain, without timestamps, since journald
/// adds its own and does not render colors. Tests render events through it
/// to check the lines the daemon logs.
pub(crate) fn journal_format(
    builder: tracing_subscriber::fmt::SubscriberBuilder,
) -> tracing_subscriber::fmt::SubscriberBuilder<
    tracing_subscriber::fmt::format::DefaultFields,
    tracing_subscriber::fmt::format::Format<tracing_subscriber::fmt::format::Full, ()>,
> {
    builder.with_ansi(false).without_time()
}
