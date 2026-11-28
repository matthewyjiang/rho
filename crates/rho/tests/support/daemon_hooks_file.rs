//! The user's hooks file read and replaced whole through the daemon's
//! socket (`rho/hooks/file/*`), as a client copying it from another daemon
//! does.

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::{HooksFileGet, HooksFileSet},
    Client, ClientError, ConfigRevision, ErrorCode, HooksFileGetParams, HooksFileGetResponse,
    HooksFileSetParams,
};

use super::sessions::{plain_canonical, Sandbox};

const HOOKS: &str = r#"# Copied with its comments.
version = 1

[[hook]]
id = "log"
on = "after_tool_use"
command = ["logger"]
timeout = "1s"
"#;

async fn get(client: &Client) -> HooksFileGetResponse {
    client
        .request::<HooksFileGet>(&HooksFileGetParams::new())
        .await
        .unwrap()
}

/// Writes `text` against `revision`: the new revision, or the code of the
/// error that refused it.
async fn set(
    client: &Client,
    revision: &ConfigRevision,
    text: Option<&str>,
) -> Result<ConfigRevision, ErrorCode> {
    client
        .request::<HooksFileSet>(&HooksFileSetParams::new(
            revision.clone(),
            text.map(str::to_owned),
        ))
        .await
        .map(|response| response.revision)
        .map_err(|error| match error {
            ClientError::Rpc { error, .. } => error.error_code().unwrap(),
            error => panic!("set failed outside the protocol: {error}"),
        })
}

// Covers: a client replaces the user's hooks file only at the revision it
// read, only with text a session would load, and only while no other rho
// process holds the file's edit lock; it reads back exactly what it wrote,
// comments included, and deleting the file reads as absent again.
// Owner: daemon hooks file protocol.
#[tokio::test]
async fn a_client_replaces_the_users_hooks_file_only_at_the_revision_it_read() {
    let sandbox = Sandbox::new();
    let path = sandbox.isolated.rho_home().join("hooks.toml");
    let client = sandbox.start_daemon("fixed").await;

    let absent = get(&client).await;
    assert_eq!(
        (absent.path, absent.text),
        (
            plain_canonical(&sandbox.isolated.rho_home()).join("hooks.toml"),
            None
        )
    );
    let written = set(&client, &absent.revision, Some(HOOKS)).await.unwrap();
    let read = get(&client).await;
    assert_eq!(
        (read.revision, read.text.as_deref()),
        (written.clone(), Some(HOOKS))
    );

    let lock = std::fs::File::create(path.with_file_name(".hooks.toml.rho-edit.lock")).unwrap();
    fs2::FileExt::try_lock_exclusive(&lock).unwrap();
    let held = set(&client, &written, None).await;
    drop(lock);
    let refused = [
        set(&client, &written, Some("version = 2\n")).await,
        set(&client, &absent.revision, None).await,
        held,
    ];
    assert_eq!(
        refused,
        [
            Err(ErrorCode::InvalidParams),
            Err(ErrorCode::ConfigConflict),
            Err(ErrorCode::ConfigConflict),
        ]
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), HOOKS);

    assert_eq!(set(&client, &written, None).await, Ok(absent.revision));
    assert!(!path.exists());
}
