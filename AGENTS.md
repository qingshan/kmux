# Contributor guide

`kmux` is a terminal client for a jailbroken Kindle Oasis 10th generation
(`kindlehf`). The Kindle displays remote tmux sessions and Herdr workspaces;
the native `kmux-proxy` companion owns SSH and backend connections.

## Repository map

- `crates/kmux/` — Kindle daemon logic and the `kmuxd` LIPC service.
- `crates/kmux-proxy/` — native host proxy and tmux/Herdr backends.
- `kpm/` — Kindle package, install scripts, WAF, fonts, and configuration.
- `common/` — shared KPM helpers and WAF base assets.
- `tests/e2e/` — the single E2E runner, shared page API, host scenarios, and
  WAF scenarios.
- `tools/` — Kindle input helper, package tooling, grid measurement, and demo
  rendering.
- `docs/` — architecture, installation, and E2E guides.

## Commands

```sh
just test        # Rust, proxy, and WAF tests
just proxy       # native release build of kmux-proxy
just e2e         # real isolated tmux + Herdr API tests; no Kindle required
just toolchain   # install the kindlehf cross-compiler once
just package     # cross-compile kmuxd and write dist/*.kpkg
just demo        # host tests, Kindle tour, and MP4/GIF rendering
just demo-build  # render dist/demo from existing target/demo/frames
```

Run `cargo test -p kmux --lib` for host-testable daemon logic. Do not natively
link `kmuxd`: Kindle `liblipc` exists only on the device. `kmux-proxy` is the
only native binary.

Run `cargo fmt` after Rust changes. Keep generated files under `target/` or
`dist/`; do not hand-edit captured frames or rendered media.

## Runtime boundaries

The WAF sends one-way JSON commands through the LIPC `cmd` property and reads
`/var/local/mesquite/kmux/status.json`. It does not perform synchronous LIPC
RPC or fetch the proxy directly. `kmuxd` translates commands to the proxy’s
HTTP API:

```text
WAF ── LIPC cmd/status.json ──> kmuxd ── HTTPS /v1/action ──> kmux-proxy
                                                        └── SSH ──> tmux/Herdr
```

The proxy API uses `POST /v1/action` for commands and `POST /v1/changes` for
event wakeups. Input includes an expected pane identity so stale input is
rejected rather than routed to a newly selected pane.

## WAF constraints

Mesquite WebKitGTK 1.0.7.2 supports ES5 JavaScript and CSS2-era layout:

- use `var`, not modules, classes, or modern syntax;
- do not use flexbox, CSS grid, or browser APIs unavailable on the Kindle;
- prefer `mousedown` handlers to `click`;
- update only dirty terminal rows;
- show one overlay at a time and preserve its close/Escape behavior;
- keep text and controls legible in grayscale and at the terminal’s effective
  font size.

WAF changes require a cache-busting `?v=` update in the package assets and a
package version bump before sideloading. The package installer preserves the
user’s `/mnt/us/kmux/var/` configuration.

## tmux and Herdr behavior

The proxy must stay on tmux control mode (`tmux -CC`). Use control-mode
commands for inventory, input, snapshots, copy mode, and tab navigation. The
selected terminal pane is maintained at 80×24. kmux can select panes that
already exist, but it does not provide pane creation or pane splitting.

For the file picker, request the pane’s working directory with:

```text
display -p 'KMUXCWD #{pane_current_path}'
```

Then run an absolute command and pipe results through `load-buffer` and
`show-buffer`. Never type `ls` into the user’s pane, and never let `run-shell`
emit untagged stdout: it can be mistaken for a terminal snapshot.

Use `send-keys` / `send-keys -l` through the proxy action API. In copy mode,
WAF uses `capture-pane -M`; scrolling uses `send-keys -X page-up/down`.

Herdr is accessed through the proxy’s socket bridge. Event subscriptions wake
refreshes and invalidate inventory; polling and reconnect are required fallback
paths. Herdr output is read by timed captures because output-match events are
not a raw terminal stream.

## E2E tests and demo

`tests/e2e/kmux_e2e.py` is the only E2E entry point. Use its shared APIs rather
than assembling SSH, HTTP, LIPC, or screenshot plumbing in a scenario:

- `ProxyPage` provides snapshot, input, selection, text, and retrying expects;
- `KindlePage` adds real XTEST taps, key presses, typing, waits, and screenshots;
- `LocalFixture` owns local proxy/tmux/Herdr setup and cleanup;
- `KindleE2E` owns temporary device configuration and named remote fixtures;
- `Artifacts` writes screenshot manifests and optional video output.

The demo fixture must remain deterministic and isolated. Pin the visible pane to
80×24, do not create or split panes, clear search fields before typing, use
measured framebuffer coordinates, and seed temporary snippets before the WAF
starts. Restore proxy configuration and temporary Kindle files in `finally`.

Touch gestures come from `tools/kindle_xinput.c`, cross-compiled by
`tools/build-kindle-xinput.sh` and copied to `/tmp` on the Kindle. It injects
XTEST events, which reach the WAF like real touches; LIPC commands cannot press
WAF buttons. Coordinates in `tests/e2e/kindle_demo.py` are framebuffer pixels
measured with `tools/grid_crop.py`.

When adding a demo stop, assert the daemon state before accepting its frame.
Use `target/demo/frames/` for raw Kindle PNGs and `tools/demo_build.py` for
rendering. Do not rely on a screenshot alone to prove that a transition worked.

## Packaging and sideloading

`kmuxd` is installed under `/mnt/us/kmux` as LIPC service
`dev.qingshan.kmuxd`. KPM reinstalls only when the package version changes.
After daemon, WAF, or script changes, update `kpm/manifest.json`, the daemon
package version, and WAF cache-busting URLs. `just package <x.y.z>` performs
the package version/cache update.

For a developer sideload, copy `dist/kmux_*_kindlehf.kpkg` to
`/mnt/us/tmp/`, unpack it under
`/mnt/us/kmc/kpm/packages/kmux/`, and run its `install.sh`. Reopen the WAF
after installation. Preserve the user’s `var/` directory and do not expose
proxy tokens in logs, screenshots, API responses, or documentation examples.

See [`docs/architecture.md`](docs/architecture.md),
[`docs/installation.md`](docs/installation.md), and
[`docs/e2e.md`](docs/e2e.md) for the longer guides.

## AI-assisted development

kmux is developed with hands-on engineering plus assistance from OpenAI Codex
and Grok. AI-generated implementation, tests, and documentation must be
reviewed against the current code, host fixtures, and real Kindle behavior
before they are accepted.
