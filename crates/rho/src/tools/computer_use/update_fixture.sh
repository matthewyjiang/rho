#!/bin/sh
# Test stand-in for cua-driver. Each test symlinks this checked-in script and
# writes its behavior to a sibling `driver-body.sh`, which is sourced rather
# than executed so no freshly written file is ever exec'd (ETXTBSY under load).
set -eu
. "${0%/*}/driver-body.sh"
