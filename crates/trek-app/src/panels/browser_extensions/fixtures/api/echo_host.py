#!/usr/bin/env python3
"""A native messaging host for the fixture: answers each message with it and the origin it was
started for (Chrome's framing: a 4-byte length in native order, then UTF-8 JSON)."""
import json
import struct
import sys

origin = sys.argv[1] if len(sys.argv) > 1 else ""
while True:
    head = sys.stdin.buffer.read(4)
    if len(head) < 4:
        break
    body = sys.stdin.buffer.read(struct.unpack("=I", head)[0])
    out = json.dumps({"echo": json.loads(body), "origin": origin}).encode()
    sys.stdout.buffer.write(struct.pack("=I", len(out)) + out)
    sys.stdout.buffer.flush()
