"""Private SSH stdio transport for the Herdr socket and directory listings.

No pane commands are executed by the shell. Requests and replies are JSON lines.
Python 3 is required on the configured machine; no helper installation is needed.
"""
import json
import os
import shutil
import socket
import subprocess
import sys
import select
import threading
import time

path = os.path.expanduser(sys.argv[1])
start_session = sys.argv[3] if len(sys.argv) > 3 else ""
output_lock = threading.Lock()
started = threading.Event()
pane_lock = threading.Lock()
pane_ids = ()


def emit(value):
    # RPC replies and invalidations share stdout, but never a partial JSON line.
    with output_lock:
        print(json.dumps(value), flush=True)


def rpc(request):
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(5)
        connection.connect(path)
        connection.sendall((json.dumps(request) + "\n").encode())
        with connection.makefile("rb") as reader:
            return json.loads(reader.readline(8 * 1024 * 1024))


def ensure_server():
    """Start the configured persistent Herdr server if its socket is absent."""
    if not start_session:
        return  # An explicit socket override is externally managed.
    try:
        reply = rpc({"id": "kmux-start-check", "method": "ping", "params": {}})
        if "error" not in reply:
            return
    except Exception:
        pass

    executable = shutil.which("herdr")
    if not executable:
        candidate = os.path.expanduser("~/.local/bin/herdr")
        if os.access(candidate, os.X_OK):
            executable = candidate
    if not executable:
        raise RuntimeError("Herdr is unavailable and no herdr executable was found")

    # Start a detached server process. It intentionally outlives this SSH
    # bridge so disconnecting kmux does not stop a persistent Herdr session.
    try:
        subprocess.Popen([executable, "--session", start_session, "server"],
                         stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                         stderr=subprocess.DEVNULL, start_new_session=True,
                         close_fds=True)
    except Exception as error:
        raise RuntimeError("could not start Herdr server: " + str(error))

    deadline = time.monotonic() + 12
    while time.monotonic() < deadline:
        try:
            reply = rpc({"id": "kmux-start-check", "method": "ping", "params": {}})
            if "error" not in reply:
                return
        except Exception:
            pass
        time.sleep(0.2)
    raise RuntimeError("Herdr server did not become ready for session " + start_session)


def subscribe():
    # OutputMatched is edge-triggered, NOT a raw output stream. Keep terminal
    # capture polling; these events invalidate inventories and wake the client.
    kinds = ["workspace.created", "workspace.updated", "workspace.renamed",
             "workspace.closed", "workspace.moved", "workspace.focused",
             "tab.created", "tab.closed", "tab.renamed", "tab.moved", "tab.focused",
             "pane.created", "pane.closed", "pane.updated", "pane.focused",
             "pane.moved", "pane.exited", "pane.agent_detected", "layout.updated"]
    delay = 1
    while True:
        try:
            with socket.socket(socket.AF_UNIX) as connection:
                connection.settimeout(5)
                connection.connect(path)
                with pane_lock:
                    watched_panes = pane_ids
                subscriptions = [{"type": k} for k in kinds]
                # Agent-state subscriptions require an explicit pane ID. The
                # RPC bootstrap supplies topology, then a fresh acknowledgement
                # invalidates inventory again to close the subscription gap.
                subscriptions += [{"type": "pane.agent_status_changed", "pane_id": p}
                                  for p in watched_panes]
                request = {"id": "kmux-events", "method": "events.subscribe",
                           "params": {"subscriptions": subscriptions}}
                connection.sendall((json.dumps(request) + "\n").encode())
                # Unbuffered: select() must see every pending line in the socket.
                with connection.makefile("rb", buffering=0) as reader:
                    ack = json.loads(reader.readline(8 * 1024 * 1024))
                    if ack.get("result", {}).get("type") != "subscription_started":
                        raise ValueError("subscription rejected")
                    emit({"kmux_event": "ready"})
                    started.set()
                    delay = 1
                    health_at = time.monotonic()
                    while True:
                        with pane_lock:
                            if watched_panes != pane_ids:
                                break  # Reconfigure without leaking old streams.
                        if select.select([connection], [], [], 1)[0]:
                            line = reader.readline(8 * 1024 * 1024)
                            if not line:
                                raise EOFError("subscription closed")
                            event = json.loads(line)
                            if "event" in event:
                                emit({"kmux_event": "changed"})
                        if time.monotonic() - health_at >= 5:
                            pong = rpc({"id": "kmux-health", "method": "ping", "params": {}})
                            if "error" in pong:
                                raise ValueError("health check failed")
                            health_at = time.monotonic()
        except Exception:
            emit({"kmux_event": "offline"})
            started.set()
            time.sleep(delay)
            delay = min(delay * 2, 30)


ensure_server()

if len(sys.argv) > 2 and sys.argv[2] == "events":
    threading.Thread(target=subscribe, daemon=True).start()
    # Subscribe before bootstrap; failure permits RPC/polling fallback.
    started.wait(6)

for line in sys.stdin:
    try:
        request = json.loads(line)
        if request.get("method") == "kmux.files":
            directory = request["params"]["path"]
            if not os.path.isabs(directory):
                raise ValueError("absolute directory required")
            names = sorted(os.listdir(directory))
            result = {"cwd": directory, "truncated": len(names) > 500,
                      "files": [{"name": n, "dir": os.path.isdir(os.path.join(directory, n))}
                                for n in names[:500]]}
            reply = {"result": result}
        else:
            reply = rpc(request)
            if request.get("method") == "session.snapshot" and "result" in reply:
                result = reply["result"]
                snapshot = result.get("snapshot", result)
                with pane_lock:
                    pane_ids = tuple(sorted(p["pane_id"] for p in snapshot.get("panes", [])
                                            if p.get("pane_id")))
        emit(reply)
    except Exception as error:
        emit({"error": {"message": str(error)}})
