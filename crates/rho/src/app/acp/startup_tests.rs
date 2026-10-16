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
    let lazy = Arc::new(LazyAgent {
        pending: async move {
            entered.send(()).unwrap();
            released.await.unwrap();
            Ok(Arc::new(RhoAcpAgent::new(startup)))
        }
        .boxed()
        .shared(),
        closed: AtomicBool::new(false),
    });
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
