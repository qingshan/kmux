# End-to-end tests

`tests/e2e/kmux_e2e.py` is the sole end-to-end entry point. It exposes a small
Playwright-like API: `ProxyPage` has `snapshot`, `type`, `press`, `select`, and
retrying `expect`; `KindlePage` adds real `tap`/`press` gestures, waits, and
framebuffer screenshots; `Artifacts` writes PNG checkpoints, an animation
manifest, and an optional MP4. `LocalFixture` and `KindleE2E` own setup and
cleanup for isolated proxy, tmux, Herdr, and device fixtures.

`just e2e` starts isolated, real tmux and Herdr servers and checks the same
versioned API against both. Install tmux, Herdr, Python 3, and allow passwordless
`sudo -u "$USER"` (the proxy's local-machine transport). No Kindle is needed.
Coverage includes inventories, literal/Unicode input and Enter, terminal output,
session creation/selection, tab creation/navigation/closing, stale-input rejection,
copy/search/selection/yank, directory listing, and detach/reconnect.
The cross-host catalog test also checks simultaneous tmux/Herdr discovery,
empty and unavailable machines, direct session selection, and preservation of
the active pane and copy mode while discovery runs. It verifies the proxy
starts a stopped named Herdr server and waits for its socket before discovery.
Cross-host agent tests discover a harmless executable-name fixture in inactive
tmux and a reported fixture in inactive Herdr, verify tab labels/agent states,
preserve active copy/selection, assert tmux discovery has no attached client,
and reject a closed agent target. WAF tests use identical IDs on different
hosts to check filtering, omitted empty/offline hosts, quiet loading, and
one-command switching.
It also verifies a real external Herdr rename wakes `/v1/changes` and refreshes
the cached inventory. `herdr_events.py` adds a fault-injected socket server to
test cache reuse, subscription loss/rejection/reconnection, missing-event
resynchronization, simultaneous input and waits, output without metadata events,
and subscription cleanup on detach. These run as part of `just e2e`.
Agent checks cover real Herdr fixture state reports and exact pane selection,
per-pane status subscriptions, all five states, session/tab rollups, and clearing
badges when an agent exits. No real coding agent is launched or prompted.

`just test` also runs host-side Rust tests and JavaScript WAF routing/snippet tests.
Control regression tests cover compose Cancel/Escape/outside dismissal, delayed
focus and Send callbacks, duplicate delivery, and physical-button scroll offsets
in both directions, including returning to live view without stale-poll rollback.
Compose tests also exercise nested icon taps, textarea focus, Shift+Enter editing,
literal multiline delivery, and sending without an extra Enter from the icon button.
Slow touch/mouse scrolling and holds are checked to page history without opening
the clipboard popup; clipboard history remains available from Tools → Paste.
`waf_overlays.js` checks all eight popup types share accessible chrome, nested
close-icon taps and Escape restore terminal focus without remote input, all 56
popup-to-popup transitions keep one overlay visible, and escaped error messages
preserve the close control without covering an active draft.
`waf_menu_groups.js` checks the Hosts/Sessions/Panes/Agents tabs show only their
own rows, with panes spanning hosts, preserve group-specific searches, and route
Enter to the exact host or pane. It also checks nested tab taps and live refresh.
`waf_setup.js` checks the missing-credentials startup prompt, HTTPS defaults,
and that status refresh does not overwrite settings drafts or expose the token.
Agent WAF tests cover escaped labels, badges, filtering, exact pane routing and
stale-machine rejection. Rust tests cover conservative tmux command detection,
unknown/future states, rollups, and daemon metadata propagation/clearing.

`just demo` runs the host suite, then the advertising tour against `ssh outbox`
and `ssh kindle`. `KindleE2E` temporarily installs its proxy hosts, starts only
named tmux/Herdr fixtures, restores the selected machine/configuration, and
removes those fixtures. Outbox needs Herdr at `~/.local/bin/herdr` and
passwordless sudo. Override SSH aliases with `KMUX_E2E_OUTBOX_HOST` /
`KMUX_E2E_KINDLE_HOST`.

The tour records the product features worth showing: remote terminal input,
scrollback/search, file paths, clipboard history and snippets, compose, tmux and
Herdr switching, existing-pane navigation, tabs, copy/yank, and agent badges.
It does not create split panes: the opening terminal pane is explicitly 80×24.
Every checkpoint is a
real **Kindle framebuffer** PNG, then `tools/demo_build.py` renders the MP4/GIF.
`tools/kindle_xinput.c` injects XTEST
pointer and key events through the Kindle's X server, the only channel that can
press a button or type into a popup, since the LIPC `cmd` property drives the
daemon rather than the page. Taps open the switcher groups, the Tools panel, the
popups and copy mode, and every stop asserts the daemon state behind the frame
before accepting it. `just demo` records that tour and `tools/demo_build.py`
renders it to `dist/demo/`. Tap coordinates are framebuffer pixels measured with
`tools/grid_crop.py`, so re-measure them after a layout change.
