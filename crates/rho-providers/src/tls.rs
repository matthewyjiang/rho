/// Install *ring* as the process rustls crypto provider if none is set.
///
/// reqwest's `rustls-no-provider` feature panics when building a client unless
/// a provider is already installed. Codex WebSockets already compile rustls
/// with ring; installing it here keeps a single provider in the binary.
///
/// Rho installs this at process start and through the crate-private
/// `reqwest_client` / `reqwest_client_builder` helpers. Embedders that
/// construct their own `reqwest::Client` must call this first. Direct
/// `reqwest::Client::new`, `reqwest::Client::builder`, and `reqwest::get` in
/// this crate and `rho` are forbidden outside those helpers
/// (`scripts/architecture.json`).
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

#[cfg(test)]
mod tests {
    use super::*;

    // Covers: rustls-no-provider panics if ring is not installed before Client build
    // Owner: TLS process init
    #[test]
    fn reqwest_client_builds_after_ring_install() {
        let _ = reqwest_client();
        reqwest_client_builder()
            .build()
            .expect("client builder with ring must succeed");
    }
}
