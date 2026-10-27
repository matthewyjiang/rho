use super::*;
use pretty_assertions::assert_eq;

// Covers: `rho update` must upgrade the tool spec mise installed rho under,
// such as a `github:` backend, rather than guess one from the directory name.
// Owner: mise tool lookup.
#[test]
fn finds_mise_tool_by_install_path() {
    let ls = br#"{
        "jq": [{"install_path": "/mise/installs/jq/1.8.2"}],
        "github:matthewyjiang/rho": [
            {"install_path": "/mise/installs/github-matthewyjiang-rho/2.30.0"},
            {"install_path": "/mise/installs/github-matthewyjiang-rho/2.31.0"}
        ]
    }"#;
    for (json, install_path, expected) in [
        (
            &ls[..],
            "/mise/installs/github-matthewyjiang-rho/2.31.0",
            Some("github:matthewyjiang/rho"),
        ),
        (&ls[..], "/mise/installs/rho/2.31.0", None),
        (b"not json", "/mise/installs/rho/2.31.0", None),
    ] {
        assert_eq!(
            mise_tool_from_ls_json(json, Path::new(install_path))
                .ok()
                .as_deref(),
            expected,
            "{install_path}",
        );
    }
}
