"""Metadata-only tmux discovery. Never attach, create, resize, or send input."""
import json
import subprocess
import sys

tmux = ["tmux"] + (["-L", sys.argv[1]] if sys.argv[1] else [])


def rows(command, fields, formats, empty_ok=False):
    result = subprocess.run(tmux + command + ["-F", "\t".join(formats)],
                            capture_output=True, text=True, timeout=5)
    if result.returncode:
        if empty_ok and ("no server running" in result.stderr or
                         "No such file or directory" in result.stderr):
            return []
        raise RuntimeError("tmux metadata unavailable")
    values = []
    for line in result.stdout.splitlines():
        parts = line.split("\t", len(fields) - 1)
        if len(parts) == len(fields):
            values.append(dict(zip(fields, parts)))
    return values


sessions = rows(["list-sessions"], ["id", "name"],
                ["#{session_id}", "#{session_name}"], empty_ok=True)
tabs = rows(["list-windows", "-a"], ["session", "id", "name"],
            ["#{session_id}", "#{window_id}", "#{window_name}"]) if sessions else []
panes = rows(["list-panes", "-a"], ["session", "tab", "id", "command", "cwd"],
             ["#{session_id}", "#{window_id}", "#{pane_id}",
              "#{pane_current_command}", "#{pane_current_path}"]) if sessions else []
print(json.dumps(dict(sessions=sessions, tabs=tabs, panes=panes)))
