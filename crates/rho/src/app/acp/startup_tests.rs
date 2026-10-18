use super::*;
use crate::{app::acp::AcpStartup, config::Config};
use pretty_assertions::assert_eq;

// Covers: disconnect during model discovery must neither wait for discovery
// nor let its eventual completion hand a usable agent to a queued request.
// Owner: deferred ACP process startup.
#[tokio::test]
async fn disconnect_during_startup_rejects_the_waiting_request() {
    let home = tempfile::tempdir().unwrap();
    let startup = AcpStartup::for_tests(Config::default(), home.path());
    let (entered, started) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let once = std::sync::Mutex::new(Some((entered, released, startup)));
    let lazy = Arc::new(LazyAgent::starting_with(
        move || {
            let (entered, released, startup) =
                once.lock().unwrap().take().expect("startup runs once");
            async move {
                entered.send(()).unwrap();
                released.await.unwrap();
                Ok(Arc::new(RhoAcpAgent::new(startup)))
            }
            .boxed()
        },
        Default::default(),
    ));
    let waiting = tokio::spawn({
        let lazy = Arc::clone(&lazy);
        async move { lazy.ready().await }
    });
    started.await.unwrap();
    lazy.shutdown()
        .now_or_never()
        .expect("closing must not wait for model discovery");
    assert_eq!(
        lazy.ready()
            .now_or_never()
            .expect("closed requests must not wait")
            .err()
            .expect("closed request")
            .code,
        agent_client_protocol::ErrorCode::InvalidRequest,
    );
    release.send(()).unwrap();
    let result = waiting.await.unwrap();
    assert_eq!(
        result.err().expect("closed startup").code,
        agent_client_protocol::ErrorCode::InvalidRequest
    );
}

// Covers: a host's sign-in after `auth_required` asks again on the same
// connection, so that failure must start a fresh run, while other startup
// failures stay put instead of repeating model discovery per request.
// Owner: deferred ACP process startup.
#[tokio::test]
async fn only_auth_required_startup_failures_run_startup_again() {
    for (failure, second) in [
        (AcpError::auth_required(), Ok::<(), ErrorCode>(())),
        (AcpError::internal_error(), Err(ErrorCode::InternalError)),
    ] {
        let home = tempfile::tempdir().unwrap();
        let cwd = home.path().to_owned();
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let lazy = LazyAgent::starting_with(
            {
                let runs = Arc::clone(&runs);
                let failure = failure.clone();
                move || {
                    let run = runs.fetch_add(1, Ordering::SeqCst);
                    let result = match run {
                        0 => Err(failure.clone()),
                        _ => Ok(Arc::new(RhoAcpAgent::new(AcpStartup::for_tests(
                            Config::default(),
                            &cwd,
                        )))),
                    };
                    async move { result }.boxed()
                }
            },
            Default::default(),
        );
        assert_eq!(
            lazy.ready().await.err().map(|error| error.code),
            Some(failure.code)
        );
        assert_eq!(
            lazy.ready().await.map(drop).map_err(|error| error.code),
            second
        );
        lazy.shutdown().await;
    }
}
