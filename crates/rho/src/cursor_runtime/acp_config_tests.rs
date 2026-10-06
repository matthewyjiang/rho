use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use CursorCategory as C;
use CursorTool as T;
use PermissionMode::{Bypass, Plan};

// Covers: config override precedence must match Cursor, without process env.
// Owner: pure Cursor path policy.
#[test]
fn resolves_injected_config_paths() {
    for (cursor, xdg, expected) in [
        (Some("/override"), Some("/xdg"), "/override"),
        (Some("  "), Some("/xdg"), "/xdg/cursor"),
        (None, Some("/xdg"), "/xdg/cursor"),
        (None, Some(""), "/home/user/.cursor"),
        (None, None, "/home/user/.cursor"),
    ] {
        let paths = CursorConfigPaths {
            cursor_config_dir: cursor.map(PathBuf::from),
            xdg_config_home: xdg.map(PathBuf::from),
            home: PathBuf::from("/home/user"),
        };
        assert_eq!(paths.user_dir(), PathBuf::from(expected));
    }
}

// Covers: coarsening must not silently widen declared tools; Plan fences
// writes/shell even when the caller supplies the full declared tool set.
// Owner: pure Cursor fencing policy (not the generic permission selector).
#[test]
fn category_fence_and_unenforced_exclusions() {
    let all_except = |removed: &[T]| {
        T::ALL
            .iter()
            .copied()
            .filter(|tool| !removed.contains(tool))
            .collect::<Vec<_>>()
    };
    let all_categories = vec![
        C::Read,
        C::Search,
        C::Write,
        C::Shell,
        C::Fetch,
        C::Mcp,
        C::Inert,
    ];
    let cases = [
        (
            Bypass,
            vec![T::Read, T::Grep, T::Glob],
            vec!["Write(**)", "Shell(*)"],
            vec![C::Read, C::Search],
            vec![
                (C::Search, vec![T::Ls, T::SemSearch, T::ReadLints]),
                (C::Inert, vec![T::UpdateTodos, T::ReadTodos, T::CreatePlan]),
            ],
        ),
        (
            Bypass,
            T::ALL.to_vec(),
            vec![],
            all_categories.clone(),
            vec![],
        ),
        (
            Bypass,
            all_except(&[T::Delete]),
            vec![],
            all_categories.clone(),
            vec![(C::Write, vec![T::Delete])],
        ),
        (
            Bypass,
            all_except(&[T::Shell, T::WriteShellStdin]),
            vec!["Shell(*)"],
            vec![C::Read, C::Search, C::Write, C::Fetch, C::Mcp, C::Inert],
            vec![],
        ),
        (
            Plan,
            T::ALL.to_vec(),
            vec!["Write(**)", "Shell(*)"],
            vec![],
            vec![],
        ),
        (
            Bypass,
            all_except(&[T::Read]),
            vec!["Read(**)"],
            vec![C::Search, C::Write, C::Shell, C::Fetch, C::Mcp, C::Inert],
            vec![],
        ),
        (
            Bypass,
            all_except(&[T::WriteShellStdin, T::WebSearch, T::ReadMcpResource]),
            vec![],
            all_categories,
            vec![
                (C::Shell, vec![T::WriteShellStdin]),
                (C::Fetch, vec![T::WebSearch]),
                (C::Mcp, vec![T::ReadMcpResource]),
            ],
        ),
        (
            Bypass,
            vec![],
            vec!["Read(**)", "Write(**)", "Shell(*)"],
            vec![],
            vec![
                (
                    C::Search,
                    vec![T::Grep, T::Glob, T::Ls, T::SemSearch, T::ReadLints],
                ),
                (C::Inert, vec![T::UpdateTodos, T::ReadTodos, T::CreatePlan]),
            ],
        ),
    ];
    for (mode, tools, deny, allowed, exclusions) in cases {
        let expected = CursorFence {
            allow: vec![],
            deny,
            allowed_categories: allowed.into_iter().collect(),
            notices: if exclusions.is_empty() {
                vec![]
            } else {
                vec![CursorFenceNotice { exclusions }]
            },
        };
        assert_eq!(
            fence(mode, &tools),
            expected,
            "mode {mode:?}, tools {tools:?}"
        );
    }
}

// Covers: replacing permission policy (including Cursor's web-search
// auto-accept, which skips permission requests) must retain auth/model/privacy; invalid
// object shapes must not become an empty config that silently loses auth.
// Owner: pure config transformation.
#[test]
fn derives_config_without_losing_unrelated_keys() {
    let fence = fence(Bypass, &[T::Read]);
    for (input, expected) in [
        (
            None,
            json!({"approvalMode": "allowlist", "autoAcceptWebSearch": false, "permissions": {"allow": [], "deny": ["Write(**)", "Shell(*)"]}}),
        ),
        (
            Some(json!({
                "approvalMode": "unrestricted", "autoAcceptWebSearch": true,
                "permissions": {"allow": ["Shell(*)"], "deny": ["Read(**)"]},
                "auth": {"token": "test-credential"}, "model": "composer-2.5", "privacy": false, "extra": [1, 2]
            })),
            json!({
                "approvalMode": "allowlist", "autoAcceptWebSearch": false,
                "permissions": {"allow": [], "deny": ["Write(**)", "Shell(*)"]},
                "auth": {"token": "test-credential"}, "model": "composer-2.5", "privacy": false, "extra": [1, 2]
            }),
        ),
    ] {
        assert_eq!(derive_config(input, &fence).unwrap(), expected);
    }
    for input in [Value::Null, json!([]), json!(true), json!("not an object")] {
        assert!(derive_config(Some(input), &fence).is_err());
    }
}

// Covers: malformed/oversize config must fail before writing; copied auth must
// be private on disk and the user's config must remain untouched.
// Owner: config I/O and OS permissions (one tempdir test, no env mutation).
#[test]
fn writes_private_run_config_and_rejects_invalid_source() {
    let temp = tempfile::tempdir().unwrap();
    let user = temp.path().join("user");
    fs::create_dir(&user).unwrap();
    let source = user.join("cli-config.json");
    let fence = fence(Bypass, T::ALL);
    let expected = json!({"auth": {"token": "test-credential"}, "approvalMode": "allowlist", "autoAcceptWebSearch": false, "permissions": {"allow": [], "deny": []}});
    let raw = br#"{"auth":{"token":"test-credential"},"approvalMode":"unrestricted"}"#;
    fs::write(&source, raw).unwrap();
    let managed = write_run_config(&user, temp.path(), &fence).unwrap();
    assert_eq!(managed.dir, temp.path().join("cursor-config"));
    assert_eq!(managed.notices, Vec::<String>::new());
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(managed.dir.join("cli-config.json")).unwrap())
            .unwrap(),
        expected
    );
    assert_eq!(fs::read(&source).unwrap(), raw.to_vec());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&managed.dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(managed.dir.join("cli-config.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    // Existing managed directories are not overwritten, even on a retry.
    assert!(write_run_config(&user, temp.path(), &fence).is_err());
    for (name, bytes) in [
        ("malformed", b"{broken".to_vec()),
        ("non-object", b"[]".to_vec()),
        (
            "oversize",
            vec![b' '; CURSOR_CONFIG_READ_BUDGET_BYTES as usize + 1],
        ),
    ] {
        let run = temp.path().join(name);
        fs::create_dir(&run).unwrap();
        fs::write(&source, bytes).unwrap();
        let error = write_run_config(&user, &run, &fence).unwrap_err();
        if name == "oversize" {
            let asked = CURSOR_CONFIG_READ_BUDGET_BYTES + 1;
            assert_eq!(error.to_string(), format!("Cursor config read budget exceeded for {}: limit {CURSOR_CONFIG_READ_BUDGET_BYTES} bytes, asked {asked} bytes", source.display()));
        }
        assert!(!run.join("cursor-config").exists());
    }
    let missing_user = temp.path().join("missing-user");
    let run = temp.path().join("missing-config-run");
    fs::create_dir(&run).unwrap();
    let managed = write_run_config(&missing_user, &run, &fence).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(managed.dir.join("cli-config.json")).unwrap())
            .unwrap(),
        json!({"approvalMode": "allowlist", "autoAcceptWebSearch": false, "permissions": {"allow": [], "deny": []}})
    );
}
