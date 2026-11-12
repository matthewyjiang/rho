use std::ffi::OsString;

use pretty_assertions::assert_eq;

use super::daemon_environment;

// Covers: an auto-started daemon keeps what describes it and the machine,
// and nothing that describes the client starting it. Each dropped name is
// one a daemon once inherited and acted on for every later session: project
// trust, the hook marker, session depth and scope, the client's journal
// stream, and its credentials: API keys, tokens, the credential store
// choice, its Ollama device key directory, and the session bus its keyring
// is on. A variable nobody listed is
// dropped too, which a denylist would keep.
// Owner: daemon auto-spawn environment policy.
#[test]
fn an_auto_started_daemon_keeps_only_allowed_variables() {
    let kept = [
        ("HOME", "/home/dev"),
        ("PATH", "/usr/bin"),
        ("XDG_CACHE_HOME", "/home/dev/.cache"),
        ("LANG", "C.UTF-8"),
        ("https_proxy", "http://proxy:3128"),
        ("RHO_LOG", "rho=debug"),
    ];
    let dropped = [
        ("RHO_TRUST_PROJECT_HOOKS", "1"),
        ("RHO_TRUST_PROJECT_AGENTS", "1"),
        ("RHO_TRUST_PROJECT_PLUGINS", "1"),
        ("RHO_TRUST_PROJECT_OPENAPI", "1"),
        ("RHO_IN_HOOK", "1"),
        ("RHO_SESSION_DEPTH", "2"),
        ("RHO_SESSION_SCOPE", "scope"),
        ("JOURNAL_STREAM", "8:12345"),
        ("VIRTUAL_ENV", "/work/.venv"),
        ("SSH_AUTH_SOCK", "/tmp/agent.sock"),
        ("RHO_CREDENTIAL_STORE", "file"),
        ("OLLAMA_DEVICE_KEY_DIR", "/home/dev/.ollama"),
        ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/run/user/1000/bus"),
        ("OPENAI_API_KEY", "sk-test"),
        ("RHO_CORP_API_KEY", "custom-host-key"),
        ("EXA_API_KEY", "search-key"),
        ("GITHUB_TOKEN", "ghp-test"),
        ("GH_TOKEN", "ghp-test"),
        ("CODEX_ACCOUNT_ID", "account"),
    ];
    let pairs = |variables: &[(&str, &str)]| -> Vec<(OsString, OsString)> {
        variables
            .iter()
            .map(|(name, value)| (name.into(), value.into()))
            .collect()
    };

    let inherited = pairs(&kept).into_iter().chain(pairs(&dropped));

    assert_eq!(daemon_environment(inherited), pairs(&kept));
}
