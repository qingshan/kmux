# Locate and source pkg-lib.sh. Sourced from package-root scripts
# (install/launch/uninstall) in the git tree or inside a kpkg.
_pkg_here=$(dirname -- "$(readlink -f -- "$0")")
if [ -f "$_pkg_here/common/pkg-lib.sh" ]; then
    . "$_pkg_here/common/pkg-lib.sh"
elif [ -f "$_pkg_here/../common/pkg-lib.sh" ]; then
    . "$_pkg_here/../common/pkg-lib.sh"
else
    echo "error: pkg-lib.sh not found from $_pkg_here" >&2
    exit 1
fi
. "$_pkg_here/pkg.env"
SCRIPT_DIR=$_pkg_here
