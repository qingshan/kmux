# End-to-end testing

All E2E entry points use `tests/e2e/kmux_e2e.py`; there is no separate
`e2e-full` runner. The shared API is intentionally close to Playwright:

```python
with KmuxE2E() as test:
    test.tmux("main")
    test.start()
    test.page.type("printf ready\\n")
    test.page.press("Enter")
    test.page.expectation.text("ready")
```

`ProxyPage` provides `snapshot`, `type`, `press`, `select`, `text`, and retrying
`expect`. `KindlePage` adds real device taps, key presses, typing, waits, and
framebuffer screenshots. `LocalFixture` owns local proxy/tmux/Herdr cleanup;
`KindleE2E` owns temporary device configuration and named remote fixtures.

## Commands

```sh
just test  # Rust, proxy, and WAF tests
just e2e   # real tmux + Herdr API scenarios; no Kindle capture
just demo  # host tests, Kindle feature tour, and media rendering
```

`just e2e` uses isolated tmux sockets and Herdr sessions. It covers inventory,
Unicode and literal input, tabs, stale-input protection, scrollback, copy/search/
yank, files, detach/reconnect, cross-host discovery, agents, and Herdr event
recovery. Fixtures do not create split panes; existing panes are only listed or
selected.

## Demo recording

`just demo` additionally requires SSH aliases `kindle` and `outbox`, ffmpeg,
Pillow, tmux, and Herdr on the proxy host. The tour:

- pins the visible tmux pane to 80×24;
- clears the scrollback search before entering `100`;
- opens a newly created tab through the WAF `+` button;
- seeds snippets before restarting the WAF so the popup has real entries;
- captures every checkpoint from the Kindle framebuffer.

Raw frames and `frames.json` go to `target/demo/`. `tools/demo_build.py` renders
the MP4, GIF, wide MP4, and poster under `dist/demo/`. `Artifacts` also supports
standalone screenshot manifests and MP4 output for smaller scenarios.

Touch coordinates are framebuffer pixels measured for the target Kindle. If WAF
layout changes, recalibrate before changing scenario coordinates. Keep device
input in the shared page API so tests do not bypass the same interaction paths
used by the demo.
