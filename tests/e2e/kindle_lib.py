#!/usr/bin/env python3
"""Shared plumbing for the device end-to-end runs.

Every script here drives the Kindle the same way: LIPC `cmd` ops change daemon
state, `/var/local/mesquite/kmux/status.json` is the observable result, and
`/usr/sbin/screenshot` is the only faithful view of the WAF. The LIPC channel
cannot press a button, so gestures go through `tools/kindle_xinput.c`, which
injects XTEST events that reach the WAF exactly like the touchscreen does.

Typical use:

    from kindle_lib import Device, ProxyFixture, tmux_server

    device = Device()
    with ProxyFixture() as proxy:
        proxy.add_hosts([{...}])
        device.step("tap 100 209\\nsleep 800", settle=2.5, path="frame.png")
        assert device.status()["connected"]
"""
import json
import os
import shlex
import subprocess
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
KINDLE_HOST = os.environ.get("KMUX_E2E_KINDLE_HOST", "kindle")
PROXY_HOST = os.environ.get("KMUX_E2E_OUTBOX_HOST", "outbox")
STATUS_PATH = "/var/local/mesquite/kmux/status.json"
PROXY_CONFIG_PATH = "/etc/kmux-proxy.json"
PROXY_SERVICE = "kmux-proxy"
XINPUT_REMOTE = "/tmp/kindle_xinput"
XINPUT_BINARY = ROOT / "target/kindlehf-x11/bin/kindle_xinput"
PNG_MAGIC = b"\x89PNG\r\n\x1a\n"
STATUS_MARKER = b"KMUX_STATUS_END"

SSH_OPTIONS = ["-o", "BatchMode=yes", "-o", "ConnectTimeout=15",
               "-o", "ServerAliveInterval=5", "-o", "ServerAliveCountMax=3"]


def run(args, **kw):
    kw.setdefault("text", True)
    kw.setdefault("check", True)
    kw.setdefault("timeout", 60)
    kw.setdefault("stdout", subprocess.PIPE)
    return subprocess.run(args, **kw)


def ssh(host, command, **kw):
    """Run a remote shell command. Never echo configuration stdout: it holds the token."""
    return run(["ssh"] + SSH_OPTIONS + [host, command], **kw)


class Device:
    """The Kindle under test: daemon state, framebuffer captures, and gestures."""

    def __init__(self, host=KINDLE_HOST, status_path=STATUS_PATH):
        self.host = host
        self.status_path = status_path
        self._input_ready = False

    def ssh(self, command, **kw):
        return ssh(self.host, command, **kw)

    def status(self):
        return json.loads(self.ssh(f"cat {self.status_path}").stdout)

    def cmd(self, op):
        """One-way JSON op through the daemon's LIPC `cmd` property."""
        payload = json.dumps(op, separators=(",", ":")).replace("'", "'\\''")
        self.ssh(f"lipc-set-prop dev.qingshan.kmuxd cmd '{payload}'")

    def prevent_sleep(self, on=True):
        """Keep the Kindle awake for the duration of a run (best effort)."""
        self.ssh(f"lipc-set-prop com.lab126.powerd preventScreenSaver "
                 f"{1 if on else 0}", check=False)

    def screenshot(self):
        """Raw device framebuffer as PNG bytes."""
        out = self.ssh('f=/tmp/kmux-shot.png; /usr/sbin/screenshot -f "$f"; cat "$f"',
                       text=False, stdout=subprocess.PIPE).stdout
        if not out.startswith(PNG_MAGIC):
            raise RuntimeError("Kindle framebuffer capture failed")
        return out

    def snap(self, path, settle=3.0):
        """Capture the framebuffer; 3 s covers the WAF's 2.5 s idle refresh."""
        time.sleep(settle)
        image = self.screenshot()
        path = Path(path)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(image)
        return path

    def ensure_input_tool(self, rebuild=False):
        """Cross-compile tools/kindle_xinput.c if needed and install it under /tmp."""
        if not self._input_ready:
            if rebuild or not XINPUT_BINARY.exists():
                run([str(ROOT / "tools/build-kindle-xinput.sh")])
            kindle_build = ssh(self.host, f"sha1sum {XINPUT_REMOTE} 2>/dev/null | cut -d' ' -f1",
                               check=False).stdout.strip()
            local_build = run(["sha1sum", str(XINPUT_BINARY)]).stdout.split()[0]
            if kindle_build != local_build:
                run(["scp", "-q", str(XINPUT_BINARY), f"{self.host}:{XINPUT_REMOTE}"])
                self.ssh(f"chmod +x {XINPUT_REMOTE}")
            self._input_ready = True

    def gesture(self, commands):
        """Run injected touch/key commands with no capture, raising if any failed."""
        self.ensure_input_tool()
        # The Kindle's X display is :0; ssh sessions have no DISPLAY set.
        out = self.ssh(f"DISPLAY=:0 {XINPUT_REMOTE} >/tmp/kindle_xinput.log 2>&1 "
                       f"<<'KMUX_INPUT'\n"
                       f"{commands.rstrip()}\nKMUX_INPUT\necho input_status=$?",
                       check=False).stdout
        if "input_status=0" not in out:
            log = self.ssh("cat /tmp/kindle_xinput.log", check=False).stdout
            raise RuntimeError(f"gesture injection failed:\n{out}\n{log}")

    def step(self, gesture, settle=3.0, status=True):
        """Run gestures, wait for the WAF to settle, then capture and read status.

        One ssh round trip per step keeps a long storyboard quick: the gestures
        and the capture share a connection. Returns (status_or_None, png_bytes).
        """
        self.ensure_input_tool()
        script = [f"DISPLAY=:0 {XINPUT_REMOTE} >/tmp/kindle_xinput.log 2>&1 "
                  f"<<'KMUX_INPUT'",
                  gesture.rstrip("\n"), "KMUX_INPUT",
                  "echo input_status=$?",
                  f"sleep {int(round(settle))}"]
        if status:
            script.append(f"cat {self.status_path}")
        script += [f"echo {STATUS_MARKER.decode()}", "f=/tmp/kmux-shot.png",
                   '/usr/sbin/screenshot -f "$f"', 'cat "$f"']
        out = self.ssh("\n".join(script), text=False, stdout=subprocess.PIPE,
                       timeout=int(settle) + 60).stdout
        head, _, image = out.partition(PNG_MAGIC)
        if not image:
            raise RuntimeError("no framebuffer image in step output")
        image = PNG_MAGIC + image
        text = head.split(STATUS_MARKER)[0].decode(errors="replace")
        lines = [line for line in text.splitlines() if line.strip()]
        state = None
        for line in lines:
            if line.startswith("input_status=") and line != "input_status=0":
                raise RuntimeError(f"gesture injection failed: {line}\n{text}")
            if line.startswith("{"):
                state = json.loads(line)
        if status and state is None:
            raise RuntimeError(f"no status in step output: {text}")
        return state, image

    def step_snap(self, gesture, path, settle=3.0):
        """`step`, written to disk, returning the status."""
        state, image = self.step(gesture, settle=settle)
        path = Path(path)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(image)
        return state

    def wait(self, label, predicate, timeout=30, snap=None):
        """Poll status until predicate holds; optionally capture at that moment."""
        deadline = time.monotonic() + timeout
        state = self.status()
        while time.monotonic() < deadline:
            if predicate(state):
                if snap:
                    self.snap(snap)
                return state
            time.sleep(0.5)
            state = self.status()
        raise TimeoutError(f"timed out waiting for {label}")


class ProxyFixture:
    """Temporarily add machines to the proxy's config, restoring it afterwards."""

    def __init__(self, host=PROXY_HOST, config=PROXY_CONFIG_PATH, service=PROXY_SERVICE):
        self.host = host
        self.config = config
        self.service = service
        self.saved = None

    def __enter__(self):
        # sudo cat: the config holds the shared token and is mode 600.
        self.saved = ssh(self.host, f"sudo cat {self.config}").stdout
        owner = ssh(self.host, f"sudo stat -c '%u %g %a' {self.config}").stdout.split()
        self.owner, self.group, self.mode = owner if len(owner) == 3 else ("0", "0", "600")
        return self

    def __exit__(self, *exc):
        if self.saved is not None:
            self.write(self.saved, restart=True)
        state = ssh(self.host, f"systemctl is-active {self.service}", check=False).stdout.strip()
        if state != "active":
            print(f"warning: {self.service} is {state!r} after restoring the proxy config")
        return False

    def write(self, contents, restart=True):
        """Install a proxy config, keeping the service user able to read it.

        The proxy runs as an unprivileged user (`User=` in its unit), so a plain
        root-owned 600 file makes it crash-loop on startup: the ownership and mode
        of the original file are restored on every write.
        """
        local = ROOT / "target/demo/proxy.json"
        local.parent.mkdir(parents=True, exist_ok=True)
        local.write_text(contents)
        local.chmod(0o600)
        remote = "/tmp/kmux-e2e-proxy.json"
        run(["scp", "-q", str(local), f"{self.host}:{remote}"])
        ssh(self.host, f"sudo install -m {self.mode} -o {self.owner} -g {self.group} "
                       f"{remote} {self.config} && rm -f {remote}")
        if restart:
            ssh(self.host, f"sudo systemctl restart {self.service}")
            state = ssh(self.host, f"systemctl is-active {self.service}", check=False).stdout.strip()
            if state != "active":
                raise RuntimeError(f"{self.service} did not start: {state}")

    def add_hosts(self, hosts, restart=True):
        config = json.loads(self.saved)
        config["hosts"].extend(hosts)
        self.write(json.dumps(config), restart=restart)
        return config

    def set_hosts(self, hosts, restart=True):
        """Replace the machine list, keeping bind/port/token: used by the demo so a
        recorded video never shows the private machine names of a real setup."""
        config = json.loads(self.saved)
        config["hosts"] = hosts
        self.write(json.dumps(config), restart=restart)
        return config


def tmux_cmd(host, socket, *args, check=True):
    """Run a tmux command on an isolated socket.

    `-f /dev/null` ignores the host's ~/.tmux.conf: fixture servers must not
    inherit base-index or renumbering, which make window names unpredictable.
    """
    return ssh(host, "tmux -L " + socket + " -f /dev/null " +
               " ".join(shlex.quote(str(arg)) for arg in args), check=check)


def tmux_server(host, socket, session="main", command="exec sh", window=None, check=True):
    """Start a detached tmux server on an isolated socket, never the user's."""
    args = ["new-session", "-d", "-s", session]
    if window:
        args += ["-n", window]
    args.append(command)
    return tmux_cmd(host, socket, *args, check=check)


def tmux_kill(host, socket):
    return tmux_cmd(host, socket, "kill-server", check=False)


def herdr_start(host, session, binary="~/.local/bin/herdr"):
    """Start a named Herdr server in the background, detached from ssh."""
    return ssh(host, f"nohup {binary} --session {session} server "
                     f">/tmp/{session}.log 2>&1 </dev/null &", check=False)


def herdr_stop(host, session, binary="~/.local/bin/herdr"):
    return ssh(host, f"{binary} session stop {session}", check=False)


def launch_waf(host=KINDLE_HOST):
    """Open the kmux WAF, detached: it must not die with the ssh session."""
    ssh(host, "setsid /var/local/kmc/bin/kpm launch kmux "
              ">/mnt/us/kmux/var/launch_ssh.txt 2>&1 </dev/null &", check=False)
    time.sleep(8)
