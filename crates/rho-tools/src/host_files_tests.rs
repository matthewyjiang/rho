use std::sync::Arc;

use pretty_assertions::assert_eq;

use super::{
    test_support::{Hold, MemoryHost},
    CommitError, HostTextFiles, TextRoute,
};
use crate::document::MAX_DOCUMENT_INPUT_BYTES;

// Covers: the input limit applies to text a tool reads in, not to the check
// before a commit, so an edit that grew a file past the limit can still be
// rolled back.
// Owner: host_files commit
#[tokio::test]
async fn commits_check_text_past_the_input_limit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.txt");
    let grown = "x".repeat(MAX_DOCUMENT_INPUT_BYTES + 1);
    let host = Arc::new(MemoryHost::new(dir.path(), &[(&path, grown.as_str())]));
    let route = TextRoute::Host(Arc::clone(&host) as Arc<dyn HostTextFiles>);

    let read = route.read(&path, "big.txt").await.is_err();
    let restored = route.commit(&path, "big.txt", &grown, "small\n").await.ok();

    assert_eq!((read, restored), (true, Some("small\n".to_string())));
}

// Covers: a host commit that outlives its cancelled call holds its path, so
// a later commit waits, then finds the file changed instead of being
// overwritten by the late write.
// Owner: host_files commit
#[tokio::test]
async fn host_commits_to_one_path_do_not_interleave() {
    use tokio::sync::Notify;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.txt");
    let (held, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let mut host = MemoryHost::new(dir.path(), &[(&path, "base\n")]);
    host.hold = Some(Hold {
        path: path.clone(),
        held: Arc::clone(&held),
        release: Arc::clone(&release),
        used: Default::default(),
    });
    let host = Arc::new(host);
    let route = TextRoute::Host(Arc::clone(&host) as Arc<dyn HostTextFiles>);

    let first = tokio::spawn({
        let (route, path) = (route.clone(), path.clone());
        async move { route.commit(&path, "a.txt", "base\n", "first\n").await.ok() }
    });
    held.notified().await;
    let second = route.commit(&path, "a.txt", "base\n", "second\n");
    tokio::pin!(second);
    tokio::select! {
        biased;
        _ = &mut second => panic!("the second commit ran while the first held the path"),
        () = tokio::task::yield_now() => {}
    }
    release.notify_one();
    let first = first.await.unwrap();
    let second = match second.await {
        Err(CommitError::Changed { live }) => Some(live),
        _ => None,
    };

    assert_eq!(
        (
            first,
            second,
            host.files.lock().unwrap().get(&path).cloned()
        ),
        (
            Some("first\n".to_string()),
            Some("first\n".to_string()),
            Some("first\n".to_string())
        )
    );
}
