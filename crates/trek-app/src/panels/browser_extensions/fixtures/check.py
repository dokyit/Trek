#!/usr/bin/env python3
"""The Chrome APIs Trek adds to WebKit's extensions, live (macOS; browser.md, "Extensions").
Run from the checkout after `trek-dev start` and `trek-dev open tool:browser`:

    python3 crates/trek-app/src/panels/browser_extensions/fixtures/check.py

It installs `api/` (a small extension that calls each API from its background), puts a native
messaging host for it in the test profile's own `NativeMessagingHosts` (never the user's), drives
it through the driver's `control browser ext-*` commands, and ends with
"Extension APIs: all N checks passed." or the ones that failed (exit 1).
"""

import http.server
import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import threading
import time
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = subprocess.run(
    ["git", "-C", HERE, "rev-parse", "--show-toplevel"], capture_output=True, text=True, check=True
).stdout.strip()
DEV = [os.path.join(ROOT, ".agents/skills/verify-trek/scripts/trek-dev")]
# The fixture's id, from its manifest's `key`.
ID = "mlckmdcniommikmhhhiodejcljnoifmh"
HOST = "com.trek.fixture_echo"

results = []


def dev(*args):
    out = subprocess.run([*DEV, *args], capture_output=True, text=True)
    if out.returncode != 0:
        raise RuntimeError(f"trek-dev {' '.join(args)}: {out.stdout.strip()} {out.stderr.strip()}")
    return out.stdout


STATUS = json.loads(dev("status"))
SHOT = os.path.join(STATUS["runtime_dir"], "shot")
DATA = STATUS["data_dir"]


def control(*args):
    dev("control", "--timeout", "240", "browser", *args)


def ext(js):
    """`js` in the fixture's own page: its answer (`{ok}` or `{error}`)."""
    path = os.path.join(SHOT, "browser-ext-eval.txt")
    if os.path.exists(path):
        os.remove(path)
    control("ext-eval", ID, js)
    with open(path) as f:
        return json.load(f)


def reply(command, arg=None):
    """The background's whole reply to `command`: `{ok, origin}` or `{error}`."""
    answer = ext(f"run({json.dumps(command)}, {json.dumps(arg)})")
    return answer.get("ok") if isinstance(answer.get("ok"), dict) else answer


def run(command, arg=None):
    """What `command` came to (the reply's `ok`), or the reply when it failed."""
    r = reply(command, arg)
    return r["ok"] if "ok" in r else r


def start(command, arg=None):
    """Start `command` without waiting (it waits on the user): `finish` reads its answer."""
    ext(f"(window.__{command} = undefined, run({json.dumps(command)}, {json.dumps(arg)}).then(v => window.__{command} = ('ok' in v ? v.ok : v)), 'started')")


def finish(command, seconds=30):
    for _ in range(int(seconds * 2)):
        got = ext(f"window.__{command} === undefined ? 'waiting' : window.__{command}").get("ok")
        if got != "waiting":
            return got
        time.sleep(0.5)
    return "waiting"


def check(name, ok, detail=""):
    results.append((name, bool(ok), detail))
    print(("ok   " if ok else "FAIL ") + name + (f": {detail}" if detail and not ok else ""))


def notifications():
    control("ext-notifications")
    with open(os.path.join(SHOT, "browser-ext-notifications.json")) as f:
        return json.load(f)


class Site(http.server.BaseHTTPRequestHandler):
    """`/auth`: a sign-in page that sends the browser back to the extension's redirect address."""

    def do_GET(self):
        if self.path.startswith("/auth"):
            self.send_response(302)
            self.send_header("Location", f"https://{ID}.chromiumapp.org/cb?code=42")
            self.end_headers()
            return
        body = b"<!doctype html><title>fixture tab</title><p>tab"
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


def main():
    # A free port of the system's choosing (other checks may hold fixed ones).
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Site)
    port = server.server_address[1]
    threading.Thread(target=server.serve_forever, daemon=True).start()

    # The host, in the test profile's own folder (Trek looks nowhere else in a test profile).
    scratch = tempfile.mkdtemp(prefix="trek-ext-fixture-")
    program = os.path.join(scratch, "echo_host.py")
    shutil.copy(os.path.join(HERE, "api", "echo_host.py"), program)
    os.chmod(program, os.stat(program).st_mode | stat.S_IXUSR)
    hosts = os.path.join(DATA, "NativeMessagingHosts")
    os.makedirs(hosts, exist_ok=True)
    with open(os.path.join(hosts, f"{HOST}.json"), "w") as f:
        json.dump({"name": HOST, "description": "Trek fixture echo", "path": program, "type": "stdio",
                   "allowed_origins": [f"chrome-extension://{ID}/"]}, f)

    # Installed from a .zip, as from the store: Trek unpacks it and adds its shim.
    package = os.path.join(scratch, "api.zip")
    with zipfile.ZipFile(package, "w") as z:
        for name in os.listdir(os.path.join(HERE, "api")):
            z.write(os.path.join(HERE, "api", name), name)
    control("ext-install", package)
    time.sleep(3)

    shim = run("shim")
    check("APIs WebKit lacks are there", shim.get("notifications") == "object" and shim.get("offscreen") == "object"
          and shim.get("identity") == "object" and shim.get("idle") == "object" and shim.get("downloads") == "object"
          and shim.get("managed") == "object" and shim.get("world") == "MAIN" and shim.get("history") == "object", shim)

    answer = reply("idle")
    check("a message's sender has an origin", answer.get("origin") == f"webkit-extension://{ID}", answer)

    check("notifications.create answers its id", run("notify") == "n1")
    shown = notifications()["notifications"]
    mine = [n for n in shown if n["extension"] == ID]
    check("Trek shows it, named for the extension, with its buttons",
          len(mine) == 1 and mine[0]["title"] == "Trek API Fixture: Fixture says hi"
          and mine[0]["buttons"] == ["Yes", "No"], mine)
    check("getAll lists it", run("all") == {"n1": True})
    control("ext-notify", "click", ID, "n1", "1")
    control("ext-notify", "click", ID, "n1")
    time.sleep(1)
    events = run("events")
    check("a button's click reaches onButtonClicked", {"event": "button", "id": "n1", "button": 1} in events, events)
    check("the banner's click reaches onClicked", {"event": "clicked", "id": "n1"} in events, events)
    check("clear takes it down", run("clear") is True and not [n for n in notifications()["notifications"] if n["extension"] == ID])

    check("an offscreen page parses HTML for the background", run("offscreen") == "parsed")
    check("idle.queryState answers", run("idle") in ("active", "idle", "locked"))

    start("native")
    time.sleep(1)
    control("ext-allow")
    got = finish("native")
    check("sendNativeMessage reaches the host once allowed", got == {"echo": {"ping": 1}, "origin": f"chrome-extension://{ID}/"}, got)
    check("connectNative keeps a port to it (asked once)", run("port") == {"echo": {"port": 2}, "origin": f"chrome-extension://{ID}/"})

    downloaded = run("download")
    path = os.path.join(DATA, "downloads", "trek-fixture.txt")
    check("downloads.download saves to the profile's Downloads",
          isinstance(downloaded, int) and os.path.exists(path) and open(path).read() == "hello from the fixture", downloaded)

    check("getRedirectURL names its chromiumapp.org address", run("redirect") == f"https://{ID}.chromiumapp.org/cb")
    start("auth", f"http://127.0.0.1:{port}/auth")
    got = finish("auth")
    check("launchWebAuthFlow ends at the redirect and hands it back", got == f"https://{ID}.chromiumapp.org/cb?code=42", got)

    tab = run("tab", f"http://127.0.0.1:{port}/tab")
    check("tabs.create answers the new tab", isinstance(tab, dict) and isinstance(tab.get("id"), int), tab)

    control("ext-remove", ID)
    server.shutdown()
    shutil.rmtree(scratch, ignore_errors=True)
    failed = [r for r in results if not r[1]]
    if failed:
        print(f"Extension APIs: {len(failed)} of {len(results)} checks failed.")
        sys.exit(1)
    print(f"Extension APIs: all {len(results)} checks passed.")


if __name__ == "__main__":
    main()
