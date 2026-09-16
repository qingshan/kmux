#!/usr/bin/env python3
"""Capture the kmux demo storyboard from a real Kindle, stop by stop.

Every frame is a genuine device framebuffer capture taken while the run drives
the WAF: chrome is driven with injected taps and keys (tools/kindle_xinput.c),
terminal input goes through the same LIPC ops the WAF itself sends, and each stop
asserts the daemon state it is supposed to show before the frame is accepted.

Fixtures, never the user's sessions: an isolated tmux server (`-L kmux-demo-*`)
and an isolated Herdr session on the proxy host. The proxy's machine list is
replaced by those fixtures for the run, so a published video cannot leak the
names of real machines, and restored in the finalizer. Agent badges come from
Herdr's own `pane report-agent`, so no coding agent is launched or prompted.

Popups are laid out by the WAF, so their row positions are measured from the
device during a short calibration pass instead of being hardcoded; the fixed
chrome coordinates below were measured with tools/grid_crop.py.

Output:
    target/demo/frames/NN-label.png   raw captures, in storyboard order
    target/demo/frames.json           captions, dwell times, run metadata

Usage:
    python3 tests/e2e/kindle_demo.py                 # full run
    python3 tests/e2e/kindle_demo.py --only 12       # one stop, for iterating
    python3 tests/e2e/kindle_demo.py --from 14-      # resume part way
    python3 tests/e2e/kindle_demo.py --list          # print the storyboard
"""
import argparse
import io
import json
import os
import shlex
import subprocess
import sys
import time
from pathlib import Path

import numpy as np
from PIL import Image

from kindle_lib import (KINDLE_HOST, PROXY_HOST, ROOT, herdr_start, launch_waf, ssh, tmux_cmd,
                        tmux_server)
from kmux_e2e import KindleE2E

FRAMES = ROOT / "target/demo/frames"
STORYBOARD = ROOT / "target/demo/frames.json"
DEBUG = ROOT / "target/demo/calibration"
WAF_DIR = "/var/local/mesquite/kmux"
SNIPPETS = WAF_DIR + "/quick-snippets.json"
PACKAGE_SNIPPETS = "/mnt/us/kmux/waf/quick-snippets.json"
SESSION = "kmux-demo-" + str(os.getpid())
SETTLE = 3.2          # the WAF's idle poll is 2.5 s; captures wait past it
TMUX_MACHINE = SESSION + "-tmux"
HERDR_MACHINE = SESSION + "-herdr"
OFFLINE_MACHINE = SESSION + "-offline"

# Fixed chrome, in framebuffer pixels (Kindle Oasis, kmux 0.5.26).
COORDS = {
    # y=165 sits on the chip/tab, below the Kindle status bar (y<101) and
    # usually below the search-chrome hit target that otherwise swallows taps.
    "session_chip": (115, 165),
    # By the tab-menu/new-tab beats the active fixture session has the
    # shell/codex/agent tabs shown below; measure controls from that layout.
    "tab_second": (447, 150),
    "tab_new": (822, 150),
    "tab_more": (870, 150),
    "settings": (1155, 165),
    "exit": (1225, 165),
    "panel_scroll": (71, 933),
    "panel_prompt": (217, 933),
    "panel_tools": (359, 933),
    "scroll_up": (29, 996),
    "scroll_down": (90, 996),
    "scroll_live": (151, 996),
    "scroll_search": (508, 996),
    "scroll_search_back": (667, 996),
    "scroll_search_forward": (728, 996),
    "scroll_search_clear": (789, 996),
    "tools_files": (70, 996),
    "tools_paste": (211, 996),
    "tools_snippets": (370, 996),
    "toolbar_esc": (253, 1064),
    # Modal overlays extend down to the shortcut bar, so an apparent
    # "outside" tap at y=860 still selects a row.  All share this close button.
    "menu_close": (1170, 248),
    "menu_group_hosts": (206, 313),
    "menu_group_sessions": (489, 313),
    "menu_group_panes": (772, 313),
    "menu_group_agents": (1055, 313),
    "menu_search": (400, 382),
    # Row centers, measured from device captures of this layout (85 px pitch).
    "rows": {
        "switcher": [461, 545, 629, 713, 797],
        "act": [300, 385, 470, 555, 640],
        "paste": [340, 425, 510, 595, 680],
        "snippets": [340, 425, 510, 595, 680],
        "files": [461, 545, 629, 713, 797],
    },
}


def detect_rows(image, min_gap=60, max_gap=95, frac=0.45):
    """Row centers of a captured popup, from the borders between its list rows.

    Only the popup's interior counts: the frame is full of other horizontal lines
    (the terminal box, the chrome rows), so detection starts below the popup's top
    border and stops at its bottom border.
    """
    gray = np.asarray(Image.open(io.BytesIO(image)).convert("L"))
    width = gray.shape[1]
    hits = (gray[:, 150:width - 150] < 128).mean(axis=1) > frac
    edges, last = [], -10
    for y in np.nonzero(hits)[0]:
        if int(y) - last > 2:
            edges.append(int(y))
        last = int(y)
    top = next((y for y in edges if y > 150), None)
    bottom = next((y for y in reversed(edges) if y < 900), None)
    if top is None or bottom is None or bottom <= top:
        return []
    inner = [y for y in edges if top + 110 < y < bottom]
    return [(a + b) // 2 for a, b in zip(inner, inner[1:]) if min_gap <= b - a <= max_gap]


def row_target(rows, index):
    """A tap point inside row `index`, extrapolating the popup's row pitch."""
    rows = rows or [461]
    if index < len(rows):
        return (400, rows[index])
    pitch = rows[1] - rows[0] if len(rows) > 1 else 85
    return (400, rows[-1] + pitch * (index - len(rows) + 1))


def tap(point, pause=450):
    """Tap a framebuffer point or a named entry in ``COORDS``.

    Storyboard gestures deliberately use readable names (``"scroll_search"``)
    beside measured popup-row tuples.  Resolving here is essential: indexing a
    string produced ``tap s c``, and the injector parsed that as `(0, 0)`, the
    Kindle status-bar corner that opens Quick Settings.
    """
    if isinstance(point, str):
        point = COORDS[point]
    return f"tap {point[0]} {point[1]}\nsleep {pause}\n"


def taps(*points, pause=450):
    return "".join(tap(point, pause) for point in points)


def key(name, pause=300):
    return f"key {name}\nsleep {pause}\n"


def type_text(text, pause=140):
    return f"text {text}\nsleep {pause}\n"


def menu_row(index):
    return row_target(COORDS["rows"].get("switcher", []), index)


def pop_row(popup, index):
    return row_target(COORDS["rows"].get(popup, []), index)


def act(index):
    return row_target(COORDS["rows"].get("act", []), index)


def herdr(session, *args, check=True):
    """Run a Herdr CLI command on the proxy host and return its JSON result."""
    command = ("~/.local/bin/herdr --session " + session + " " +
               " ".join(shlex.quote(str(arg)) for arg in args))
    result = ssh(PROXY_HOST, command, check=check)
    out = result.stdout
    # Some subcommands (report-agent) succeed silently; only parse real output.
    if not out.strip():
        return {}
    try:
        return json.loads(out).get("result", {})
    except json.JSONDecodeError as error:
        stderr = getattr(result, "stderr", "") or ""
        raise RuntimeError(f"herdr {' '.join(str(a) for a in args)} failed: {error}; "
                           f"stdout={out[:400]!r} stderr={stderr[:400]!r}") from error


def pane_text(host, socket, target, text):
    """Type a fixture command into an isolated pane (never a user's session)."""
    tmux_cmd(host, socket, "send-keys", "-t", target, "-l", text)
    tmux_cmd(host, socket, "send-keys", "-t", target, "Enter")


def fixture_tmux(host, socket):
    """Isolated tmux server: three sessions, tabs, and agent fixtures."""
    tmux_server(host, socket, "api", "exec sh", window="api")
    # No attached client exists when the isolated server is created; pin the
    # initial window explicitly so the demo cannot inherit host terminal size.
    tmux_cmd(host, socket, "resize-window", "-t", "api:api", "-x", "80", "-y", "24")
    tmux_cmd(host, socket, "new-window", "-t", "api:", "-n", "logs", "exec sh")
    tmux_cmd(host, socket, "new-window", "-t", "api:", "-n", "tests", "exec sh")
    tmux_cmd(host, socket, "new-session", "-d", "-s", "build", "-n", "shell", "exec sh")
    tmux_cmd(host, socket, "new-session", "-d", "-s", "infra", "-n", "shell", "exec sh")
    pane_text(host, socket, "api:api", "command ls -l /usr/bin | head -18")
    pane_text(host, socket, "build:shell", "printf 'build: compiling kmux\\n'")
    pane_text(host, socket, "api:logs", "printf 'logs: kmux-proxy connected\\n'")
    pane_text(host, socket, "api:tests", "printf 'tests: waiting for the next run\\n'")
    pane_text(host, socket, "infra:shell", "printf 'cluster: 3 nodes healthy\\n'")
    # Agent identity fixtures: harmless copies of sleep under agent names, as
    # tests/e2e/unified.py does. No coding agent is launched or prompted.
    directory = f"/tmp/{SESSION}"
    ssh(host, f"mkdir -p {directory}")
    for name in ("codex", "claude"):
        ssh(host, f"cp \"$(command -v sleep)\" {directory}/{name}")
    tmux_cmd(host, socket, "new-window", "-t", "build:", "-n", "codex", f"{directory}/codex 900")
    tmux_cmd(host, socket, "new-window", "-t", "infra:", "-n", "claude", f"{directory}/claude 900")


def fixture_herdr(host, session):
    """Isolated Herdr server: named tabs and reported agent states."""
    herdr_start(host, session)
    deadline = time.monotonic() + 25
    while True:
        try:
            herdr(session, "workspace", "create", "--label", "kmux demo", "--no-focus")
            break
        except (subprocess.CalledProcessError, json.JSONDecodeError):
            if time.monotonic() > deadline:
                raise RuntimeError("Herdr fixture server did not come up")
            time.sleep(1)
    workspace = herdr(session, "workspace", "list")["workspaces"][0]["workspace_id"]
    first_tab = herdr(session, "tab", "list")["tabs"][0]["tab_id"]
    herdr(session, "tab", "rename", first_tab, "api")
    herdr(session, "tab", "create", "--label", "web", "--workspace", workspace, "--no-focus")
    tabs = herdr(session, "tab", "list")["tabs"]
    panes = herdr(session, "pane", "list")["panes"]
    main = next(pane for pane in panes if pane["tab_id"] == tabs[0]["tab_id"])
    web = next(pane for pane in panes if pane["tab_id"] == tabs[1]["tab_id"])
    for pane, agent, state, message in (
            (main["pane_id"], "codex", "working", "running the test suite"),
            (web["pane_id"], "claude", "blocked", "waiting for approval")):
        args = ["pane", "report-agent", "--source", "custom:kmux-demo", "--agent", agent,
                "--state", state]
        if message:
            args += ["--message", message]
        args.append(pane)
        herdr(session, *args)
    herdr(session, "pane", "run", main["pane_id"],
          "cargo test -p kmux-proxy -- --nocapture")
    herdr(session, "pane", "run", web["pane_id"], "git status --short")


def chrome_present(image):
    """True when the WAF owns the screen: the terminal's top border is at y 186.

    That line is there whether or not the Kindle keyboard is open. The device's
    own panels (quick settings, Home) do not draw it, so a missing line means
    injected taps would hit system chrome instead of the app.
    """
    gray = np.asarray(Image.open(io.BytesIO(image)).convert("L"))
    row = (gray[:, 60:gray.shape[1] - 60] < 128).mean(axis=1)
    return bool((row[180:200] > 0.8).any())


def quick_settings_visible(image):
    """Recognize the Kindle Quick Settings sheet without guessing from text.

    Its bottom edge is a full-width black divider around y=1270. The kmux WAF
    has no such bar there, including while its on-screen keyboard is visible.
    """
    gray = np.asarray(Image.open(io.BytesIO(image)).convert("L"))
    return bool(((gray[1265:1280, :] < 32).mean(axis=1) > 0.95).any())


def application_error_visible(image):
    """Recognize Mesquite's centered 'Application Error' dialog by its frame."""
    gray = np.asarray(Image.open(io.BytesIO(image)).convert("L"))
    return bool(((gray[575:590, 100:1165] < 32).mean(axis=1) > 0.9).any())


def waf_running(device):
    out = device.ssh('ps aux 2>/dev/null | grep -c "[m]esquite -l dev.qingshan.kmux"',
                     check=False).stdout.strip()
    return bool(out) and out != "0"


def dismiss_system_ui(device):
    """Close Quick Settings only when it is actually visible.

    The upward chevron is at (632, 1215). The former generic three taps hit
    Settings controls and the black navigation area rather than this chevron,
    so the sheet remained open and swallowed the following WAF gesture.
    """
    image = device.screenshot()
    if quick_settings_visible(image):
        device.gesture(tap((632, 1215), pause=900))
        return True
    if application_error_visible(image):
        # Mesquite puts CLOSE at the lower-right of its centered error dialog.
        device.gesture(tap((976, 997), pause=900))
        return True
    return False


def ensure_waf_foreground(device, attempts=6):
    """Wait until the WAF owns the screen, recovering recognized system UI."""
    for attempt in range(attempts):
        image = device.screenshot()
        (DEBUG / f"not-foreground-{attempt}.png").write_bytes(image)
        if quick_settings_visible(image):
            print("Kindle Quick Settings is open; closing it")
            dismiss_system_ui(device)
            time.sleep(1.5)
            continue
        if application_error_visible(image):
            print("Mesquite application-error dialog is open; closing it")
            dismiss_system_ui(device)
            time.sleep(1.5)
            continue
        if chrome_present(image):
            return
        if not waf_running(device):
            print("kmux is not running; launching it")
            launch_waf()
            time.sleep(2)
            continue
        raise RuntimeError("kmux WAF is running but another unrecognized screen is foreground; "
                           f"see {DEBUG / f'not-foreground-{attempt}.png'}")
    raise RuntimeError("kmux WAF did not return to the foreground after closing Quick Settings")


def wait_for_waf(device, timeout=90):
    """The app is up when Mesquite is hosting this WAF's page."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        out = device.ssh('ps aux 2>/dev/null | grep -c "[m]esquite -l dev.qingshan.kmux"',
                         check=False).stdout.strip()
        if out and out != "0":
            return
        time.sleep(2)
    raise RuntimeError("kmux WAF did not come up on the Kindle")


def install_snippets(device):
    """Give the snippets popup real content; the caller restores the original file.

    The WAF fetches quick-snippets.json once at load and uses `label`/`text`
    records, so this must happen before the app is launched.
    """
    saved = {path: device.ssh(f"cat {path}").stdout
             for path in (SNIPPETS, PACKAGE_SNIPPETS)}
    snippets = {"snippets": [
        {"label": "unit tests", "text": "cargo test -p kmux --lib"},
        {"label": "proxy build", "text": "cargo build --release -p kmux-proxy"},
        {"label": "follow logs", "text": "journalctl -u kmux-proxy -f"},
        {"label": "package", "text": "just package 0.5.26"},
        {"label": "device shell", "text": "ssh kindle"},
    ]}
    encoded = json.dumps(snippets, indent=2)
    for path in (SNIPPETS, PACKAGE_SNIPPETS):
        device.ssh(f"cat > {path} <<'KMUX_JSON'\n{encoded}\nKMUX_JSON")
    return saved


def restart_waf_for_snippets(test):
    """Restart Mesquite cleanly so it reloads the just-written snippets file."""
    device = test.device
    if waf_running(device):
        device.gesture(taps("exit", pause=900))
        deadline = time.monotonic() + 20
        while waf_running(device) and time.monotonic() < deadline:
            time.sleep(1)
        if waf_running(device):
            # Some 5.16 builds ignore appmgr.back for a WAF.  Terminate only
            # this app's Mesquite process; never kill unrelated WAFs.
            device.ssh("ps aux | awk '/[m]esquite -l dev.qingshan.kmux/ {print $2}' "
                       "| while read p; do kill \"$p\"; done")
            deadline = time.monotonic() + 10
            while waf_running(device) and time.monotonic() < deadline:
                time.sleep(1)
            if waf_running(device):
                raise RuntimeError("kmux WAF did not exit before snippet reload")
    test.launch()
    wait_for_waf(device)


def calibrate(device):
    """Optionally re-measure popup rows; defaults come from a measured layout."""
    DEBUG.mkdir(parents=True, exist_ok=True)
    steps = [
        ("switcher", taps("session_chip")),
        ("act", taps("menu_close") + taps("tab_more")),
        ("paste", taps("menu_close") + taps("panel_tools") + taps("tools_paste")),
        ("snippets", taps("menu_close") + taps("tools_snippets")),
        ("files", taps("menu_close") + taps("tools_files")),
    ]
    for name, gesture in steps:
        ensure_waf_foreground(device)
        state, image = device.step(gesture, settle=SETTLE)
        (DEBUG / f"{name}.png").write_bytes(image)
        rows = detect_rows(image)
        if rows:
            COORDS["rows"][name] = rows
        print(f"calibrate {name:9} rows at {COORDS['rows'].get(name)}")
    device.gesture(taps("menu_close"))


def compose_send():
    """Edit the pre-filled draft, add a line with Shift+Enter, then send with Enter."""
    return (type_text(" && echo kmux-demo-ok", 260) + "keydown Shift_L\n"
            + key("Return", 220) + "keyup Shift_L\n"
            + type_text("echo second-line", 200) + key("Return", 500))


def show_terminal_size(page, _state):
    """Prove the actual tmux geometry in the first advertised terminal frame."""
    size = tmux_cmd(PROXY_HOST, SESSION, "display-message", "-p", "-t", "api:api.0",
                    "#{window_width} #{window_height}").stdout.strip()
    if size != "80 24":
        raise AssertionError(f"fixture tmux geometry is {size!r}, expected '80 24'")
    page.cmd({"op": "text", "text": "printf 'kmux terminal: 80 columns x 24 rows\\n'"})
    page.cmd({"op": "key", "key": "Enter"})
    marker = "kmux terminal: 80 columns x 24 rows"
    return page.wait("80x24 terminal", lambda s: terminal_has_line(s, marker))


def populate_new_tab(page, _state):
    """A newly created shell is intentionally blank; give its frame useful proof."""
    page.cmd({"op": "text", "text": "printf 'new tab: ready to work\\n'"})
    page.cmd({"op": "key", "key": "Enter"})
    marker = "new tab: ready to work"
    return page.wait("new tab output", lambda s: terminal_has_line(s, marker))


def prepare_yank(page):
    """Put an unmistakable line in the selected pane before copy-mode freezes it."""
    page.cmd({"op": "text", "text": "printf 'KMUX_YANK: copied from Kindle\\n'"})
    page.cmd({"op": "key", "key": "Enter"})
    marker = "KMUX_YANK: copied from Kindle"
    page.wait("yank fixture", lambda s: terminal_has_line(s, marker))
    page.cmd({"op": "copy_mode"})


def terminal_text(state):
    return "\n".join(state.get("screen", []) + state.get("scrollback", []))


def terminal_has_line(state, marker):
    return any(line.strip() == marker for line in
               state.get("screen", []) + state.get("scrollback", []))


def yank_matched_line(page):
    """Select and yank the matched line through the same daemon key API as WAF."""
    page.cmd({"op": "copy_search", "query": "KMUX_YANK", "backwards": True})
    page.cmd({"op": "key", "key": "V"})
    page.cmd({"op": "key", "key": "y"})


def storyboard():
    """The demo, as ordered stops: what to drive, and what the frame must prove."""
    stops = [
        dict(label="01-title", kind="card", dwell=3600,
             subtitle="a terminal for remote tmux on Kindle",
             lines=["tmux and Herdr behind one API", "e-ink terminal, touch controls",
                    "Kindle Oasis, kindlehf"],
             footer="captured on a real Kindle",
             headline="kmux",
             detail="A Kindle app and a host-side proxy: your tmux sessions and Herdr "
                    "workspaces on e-ink."),
        dict(label="02-live", caption="Connect to a machine", dwell=3000,
            setup=lambda device: device.cmd({"op": "host", "id": TMUX_MACHINE}),
            wait=lambda s: s.get("connected") and s.get("activeHost") == TMUX_MACHINE,
            after=show_terminal_size,
            detail="kmux-proxy holds the tmux control-mode connection; the Kindle only "
                    "repaints dirty rows."),
        dict(label="03-type", caption="Type a command, see it run", dwell=3000,
             # Same LIPC ops the on-screen keyboard sends; injected keys need the
             # hidden capture field focused, which a system overlay can steal.
             setup=lambda device: (device.cmd({"op": "text", "text": "printf 'kmux demo ready\\n'"}),
                                   device.cmd({"op": "key", "key": "Enter"})),
             wait=lambda s: "kmux demo ready" in "\n".join(
                 s.get("screen", []) + s.get("scrollback", [])),
             detail="Literal and Unicode input both travel as tmux send-keys."),
        dict(label="04-history", caption="Scroll back through history", dwell=3000,
             setup=lambda device: (device.cmd({"op": "text", "text": "seq 1 140"}),
                                   device.cmd({"op": "key", "key": "Enter"}),
                                   device.wait("history",
                                               lambda s: len(s.get("scrollback", [])) >= 80),
                                   device.cmd({"op": "key", "key": "ScrollUp"}),
                                   device.cmd({"op": "key", "key": "ScrollUp"})),
             gesture=taps("panel_scroll") + taps("scroll_up"),
             wait=lambda s: s.get("scrollOffset", 0) > 0,
             detail="The Scroll panel freezes the view and shows how far back you are."),
        dict(label="05-search", caption="Search the scrollback", dwell=3000,
             # WAF state survives a relaunch on Mesquite; clear an earlier
             # query before entering the known scrollback line.
             gesture=taps("scroll_search_clear") + taps("scroll_search") + type_text("100")
             + taps("scroll_search_forward"),
             detail="Matches highlight in the frozen view; the search stays on the Kindle."),
        dict(label="06-prompt", caption="Prompt panel: context-aware keys", dwell=3000,
             setup=lambda device: device.cmd({"op": "scroll_to", "offset": 0}),
             gesture=taps("panel_prompt"),
             detail="Keys for the program in the pane, not a generic keyboard."),
        dict(label="07-files", caption="Tools: insert a file path", dwell=3000,
             setup=lambda device: device.cmd({"op": "list_files", "path": "/tmp"}),
             gesture=taps("panel_tools") + taps("tools_files"),
             wait=lambda s: bool(s.get("cwd")),
             detail="The picker asks the pane for its working directory and lists it."),
        dict(label="08-paste", caption="Clipboard history", dwell=3000,
             gesture=taps("menu_close") + taps("tools_paste"),
             detail="Anything yanked in copy mode or sent from the Kindle lands here."),
        dict(label="09-snippets", caption="Quick snippets, defined in JSON", dwell=3000,
             gesture=taps("menu_close") + taps("tools_snippets"),
             detail="The snippet list is a file on the Kindle, not baked into the app."),
        dict(label="10-compose", caption="Edit, then send to the pane", dwell=3000,
             gesture=taps("menu_close") + taps("tools_snippets") + taps(pop_row("snippets", 0))
             + compose_send(),
             detail="Enter sends the text plus a newline; Shift+Enter keeps editing."),
        dict(label="11-sessions", caption="Switch session from the chip", dwell=3000,
             gesture=taps("menu_close") + taps("session_chip"),
             wait=lambda s: bool(s.get("sessions")),
             detail="One chip reaches every session on every configured machine."),
        dict(label="12-hosts", caption="Hosts group: pick another machine", dwell=3000,
             gesture=taps("menu_close") + taps("session_chip") + taps("menu_group_hosts"),
             detail="The same chip switches between tmux and Herdr machines."),
        dict(label="13-herdr", caption="Herdr workspaces look the same", dwell=3000,
             # The fixture host order is tmux, Herdr, offline.  Pick Herdr from
             # the Hosts group directly: text focus can be lost if Mesquite has
             # just dismissed a system dialog, but this route is pure touch.
             gesture=taps("menu_close") + taps("session_chip") + taps("menu_group_hosts")
             + taps(menu_row(1)) + taps("menu_close"),
             wait=lambda s: s.get("connected") and s.get("activeHost") == HERDR_MACHINE,
             detail="Same navigation, same terminal, for the other backend."),
        dict(label="14-panes", caption="Panes group: jump straight to a pane", dwell=3000,
             gesture=taps("menu_close") + taps("session_chip") + taps("menu_group_panes"),
             detail="Jump directly to any pane across every machine."),
        dict(label="15-agents", caption="Agents group: who needs you", dwell=3000,
             gesture=taps("menu_close") + taps("session_chip") + taps("menu_group_agents"),
             detail="Badges show Needs input, Working, Done, Idle across all hosts."),
        dict(label="16-agent-search", caption="Search agents by host or state", dwell=3000,
             gesture=taps("menu_close") + taps("session_chip") + taps("menu_group_agents")
             + taps("menu_search") + type_text("codex"),
             detail="For example `@agents blocked` from anywhere in the switcher."),
        dict(label="17-tabs", caption="Tabs, and the tab menu", dwell=3000,
             gesture=taps("menu_close") + taps("tab_second") + taps("tab_more"),
             detail="Tabs are tmux windows and Herdr tabs; the menu switches panes."),
        dict(label="18-newtab", caption="Open a tab; close it with ×", dwell=3000,
             gesture=taps("menu_close") + taps("tab_new"),
             after=populate_new_tab,
             detail="Creating and closing tabs never needs a tmux command."),
        dict(label="19-copy", caption="Copy mode: freeze, search, select", dwell=3000,
             setup=prepare_yank,
             wait=lambda s: s.get("copyMode") is True,
             detail="The pane stops updating so you can read and mark text."),
        dict(label="20-yank", caption="Yank a line into the clipboard", dwell=3000,
             setup=yank_matched_line,
             expect=lambda s: "KMUX_YANK: copied from Kindle" in (s.get("clipboard") or ""),
             detail="Selections land in clipboard history and can be sent back."),
        dict(label="21-paste-back", caption="Clipboard history after yank", dwell=3000,
             gesture=taps("panel_tools") + taps("tools_paste"),
             detail="The freshly yanked line is ready to paste back without retyping."),
        dict(label="22-badges", caption="Agent state updates live", dwell=3000,
             setup=lambda device: device.cmd({"op": "host", "id": HERDR_MACHINE}),
             wait=lambda s: s.get("activeHost") == HERDR_MACHINE and s.get("connected"),
             gesture=taps("menu_close") + taps("session_chip") + taps("menu_group_agents"),
             detail="Herdr events wake the views; other hosts refresh in the background."),
        dict(label="23-settings", caption="Settings: endpoint and token", dwell=3000,
             gesture=taps("menu_close") + taps("settings"),
             detail="The saved token is never exposed back to the page."),
        dict(label="24-error", caption="A dead host explains itself", dwell=3400,
             setup=lambda device: device.cmd({"op": "host", "id": OFFLINE_MACHINE}),
             wait=lambda s: bool(s.get("lastError")),
             gesture=taps("menu_close"),
             detail="Background failures never cover the terminal; tap the status line "
                    "for details."),
        dict(label="25-recover", caption="Switch back and keep working", dwell=3000,
             setup=lambda device: device.cmd({"op": "host", "id": TMUX_MACHINE}),
             wait=lambda s: s.get("connected") and not s.get("lastError"),
             gesture=taps("menu_close"),
             detail="A background connection error does not close your draft or settings."),
        dict(label="26-end", kind="card", dwell=3800,
             subtitle="kmux",
             lines=["one API for tmux and Herdr", "session, tab, pane, agent navigation",
                    "copy mode, scrollback search, snippets"],
             footer="frames captured from a Kindle Oasis",
             headline="kmux",
             detail="Everything here was driven by script against the real device."),
    ]
    # A short product story is more useful than an exhaustive manual test log.
    # These stops show the distinct reasons to use kmux: terminal work,
    # e-ink-friendly history and copy, tools, unified navigation, and agents.
    advertised = {"01", "02", "03", "04", "05", "07", "09", "10", "11", "13",
                  "14", "15", "17", "18", "19", "20", "21", "26"}
    return [stop for stop in stops if stop["label"][:2] in advertised]


class Capture:
    """Runs the stops and writes frames plus the storyboard the builder reads."""

    def __init__(self, page, only=None, start_from=None):
        self.page = page
        self.device = page.device
        self.frames = []
        self.only = only.split(",") if only else None
        self.start_from = start_from
        self.skipping = bool(start_from)

    def selected(self, label):
        if self.only:
            return any(label.startswith(item) for item in self.only)
        if self.skipping:
            if label.startswith(self.start_from):
                self.skipping = False
            else:
                return False
        return True

    def run(self, stops):
        for stop in stops:
            if self.selected(stop["label"]):
                self.capture(stop)
        return self.frames

    def capture(self, stop):
        label = stop["label"]
        page = self.page
        ensure_waf_foreground(page.device)
        if stop.get("setup"):
            stop["setup"](page)
        if stop.get("kind") == "card":
            print(f"{label}: card")
        else:
            gesture = stop.get("gesture", "")
            path = FRAMES / f"{label}.png"
            if stop.get("wait"):
                if gesture:
                    page.gesture(gesture)
                state = page.wait(label, stop["wait"])
                if stop.get("after"):
                    state = stop["after"](page, state) or state
                ensure_waf_foreground(page.device)
                page.screenshot_file(path, settle=2.4)
                image = path.read_bytes()
                if not chrome_present(image):
                    ensure_waf_foreground(page.device)
                    page.screenshot_file(path, settle=1.2)
                    image = path.read_bytes()
                if not chrome_present(image):
                    raise AssertionError(f"{label}: the WAF chrome is not on screen; "
                                         f"system UI probably covered the app")
            else:
                state, image = page.step(gesture, settle=SETTLE)
                if stop.get("after"):
                    state = stop["after"](page, state) or state
                    time.sleep(SETTLE)
                    image = page.device.screenshot()
                if not chrome_present(image):
                    ensure_waf_foreground(page.device)
                    state, image = page.step(gesture, settle=SETTLE)
                if not chrome_present(image):
                    raise AssertionError(f"{label}: the WAF chrome is not on screen; "
                                         f"system UI probably covered the app")
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(image)
            if stop.get("expect") and not stop["expect"](state):
                raise AssertionError(f"{label}: unexpected state: "
                                     f"connected={state.get('connected')} "
                                     f"host={state.get('activeHost')} "
                                     f"scroll={state.get('scrollOffset')} "
                                     f"copy={state.get('copyMode')}")
            print(f"{label}: settled")
        entry = {key: stop[key] for key in ("label", "kind", "caption", "subtitle", "lines",
                                            "footer", "headline", "detail") if key in stop}
        entry["dwell_ms"] = stop.get("dwell", 3000)
        entry.setdefault("kind", "screen")
        if entry["kind"] == "screen":
            entry["file"] = f"{label}.png"
        self.frames.append(entry)
        self.write()

    def write(self):
        note = ("Fixtures: isolated tmux server and Herdr session on the proxy host, "
                "removed after the run; agent badges come from Herdr report-agent.")
        STORYBOARD.write_text(json.dumps({"frames": self.frames, "note": note}, indent=2))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--only", help="comma-separated label prefixes to capture")
    parser.add_argument("--from", dest="start_from", help="resume at this label prefix")
    parser.add_argument("--list", action="store_true", help="print the storyboard and exit")
    parser.add_argument("--calibrate", action="store_true",
                        help="re-measure popup row positions before capturing")
    parser.add_argument("--calibrate-only", action="store_true",
                        help="measure popup geometry, then stop")
    args = parser.parse_args()

    if args.list:
        for index, stop in enumerate(storyboard()):
            print(f"{index + 1:02d}  {stop['label']:16} "
                  f"{stop.get('caption') or stop.get('subtitle', '')}")
        return

    FRAMES.mkdir(parents=True, exist_ok=True)
    DEBUG.mkdir(parents=True, exist_ok=True)
    saved_snippets = None
    with KindleE2E() as test:
        device, page = test.device, test.page
        capture = Capture(page, only=args.only, start_from=args.start_from)
        try:
            test.hosts([
                {"id": TMUX_MACHINE, "name": "atlas (tmux)", "target": "local:qingshan",
                 "session": "api", "tmuxSocket": SESSION},
                {"id": HERDR_MACHINE, "name": "atlas (Herdr)", "target": "local:qingshan",
                 "backend": "herdr", "herdrSession": SESSION},
                {"id": OFFLINE_MACHINE, "name": "offline host", "target": "local:qingshan",
                 "backend": "herdr", "herdrSocket": f"/tmp/{SESSION}-missing.sock"}])
            test.tmux(SESSION, fixture_tmux)
            test.herdr(SESSION, fixture_herdr)
            saved_snippets = install_snippets(device)
            dismiss_system_ui(device)
            # Snippets load only at app startup. Exit before launching so
            # Mesquite cannot present its duplicate-application error dialog.
            restart_waf_for_snippets(test)
            dismiss_system_ui(device)
            ensure_waf_foreground(device)
            page.command({"op": "host", "id": TMUX_MACHINE})
            page.expect(lambda s: s.get("connected") and s.get("activeHost") == TMUX_MACHINE
                        and s.get("sessions"), "tmux connected", timeout=90)
            for text in ("just package 0.5.26", "ssh kindle", "cargo test -p kmux --lib"):
                page.command({"op": "clipboard_remember", "text": text})
            if args.calibrate or args.calibrate_only:
                calibrate(device)
            if args.calibrate_only:
                return
            capture.run(storyboard())
        finally:
            if saved_snippets is not None:
                for path, contents in saved_snippets.items():
                    device.ssh(f"cat > {path} <<'KMUX_JSON'\n{contents}\nKMUX_JSON")
            ssh(PROXY_HOST, f"rm -rf /tmp/{SESSION}", check=False)
    print(f"\ncaptured {len(capture.frames)} frames into {FRAMES}")


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print("demo capture:", error, file=sys.stderr)
        raise SystemExit(1)
