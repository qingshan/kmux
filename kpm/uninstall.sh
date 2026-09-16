#!/bin/sh
# Stops kmux (if running) and removes the package.
#
# KPM also runs this BEFORE an upgrade, as `uninstall.sh upgrade`. In that
# case the user's var/ (config.json, logs) is kept so the reinstall never
# wipes the proxy URL and token - only the daemon, upstart job, WAF
# registration and scriptlet are torn down. A plain `kpm uninstall` removes
# everything under /mnt/us/kmux.

SCRIPT_DIR=$(dirname -- "$(readlink -f -- "$0")")
if [ -f "$SCRIPT_DIR/common/pkg-lib.sh" ]; then
    . "$SCRIPT_DIR/common/pkg-lib.sh"
elif [ -f "$SCRIPT_DIR/../common/pkg-lib.sh" ]; then
    . "$SCRIPT_DIR/../common/pkg-lib.sh"
else
    echo "error: pkg-lib.sh not found" >&2
    exit 1
fi
. "$SCRIPT_DIR/pkg.env"

pkg_uninstall "$1"
