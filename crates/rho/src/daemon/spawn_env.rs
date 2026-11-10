//! The environment an auto-started daemon runs with (#1779).
//!
//! A daemon outlives the client that starts it and serves every later one,
//! so it must not take that client's identity: the client's hook marker,
//! session depth, project trust, credentials, or whatever else it happened
//! to export. The daemon starts from an empty environment and gets only the
//! variables allowed here, which say where its files are, how it reaches the
//! network, and how the daemon itself is set up. Everything a session needs
//! from a client arrives with that client's requests instead (see
//! [`crate::session_environment`]): its API keys and its choice of
//! credential store among them, so none is listed here.
//!
//! A variable belongs here only when it describes the daemon or the machine,
//! the same for every client. One that could differ between two clients of
//! one daemon is a session input and travels on the wire.
//!
//! `rho daemon run` started by hand or by a service manager keeps the
//! environment it was given: whoever starts it that way chose it.

use std::ffi::OsString;

/// Where the daemon's files are, who it runs as, and its locale. `RHO_HOME`
/// is not here: the spawner pins it to the resolved home.
const MACHINE: &[&str] = &[
    "HOME",
    "USER",
    "LOGNAME",
    // The daemon's own executable lookups; a session's use its client's.
    "PATH",
    "TMPDIR",
    // What the client's loader found this executable's libraries with.
    "LD_LIBRARY_PATH",
    "DYLD_LIBRARY_PATH",
    // The socket's directory, which the client resolved its path from.
    "XDG_RUNTIME_DIR",
    // Model catalog caches.
    "XDG_CACHE_HOME",
    // The global ignore file of the `git` the write gate asks, and of
    // workspace walks.
    "XDG_CONFIG_HOME",
    "GIT_CONFIG_GLOBAL",
    // Where this machine's Ollama device key is kept: a place, as `HOME`
    // is, which the daemon creates the key in and every session reads.
    "OLLAMA_DEVICE_KEY_DIR",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TZ",
];

/// How the daemon's own requests, to providers and from web tools, reach the
/// network from this machine.
const NETWORK: &[&str] = &[
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
    "ALL_PROXY",
    "all_proxy",
    "NO_PROXY",
    "no_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    // Address ranges a TUN or fake-IP proxy on this machine answers from.
    "RHO_SSRF_ALLOW_RANGES",
];

/// Settings of the daemon itself, which hold for every session.
const DAEMON_SETTINGS: &[&str] = &[
    "RHO_LOG",
    "RHO_CLAUDE_AGENT_CONCURRENCY",
    "RHO_MODELS_PATH",
    "RUST_BACKTRACE",
];

/// The fixture providers a test suite's daemon answers with. Release builds
/// compile no fixture, and pass none of these.
#[cfg(debug_assertions)]
const FIXTURE_PREFIXES: &[&str] = &["RHO_AUTOMATION_TEST_", "RHO_TUI_TEST_", "RHO_TUI_FIXTURE_"];

#[cfg(not(debug_assertions))]
const FIXTURE_PREFIXES: &[&str] = &[];

/// Whether an auto-started daemon keeps variable `name` of the client that
/// starts it.
fn is_allowed(name: &str) -> bool {
    [MACHINE, NETWORK, DAEMON_SETTINGS]
        .iter()
        .any(|names| names.contains(&name))
        || FIXTURE_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
}

/// The environment an auto-started daemon runs with: the allowed variables
/// of `inherited`, the environment of the client starting it. A variable
/// whose name is not UTF-8 is not one of them.
pub(super) fn daemon_environment(
    inherited: impl IntoIterator<Item = (OsString, OsString)>,
) -> Vec<(OsString, OsString)> {
    inherited
        .into_iter()
        .filter(|(name, _)| name.to_str().is_some_and(is_allowed))
        .collect()
}

#[cfg(test)]
#[path = "spawn_env_tests.rs"]
mod tests;
