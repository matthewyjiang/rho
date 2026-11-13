use pretty_assertions::assert_eq;

use super::AddDirError;
use crate::{app::session_runtime::test_support::idle_runtime, config::Config};

// Covers: a directory added between turns is granted to the runtime the
// session's tools run on, not only recorded; one the session already
// reaches through it is refused and changes nothing.
// Owner: session runtime added directories
#[tokio::test]
async fn an_added_directory_is_granted_to_the_sessions_tools() {
    let (home, outside) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let config = Config {
        provider: "test".into(),
        model: "model".into(),
        ..Config::default()
    };
    let mut runtime = idle_runtime(home.path(), &config).await;
    let dir = outside.path().canonicalize().unwrap();
    let nested = dir.join("nested");
    std::fs::create_dir(&nested).unwrap();

    let added = runtime
        .add_dir(dir.clone(), &config)
        .await
        .map_err(|error| error.to_string());
    let covered = match runtime.add_dir(nested, &config).await {
        Err(AddDirError::Covered { root, .. }) => Some(root),
        Ok(()) | Err(AddDirError::Change(_)) => None,
    };

    assert_eq!(
        (
            added,
            covered,
            runtime
                .built
                .runtime
                .diagnostics()
                .granted_workspace_roots()
                .to_vec(),
            runtime.added_dirs().as_slice().to_vec(),
        ),
        (Ok(()), Some(dir.clone()), vec![dir.clone()], vec![dir])
    );
    runtime.shutdown().await;
}
