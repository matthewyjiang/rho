use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use pretty_assertions::assert_eq;

use super::super::test_server::{Reply, TestServer};
use super::*;

// Covers: a downloaded spec is cached, revalidated with its ETag next session,
// and served stale with a warning when the refresh fails. Without a cache the
// failure is an error.
// Owner: OpenAPI spec cache
#[tokio::test]
async fn revalidates_with_etag_and_falls_back_to_the_cache() {
    let failing = Arc::new(AtomicBool::new(false));
    let fail = failing.clone();
    let server = TestServer::start(move |request| {
        if fail.load(Ordering::SeqCst) {
            Reply::new(500, "down")
        } else if request.headers.get("if-none-match").map(String::as_str) == Some("\"v1\"") {
            Reply::new(304, "")
        } else {
            Reply::new(200, "spec v1").header("etag", "\"v1\"")
        }
    })
    .await;
    let url = Url::parse(&server.url("/spec.json")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cache = SpecCache::new(dir.path().join("openapi"));
    let client = crate::reqwest_client();
    let fetch = |cache| SpecFetch {
        client: &client,
        cache,
        timeout: Duration::from_secs(30),
        max_spec_bytes: 1024,
    };
    let location = SpecLocation::Url(url.clone());
    let result = |fetched: Result<FetchedSpec, String>| {
        fetched.map(|fetched| (String::from_utf8(fetched.bytes).unwrap(), fetched.warning))
    };

    let fresh = result(fetch(Some(&cache)).fetch(&location).await);
    let revalidated = result(fetch(Some(&cache)).fetch(&location).await);
    failing.store(true, Ordering::SeqCst);
    let stale = result(fetch(Some(&cache)).fetch(&location).await);
    let uncached = result(fetch(None).fetch(&location).await);

    assert_eq!(
        (fresh, revalidated, stale, uncached),
        (
            Ok(("spec v1".to_owned(), None)),
            Ok(("spec v1".to_owned(), None)),
            Ok((
                "spec v1".to_owned(),
                Some(format!(
                    "could not refresh {url}: HTTP 500 Internal Server Error; using the cached copy"
                ))
            )),
            Err(format!(
                "could not download {url}: HTTP 500 Internal Server Error"
            )),
        )
    );
    let requests = server.requests();
    let validators: Vec<Option<&str>> = requests
        .iter()
        .map(|request| request.headers.get("if-none-match").map(String::as_str))
        .collect();
    assert_eq!(validators, vec![None, Some("\"v1\""), Some("\"v1\""), None]);
}
