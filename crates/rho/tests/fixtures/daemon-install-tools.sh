#!/bin/sh
# Stands in for the tools `rho update` and `rho uninstall` run, through a
# symlink named after each; the file itself stays immutable in tests.
#
# `cargo install` puts a new rho at $RHO_TEST_INSTALL_PATH by a rename, as an
# installer does. That rho runs the binary under test, $RHO_TEST_RHO, and
# leaves `<install path>.daemon` behind once it ran as the daemon. With
# $RHO_TEST_INSTALL_OUTLASTS_RESTART set, the install returns only then.
# `mise` names $RHO_TEST_MISE_INSTALL as the one install of its tool `rho`,
# and its upgrade installs nothing over it, as mise gives each version a
# directory of its own.
# `systemctl` and `launchctl` append their arguments to $RHO_TEST_TOOL_LOG.
set -eu
tool=${0##*/}
case "$tool" in
    cargo)
        [ "$1 $2" = "install rho-coding-agent" ] || exit 1
        staged="$RHO_TEST_INSTALL_PATH.new"
        cat > "$staged" <<SCRIPT
#!/bin/sh
if [ "\$1" = daemon ]; then : > '$RHO_TEST_INSTALL_PATH.daemon'; fi
exec '$RHO_TEST_RHO' "\$@"
SCRIPT
        chmod 755 "$staged"
        mv "$staged" "$RHO_TEST_INSTALL_PATH"
        # As an installer that has more to do after it wrote the binary.
        if [ -n "${RHO_TEST_INSTALL_OUTLASTS_RESTART:-}" ]; then
            until [ -e "$RHO_TEST_INSTALL_PATH.daemon" ]; do sleep 0.05; done
        fi
        ;;
    mise)
        case "$1" in
            ls) printf '{"rho":[{"install_path":"%s"}]}\n' "$RHO_TEST_MISE_INSTALL" ;;
            upgrade) ;;
            *) exit 1 ;;
        esac
        ;;
    systemctl | launchctl) printf '%s %s\n' "$tool" "$*" >> "$RHO_TEST_TOOL_LOG" ;;
    *) exit 1 ;;
esac
