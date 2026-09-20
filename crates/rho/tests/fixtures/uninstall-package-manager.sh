#!/bin/sh
# Invoked through a tool-name symlink; executable bytes stay immutable in tests.
tool=${0##*/}
[ "$tool" = "$RHO_TEST_PACKAGE_OWNER" ] || exit 1
case "$tool" in
    cargo) printf '%s v2.9.1:\n    rho\n' "$RHO_TEST_PACKAGE_NAME" ;;
    pacman) printf '%s\n' "$RHO_TEST_PACKAGE_NAME" ;;
    *) exit 1 ;;
esac
