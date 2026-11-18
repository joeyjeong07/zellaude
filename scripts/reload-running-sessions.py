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
import time
from pathlib import Path

ZELLIJ = shutil.which("zellij")
ATTACH_SETTLE_SECONDS = 10
RELOAD_SETTLE_SECONDS = 15
SLOT_REBUILD_SECONDS = 25
COMMAND_TIMEOUT_SECONDS = 10
DETACH_TIMEOUT_SECONDS = 5
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
                             capture_output=True, text=True, timeout=COMMAND_TIMEOUT_SECONDS).stdout
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
        self.is_open = True

    def fileno(self):
        return self.fd

    def detach(self):
        os.close(self.fd)
        os.kill(self.pid, signal.SIGHUP)
        deadline = time.monotonic() + DETACH_TIMEOUT_SECONDS
        while os.waitpid(self.pid, os.WNOHANG) == (0, 0):
            if time.monotonic() > deadline:
                os.kill(self.pid, signal.SIGKILL)
                os.waitpid(self.pid, 0)
                return
            time.sleep(0.1)


def wait_draining(clients, seconds):
    """Waits `seconds`, discarding every client's screen output so none blocks on a full pty."""
    deadline = time.monotonic() + seconds
    while (remaining := deadline - time.monotonic()) > 0:
        readable = [client for client in clients if client.is_open]
        if not readable:
            time.sleep(remaining)
            return
        for client in select.select(readable, [], [], min(remaining, 1.0))[0]:
            try:
                client.is_open = bool(os.read(client.fd, 65536))
            except OSError:
                client.is_open = False


def main():
    if ZELLIJ is None:
        sys.exit("zellij is not on PATH")
    plans = {}
    for session in live_session_names():
        facts = read_session_metadata(session)
        if facts is None:
            print(f"{session}: skipped, no session metadata to size temporary clients by")
        elif not facts["plugin_urls"]:
            print(f"{session}: skipped, does not run zellaude")
        else:
            plans[session] = facts
    if not plans:
        return
    temporary = []
    try:
        detached = [session for session, facts in plans.items() if facts["attached"] == 0]
        if detached:
            print(f"Attaching a temporary client to {len(detached)} detached session(s), {ATTACH_SETTLE_SECONDS} s...")
            for session in detached:
                temporary.append(TemporaryClient(session, plans[session]["columns"], plans[session]["rows"]))
            wait_draining(temporary, ATTACH_SETTLE_SECONDS)
        print(f"Reloading zellaude in {len(plans)} session(s), {RELOAD_SETTLE_SECONDS} s...")
        for session, facts in plans.items():
            for url in facts["plugin_urls"]:
                # A reload upgrades only the attached clients' copies, and a detached session ignores it.
                try:
                    subprocess.run([ZELLIJ, "-s", session, "action", "start-or-reload-plugin", url], env=CLEAN_ENV,
                                   capture_output=True, timeout=COMMAND_TIMEOUT_SECONDS)
                except subprocess.TimeoutExpired:
                    print(f"{session}: the reload command did not return within {COMMAND_TIMEOUT_SECONDS} s")
        wait_draining(temporary, RELOAD_SETTLE_SECONDS)
        # Copies kept for clients that left are rebuilt, from the module the reload just loaded, by whichever
        # client takes their slot next: fill every slot ever used, then let go.
        leftover_slots = {session: facts["client_slots"] - max(facts["attached"], 1) for session, facts in plans.items()}
        if any(leftover_slots.values()):
            print(f"Rebuilding {sum(leftover_slots.values())} leftover client slot(s), {SLOT_REBUILD_SECONDS} s...")
            for session, count in leftover_slots.items():
                for _ in range(count):
                    temporary.append(TemporaryClient(session, max(plans[session]["columns"], LARGE_COLUMNS),
                                                     max(plans[session]["rows"], LARGE_ROWS)))
            wait_draining(temporary, SLOT_REBUILD_SECONDS)
    finally:
        for client in temporary:
            client.detach()
    for session in plans:
        print(f"{session}: reloaded, {leftover_slots[session]} leftover client slot(s) rebuilt")
    print("Work the old build had already queued keeps running; CPU settles within a few minutes.")


if __name__ == "__main__":
    main()
