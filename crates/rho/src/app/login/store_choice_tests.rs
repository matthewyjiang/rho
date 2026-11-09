use pretty_assertions::assert_eq;

use super::*;

fn offers(os_unavailable: Option<&str>, file_unavailable: Option<&str>) -> StoreOffers {
    StoreOffers {
        os_unavailable: os_unavailable.map(str::to_owned),
        file_unavailable: file_unavailable.map(str::to_owned),
    }
}

// Covers: Enter takes the OS store while it can be reached and falls back to
// the file store when it cannot, as for a daemon with no session D-Bus;
// naming a store picks it, and one that cannot be reached, or nothing
// usable at all, is refused rather than swapped for another.
// Owner: rho login credential-store choice (pure)
#[test]
fn enter_falls_back_to_the_file_store_only_when_the_os_store_is_unreachable() {
    use CredentialStoreBackend::{File, Os};
    let reachable = offers(None, None);
    let headless = offers(Some("no session D-Bus"), None);
    let neither = offers(Some("no session D-Bus"), Some("read-only home"));
    let picked = |offers: &StoreOffers, answer: &str| offers.pick(answer).ok();
    assert_eq!(
        [
            picked(&reachable, "\n"),
            picked(&headless, "\n"),
            picked(&reachable, "file\n"),
            picked(&headless, "2"),
            picked(&headless, "os"),
            picked(&neither, ""),
            picked(&reachable, "sqlite"),
        ],
        [
            Some(Os),
            Some(File),
            Some(File),
            Some(File),
            None,
            None,
            None
        ]
    );
}
