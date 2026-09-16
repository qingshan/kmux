#!/bin/sh
# Copies the kmux WAF's UI into Mesquite's app directory and (re)registers
# it in appreg.db so it can be launched with lipc.
#
# /var is tmpfs on stock Kindle firmware (wiped on every reboot), and kpm
# has no boot-time hook yet - so launch.sh re-runs this on every tap of the
# Home screen icon instead of relying on it having survived since install.

set -e
. "$(dirname -- "$(readlink -f -- "$0")")/pkg-lib.sh"
. "$(dirname -- "$(readlink -f -- "$0")")/pkg.env"
pkg_register_waf
