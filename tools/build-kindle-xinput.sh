#!/bin/sh
# Cross-compile tools/kindle_xinput.c for the Kindle (kindlehf).
#
# The client links against X11 and Xtst, which exist only on the device, so the
# runtime libraries are fetched from the Kindle once and cached under target/.
# The Kindle host is $KMUX_E2E_KINDLE_HOST (default: kindle).
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
XTOOLS=${XTOOLS:-$HOME/x-tools/arm-kindlehf-linux-gnueabihf/bin}
GCC=$XTOOLS/arm-kindlehf-linux-gnueabihf-gcc
KINDLE_HOST=${KMUX_E2E_KINDLE_HOST:-kindle}
CACHE=$ROOT/target/kindlehf-x11/lib
BIN=$ROOT/target/kindlehf-x11/bin/kindle_xinput
LIBS="libX11.so.6 libXtst.so.6 libxcb.so.1 libXau.so.6 libXdmcp.so.6 libXi.so.6 libXext.so.6"

if [ ! -x "$GCC" ]; then
    echo "error: kindlehf gcc not found at $GCC" >&2
    echo "Run: just toolchain" >&2
    exit 1
fi

mkdir -p "$CACHE" "$ROOT/target/kindlehf-x11/bin"
for lib in $LIBS; do
    if [ ! -f "$CACHE/$lib" ]; then
        echo "Fetching $lib from $KINDLE_HOST..."
        scp -q "$KINDLE_HOST:/usr/lib/$lib" "$CACHE/$lib"
    fi
done
ln -sf libX11.so.6 "$CACHE/libX11.so"
ln -sf libXtst.so.6 "$CACHE/libXtst.so"

echo "Cross-compiling kindle_xinput for kindlehf..."
"$GCC" -O2 -Wall -o "$BIN" "$ROOT/tools/kindle_xinput.c" \
    -L"$CACHE" -Wl,-rpath-link,"$CACHE" -lX11 -lXtst

echo "$BIN"
