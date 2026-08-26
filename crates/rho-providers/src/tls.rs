/// Install *ring* as the process rustls crypto provider if none is set.
///
/// reqwest's `rustls-no-provider` feature panics when building a client unless
/// a provider is already installed. Codex WebSockets already compile rustls
/// with ring; installing it here keeps a single provider in the binary.
pub fn ensure_rustls_ring_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

pub(crate) fn reqwest_client() -> reqwest::Client {
    ensure_rustls_ring_provider();
    reqwest::Client::new()
}

pub(crate) fn reqwest_client_builder() -> reqwest::ClientBuilder {
    ensure_rustls_ring_provider();
    reqwest::Client::builder()
}
