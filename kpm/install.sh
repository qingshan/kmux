#!/bin/sh
# Installs kmux to /mnt/us/kmux.
#
# Everything is bundled (the kmuxd binary, WAF, scripts, scriptlet), so
# the install is fully local and deterministic - no network needed. Binaries
# and scripts are always refreshed so upgrades pick up new code, but the
# user's var/ (config.json) is left untouched, so upgrading never wipes the
# proxy URL and token.

set -e

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

pkg_install

echo "kmux installed. Tap the kmux entry on your Home screen, then"
echo "set the proxy URL and token in the WAF Settings tab (see README)."
