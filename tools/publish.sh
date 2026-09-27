#!/bin/sh
# Publish an existing GitHub Release asset URL to the Qingshan KPM catalog.

set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
PKG_ID=$(python3 -c "import json; print(json.load(open('$ROOT/kpm/manifest.json'))['id'])")
CATALOG_REPO=${KINDLE_CATALOG_REPO:-https://github.com/qingshan/kindle.git}

# Publish only an asset that already exists on a public GitHub Release.
VERSION=${1:-$(python3 -c "import json; print('.'.join(str(v) for v in json.load(open('$ROOT/kpm/manifest.json'))['version']))")}
RELEASE_REPO=${KMUX_RELEASE_REPO:-qingshan/kmux}
ASSET=${PKG_ID}_${VERSION}_kindlehf.kpkg
RELEASE_URL=https://github.com/$RELEASE_REPO/releases/download/v$VERSION/$ASSET
curl --fail --silent --show-error --location --head "$RELEASE_URL" >/dev/null

CATALOG_DIR=$(mktemp -d)
trap 'rm -rf "$CATALOG_DIR"' EXIT

if [ -n "${KINDLE_CATALOG_TOKEN:-}" ]; then
    auth=$(printf 'x-access-token:%s' "$KINDLE_CATALOG_TOKEN" | base64 | tr -d '\n')
    git -c "http.extraheader=AUTHORIZATION: basic $auth" clone --depth 1 "$CATALOG_REPO" "$CATALOG_DIR"
else
    git clone --depth 1 "$CATALOG_REPO" "$CATALOG_DIR"
fi

python3 - "$CATALOG_DIR/repo/manifest.v2.json" "$ROOT/kpm/manifest.json" "$VERSION" "$RELEASE_URL" <<'PYTHON'
import json
import sys

catalog_path, package_path, version, url = sys.argv[1:]
with open(catalog_path) as f:
    catalog = json.load(f)
with open(package_path) as f:
    package = json.load(f)
entry = catalog["packages"].setdefault(package["id"], {})
for key in ("name", "author", "description"):
    entry[key] = package[key]
# Replace kindlehf artifacts, preserving any other platform variants.
artifacts = []
for artifact in entry.get("artifacts", []):
    platforms = artifact.get("supported_platforms")
    if platforms and "kindlehf" not in platforms:
        artifacts.append(artifact)
    elif platforms and len(platforms) > 1:
        artifacts.append(dict(artifact, supported_platforms=[p for p in platforms if p != "kindlehf"]))
artifacts.append({"url": url, "version": [int(v) for v in version.split(".")],
                  "dependencies": package.get("dependencies", []),
                  "supported_platforms": ["kindlehf"]})
entry["artifacts"] = artifacts
with open(catalog_path, "w") as f:
    json.dump(catalog, f, indent=2)
    f.write("\n")
PYTHON

# Remove legacy tracked archives; new releases are hosted on GitHub.
git -C "$CATALOG_DIR" rm -r --ignore-unmatch -- "repo/packages/$PKG_ID"
git -C "$CATALOG_DIR" add repo/manifest.v2.json
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
