use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;
use rho_protocol::{HookStatus, HooksFileScope, HooksFileStatus};
use tempfile::TempDir;

use super::*;

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

const USER_HOOKS: &str =
    "version = 1\n\n[[hook]]\nid = \"log\"\non = \"after_tool_use\"\ncommand = [\"logger\"]\ntimeout = \"1s\"\n";
const PROJECT_HOOKS: &str =
    "version = 1\n\n[[hook]]\nid = \"fmt\"\non = \"after_tool_use\"\ncommand = [\"./x\"]\ntimeout = \"1s\"\n";
const BROKEN: &str = "version = 1\n[[hook]]\nid = \n";

/// What a listed file shows a settings page, with each error reduced to
/// whether there is one and a disabling file named by its scope.
#[derive(Debug, PartialEq, Eq)]
struct Listed {
    path: PathBuf,
    scope: HooksFileScope,
    status: &'static str,
    hooks: Vec<(String, HookStatus)>,
}

fn listed(files: Vec<HooksFile>) -> Vec<Listed> {
    let scope_of = |path: &Path| {
        files
            .iter()
            .find(|file| file.path == path)
            .map(|file| file.scope)
    };
    let statuses = files
        .iter()
        .map(|file| match &file.status {
            HooksFileStatus::Absent => "absent",
            HooksFileStatus::Loaded => "loaded",
            HooksFileStatus::Disabled { by } => match scope_of(by) {
                Some(HooksFileScope::User) => "disabled by user",
                Some(HooksFileScope::Project) => "disabled by project",
                _ => "disabled by another file",
            },
            HooksFileStatus::Untrusted { error: None } => "untrusted",
            HooksFileStatus::Untrusted { error: Some(_) } => "untrusted, invalid",
            HooksFileStatus::Invalid { .. } => "invalid",
            _ => "unknown",
        })
        .collect::<Vec<_>>();
    files
        .into_iter()
        .zip(statuses)
        .map(|(file, status)| Listed {
            status,
            path: file.path,
            scope: file.scope,
            hooks: file
                .hooks
                .into_iter()
                .map(|hook| (hook.id, hook.status))
                .collect(),
        })
        .collect()
}

// Covers: each hooks file is listed on its own, so a broken file reports its
// error, as untrusted when the project is, without hiding the other file's
// hooks, which are disabled when that broken file stops a session loading
// any. The daemon test `configured` covers trust from a client.
// Owner: host hook discovery.
#[test]
fn each_hooks_file_is_listed_with_its_own_status() {
    struct Case {
        name: &'static str,
        user: Option<&'static str>,
        project: Option<&'static str>,
        trust: ProjectTrust,
        expected: [(&'static str, &'static [(&'static str, HookStatus)]); 2],
    }
    let cases = [
        Case {
            name: "no files",
            user: None,
            project: None,
            trust: ProjectTrust::Trusted,
            expected: [("absent", &[]), ("absent", &[])],
        },
        Case {
            name: "a broken untrusted project file",
            user: Some(USER_HOOKS),
            project: Some(BROKEN),
            trust: ProjectTrust::Untrusted,
            expected: [
                ("loaded", &[("user:log", HookStatus::Active)]),
                ("untrusted, invalid", &[]),
            ],
        },
        Case {
            name: "a broken user file disables the project's hooks",
            user: Some(BROKEN),
            project: Some(PROJECT_HOOKS),
            trust: ProjectTrust::Trusted,
            expected: [
                ("invalid", &[]),
                ("disabled by user", &[("project:fmt", HookStatus::Disabled)]),
            ],
        },
        Case {
            name: "a broken trusted project file disables the user's hooks",
            user: Some(USER_HOOKS),
            project: Some(BROKEN),
            trust: ProjectTrust::Trusted,
            expected: [
                ("disabled by project", &[("user:log", HookStatus::Disabled)]),
                ("invalid", &[]),
            ],
        },
    ];

    for case in cases {
        let home = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        let user_file = home.path().join("hooks.toml");
        let project_file = project.path().join(".rho/hooks.toml");
        write(&project.path().join("x"), "#!/bin/sh\n");
        if let Some(contents) = case.user {
            write(&user_file, contents);
        }
        if let Some(contents) = case.project {
            write(&project_file, contents);
        }

        let files = configured_files(Some(home.path()), Some(project.path()), case.trust);

        let [user, project] = case.expected;
        let expect =
            |path: &Path, scope, (status, hooks): (&'static str, &[(&str, HookStatus)])| Listed {
                path: path.to_path_buf(),
                scope,
                status,
                hooks: hooks
                    .iter()
                    .map(|(id, status)| ((*id).to_owned(), *status))
                    .collect(),
            };
        assert_eq!(
            listed(files),
            vec![
                expect(&user_file, HooksFileScope::User, user),
                expect(&project_file, HooksFileScope::Project, project),
            ],
            "{}",
            case.name
        );
    }
}

// Covers: a broken hooks file's error reads the same in `rho/hooks/list` as
// in a session's hooks report, and neither repeats the file's path, which
// both give in a field of its own, so a client shows it once.
// Owner: host hook discovery.
#[test]
fn a_broken_files_error_leaves_out_its_path_as_the_session_report_does() {
    use super::super::{HookCatalog, HookSnapshot};

    struct Case {
        name: &'static str,
        user: &'static str,
        project: &'static str,
        trust: ProjectTrust,
    }
    let cases = [
        Case {
            name: "a broken user file",
            user: BROKEN,
            project: PROJECT_HOOKS,
            trust: ProjectTrust::Trusted,
        },
        Case {
            name: "a broken trusted project file",
            user: USER_HOOKS,
            project: BROKEN,
            trust: ProjectTrust::Trusted,
        },
        Case {
            name: "a broken untrusted project file",
            user: USER_HOOKS,
            project: BROKEN,
            trust: ProjectTrust::Untrusted,
        },
    ];

    for case in cases {
        let home = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        write(&home.path().join("hooks.toml"), case.user);
        write(&project.path().join(".rho/hooks.toml"), case.project);
        write(&project.path().join("x"), "#!/bin/sh\n");

        let listed: Vec<_> = configured_files(Some(home.path()), Some(project.path()), case.trust)
            .into_iter()
            .filter_map(|file| match file.status {
                HooksFileStatus::Invalid { error }
                | HooksFileStatus::Untrusted { error: Some(error) } => Some((file.path, error)),
                _ => None,
            })
            .collect();
        let report = HookSnapshot::checked(
            None,
            HookCatalog::discover(Some(home.path()), Some(project.path()), case.trust),
        )
        .session_report();
        let reported = report
            .invalid
            .map(|file| (file.path, file.error))
            .or_else(|| {
                let skipped = report.skipped_untrusted?;
                Some((skipped.path, skipped.error?))
            });

        let [(path, error)] = &listed[..] else {
            panic!("{}: one broken file, listed {listed:?}", case.name);
        };
        assert_eq!(
            Some((path.clone(), error.clone())),
            reported,
            "{}",
            case.name
        );
        assert!(
            !error.contains(&*path.to_string_lossy()),
            "{}: {error}",
            case.name
        );
    }
}
