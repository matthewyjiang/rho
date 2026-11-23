use std::io;
#[cfg(unix)]
use std::path::Path;

use pretty_assertions::assert_eq;

use super::*;
#[cfg(unix)]
use crate::app::pull_requests::model::{BranchId, Budget, RepoId};

// Covers: why a read failed decides what the user is told to do: install
// gh, sign in to the host, or nothing they can fix. gh's first line, without
// its `gh: ` prefix, is the reason.
// Owner: pure unit (typed error mapping)
#[test]
fn failures_tell_not_installed_and_signed_out_from_the_rest() {
    let missing = io::Error::from(io::ErrorKind::NotFound);
    let denied = io::Error::from(io::ErrorKind::PermissionDenied);
    let cases = [
        (
            Ended::Unstarted(&missing),
            ReadError::NotInstalled(missing.to_string()),
        ),
        (
            Ended::Unstarted(&denied),
            ReadError::Failed(denied.to_string()),
        ),
        (
            Ended::Said("gh: HTTP 401: Bad credentials (https://api.github.com/graphql)"),
            ReadError::SignedOut(
                "HTTP 401: Bad credentials (https://api.github.com/graphql)".to_owned(),
            ),
        ),
        (
            Ended::Said("To get started with GitHub CLI, please run:  gh auth login"),
            ReadError::SignedOut(
                "To get started with GitHub CLI, please run:  gh auth login".to_owned(),
            ),
        ),
        (
            Ended::Said("\nerror connecting to api.github.com\ncheck your internet connection"),
            ReadError::Failed("error connecting to api.github.com".to_owned()),
        ),
    ];
    for (ended, expected) in cases {
        let label = format!("{ended:?}");
        assert_eq!(classify(ended), expected, "{label}");
    }
}

/// Writes `script` to `path` as a program. A child process writes it: a
/// write fd held by this test binary can be inherited across a concurrent
/// test's fork, and exec of the fresh script would then fail with ETXTBSY.
#[cfg(unix)]
fn write_program(path: &Path, script: &str) {
    let written = std::process::Command::new("/bin/sh")
        .args(["-c", r#"printf '%s' "$1" > "$2" && chmod +x "$2""#, "sh"])
        .arg(script)
        .arg(path)
        .status()
        .unwrap();
    assert!(written.success(), "writing {}", path.display());
}

// Covers: a daemon started with a bare PATH still finds gh through the
// user's login shell, after whatever its login files print; a file on the
// PATH that can't run is skipped.
// Owner: OS or process
#[cfg(unix)]
#[tokio::test]
async fn gh_is_found_on_the_path_else_through_the_login_shell() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    let bin = dir.join("bin");
    let login_bin = dir.join("login-bin");
    let unrunnable = dir.join("unrunnable");
    let empty = dir.join("empty");
    for folder in [&bin, &login_bin, &unrunnable, &empty] {
        std::fs::create_dir(folder).unwrap();
    }
    let gh = bin.join("gh");
    let via_login = login_bin.join("gh");
    write_program(&gh, "#!/bin/sh\n");
    write_program(&via_login, "#!/bin/sh\n");
    std::fs::write(unrunnable.join("gh"), "#!/bin/sh\n").unwrap();
    // A login shell whose PATH holds `login-bin`, greeting first, as login
    // files do. It runs as `shell -l -c 'command -v gh'`.
    let shell = dir.join("login-sh");
    let login_bin_path = login_bin.display();
    write_program(
        &shell,
        &format!("#!/bin/sh\necho 'welcome'\nPATH={login_bin_path}:/usr/bin:/bin\nexec /bin/sh -c \"$3\"\n"),
    );
    let no_shell = dir.join("no-such-shell");
    let skipping = std::env::join_paths([&unrunnable, &bin]).unwrap();
    let cases = [
        (bin.clone().into_os_string(), Some(&shell), Some(gh.clone())),
        (skipping, None, Some(gh)),
        (
            empty.clone().into_os_string(),
            Some(&shell),
            Some(via_login),
        ),
        (empty.clone().into_os_string(), None, None),
        (empty.into_os_string(), Some(&no_shell), None),
    ];
    for (path, shell, expected) in cases {
        let label = format!("PATH {path:?}, shell {shell:?}");
        let cli = GhCli::new(
            Some(path),
            shell.map(|shell| shell.clone().into_os_string()),
        );
        assert_eq!(cli.find().await, expected, "{label}");
    }
}

// Covers: gh exits 1 when part of a query failed, yet prints the rest of
// the answer, so a read parses what gh printed whatever its status; a
// refused token reads as signed out; no gh at all reads as not installed.
// Owner: OS or process
#[cfg(unix)]
#[tokio::test]
async fn a_read_takes_what_gh_printed_whatever_it_exits_with() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    let gone = RepoId {
        host: "github.com".to_owned(),
        owner: "acme".to_owned(),
        name: "gone".to_owned(),
    };
    let plan = FetchPlan {
        branches: vec![BranchId {
            repo: gone.clone(),
            branch: "main".to_owned(),
        }],
        links: Vec::new(),
        details: Vec::new(),
    };
    let not_found = "Could not resolve to a Repository with the name 'acme/gone'.";
    let answer = dir.join("answer.json");
    std::fs::write(
        &answer,
        format!(
            r#"{{"data":{{"viewer":{{"login":"rho-bot"}},"rateLimit":{{"cost":1,"remaining":4990,"resetAt":"2026-11-15T06:18:47Z"}},"r0":null}},"errors":[{{"type":"NOT_FOUND","path":["r0"],"message":"{not_found}"}}]}}"#
        ),
    )
    .unwrap();
    let answer = answer.display();
    // Each gh checks that it was asked a GraphQL query of github.com.
    let asked = r#"[ "$1 $2 $3 $4 $5" = "api graphql --hostname github.com -f" ] || exit 9"#;
    let cases = [
        (
            format!("cat '{answer}'\necho \"gh: {not_found}\" >&2\nexit 1"),
            Ok(Batch {
                budget: Some(Budget {
                    remaining: 4990,
                    // 2026-11-15T06:18:47Z
                    resets_at: 1_794_723_527,
                    cost: Some(1),
                }),
                missing: vec![(gone, not_found.to_owned())],
                ..Batch::default()
            }),
        ),
        (
            "echo 'gh: HTTP 401: Bad credentials (https://api.github.com/graphql)' >&2\nexit 1"
                .to_owned(),
            Err(ReadError::SignedOut(
                "HTTP 401: Bad credentials (https://api.github.com/graphql)".to_owned(),
            )),
        ),
    ];
    for (ix, (body, expected)) in cases.into_iter().enumerate() {
        let bin = dir.join(format!("bin{ix}"));
        std::fs::create_dir(&bin).unwrap();
        write_program(&bin.join("gh"), &format!("#!/bin/sh\n{asked}\n{body}\n"));
        let cli = GhCli::new(Some(bin.into_os_string()), None);
        assert_eq!(cli.read("github.com", &plan).await, expected, "{body}");
    }

    let nowhere = GhCli::new(Some(dir.join("empty").into_os_string()), None);
    let read = nowhere.read("github.com", &plan).await;
    assert!(
        matches!(read, Err(ReadError::NotInstalled(_))),
        "no gh: {read:?}"
    );
}
