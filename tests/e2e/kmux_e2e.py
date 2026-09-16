#!/usr/bin/env python3
"""The single kmux end-to-end entry point.

This is intentionally shaped like a tiny Playwright API.  Scenarios interact
with ``page`` rather than hand-assembling LIPC, HTTP, gesture and screenshot
plumbing.  The API is useful for a new regression as well as for the complete
suite below:

    with KmuxE2E() as test:
        test.tmux("main")
        test.start()
        test.page.type("printf ready\\n")
        test.page.press("Enter")
        test.page.expect(lambda state: "ready" in test.page.text(state))

``just e2e`` runs the real tmux/Herdr proxy contract and its event failure
cases. ``just demo`` adds the Kindle WAF tour and renders its video/animation
artifacts.
"""
import argparse
import importlib
import json
import os
import pwd
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path

# When this file is the executable, demo modules still import this exact API
# rather than creating a second ``kmux_e2e`` module instance.
sys.modules.setdefault("kmux_e2e", sys.modules[__name__])

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))
if str(ROOT / "tools") not in sys.path:
    sys.path.insert(0, str(ROOT / "tools"))


class Expect:
    """Assertions with useful, retrying failure messages."""

    def __init__(self, page):
        self.page = page

    def to_have(self, predicate, message="state did not match", timeout=15):
        return self.page.expect(predicate, message, timeout)

    def text(self, value, timeout=15):
        return self.to_have(lambda state: value in self.page.text(state),
                            "terminal never contained {!r}".format(value), timeout)


class Artifacts:
    """Screenshots plus a storyboard suitable for GIF/MP4 rendering."""

    def __init__(self, directory=None):
        self.directory = Path(directory or ROOT / "target/e2e/artifacts")
        self.frames = []

    def screenshot(self, label, png, caption=None, dwell_ms=1000):
        self.directory.mkdir(parents=True, exist_ok=True)
        filename = "{:03d}-{}.png".format(len(self.frames) + 1, label)
        path = self.directory / filename
        path.write_bytes(png)
        self.frames.append({"label": label, "file": filename,
                            "caption": caption or label, "dwell_ms": dwell_ms})
        return path

    def animation(self, path=None):
        """Write a portable ordered-frame manifest (an animation source)."""
        path = Path(path or self.directory / "animation.json")
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps({"frames": self.frames}, indent=2) + "\n")
        return path

    def video(self, output=None, fps=4):
        """Encode captured frames to MP4 when ffmpeg is available."""
        if not self.frames:
            raise RuntimeError("cannot render a video without screenshots")
        output = Path(output or self.directory / "kmux-e2e.mp4")
        manifest = self.directory / "frames.txt"
        with manifest.open("w") as handle:
            handle.write("ffconcat version 1.0\n")
            for frame in self.frames:
                handle.write("file '{}'\n".format((self.directory / frame["file"]).as_posix()))
                handle.write("duration {:.3f}\n".format(frame["dwell_ms"] / 1000))
            handle.write("file '{}'\n".format((self.directory / self.frames[-1]["file"]).as_posix()))
        try:
            subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-f", "concat", "-safe", "0",
                            "-i", str(manifest), "-r", str(fps), "-pix_fmt", "yuv420p", str(output)],
                           check=True)
        finally:
            manifest.unlink(missing_ok=True)
        return output


class ProxyPage:
    """Playwright-style page for the versioned proxy API."""

    def __init__(self, base_url, token="fixture", machine=None):
        self.base_url, self.token, self.machine = base_url.rstrip("/"), token, machine
        self.selection = None
        self.expectation = Expect(self)

    def _post(self, endpoint, **payload):
        payload.setdefault("version", 1)
        payload.setdefault("token", self.token)
        if self.machine is not None:
            payload.setdefault("machine", self.machine)
        request = urllib.request.Request(self.base_url + endpoint, json.dumps(payload).encode(),
                                         {"Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(request, timeout=25) as response:
                data = json.load(response)
        except urllib.error.HTTPError as error:
            raise RuntimeError(error.read().decode()) from error
        self.selection = data.get("selection", self.selection)
        return data

    def action(self, op="snapshot", **fields):
        expected = fields.pop("expected_pane", None)
        if expected is None and self.selection and op in {"text", "key", "copy_enter", "copy_key", "copy_search"}:
            expected = self.selection["pane"]
        return self._post("/v1/action", action={"op": op, **fields}, expected_pane=expected)

    def snapshot(self): return self.action()
    def type(self, text): return self.action("text", text=text)
    def press(self, key): return self.action("key", key=key)
    def select(self, **target): return self.action("select", **target)
    def text(self, state): return "\n".join(state.get("terminal", {}).get("screen", []))

    def expect(self, predicate, message="state did not match", timeout=15):
        deadline, state = time.monotonic() + timeout, self.snapshot()
        while time.monotonic() < deadline:
            if predicate(state):
                return state
            time.sleep(.15)
            state = self.snapshot()
        raise AssertionError("{}: {}".format(message, state))


class KindlePage:
    """The same page vocabulary for the physical WAF.

    ``tap`` and ``press`` are genuine XTEST input; ``command`` is reserved for
    daemon operations whose WAF control has already been tested by the gesture
    tour.  Every visual artifact is a Kindle framebuffer capture.
    """

    def __init__(self, device, artifacts=None):
        self.device, self.artifacts = device, artifacts or Artifacts()
        self.expectation = Expect(self)

    def command(self, op): self.device.cmd(op)
    cmd = command  # Existing concise storyboards can use the page as their device.
    def prevent_sleep(self, on=True): self.device.prevent_sleep(on)
    def gesture(self, commands): return self.device.gesture(commands)
    def step(self, gesture, settle=3.0): return self.device.step(gesture, settle=settle)
    def snap(self, path, settle=3.0): return self.device.snap(path, settle=settle)
    def wait(self, label, predicate, timeout=30, snap=None):
        return self.device.wait(label, predicate, timeout=timeout, snap=snap)
    def tap(self, x, y, pause=300): self.device.gesture("tap {} {}\nsleep {}".format(x, y, pause))
    def press(self, key, pause=300): self.device.gesture("key {}\nsleep {}".format(key, pause))
    def type(self, text, pause=150): self.device.gesture("text {}\nsleep {}".format(text, pause))
    def snapshot(self): return self.device.status()
    def text(self, state): return "\n".join(state.get("screen", []) + state.get("scrollback", []))

    def expect(self, predicate, message="state did not match", timeout=30):
        return self.device.wait(message, predicate, timeout=timeout)

    def screenshot(self, label, caption=None, settle=2.5):
        time.sleep(settle)
        return self.artifacts.screenshot(label, self.device.screenshot(), caption)

    def screenshot_file(self, path, settle=2.5):
        """Save a raw device framebuffer at an exact storyboard path."""
        time.sleep(settle)
        path = Path(path)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(self.device.screenshot())
        return path


class KindleE2E:
    """Remote fixture for a Kindle WAF scenario or recorded product tour.

    It owns the temporary proxy configuration, named tmux/Herdr fixtures,
    selected host, and sleep inhibition. The real device is never left pointed
    at a fixture after a normal failure.
    """

    def __init__(self):
        from kindle_lib import Device, KINDLE_HOST, PROXY_HOST, ProxyFixture, ssh
        self._ssh, self._kindle_host, self._proxy_host = ssh, KINDLE_HOST, PROXY_HOST
        self.device = Device()
        self.page = KindlePage(self.device)
        self._proxy = ProxyFixture()
        self._old_host = ""
        self._tmux, self._herdr = [], []

    def __enter__(self):
        config = json.loads(self._ssh(self._kindle_host,
                                      "cat /mnt/us/kmux/var/config.json").stdout)
        self._old_host = config.get("activeHost", "")
        self._proxy.__enter__()
        self.page.prevent_sleep(True)
        return self

    def hosts(self, hosts):
        self._proxy.set_hosts(hosts)

    def tmux(self, socket_name, setup):
        setup(self._proxy_host, socket_name)
        self._tmux.append(socket_name)

    def herdr(self, session, setup):
        setup(self._proxy_host, session)
        self._herdr.append(session)

    def launch(self):
        from kindle_lib import launch_waf
        launch_waf(self._kindle_host)

    def __exit__(self, *unused):
        from kindle_lib import herdr_stop, tmux_kill
        try:
            if self._old_host:
                self.page.command({"op": "host", "id": self._old_host})
        except Exception:
            pass
        self.page.prevent_sleep(False)
        for socket_name in self._tmux:
            tmux_kill(self._proxy_host, socket_name)
        for session in self._herdr:
            herdr_stop(self._proxy_host, session)
        self._proxy.__exit__(*unused)


class LocalFixture:
    """One cleanup boundary for isolated tmux and Herdr servers."""

    def __init__(self):
        self.name = "kmux-e2e-{}".format(os.getpid())
        self.directory = tempfile.TemporaryDirectory(prefix="kmux-e2e-")
        self.proxy = None
        self.herdr_processes = []
        self.hosts = []

    def tmux(self, session="main", command="exec sh", host_id="tmux"):
        """Start a config-free tmux fixture and return its proxy host entry."""
        subprocess.run(["tmux", "-L", self.name, "-f", "/dev/null", "new-session", "-d",
                        "-s", session, command], check=True, capture_output=True)
        entry = {"id": host_id, "target": "local:" + pwd.getpwuid(os.getuid()).pw_name,
                 "session": session, "tmuxSocket": self.name}
        self.hosts.append(entry)
        return entry

    def herdr(self, session=None, host_id="herdr"):
        """Start a named Herdr fixture; cleanup always stops only that session."""
        session = session or self.name
        process = subprocess.Popen(["herdr", "--session", session, "server"],
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.herdr_processes.append((session, process))
        entry = {"id": host_id, "target": "local:" + pwd.getpwuid(os.getuid()).pw_name,
                 "backend": "herdr", "herdrSession": session}
        self.hosts.append(entry)
        return entry

    def start_proxy(self, hosts):
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0)); port = listener.getsockname()[1]
        config = Path(self.directory.name) / "proxy.json"
        config.write_text(json.dumps({"bind": "127.0.0.1", "port": port, "token": "fixture", "hosts": hosts}))
        self.proxy = subprocess.Popen([str(ROOT / "target/debug/kmux-proxy"), str(config)], stdout=subprocess.DEVNULL)
        return ProxyPage("http://127.0.0.1:{}".format(port))

    def close(self):
        if self.proxy:
            self.proxy.terminate(); self.proxy.wait(timeout=10)
        subprocess.run(["tmux", "-L", self.name, "kill-server"], check=False,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        for session, process in self.herdr_processes:
            subprocess.run(["herdr", "session", "stop", session], check=False,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            process.terminate()
            process.wait(timeout=10)
        self.directory.cleanup()


class KmuxE2E:
    """Fixture-first setup for a small new backend regression.

    Add `tmux()` and/or `herdr()` before `start()`. The context manager owns
    only its named servers, so it can never modify a developer's tmux session.
    """

    def __init__(self):
        self.fixtures = LocalFixture()
        self.page = None

    def tmux(self, *args, **kwargs): return self.fixtures.tmux(*args, **kwargs)
    def herdr(self, *args, **kwargs): return self.fixtures.herdr(*args, **kwargs)

    def start(self):
        if not self.fixtures.hosts:
            raise RuntimeError("add a tmux() or herdr() fixture before start()")
        self.page = self.fixtures.start_proxy(self.fixtures.hosts)
        self.page.machine = self.fixtures.hosts[0]["id"]
        return self.page

    def __enter__(self): return self
    def __exit__(self, *unused): self.fixtures.close()


def scenario(name, fn):
    print("\n== {} ==".format(name), flush=True)
    fn()


def host_suite():
    # These scenarios are retained as modules during the transition so their
    # mature real-server assertions remain intact; this file is the only test
    # entry point and owns their ordering/reporting.
    scenario("WAF controls and overlays", lambda: subprocess.run(
        ["node", str(HERE / "waf_e2e.js")], cwd=ROOT, check=True))
    scenario("unified tmux + Herdr API", importlib.import_module("unified").main)
    scenario("Herdr event recovery", importlib.import_module("herdr_events").main)


def demo_suite():
    demo, builder = importlib.import_module("kindle_demo"), importlib.import_module("demo_build")
    saved = sys.argv[:]
    try:
        sys.argv = ["kindle_demo.py"]
        scenario("Kindle WAF advertising tour", demo.main)
        sys.argv = ["demo_build.py"]
        scenario("demo video + animation", builder.main)
    finally:
        sys.argv = saved


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--record", action="store_true", help="record the WAF tour and encode GIF/MP4")
    args = parser.parse_args()
    host_suite()
    if args.record:
        demo_suite()
    print("\nkmux E2E: all selected scenarios passed", flush=True)


if __name__ == "__main__":
    main()
