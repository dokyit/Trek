#!/usr/bin/env python3
"""The Browser's popup, sign-in and asks check (macOS), against isolated Trek (browser.md,
"Popups, sign-in and asks"). Run from the checkout after `trek-dev start` and
`trek-dev open tool:browser`:

    python3 crates/trek-app/src/panels/browser/fixtures/check.py [port]

It serves this folder's pages (serve.py), drives the Browser through them with the driver's
`control browser` commands, checks each step, and ends with "Browser popups: all N checks passed."
or the ones that failed (exit 1).
"""

import json
import os
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = subprocess.run(
    ["git", "-C", HERE, "rev-parse", "--show-toplevel"], capture_output=True, text=True, check=True
).stdout.strip()
DEV = os.path.join(ROOT, ".agents/skills/verify-trek/scripts/trek-dev")
PORT = int(sys.argv[1]) if __name__ == "__main__" and len(sys.argv) > 1 else 47615
SITE = f"http://127.0.0.1:{PORT}"
IDP = f"http://localhost:{PORT}"

results = []


def dev(*args):
    out = subprocess.run([DEV, *args], capture_output=True, text=True)
    if out.returncode != 0:
        raise RuntimeError(f"trek-dev {' '.join(args)}: {out.stdout.strip()} {out.stderr.strip()}")
    return out.stdout


SHOT = os.path.join(json.loads(dev("status"))["runtime_dir"], "shot")


def control(*args):
    # Generous: on a Mac under heavy load a page can take most of a minute to answer.
    dev("control", "--timeout", "240", "browser", *args)


def state():
    control("state")
    with open(os.path.join(SHOT, "browser.json")) as f:
        return json.load(f)


def js(expr):
    """The page in front's answer to `expr` (wry hands it over as JSON)."""
    control("eval", expr)
    with open(os.path.join(SHOT, "browser-eval.json")) as f:
        raw = f.read().strip()
    try:
        return json.loads(raw) if raw else None
    except json.JSONDecodeError:
        return raw


def check(name, ok, detail=""):
    results.append((name, bool(ok), detail))
    stamp = time.strftime("%H:%M:%S", time.gmtime())
    print(stamp + " " + ("ok   " if ok else "FAIL ") + name + ("" if ok else f"  ({detail})"), flush=True)


def wait(what, poll, timeout=40):
    """`poll()`'s first truthy answer within `timeout` seconds (else its last)."""
    end = time.monotonic() + timeout
    last = None
    while time.monotonic() < end:
        try:
            last = poll()
        except RuntimeError as e:
            last = None
            err = e
        if last:
            return last
        time.sleep(0.5)
    return last


def front():
    s = state()
    return next((t for t in s["all_tabs"] if t["id"] == s["active"]), None), s


def main():
    server = subprocess.Popen([sys.executable, os.path.join(HERE, "serve.py"), str(PORT)],
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        time.sleep(0.6)
        run()
    finally:
        server.terminate()
    failed = [r for r in results if not r[1]]
    if failed:
        print(f"Browser popups: {len(failed)} of {len(results)} checks failed: "
              + "; ".join(r[0] for r in failed))
        sys.exit(1)
    print(f"Browser popups: all {len(results)} checks passed.")


def run():
    # 1. A Google-shaped sign-in: a sized popup on another origin that posts back and closes.
    control("go", f"{SITE}/signin.html")
    wait("page", lambda: js("document.readyState") == "complete" and js("typeof signIn") == "function")
    opener_id = state()["active"]
    check("the popup opens (window.open returns a window)", js("signIn()") is True)
    tab = wait("popup", lambda: (lambda t: t if t and t[0]["id"] != opener_id else None)(front()))
    popup, s = tab if tab else (None, state())
    check("it's a tab of its own, in front", popup is not None, s.get("all_tabs"))
    if popup is None:
        return
    check("its opener is the sign-in page", popup["opener"] == opener_id, popup)
    check("it's a sheet at the size asked for", popup["sheet"] == [500, 600], popup)
    idp = wait("idp", lambda: (lambda v: v if isinstance(v, dict) and v.get("safari") is not None else None)(js("window.__idp")))
    check("the popup has window.opener", isinstance(idp, dict) and idp.get("opener") is True, idp)
    check("navigator.webdriver is false", isinstance(idp, dict) and idp.get("webdriver") is False, idp)
    check("its user agent is Safari's", isinstance(idp, dict) and idp.get("safari") is True, idp and idp.get("ua"))
    # Google's last step: the form POST to /gsi/transform, which posts to the opener and closes.
    js("setTimeout(() => document.getElementById('f').submit(), 0), true")
    closed = wait("closed", lambda: (lambda s: s if not any(t["id"] == popup["id"] for t in s["all_tabs"]) else None)(state()))
    check("window.close() closes the popup", closed is not None)
    check("the opener comes back to the front", closed is not None and closed["active"] == opener_id, closed and closed["active"])
    got = wait("credential", lambda: (lambda v: v if isinstance(v, dict) and v.get("credential") else None)(js("window.__check")))
    check("the credential reached the opener", isinstance(got, dict) and got.get("credential") == "test-credential-123", got)
    check("…from the identity provider's origin", isinstance(got, dict) and got.get("from") == IDP, got)
    check("the opener saw popup.closed", isinstance(got, dict) and got.get("closed") is True, got)

    # 2. Tabs, written popups, links for apps, downloads, dialogs.
    control("go", f"{SITE}/tabs.html")
    wait("page", lambda: js("typeof written") == "function")
    home = state()["active"]
    js("setTimeout(() => document.getElementById('blank').click(), 0), true")
    t = wait("blank tab", lambda: (lambda f: f if f[0] and f[0]["id"] != home else None)(front()))
    check("target=_blank opens a tab (not a sheet)", t and t[0]["opener"] == home and t[0]["sheet"] is None, t and t[0])
    if t:
        loaded = wait("loaded", lambda: js("document.getElementById('ok') ? String(!!window.opener) : ''"))
        check("…which loads a page that refuses frames, without window.opener (implied noopener)", loaded == "false", loaded)
        js("window.close(), true")
        back = wait("back", lambda: (lambda s: s if s["active"] == home else None)(state()))
        check("…and closes itself", back is not None)
    js("setTimeout(() => document.getElementById('withopener').click(), 0), true")
    t = wait("opener tab", lambda: (lambda f: f if f[0] and f[0]["id"] != home else None)(front()))
    if t:
        loaded = wait("loaded", lambda: js("document.getElementById('ok') ? String(!!window.opener) : ''"))
        check("target=_blank rel=opener keeps window.opener", loaded == "true", loaded)
        js("window.close(), true")
        wait("back", lambda: (lambda s: s if s["active"] == home else None)(state()))
    else:
        check("target=_blank rel=opener opens a tab", False)
    js("setTimeout(() => document.getElementById('written').click(), 0), true")
    t = wait("written", lambda: (lambda f: f if f[0] and f[0]["id"] != home else None)(front()))
    check("window.open() opens a blank tab", t is not None)
    if t:
        text = wait("written text", lambda: js("document.getElementById('w') && document.getElementById('w').textContent"))
        check("…that its opener writes into", text == "Written by its opener", text)
        js("window.close(), true")
        wait("back", lambda: (lambda s: s if s["active"] == home else None)(state()))

    # Each ask is waited for by its own words, and whatever an earlier step left is answered
    # first: on a loaded Mac an ask can come long after the click.
    def clear_asks():
        while state()["asks"]:
            control("ask", "no")

    def expect_ask(words, timeout=90):
        return wait(words, lambda: next((a for a in state()["asks"] if words in a["headline"] + (a["message"] or "")), None), timeout)

    def click(id):
        js(f"setTimeout(() => document.getElementById('{id}').click(), 0), true")

    clear_asks()
    click("mail")
    check("mailto: asks to open Mail", expect_ask("wants to open Mail"), "no ask")
    clear_asks()
    click("app")
    check("a scheme no app opens says so", expect_ask("no app on this Mac"), "no ask")
    clear_asks()
    click("toapp")
    ask = expect_ask("trek-test-app: link")
    check("a redirect to an app's scheme asks too (no error page)", ask and not state()["failed"], ask)
    clear_asks()

    # A page held by a dialog runs no script, so nothing is read from it until the bar answers.
    click("alert")
    ask = expect_ask("Hello from the page")
    check("alert() waits on the bar", ask and ask["blocks_page"], ask)
    if not ask:
        return
    control("ask", "yes")
    check("…and the page goes on once it's answered", wait("alert done", lambda: js("window.__check.alert")) == "done")
    click("confirm")
    if not expect_ask("Leave this page?"):
        check("confirm() waits on the bar", False)
        return
    control("ask", "yes")
    check("confirm() answers what the bar said", wait("confirm done", lambda: js("window.__check.confirm") is True) is True)
    click("prompt")
    if not expect_ask("Your name?"):
        check("prompt() waits on the bar", False)
        return
    control("ask", "text", "Grace")
    check("prompt() gets the bar's text", wait("prompt done", lambda: js("window.__check.prompt")) == "Grace")
    # Camera and microphone go through macOS's own permission for the app first, which for the
    # driver's bare binary (no Info.plist) can take a minute: opt in with TREK_CHECK_CAMERA=1.
    if os.environ.get("TREK_CHECK_CAMERA") == "1" and js("!!navigator.mediaDevices") is True:
        click("camera")
        ask = expect_ask("camera and microphone", timeout=120)
        check("camera and microphone ask on the bar", ask, "no ask")
        if ask:
            control("ask", "no")
            media = wait("media", lambda: js("window.__check.media"))
            check("…and Don't Allow reaches the page", media == "NotAllowedError", media)
    else:
        print("skip camera and microphone (TREK_CHECK_CAMERA=1 to run it)")
    click("notify")
    note = wait("notifications", lambda: js("window.__check.notifications"))
    check("notifications are denied quietly", note == "denied" and not state()["asks"], note)

    # The download lands in the real ~/Downloads (WebKit's, as wry sets it): removed again here.
    path = os.path.expanduser("~/Downloads/trek-download-check.txt")
    if os.path.exists(path):
        os.remove(path)
    js("setTimeout(() => location.href = '/download', 0), true")
    time.sleep(2.5)
    s = state()
    check("a redirect to a download leaves the page as it was", s["url"].endswith("/tabs.html") and not s["failed"], (s["url"], s["failed"]))
    check("…and the file is in Downloads", os.path.exists(path), path)
    if os.path.exists(path):
        os.remove(path)

    control("go", "https://nope.invalid/")
    s = wait("error", lambda: (lambda s: s if s["failed"] else None)(state()), timeout=20)
    check("a host that doesn't exist shows Trek's error page", s is not None)
    why = s and wait("why", lambda: js("document.getElementById('failed-why').textContent"))
    check("…saying why", isinstance(why, str) and "can’t find the server at nope.invalid" in why, why)


if __name__ == "__main__":
    main()
