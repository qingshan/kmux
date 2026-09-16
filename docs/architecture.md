# Architecture

kmux is a Kindle display and input client for work that remains on another
machine. It has three runtime boundaries:

```text
Kindle WAF
    │ one-way LIPC commands + status.json polling/events
    ▼
kmuxd (Kindle, dev.qingshan.kmuxd)
    │ HTTPS /v1/action and /v1/changes
    ▼
kmux-proxy (native host)
    ├── tmux -CC over SSH
    └── Herdr socket bridge over SSH
```

## Kindle side

`kmuxd` owns the local terminal model, copy mode, scrollback, clipboard
history, file-list parsing, and WAF-facing status. The WAF never performs a
synchronous RPC: it sends a JSON command through the LIPC `cmd` property and
reads `/var/local/mesquite/kmux/status.json` on its refresh loop.

The WAF is deliberately compatible with the Kindle’s old Mesquite WebKit:
JavaScript is ES5 and layout uses CSS2-era techniques. Terminal updates repaint
dirty rows, and only one overlay is shown at a time. Touches that must activate
the WAF use XTEST in the E2E tooling; LIPC cannot press a page button.

## Proxy side

The proxy exposes one API for two backends:

| Concept | tmux | Herdr |
| --- | --- | --- |
| machine | SSH target | SSH target |
| session | tmux session | workspace |
| tab | tmux window | tab |
| pane | existing tmux pane | existing Herdr pane |

There is one active backend connection per proxy. Cross-host catalog discovery
is read-only and does not change the selected terminal. A selection carries
opaque machine/session/tab/pane IDs, and input includes `expected_pane`; stale
input is rejected instead of being sent to a newly selected pane.

tmux uses a persistent control-mode client. The proxy refreshes the selected
pane at 80×24 and sends keys through control mode. It does not expose a
pane-create or pane-split action. Existing panes can be listed and selected.

Herdr uses a Python socket bridge over SSH. Lifecycle subscriptions invalidate
inventory and wake the Kindle refresh path; polling and reconnect logic remain
available when subscriptions are unavailable. Herdr output is captured through
timed reads because output-match events are not a raw terminal stream.

## Security boundaries

The proxy configuration contains the shared token and SSH targets. Public API
responses omit SSH targets, keys, and the token. The Kindle stores its proxy
URL and token under `/mnt/us/kmux/var/`; the WAF never displays the saved token
back into the page. `diagnose` probes configured machines without switching the
active pane or leaving copy mode.

For API actions and response shapes, see
[`crates/kmux-proxy/README.md`](../crates/kmux-proxy/README.md).
