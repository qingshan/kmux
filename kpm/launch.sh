#!/bin/sh
# Invoked via `kpm launch kmux` (called from the scriptlet in
# /mnt/us/documents/kmux.sh). Opens the kmux WAF - a small Mesquite
# app that renders a remote tmux session.
#
# The WAF is re-registered on every tap rather than just once at install
# time, because /var is tmpfs (wiped on every reboot) and kpm has no
# boot-time hook yet to redo this automatically. kmuxd writes status.json
# itself on start, so no status seeding is needed here.

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

pkg_launch
