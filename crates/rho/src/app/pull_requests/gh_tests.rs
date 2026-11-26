use std::{ffi::OsString, io, path::Path};

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

/// Environment variables, by name.
type Vars<'a> = [(&'a str, &'a Path)];

/// An environment of `vars`, for [`GhCli::new`].
fn env<'a>(vars: &'a Vars<'a>) -> impl Fn(&str) -> Option<OsString> + 'a {
    |name| {
        vars.iter()
            .find(|(set, _)| *set == name)
            .map(|(_, value)| value.as_os_str().to_owned())
    }
}

// Covers: the daemon looks among mise's shims in the folder mise itself
// puts them in: MISE_SHIMS_DIR, else MISE_DATA_DIR, else XDG_DATA_HOME
// (LOCALAPPDATA on Windows), else the home folder; empty reads as unset,
// and a relative folder finds nothing.
// Owner: pure unit (policy table)
#[test]
fn mise_shims_are_where_mise_puts_them() {
    let root = Path::new(if cfg!(windows) { r"C:\" } else { "/" });
    let home = &root.join("home/rho");
    let shims = &root.join("opt/mise-shims");
    let data = &root.join("opt/mise");
    let xdg = &root.join("xdg");
    let local = &root.join("local");
    let empty = Path::new("");
    let relative = Path::new("relative/shims");
    let by_home = if cfg!(windows) {
        home.join("AppData/Local/mise/shims")
    } else {
        home.join(".local/share/mise/shims")
    };
    let by_local = if cfg!(windows) {
        local.join("mise/shims")
    } else {
        by_home.clone()
    };
    let cases: [(&Vars<'_>, Option<PathBuf>); 8] = [
        (
            &[
                ("MISE_SHIMS_DIR", shims),
                ("MISE_DATA_DIR", data),
                ("HOME", home),
            ],
            Some(shims.to_path_buf()),
        ),
        (
            &[
                ("MISE_DATA_DIR", data),
                ("XDG_DATA_HOME", xdg),
                ("HOME", home),
            ],
            Some(data.join("shims")),
        ),
        (
            &[
                ("XDG_DATA_HOME", xdg),
                ("LOCALAPPDATA", local),
                ("HOME", home),
            ],
            Some(xdg.join("mise/shims")),
        ),
        (&[("LOCALAPPDATA", local), ("HOME", home)], Some(by_local)),
        (&[("HOME", home)], Some(by_home.clone())),
        (
            &[
                ("MISE_SHIMS_DIR", empty),
                ("MISE_DATA_DIR", empty),
                ("XDG_DATA_HOME", empty),
                ("HOME", home),
            ],
            Some(by_home),
        ),
        (&[("MISE_SHIMS_DIR", relative), ("HOME", home)], None),
        (&[], None),
    ];
    let gh = if cfg!(windows) { "gh.exe" } else { "gh" };
    for (vars, expected) in cases {
        let cli = GhCli::new(env(vars));
        assert_eq!(
            cli.mise_gh,
            expected.map(|shims| shims.join(gh)),
            "{vars:?}"
        );
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
// user's login shell, after whatever its login files print, else among
// mise's shims, which come after the login shell; a file that can't run is
// skipped.
// Owner: OS or process
#[cfg(unix)]
#[tokio::test]
async fn gh_is_found_on_the_path_else_through_the_login_shell_else_mise() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    let bin = dir.join("bin");
    let login_bin = dir.join("login-bin");
    let shims = dir.join("shims");
    let unrunnable = dir.join("unrunnable");
    let empty = dir.join("empty");
    for folder in [&bin, &login_bin, &shims, &unrunnable, &empty] {
        std::fs::create_dir(folder).unwrap();
    }
    let gh = bin.join("gh");
    let via_login = login_bin.join("gh");
    let via_mise = shims.join("gh");
    for program in [&gh, &via_login, &via_mise] {
        write_program(program, "#!/bin/sh\n");
    }
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
    let skipping = PathBuf::from(std::env::join_paths([&unrunnable, &bin]).unwrap());
    let cases: [(&Vars<'_>, Option<&PathBuf>); 6] = [
        (
            &[
                ("PATH", &bin),
                ("SHELL", &shell),
                ("MISE_SHIMS_DIR", &shims),
            ],
            Some(&gh),
        ),
        (&[("PATH", &skipping)], Some(&gh)),
        (
            &[
                ("PATH", &empty),
                ("SHELL", &shell),
                ("MISE_SHIMS_DIR", &shims),
            ],
            Some(&via_login),
        ),
        (
            &[
                ("PATH", &empty),
                ("SHELL", &no_shell),
                ("MISE_SHIMS_DIR", &shims),
            ],
            Some(&via_mise),
        ),
        (&[("PATH", &empty), ("MISE_SHIMS_DIR", &unrunnable)], None),
        (&[("PATH", &empty), ("SHELL", &no_shell)], None),
    ];
    for (vars, expected) in cases {
        let cli = GhCli::new(env(vars));
        assert_eq!(cli.find().await.as_ref(), expected, "{vars:?}");
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
        let cli = GhCli::new(env(&[("PATH", &bin)]));
        assert_eq!(cli.read("github.com", &plan).await, expected, "{body}");
    }

    let nowhere = GhCli::new(env(&[("PATH", &dir.join("empty"))]));
    let read = nowhere.read("github.com", &plan).await;
    assert!(
        matches!(read, Err(ReadError::NotInstalled(_))),
        "no gh: {read:?}"
    );
}
