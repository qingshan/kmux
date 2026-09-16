#!/bin/sh
# Removes the kmux WAF's appreg.db entry and its copy under
# /var/local/mesquite. Best-effort: it's fine if either is already gone.

. "$(dirname -- "$(readlink -f -- "$0")")/pkg-lib.sh"
. "$(dirname -- "$(readlink -f -- "$0")")/pkg.env"
pkg_unregister_waf
