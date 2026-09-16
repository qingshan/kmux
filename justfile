# kmux Kindle daemon and its native tmux control-mode proxy.

set shell := ["bash", "-euo", "pipefail", "-c"]

default:
    @just --list

# Kindle logic is host-testable; kmux-proxy is the only native binary.
test:
    cargo test -p kmux --lib
    cargo test -p kmux-proxy
    node tests/e2e/waf_e2e.js

# Install kindlehf gcc and the liblipc link stub.
toolchain:
    ./tools/setup-toolchain.sh

# Build the proxy for the host that runs tmux.
proxy:
    cargo build --release -p kmux-proxy

# Exercise both backends through the same API using isolated servers.
e2e:
    cargo build -p kmux-proxy
    python3 tests/e2e/kmux_e2e.py

# Record the feature demo from a real Kindle and render it for sharing.
demo:
    ./tools/build-kindle-xinput.sh
    cargo build -p kmux-proxy
    python3 tests/e2e/kmux_e2e.py --record

# Re-encode dist/demo from frames already captured (no device needed).
demo-build:
    python3 tools/demo_build.py

# Cross-compile kmuxd and write a .kpkg to dist/.
# Usage: just package [version]
package version="": toolchain
    ./tools/pkg-build.sh {{version}}
