#!/bin/sh
# Install the kindlehf gcc toolchain (~/x-tools) and a liblipc link stub.
#
# The toolchain is KindleModding's koxtoolchain kindlehf build. liblipc is not
# part of it; we build a SONAME-compatible stub under target/kindlehf-sysroot
# so -llipc resolves at link time. The Kindle loads the real liblipc.so.1 at
# runtime.

set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
XTOOLS_ROOT=${XTOOLS_ROOT:-$HOME/x-tools}
TC_NAME=arm-kindlehf-linux-gnueabihf
TC_DIR=$XTOOLS_ROOT/$TC_NAME
GCC=$TC_DIR/bin/$TC_NAME-gcc
STUB_DIR=$ROOT/target/kindlehf-sysroot/lib
KOX_URL=${KOX_URL:-https://github.com/KindleModding/koxtoolchain/releases/download/2026.04/kindlehf.tar.gz}

if [ ! -x "$GCC" ]; then
    echo "Downloading kindlehf koxtoolchain..."
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    curl -fL --progress-bar -o "$tmp/kindlehf.tar.gz" "$KOX_URL"
    echo "Extracting to $XTOOLS_ROOT..."
    mkdir -p "$HOME"
    tar -C "$HOME" -xzf "$tmp/kindlehf.tar.gz"
    trap - EXIT
    rm -rf "$tmp"
    if [ ! -x "$GCC" ]; then
        echo "error: expected $GCC after extracting the toolchain" >&2
        exit 1
    fi
fi

if [ ! -e "$STUB_DIR/liblipc.so" ]; then
    echo "Building liblipc link stub..."
    mkdir -p "$STUB_DIR"
    "$GCC" -shared -fPIC -Wl,-soname,liblipc.so.1 \
        -o "$STUB_DIR/liblipc.so.1" "$ROOT/tools/lipc-stub.c"
    ln -sf liblipc.so.1 "$STUB_DIR/liblipc.so"
fi

rustup target add armv7-unknown-linux-gnueabihf >/dev/null
echo "kindlehf gcc: $GCC"
"$GCC" --version | head -n 1
