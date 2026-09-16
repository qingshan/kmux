# kmux-proxy

`kmux-proxy` is the native companion for kmux. It runs on a server that can
reach your development machines, owns the SSH connections and backend clients,
and exposes one small HTTP API to the Kindle.

```text
Kindle kmuxd ── HTTPS ──> kmux-proxy ── SSH ──> tmux or Herdr
```

The Kindle never needs an SSH client, tmux installation, or Herdr installation.
The proxy keeps the remote work running while the Kindle connects, disconnects,
or changes machines.

## Backend model

Each configured machine uses exactly one backend. kmux presents both backends
through the same navigation model:

| kmux | tmux | Herdr |
| --- | --- | --- |
| Machine | SSH target | SSH target |
| Session | tmux session | Workspace |
| Tab | tmux window | Tab |
| Pane | Existing pane | Existing pane |

The proxy can discover sessions, tabs, panes, and agents on inactive machines
without changing the active terminal. kmux can select existing panes, but does
not expose actions to create or split panes.

## Build and run

Build the proxy on the host that owns the SSH configuration and keys:

```sh
just proxy
./target/release/kmux-proxy /etc/kmux-proxy.json
```

The default configuration path is `/etc/kmux-proxy.json`. Run the service as
the user whose SSH config and identities can reach every configured target.
The proxy binds the host’s Tailscale IPv4 by default; set `bind` to override
that address.

## Configuration

```json
{
  "token": "shared-secret",
  "port": 8766,
  "defaultHost": "devbox",
  "hosts": [
    {
      "id": "devbox",
      "name": "development",
      "session": "main"
    },
    {
      "id": "work",
      "backend": "herdr",
      "target": "workstation",
      "herdrSession": "coding"
    }
  ]
}
```

`id` is the public machine identifier and, by default, the SSH target and
display name. Set `target` when the SSH alias differs from the public ID. The
default backend is tmux; set `backend` to `herdr` for a Herdr machine. A host’s
`session` selects the initial tmux session. `defaultHost` selects the initial
machine.

For a tmux server using a named socket, add `tmuxSocket`:

```json
{
  "id": "fixture",
  "target": "devbox",
  "session": "api",
  "tmuxSocket": "kmux-demo"
}
```

`local:<user>` targets use passwordless `sudo -H -u <user>` on the proxy host
instead of SSH. This is useful when the proxy and tmux server run on the same
machine but under different users.

Herdr hosts may set `herdrSession` to choose a named server; an empty value uses
the default server. The proxy starts a missing named Herdr server and waits for
its socket. An explicit `herdrSocket` is treated as externally managed and is
not auto-started. Herdr and Python 3 must be installed on the target machine.
Directory listings also require Python 3 on the target.

Put the same `token` in kmux Settings. Keep this file private: it contains the
shared authentication secret and the SSH target names.

## HTTP API

The API is intentionally small and versioned at the request level. Health does
not require authentication:

```http
GET /health
```

```json
{"ok": true, "version": 1}
```

All actions use `POST /v1/action`:

```json
{
  "version": 1,
  "token": "shared-secret",
  "machine": "devbox",
  "action": { "op": "snapshot" }
}
```

Responses contain the selected machine, machine catalog, sessions, tabs, panes,
selection, and terminal state. Terminal state includes screen rows, attributes,
scrollback, cursor information when available, copy mode, clipboard text, and
file-list results. SSH targets, private keys, and the token are never returned.

### Actions

- `snapshot` refreshes inventory and the selected terminal.
- `select` chooses optional `session`, `tab`, and `pane` IDs.
- `text` sends literal text; `key` sends a logical key such as `Enter` or `C-c`.
- `create_session` creates a session; `create_tab` creates a tab with optional
  `cwd`.
- `close_tab`, `next_tab`, and `detach` control the current connection.
- `copy_enter`, `copy_key`, and `copy_search` operate on the frozen copy buffer.
- `list_files` lists an absolute path or the selected pane’s working directory.
- `diagnose` probes configured machines without changing the selected pane or
  leaving copy mode.

Input actions should include `expected_pane`, returned in the latest selection.
If the selected pane changed, the proxy rejects the input instead of sending it
to the wrong target. Backend actions are serialized, and a timed-out request is
not replayed automatically.

Errors use JSON and distinguish authentication (`401`), malformed requests
(`400`), and backend or state conflicts (`409`).

## tmux transport

The proxy maintains a persistent `tmux -CC` control client. It uses control-mode
commands for input, inventory, terminal snapshots, copy mode, file discovery,
and tab navigation. The selected terminal pane is maintained at 80×24; the
proxy does not create or split panes.

The tmux catalog is machine-scoped. Inactive discovery only lists sessions,
windows, panes, and conservative agent process identities. It does not attach
to a user’s desktop client, create sessions, or switch the active pane.

## Herdr transport and events

The proxy starts a small Python socket bridge over SSH for Herdr. It uses the
socket API for workspace, tab, pane, and agent metadata and attaches a sized
terminal client for the selected workspace. Herdr’s public API does not expose
a live cursor position, so the Kindle hides that cursor.

The active bridge subscribes to pane status events. Topology changes rebuild
subscriptions and invalidate cached inventory. If the subscription disconnects
or is rejected, the bridge falls back to polling and reconnects automatically.
Terminal output still uses timed pane reads: output-match notifications are
state transitions, not a raw output stream.

`POST /v1/changes` waits for a machine’s event revision without acquiring the
backend input lock:

```json
{
  "version": 1,
  "token": "shared-secret",
  "machine": "work",
  "after": 12,
  "timeout_ms": 5000
}
```

The response contains the newest opaque `revision` and an `event_updates` flag.
The Kindle uses this as a wake-up signal and then requests an ordinary
snapshot.

## Terminal behavior

Both backends share the same client-side model:

- 80×24 selected terminal view.
- Local scrollback and search.
- Copy mode over captured history, including line and rectangle selection.
- Clipboard history populated by copy-mode yanks and compose sends.
- Dirty-row updates suitable for e-ink refresh behavior.

The proxy preserves ANSI bold and inverse attributes. Terminal graphics are not
supported. Herdr output is refreshed by timed captures; tmux uses control-mode
polling and events from its persistent client.

## Test the proxy

```sh
just test  # Rust, proxy, and WAF tests
just e2e   # real isolated tmux + Herdr API scenarios
```

The shared test API lives in [`tests/e2e/kmux_e2e.py`](../../tests/e2e/kmux_e2e.py).
It exercises both backends with isolated named fixtures, stale-input checks,
cross-host discovery, agent metadata, copy/yank, files, detach/reconnect, and
Herdr event recovery. The real Kindle advertising tour is documented in
[`docs/e2e.md`](../../docs/e2e.md).

## License

kmux-proxy is distributed under the [MIT License](../../LICENSE).
