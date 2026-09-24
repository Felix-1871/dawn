#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Logs in on a QEMU serial console and reports on the installed system.

qemu-run.sh's verify-login serves the VM's serial console on a Unix
socket. This waits for the login prompt, logs in as the new user with
the password the GUI was given (so a wrong password from step 8 fails
here), and runs one command that prints

    DAWN-E2E-LOGGED-IN secure_boot=<value>

where <value> is the firmware's SecureBoot variable: 1 when Secure Boot
is enforcing, 0 when it isn't, empty when the firmware has none. That
line goes to stdout, for run-e2e.sh.

Usage: serial-login.py <socket> <user> <password> <timeout-seconds>
"""

import socket
import sys
import time

EFI_GLOBAL = "8be4df61-93ca-11d2-aa0d-00e098032b8c"
MARKER = b"DAWN-E2E-LOGGED-IN"
# printf's format keeps the marker out of the command line the console
# echoes back, so only the command's output can match it.
COMMAND = (
    "printf 'DAWN-E2E-%s secure_boot=%s\\n' LOGGED-IN "
    f'"$(od -An -tu1 -j4 -N1 /sys/firmware/efi/efivars/SecureBoot-{EFI_GLOBAL} '
    "2>/dev/null | tr -d ' ')\""
)


class Console:
    def __init__(self, path, timeout):
        self.deadline = time.monotonic() + timeout
        self.seen = b""
        while True:
            self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            try:
                self.sock.connect(path)
                return
            except (FileNotFoundError, ConnectionRefusedError):
                self.sock.close()
                self.give_up_if_late("the serial console's socket")
                time.sleep(0.5)

    def give_up_if_late(self, what):
        if time.monotonic() > self.deadline:
            tail = self.seen[-2000:].decode(errors="replace")
            sys.exit(f"error: timed out waiting for {what}; the console last showed:\n{tail}")

    def read_more(self, what):
        self.give_up_if_late(what)
        self.sock.settimeout(max(0.1, self.deadline - time.monotonic()))
        try:
            chunk = self.sock.recv(4096)
        except socket.timeout:
            return
        if not chunk:
            sys.exit("error: the serial console closed")
        self.seen += chunk

    def wait_for(self, needle):
        """Reads until `needle` shows up in what arrives from now on."""
        start = len(self.seen)
        while needle not in self.seen[start:]:
            self.read_more(repr(needle.decode()))

    def wait_for_line(self, marker):
        """Reads until a whole line starting with `marker` has arrived."""
        while True:
            index = self.seen.rfind(marker)
            if index != -1:
                end = self.seen.find(b"\n", index)
                if end != -1:
                    return self.seen[index:end].decode(errors="replace").strip()
            self.read_more(repr(marker.decode()))

    def type(self, text):
        self.sock.sendall(text.encode() + b"\r")


def main():
    if len(sys.argv) != 5:
        sys.exit(__doc__)
    path, user, password, timeout = sys.argv[1:]
    console = Console(path, float(timeout))
    console.wait_for(b"login: ")
    console.type(user)
    console.wait_for(b"Password: ")
    console.type(password)
    console.wait_for(b"$ ")
    console.type(COMMAND)
    print(console.wait_for_line(MARKER), flush=True)
    console.type("exit")


if __name__ == "__main__":
    main()
