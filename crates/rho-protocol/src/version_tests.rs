use pretty_assertions::assert_eq;

use super::*;

// Covers: only a major difference blocks a connection, and the fix names the
// older side (restart an older daemon, update an older client).
// Owner: protocol version policy.
#[test]
fn version_mismatch_follows_majors_and_blames_the_older_side() {
    let v = ProtocolVersion::new;
    for (daemon, client, fix) in [
        (v(1, 0), v(1, 0), None),
        (v(1, 0), v(1, 3), None),
        (v(1, 5), v(1, 0), None),
        (v(1, 3), v(2, 0), Some(VersionFix::RestartDaemon)),
        (v(0, 9), v(1, 0), Some(VersionFix::RestartDaemon)),
        (v(2, 0), v(1, 3), Some(VersionFix::UpdateClient)),
    ] {
        assert_eq!(
            VersionMismatch::between(daemon, client),
            fix.map(|fix| VersionMismatch {
                daemon,
                client,
                fix
            }),
            "daemon {daemon}, client {client}"
        );
    }
}
