use std::path::Path;

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::*;

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn user_hooks(ids: &[&str]) -> String {
    let mut contents = String::from("version = 1\n");
    for id in ids {
        contents.push_str(&format!(
            "\n[[hook]]\nid = \"{id}\"\non = \"after_tool_use\"\ncommand = [\"logger\"]\ntimeout = \"1s\"\n"
        ));
    }
    contents
}

#[test]
fn a_missing_hooks_file_yields_an_empty_catalog() {
    let home = TempDir::new().unwrap();

    let catalog = HookCatalog::discover(Some(home.path()), None, ProjectTrust::Untrusted).unwrap();

    assert!(catalog.is_empty());
    assert!(catalog.files().is_empty());
    assert_eq!(catalog.skipped_untrusted(), None);
}

#[test]
fn user_hooks_load_without_any_trust_grant() {
    let home = TempDir::new().unwrap();
    write(&home.path().join("hooks.toml"), &user_hooks(&["log"]));

    let catalog = HookCatalog::discover(Some(home.path()), None, ProjectTrust::Untrusted).unwrap();

    assert_eq!(catalog.len(), 1);
    assert_eq!(catalog.hooks()[0].qualified_id(), "user:log");
}

// Covers: project commands must be inspectable before they become executable.
// Owner: host hook discovery.
#[test]
fn an_untrusted_project_file_is_inactive_but_inspectable() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    write(&home.path().join("hooks.toml"), &user_hooks(&["log"]));
    write(
        &project.path().join(".rho/hooks.toml"),
        "version = 1\n\n[[hook]]\nid = \"p\"\non = \"after_tool_use\"\ncommand = [\"./x\"]\ntimeout = \"1s\"\n",
    );
    write(&project.path().join("x"), "#!/bin/sh\n");

    let catalog = HookCatalog::discover(
        Some(home.path()),
        Some(project.path()),
        ProjectTrust::Untrusted,
    )
    .unwrap();

    assert_eq!(catalog.len(), 1);
    assert_eq!(
        catalog
            .spawn_contract()
            .into_iter()
            .map(|contract| (contract.id, contract.active))
            .collect::<Vec<_>>(),
        vec![("user:log".into(), true), ("project:p".into(), false)]
    );
    assert_eq!(
        catalog
            .skipped_untrusted()
            .map(|skipped| skipped.path.clone()),
        Some(project.path().join(".rho/hooks.toml"))
    );
}

#[test]
fn an_untrusted_workspace_without_a_project_file_reports_nothing() {
    let project = TempDir::new().unwrap();

    let catalog =
        HookCatalog::discover(None, Some(project.path()), ProjectTrust::Untrusted).unwrap();

    assert_eq!(catalog.skipped_untrusted(), None);
}

#[test]
fn a_trusted_project_file_loads_after_user_hooks() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    write(&home.path().join("hooks.toml"), &user_hooks(&["first"]));
    write(&project.path().join(".rho/hooks/run"), "#!/bin/sh\n");
    write(
        &project.path().join(".rho/hooks.toml"),
        "version = 1\n\n[[hook]]\nid = \"second\"\non = \"after_tool_use\"\ncommand = [\"./.rho/hooks/run\"]\ntimeout = \"1s\"\n",
    );

    let catalog = HookCatalog::discover(
        Some(home.path()),
        Some(project.path()),
        ProjectTrust::Trusted,
    )
    .unwrap();

    assert_eq!(
        catalog
            .hooks()
            .iter()
            .map(HookDefinition::qualified_id)
            .collect::<Vec<_>>(),
        vec!["user:first", "project:second"]
    );
}

#[test]
fn a_project_may_reuse_a_user_hook_id_without_collision() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    write(&home.path().join("hooks.toml"), &user_hooks(&["shared"]));
    write(&project.path().join(".rho/hooks/run"), "#!/bin/sh\n");
    write(
        &project.path().join(".rho/hooks.toml"),
        "version = 1\n\n[[hook]]\nid = \"shared\"\non = \"after_tool_use\"\ncommand = [\"./.rho/hooks/run\"]\ntimeout = \"1s\"\n",
    );

    let catalog = HookCatalog::discover(
        Some(home.path()),
        Some(project.path()),
        ProjectTrust::Trusted,
    )
    .unwrap();

    assert_eq!(
        catalog
            .hooks()
            .iter()
            .map(HookDefinition::qualified_id)
            .collect::<Vec<_>>(),
        vec!["user:shared", "project:shared"]
    );
}

#[test]
fn an_invalid_trusted_project_file_fails_discovery() {
    let project = TempDir::new().unwrap();
    write(&project.path().join(".rho/hooks.toml"), "version = 9\n");

    let error = HookCatalog::discover(None, Some(project.path()), ProjectTrust::Trusted)
        .expect_err("an invalid trusted file must not be skipped silently");

    assert_eq!(error.field.as_deref(), Some("version"));
}

#[test]
fn matching_preserves_configured_order_and_applies_the_tool_matcher() {
    let home = TempDir::new().unwrap();
    write(
        &home.path().join("hooks.toml"),
        r#"
version = 1

[[hook]]
id = "first"
on = "before_tool_use"
tools = ["bash"]
command = ["a"]
timeout = "1s"

[[hook]]
id = "second"
on = "before_tool_use"
command = ["b"]
timeout = "1s"

[[hook]]
id = "third"
on = "before_tool_use"
tools = ["read_file"]
command = ["c"]
timeout = "1s"

[[hook]]
id = "post"
on = "after_tool_use"
command = ["d"]
timeout = "1s"
"#,
    );
    let catalog = HookCatalog::discover(Some(home.path()), None, ProjectTrust::Untrusted).unwrap();

    let matched: Vec<_> = catalog
        .matching(rho_sdk::hooks::HookEventKind::BeforeToolUse, Some("bash"))
        .into_iter()
        .map(HookDefinition::qualified_id)
        .collect();

    assert_eq!(matched, vec!["user:first", "user:second"]);
}

#[test]
fn matching_without_a_tool_selects_only_the_event() {
    let home = TempDir::new().unwrap();
    write(&home.path().join("hooks.toml"), &user_hooks(&["log"]));
    let catalog = HookCatalog::discover(Some(home.path()), None, ProjectTrust::Untrusted).unwrap();

    assert_eq!(
        catalog
            .matching(rho_sdk::hooks::HookEventKind::RunCompleted, None)
            .len(),
        0
    );
    assert_eq!(
        catalog
            .matching(rho_sdk::hooks::HookEventKind::AfterToolUse, None)
            .len(),
        1
    );
}

#[test]
fn the_spawn_contract_shows_everything_that_decides_what_runs() {
    let project = TempDir::new().unwrap();
    write(&project.path().join(".rho/hooks/fmt"), "#!/bin/sh\n");
    write(
        &project.path().join(".rho/hooks.toml"),
        "version = 1\n\n[[hook]]\nid = \"fmt\"\non = \"after_tool_use\"\ntools = [\"edit_file\"]\ncommand = [\"./.rho/hooks/fmt\", \"--all\"]\ntimeout = \"5s\"\nenv = [\"MY_TOKEN\"]\n",
    );
    let catalog = HookCatalog::discover(None, Some(project.path()), ProjectTrust::Trusted).unwrap();

    let contract = catalog.spawn_contract();
    let entry = &contract[0];

    assert_eq!(entry.id, "project:fmt");
    assert_eq!(entry.event, "after_tool_use");
    assert_eq!(entry.tools, "edit_file");
    assert_eq!(
        entry.command,
        vec![
            crate::paths::display(&project.path().join(".rho/hooks/fmt")),
            "--all".to_owned()
        ]
    );
    assert_eq!(entry.working_directory, project.path());
    assert_eq!(entry.timeout, std::time::Duration::from_secs(5));
    assert!(entry.environment.contains(&"PATH".to_owned()));
    assert!(entry
        .environment
        .contains(&crate::hooks::IN_HOOK_ENV.to_owned()));
    assert!(entry.environment.contains(&"MY_TOKEN".to_owned()));
}

#[test]
fn project_trust_comes_from_an_explicit_opt_in() {
    assert_eq!(ProjectTrust::from_env(Some("1")), ProjectTrust::Trusted);
    assert_eq!(ProjectTrust::from_env(Some("0")), ProjectTrust::Untrusted);
    assert_eq!(
        ProjectTrust::from_env(Some("true")),
        ProjectTrust::Untrusted
    );
    assert_eq!(ProjectTrust::from_env(None), ProjectTrust::Untrusted);
}
