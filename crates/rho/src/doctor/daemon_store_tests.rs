use pretty_assertions::assert_eq;

use super::*;

fn reported(
    store: CredentialStoreKind,
    selected_by: StoreSelectedBy,
    unreachable: Option<&str>,
) -> DaemonStore {
    DaemonStore::Reported(CredentialStoreStatus {
        store,
        selected_by,
        unreachable: unreachable.map(str::to_owned),
    })
}

// Covers: the row's severity follows what the daemon reported: a store it
// reaches is ok, one it cannot reach warns while nothing chose it (the next
// login asks) and fails once config or the environment chose it, and a
// daemon that is not running or does not answer never fails the report.
// Owner: doctor daemon credential store row (pure)
#[test]
fn row_severity_follows_the_daemons_store() {
    let cases = [
        (DaemonStore::NotRunning, DoctorStatus::Info),
        (
            DaemonStore::Unanswered("protocol 2.0".into()),
            DoctorStatus::Warn,
        ),
        (
            reported(CredentialStoreKind::File, StoreSelectedBy::Config, None),
            DoctorStatus::Ok,
        ),
        (
            reported(
                CredentialStoreKind::Os,
                StoreSelectedBy::Default,
                Some("no session D-Bus"),
            ),
            DoctorStatus::Warn,
        ),
        (
            reported(
                CredentialStoreKind::Os,
                StoreSelectedBy::Config,
                Some("no session D-Bus"),
            ),
            DoctorStatus::Fail,
        ),
        (
            reported(
                CredentialStoreKind::Os,
                StoreSelectedBy::Environment,
                Some("no session D-Bus"),
            ),
            DoctorStatus::Fail,
        ),
    ];
    let (stores, expected): (Vec<_>, Vec<_>) = cases.into_iter().unzip();
    let actual: Vec<_> = stores
        .iter()
        .map(|store| {
            let check = daemon_store_check(store);
            assert_eq!(check.id, DoctorCheckId::DaemonCredentialStore);
            check.status
        })
        .collect();
    assert_eq!(actual, expected);
}

// Covers: when config chose a store the daemon cannot reach, the hint names
// the other store, never the one that failed.
// Owner: doctor daemon credential store row (pure)
#[test]
fn a_chosen_unreachable_store_points_at_the_other_one() {
    let hint = |store| {
        daemon_store_check(&reported(
            store,
            StoreSelectedBy::Config,
            Some("unreachable"),
        ))
        .hint
        .unwrap()
    };
    assert_eq!(
        [
            hint(CredentialStoreKind::Os).contains("rho credential-store set file"),
            hint(CredentialStoreKind::File).contains("rho credential-store set os"),
        ],
        [true, true]
    );
}
