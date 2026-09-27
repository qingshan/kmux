#!/bin/sh
# Build the package and publish it to the Qingshan KPM catalog.

set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
PKG_ID=$(python3 -c "import json; print(json.load(open('$ROOT/kpm/manifest.json'))['id'])")
CATALOG_REPO=${KINDLE_CATALOG_REPO:-https://github.com/qingshan/kindle.git}

"$ROOT/tools/pkg-build.sh" "$@"

VERSION=$(python3 -c "import json; print('.'.join(str(v) for v in json.load(open('$ROOT/kpm/manifest.json'))['version']))")
KPKG=$ROOT/dist/${PKG_ID}_${VERSION}_kindlehf.kpkg

if [ ! -f "$KPKG" ]; then
    echo "error: expected $KPKG after packaging" >&2
    exit 1
fi

CATALOG_DIR=$(mktemp -d)
trap 'rm -rf "$CATALOG_DIR"' EXIT

if [ -n "${KINDLE_CATALOG_TOKEN:-}" ]; then
    auth=$(printf 'x-access-token:%s' "$KINDLE_CATALOG_TOKEN" | base64 | tr -d '\n')
    git -c "http.extraheader=AUTHORIZATION: basic $auth" clone --depth 1 "$CATALOG_REPO" "$CATALOG_DIR"
else
    git clone --depth 1 "$CATALOG_REPO" "$CATALOG_DIR"
fi

rm -rf "$CATALOG_DIR/repo/packages/$PKG_ID"
python3 "$CATALOG_DIR/tools/kpm-helper.py" repo remove \
    "$CATALOG_DIR/repo/manifest.v2.json" "$PKG_ID" \
    --supported_platform kindlehf 2>/dev/null || true
python3 "$CATALOG_DIR/tools/kpm-helper.py" repo add \
    "$CATALOG_DIR/repo/manifest.v2.json" "$KPKG"

git -C "$CATALOG_DIR" add repo/manifest.v2.json
git -C "$CATALOG_DIR" add -f "repo/packages/$PKG_ID"
if git -C "$CATALOG_DIR" diff --cached --quiet; then
    echo "$PKG_ID $VERSION is already published"
    exit 0
fi

git -C "$CATALOG_DIR" -c user.name="${KINDLE_CATALOG_GIT_NAME:-kindle-publisher}" \
    -c user.email="${KINDLE_CATALOG_GIT_EMAIL:-kindle-publisher@users.noreply.github.com}" \
    commit -m "Publish $PKG_ID $VERSION"
if [ -n "${KINDLE_CATALOG_TOKEN:-}" ]; then
    git -C "$CATALOG_DIR" -c "http.extraheader=AUTHORIZATION: basic $auth" push origin HEAD:main
else
    git -C "$CATALOG_DIR" push origin HEAD:main
fi
