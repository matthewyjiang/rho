use pretty_assertions::assert_eq;

use crate::{model::ModelIdentity, provider::ScriptedProvider, Rho, Workspace};

#[test]
fn reports_unrestricted_file_access() {
    let root = tempfile::tempdir().unwrap();
    let canonical_root = std::fs::canonicalize(root.path()).unwrap();
    let workspace = Workspace::new(root.path())
        .unwrap()
        .with_unrestricted_file_access();
    let provider = ScriptedProvider::new(ModelIdentity::new("scripted", "test", "model"), []);
    let runtime = Rho::builder()
        .provider(provider)
        .workspace(workspace)
        .build()
        .unwrap();

    let diagnostics = runtime.diagnostics();

    assert_eq!(diagnostics.workspace_root(), Some(canonical_root.as_path()));
    assert!(diagnostics.has_unrestricted_file_access());
}
