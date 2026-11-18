#!/usr/bin/env python3
"""Load the installed zellaude build into every running Zellij session on this machine.

Usage: python3 scripts/reload-running-sessions.py
       curl -fsSL https://raw.githubusercontent.com/joeyjeong07/zellaude/main/scripts/reload-running-sessions.py | python3 -
"""
import fcntl
import os
import platform
import pty
import re
import select
import shutil
import signal
import struct
import subprocess
import sys
import termios
import threading
import time
from pathlib import Path

ZELLIJ = shutil.which("zellij")
ATTACH_SETTLE_SECONDS = 10
RELOAD_SETTLE_SECONDS = 15
SLOT_REBUILD_SECONDS = 25
RELOAD_TIMEOUT_SECONDS = 60
# Zellij sizes a session to its smallest client, so a temporary client this large never shrinks anyone's panes.
LARGE_COLUMNS, LARGE_ROWS = 500, 200


def zellij_cache_dir():
    if platform.system() == "Darwin":
        return Path.home() / "Library/Caches/org.Zellij-Contributors.Zellij"
    return Path(os.environ.get("XDG_CACHE_HOME") or Path.home() / ".cache") / "zellij"


SESSION_INFO_DIR = zellij_cache_dir() / "contract_version_1" / "session_info"
# Inside a pane these would point every command at that pane's session and block attaching.
CLEAN_ENV = {k: v for k, v in os.environ.items() if k not in ("ZELLIJ", "ZELLIJ_SESSION_NAME", "ZELLIJ_PANE_ID")}


def live_session_names():
    listing = subprocess.run([ZELLIJ, "list-sessions", "--no-formatting"], env=CLEAN_ENV,
                             capture_output=True, text=True).stdout
    return [line.split(" [Created")[0] for line in listing.splitlines() if line and "EXITED" not in line]


def read_session_metadata(session):
    """Zellaude plugin URLs, attached clients, client slots ever used and screen size; None without metadata."""
    try:
        metadata = (SESSION_INFO_DIR / session / "session-metadata.kdl").read_text()
    except OSError:
        return None
    attached = re.search(r"^\s*connected_clients (\d+)", metadata, re.M)
    columns = re.search(r"^\s*display_area_columns (\d+)", metadata, re.M)
    rows = re.search(r"^\s*display_area_rows (\d+)", metadata, re.M)
    if not (attached and columns and rows):
        return None
    pane_history = metadata.partition("\npane_history {")[2].partition("\n}")[0]
    slots = [int(slot) for slot in re.findall(r"^\s*id (\d+)$", pane_history, re.M)]
    return {
        "plugin_urls": sorted(set(re.findall(r'plugin_url "([^"]*zellaude[^"]*\.wasm)"', metadata))),
        "attached": int(attached.group(1)),
        "client_slots": max(slots + [int(attached.group(1)), 1]),
        "columns": int(columns.group(1)),
        "rows": int(rows.group(1)),
    }


class TemporaryClient:
    """An attached client nobody sees; its size is set before Zellij starts, so the server never sees a default."""

    def __init__(self, session, columns, rows):
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            try:
                fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", rows, columns, 0, 0))
                os.execve(ZELLIJ, [ZELLIJ, "attach", session], CLEAN_ENV)
            finally:
                os._exit(127)
        threading.Thread(target=self._discard_output, daemon=True).start()

    def _discard_output(self):
        while True:
            try:
                if select.select([self.fd], [], [], 1.0)[0] and not os.read(self.fd, 65536):
                    return
            except OSError:
                return

    def detach(self):
        os.close(self.fd)
        os.kill(self.pid, signal.SIGHUP)
        os.waitpid(self.pid, 0)


def reload_session(session):
    facts = read_session_metadata(session)
    if facts is None:
        return f"{session}: skipped, no session metadata to size temporary clients by"
    if not facts["plugin_urls"]:
        return f"{session}: skipped, does not run zellaude"
    temporary = []
    try:
        if facts["attached"] == 0:
            temporary.append(TemporaryClient(session, facts["columns"], facts["rows"]))
            time.sleep(ATTACH_SETTLE_SECONDS)
        for url in facts["plugin_urls"]:
            # A reload upgrades only the attached clients' copies, and a detached session ignores it.
            subprocess.run([ZELLIJ, "-s", session, "action", "start-or-reload-plugin", url],
                           env=CLEAN_ENV, timeout=RELOAD_TIMEOUT_SECONDS, capture_output=True)
        time.sleep(RELOAD_SETTLE_SECONDS)
        # Copies kept for clients that left are rebuilt, from the module the reload just loaded, by whichever
        # client takes their slot next: fill every slot ever used, then let go.
        leftover_slots = facts["client_slots"] - max(facts["attached"], 1)
        for _ in range(leftover_slots):
            temporary.append(TemporaryClient(session, max(facts["columns"], LARGE_COLUMNS),
                                             max(facts["rows"], LARGE_ROWS)))
        if leftover_slots:
            time.sleep(SLOT_REBUILD_SECONDS)
    except subprocess.TimeoutExpired:
        return f"{session}: reload timed out after {RELOAD_TIMEOUT_SECONDS} s"
    finally:
        for client in temporary:
            client.detach()
    return f"{session}: reloaded, {leftover_slots} leftover client slot(s) rebuilt"


def main():
    if ZELLIJ is None:
        sys.exit("zellij is not on PATH")
    sessions = live_session_names()
    results = {}
    workers = [threading.Thread(target=lambda s=s: results.__setitem__(s, reload_session(s))) for s in sessions]
    for worker in workers:
        worker.start()
    for worker in workers:
        worker.join()
    for session in sessions:
        print(results[session])
    if sessions:
        print("Work the old build had already queued keeps running; CPU settles within a few minutes.")


if __name__ == "__main__":
    main()
