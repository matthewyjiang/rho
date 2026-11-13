use pretty_assertions::assert_eq;

use super::*;

// Covers: an unstable protocol (major 0) connects only equal versions, a
// stable one any two minors of a major, and the fix names the older side
// (restart an older daemon, update an older client).
// Owner: protocol version policy.
#[test]
fn version_mismatch_follows_the_compatibility_rule_and_blames_the_older_side() {
    let v = ProtocolVersion::new;
    for (daemon, client, fix) in [
        (v(0, 1), v(0, 1), None),
        (v(0, 1), v(0, 2), Some(VersionFix::RestartDaemon)),
        (v(0, 2), v(0, 1), Some(VersionFix::UpdateClient)),
        (v(0, 9), v(1, 0), Some(VersionFix::RestartDaemon)),
        (v(1, 0), v(0, 9), Some(VersionFix::UpdateClient)),
        (v(1, 0), v(1, 0), None),
        (v(1, 0), v(1, 3), None),
        (v(1, 5), v(1, 0), None),
        (v(1, 3), v(2, 0), Some(VersionFix::RestartDaemon)),
        (v(2, 0), v(1, 3), Some(VersionFix::UpdateClient)),
    ] {
        assert_eq!(
            VersionMismatch::between(daemon, client),
            fix.map(|fix| VersionMismatch {
                daemon,
                client,
                fix,
                restart: None,
            }),
            "daemon {daemon}, client {client}"
        );
    }
}
