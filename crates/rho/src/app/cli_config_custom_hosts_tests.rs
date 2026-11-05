//! Custom-host model discovery at startup.

use std::sync::Arc;

use clap::Parser;
use pretty_assertions::assert_eq;

use {
    crate::cli::Cli,
    crate::config::Config,
    rho_providers::credentials::MemoryCredentialStore,
    rho_providers::model::{
        provider_models::{
            cached_provider_models, replace_cached_provider_models_for_tests,
            set_provider_models_cache_dir_for_tests, ProviderModel,
        },
        ReasoningCapabilities,
    },
};

use super::{
    refresh_custom_provider_models, refresh_custom_provider_models_for_session,
    tests::unique_cache_dir,
};

/// Clears the process-wide custom provider registry when a test ends.
struct RestoreCustomProviders;

impl Drop for RestoreCustomProviders {
    fn drop(&mut self) {
        rho_providers::provider::reset_custom_openai_compatible_providers_for_tests();
    }
}

fn cached_model_ids(provider: &str) -> Vec<String> {
    cached_provider_models(provider)
        .into_iter()
        .map(|model| model.model)
        .collect()
}

// Covers: custom hosts must populate the picker from /v1/models without a manual refresh
// Owner: app startup
// The registry lock must cover the whole test so parallel suites cannot mutate the
// process-wide custom provider set while this current-thread test is awaiting I/O.
#[allow(clippy::await_holding_lock)]
#[tokio::test(flavor = "current_thread")]
async fn custom_hosts_fetch_models_from_the_openai_compatible_endpoint() {
    let _lock = rho_providers::provider::custom_provider_registry_test_lock();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_base = format!("http://{}/v1", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buffer = [0; 2048];
        let bytes = tokio::io::AsyncReadExt::read(&mut stream, &mut buffer)
            .await
            .unwrap();
        let request = String::from_utf8_lossy(&buffer[..bytes]);
        assert!(request.starts_with("GET /v1/models HTTP/1.1"));
        assert!(!request.to_ascii_lowercase().contains("authorization:"));
        let body = r#"{"data":[{"id":"composer-2.5"},{"id":"composer-2.5-fast"}]}"#;
        let reply = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        tokio::io::AsyncWriteExt::write_all(&mut stream, reply.as_bytes())
            .await
            .unwrap();
    });

    let cache_dir = unique_cache_dir("custom-host-refresh");
    set_provider_models_cache_dir_for_tests(Some(cache_dir.clone()));
    rho_providers::provider::reset_custom_openai_compatible_providers_for_tests();
    let _restore = RestoreCustomProviders;
    let mut cfg = Config::default();
    cfg.providers.set_endpoint("composer", &api_base).unwrap();
    cfg.providers.activate().unwrap();
    let store = MemoryCredentialStore::default();

    refresh_custom_provider_models(&cfg.providers, &store).await;
    let models = cached_model_ids("composer");
    set_provider_models_cache_dir_for_tests(None);
    let _ = std::fs::remove_dir_all(cache_dir);

    assert_eq!(models, ["composer-2.5", "composer-2.5-fast"]);
}

// Covers: unavailable custom hosts must not serialize their discovery timeouts
// Owner: app startup
// Same process-wide registry lock as the fetch test above.
#[allow(clippy::await_holding_lock)]
#[tokio::test(flavor = "current_thread")]
async fn custom_hosts_refresh_models_concurrently() {
    let _lock = rho_providers::provider::custom_provider_registry_test_lock();
    let listener_a = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let listener_b = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_a = format!("http://{}/v1", listener_a.local_addr().unwrap());
    let api_b = format!("http://{}/v1", listener_b.local_addr().unwrap());
    let (ready_a, wait_a) = tokio::sync::oneshot::channel();
    let (ready_b, wait_b) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::watch::channel(false);

    let spawn_hold = |listener: tokio::net::TcpListener,
                      ready: tokio::sync::oneshot::Sender<()>,
                      mut release_rx: tokio::sync::watch::Receiver<bool>| {
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 2048];
            let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut buffer)
                .await
                .unwrap();
            let _ = ready.send(());
            release_rx.wait_for(|released| *released).await.unwrap();
            let body = r#"{"data":[{"id":"m"}]}"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = tokio::io::AsyncWriteExt::write_all(&mut stream, reply.as_bytes()).await;
        })
    };
    spawn_hold(listener_a, ready_a, release_rx.clone());
    spawn_hold(listener_b, ready_b, release_rx);

    let cache_dir = unique_cache_dir("custom-host-concurrent");
    set_provider_models_cache_dir_for_tests(Some(cache_dir.clone()));
    rho_providers::provider::reset_custom_openai_compatible_providers_for_tests();
    let _restore = RestoreCustomProviders;
    let mut cfg = Config::default();
    cfg.providers.set_endpoint("composer", &api_a).unwrap();
    cfg.providers.set_endpoint("vllm", &api_b).unwrap();
    cfg.providers.activate().unwrap();
    let store = MemoryCredentialStore::default();

    let refresh = tokio::spawn(async move {
        refresh_custom_provider_models(&cfg.providers, &store).await;
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        wait_a.await.unwrap();
        wait_b.await.unwrap();
    })
    .await
    .expect("both custom hosts should accept before either responds");
    release_tx.send(true).unwrap();
    refresh.await.unwrap();
    set_provider_models_cache_dir_for_tests(None);
    let _ = std::fs::remove_dir_all(cache_dir);
}

// Covers: automation and ACP wait for the session's custom host only when its
// cache cannot serve the session, never for other hosts, and every host still
// refreshes in the background
// Owner: app startup
// Same process-wide registry lock as the fetch test above.
#[allow(clippy::await_holding_lock)]
#[tokio::test(flavor = "current_thread")]
async fn session_refresh_waits_only_when_the_session_host_cache_falls_short() {
    let _lock = rho_providers::provider::custom_provider_registry_test_lock();
    struct Case {
        name: &'static str,
        /// Session host cache before startup; `None` when never fetched.
        cached: Option<&'static [&'static str]>,
        /// Session host cache when the startup refresh returns.
        at_return: &'static [&'static str],
    }
    let cases = [
        Case {
            name: "never fetched",
            cached: None,
            at_return: &["m", "new"],
        },
        Case {
            name: "fresh with the model",
            cached: Some(&["m"]),
            at_return: &["m"],
        },
        Case {
            name: "fresh without the model",
            cached: Some(&["old"]),
            at_return: &["m", "new"],
        },
    ];
    for Case {
        name: case,
        cached,
        at_return: expected_at_return,
    } in cases
    {
        let session_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let other_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let session_api = format!("http://{}/v1", session_listener.local_addr().unwrap());
        let other_api = format!("http://{}/v1", other_listener.local_addr().unwrap());
        // The session host answers at once. The other host answers only after
        // the session refresh returned, so waiting on it would time out.
        let (_session_sender, session_released) = tokio::sync::watch::channel(true);
        serve_models(
            session_listener,
            r#"[{"id":"m"},{"id":"new"}]"#,
            session_released,
        );
        let (release_other, other_released) = tokio::sync::watch::channel(false);
        serve_models(other_listener, r#"[{"id":"o"}]"#, other_released);

        let cache_dir = unique_cache_dir("custom-host-session");
        set_provider_models_cache_dir_for_tests(Some(cache_dir.clone()));
        rho_providers::provider::reset_custom_openai_compatible_providers_for_tests();
        let _restore = RestoreCustomProviders;
        let mut cfg = Config::default();
        cfg.providers.set_endpoint("session", &session_api).unwrap();
        cfg.providers.set_endpoint("other", &other_api).unwrap();
        cfg.providers.activate().unwrap();
        if let Some(cached) = cached {
            let models = cached
                .iter()
                .map(|model| ProviderModel {
                    provider: "session".into(),
                    model: (*model).into(),
                    display_name: (*model).into(),
                    context_window: None,
                    max_output_tokens: None,
                    reasoning_capabilities: ReasoningCapabilities::Unknown,
                })
                .collect::<Vec<_>>();
            replace_cached_provider_models_for_tests("session", &models).unwrap();
        }
        let cli = Cli::try_parse_from(["rho", "--model", "session/m"]).unwrap();

        // Fails well before the 5 s discovery timeout if the other host is awaited.
        let background = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            refresh_custom_provider_models_for_session(
                &cli,
                &cfg,
                Arc::new(MemoryCredentialStore::default()),
            ),
        )
        .await
        .unwrap_or_else(|_| panic!("{case}: session refresh waited on the other host"))
        .unwrap()
        .expect("other hosts refresh in the background");
        let at_return = (
            background.is_finished(),
            cached_model_ids("session"),
            cached_model_ids("other"),
        );
        release_other.send(true).unwrap();
        background.await.unwrap();
        let after_background = (cached_model_ids("session"), cached_model_ids("other"));
        set_provider_models_cache_dir_for_tests(None);
        let _ = std::fs::remove_dir_all(cache_dir);

        assert_eq!(
            at_return,
            (false, strings(expected_at_return), Vec::new()),
            "{case}"
        );
        assert_eq!(
            after_background,
            (strings(&["m", "new"]), strings(&["o"])),
            "{case}"
        );
    }
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

/// Answers one `/v1/models` request with `data` once `release` turns true.
fn serve_models(
    listener: tokio::net::TcpListener,
    data: &'static str,
    mut release: tokio::sync::watch::Receiver<bool>,
) {
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buffer = [0; 2048];
        let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut buffer)
            .await
            .unwrap();
        release.wait_for(|released| *released).await.unwrap();
        let body = format!(r#"{{"data":{data}}}"#);
        let reply = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = tokio::io::AsyncWriteExt::write_all(&mut stream, reply.as_bytes()).await;
    });
}
