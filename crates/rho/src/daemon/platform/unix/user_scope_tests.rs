use pretty_assertions::assert_eq;

use super::caller_slice;

// Covers: a client the user's systemd manager runs in a unit of its own
// must start its daemon beside that unit, in the unit's slice, and a client
// the manager cannot take a process from must leave the daemon alone; the
// daemon otherwise dies with the unit of whichever client started it
// (#1685).
// Owner: daemon detach (Linux), which decides where a started daemon lives.
#[test]
fn the_daemon_moves_beside_a_unit_of_the_users_manager_only() {
    let manager = "0::/user.slice/user-1000.slice/user@1000.service";
    for (case, cgroup, slice) in [
        (
            "a host's transient agent scope",
            format!("{manager}/app.slice/app-t3code.slice/app-t3code-agents-1404.slice/t3code-acp-55e1afd6.scope\n"),
            Some("app-t3code-agents-1404.slice"),
        ),
        (
            "a service with control groups of its own",
            format!("{manager}/app.slice/rho-mcp.service/payload\n"),
            Some("app.slice"),
        ),
        (
            "cgroup v1 controllers listed first",
            format!("12:pids:/user.slice\n1:name=systemd:/user.slice\n{manager}/app.slice/vte-spawn-1.scope\n"),
            Some("app.slice"),
        ),
        (
            "the manager itself",
            format!("{manager}/init.scope\n"),
            None,
        ),
        (
            "a login session of the system manager",
            "0::/user.slice/user-1000.slice/session-25817.scope\n".to_owned(),
            None,
        ),
        (
            "a system service",
            "0::/system.slice/actions.runner.service\n".to_owned(),
            None,
        ),
        ("a container's root", "0::/\n".to_owned(), None),
        (
            "cgroup v1 only",
            "1:name=systemd:/user.slice/user-1000.slice/user@1000.service/app.slice/a.scope\n"
                .to_owned(),
            None,
        ),
    ] {
        assert_eq!(caller_slice(&cgroup), slice, "{case}");
    }
}
