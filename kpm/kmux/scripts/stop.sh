#!/bin/sh
# Stops kmuxd: ask the LIPC service to exit, stop the upstart job, and
# pkill any detached daemon as a fallback.

. "$(dirname -- "$(readlink -f -- "$0")")/pkg-lib.sh"
. "$(dirname -- "$(readlink -f -- "$0")")/pkg.env"
pkg_stop_daemon
