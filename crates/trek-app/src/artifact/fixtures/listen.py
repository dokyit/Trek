#!/usr/bin/env python3
"""Hears what a page gets out of its sandbox (interactive-artifacts.md, "What a page can reach").

    python3 listen.py [seconds]

`mock:outbound` answers with a page that tries every way a page has of reaching the network, each
to a port of its own on this machine (47700 + the way's number, over https, so a knock is a TCP
connection and never a finished request), with WebRTC's STUN on UDP, and a video from the web on
47759. This listens on all of them for `seconds` (default 20) and prints the ports that were
knocked on, as JSON: `{"knocked": {"47700": "tcp", ...}}`. Nothing is answered.
"""

import json
import select
import socket
import sys
import time

BASE, COUNT = 47700, 60
seconds = float(sys.argv[1]) if len(sys.argv) > 1 else 20.0
socks = {}
for port in range(BASE, BASE + COUNT):
    for kind in (socket.SOCK_STREAM, socket.SOCK_DGRAM):
        s = socket.socket(socket.AF_INET, kind)
        s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        try:
            s.bind(("127.0.0.1", port))
        except OSError as e:
            print(f"port {port} is taken: {e}", file=sys.stderr)
            sys.exit(2)
        if kind == socket.SOCK_STREAM:
            s.listen(16)
        socks[s] = (port, "tcp" if kind == socket.SOCK_STREAM else "udp")
print("listening", flush=True)
knocked = {}
until = time.time() + seconds
while time.time() < until:
    ready, _, _ = select.select(list(socks), [], [], 0.25)
    for s in ready:
        port, kind = socks[s]
        if kind == "tcp":
            conn, _ = s.accept()
            conn.close()
        else:
            s.recvfrom(2048)
        knocked.setdefault(str(port), kind)
print(json.dumps({"knocked": dict(sorted(knocked.items()))}))
