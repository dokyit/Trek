#!/usr/bin/env python3
"""The Browser's passkeys check (macOS), against isolated Trek (browser.md, "Passkeys"). Run from
the checkout after `trek-dev start` and `trek-dev open tool:browser`:

    python3 crates/trek-app/src/panels/browser/fixtures/passkeys_check.py [port]

It serves passkeys.html (a relying party in one page, which checks every answer as a server would
and verifies the signatures with WebCrypto) on http://localhost, drives the Browser through it
with the driver's `control browser` commands, and ends with "Browser passkeys: all N checks
passed." or the ones that failed (exit 1).

The isolated app's authenticator is the stand-in (`trek_passkeys::keys::StandIn`): keys in the
throwaway data folder, and `control browser passkey-uv allow|deny` in place of Touch ID. The real
one (the Secure Enclave, Touch ID) needs a person at the Mac, and no part of this check shows a
prompt.
"""

import os
import subprocess
import sys
import time

sys.argv, ARGV = sys.argv[:1], sys.argv[1:]
from check import HERE, check, control, dev, js, results, state, wait  # noqa: E402

PORT = int(ARGV[0]) if ARGV else 47616
SITE = f"http://localhost:{PORT}"
PAGE = f"{SITE}/passkeys.html"


def later(expr):
    """Start `expr` in the page without waiting on it (it waits on Trek's bar)."""
    return js(f"setTimeout(() => {{ {expr} }}, 0), true")


def asks():
    return (state().get("passkeys") or {}).get("asks") or []


def kept():
    return (state().get("passkeys") or {}).get("kept") or []


def bar(words, timeout=60):
    """The passkey request on the bar whose headline has `words`."""
    return wait(words, lambda: next((a for a in asks() if words in (a.get("headline") or "")), None), timeout)


def result(key, timeout=60):
    """What the page's own check of the answer came to, once it has one."""
    return wait(key, lambda: (lambda v: v if isinstance(v, dict) and not v.get("pending") else None)(
        js(f"window.__pk.{key}")), timeout)


def failed_checks(r):
    return [k for k, v in (r or {}).get("checks", {}).items() if not v]


def load(url=PAGE):
    control("go", url)
    wait("page", lambda: js("document.readyState") == "complete" and js("typeof register") == "function")


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
        print(f"Browser passkeys: {len(failed)} of {len(results)} checks failed: "
              + "; ".join(r[0] for r in failed))
        sys.exit(1)
    print(f"Browser passkeys: all {len(results)} checks passed.")


def run():
    control("passkeys", "on")
    control("passkey-uv", "allow")
    control("passkey-delete", "localhost")
    load()

    # 1. What a page sees.
    js("window.__pk.features = null, features(), true")
    f = wait("features", lambda: js("window.__pk.features"))
    check("the page has navigator.credentials and PublicKeyCredential",
          f and f.get("create") == "function" and f.get("PublicKeyCredential") == "function", f)
    check("a platform authenticator is reported (the check GitHub's banner turns on)",
          f and f.get("uvpaa") is True, f)
    check("conditional mediation is reported", f and f.get("conditional") is True, f)
    caps = (f or {}).get("capabilities") or {}
    check("the capabilities say what there is and isn't",
          isinstance(caps, dict) and caps.get("passkeyPlatformAuthenticator") is True
          and caps.get("hybridTransport") is False, caps)

    # 2. Register.
    heights = {"none": js("innerHeight")}
    later("register('ada@example.test')")
    ask = bar("On localhost: create a passkey for “ada@example.test”?")
    heights["create"] = js("innerHeight")
    check("creating a passkey asks on Trek's bar, naming the account and the site", ask, asks())
    check("…and says where it will be kept",
          ask and "on this Mac" in (ask.get("note") or "") and "iCloud Keychain" in (ask.get("note") or ""), ask)
    check("nothing is kept before the user says so", kept() == [], kept())
    control("passkey", "yes")
    r = result("register")
    check("the page gets a credential", r and r.get("ok") is True, r)
    check("…that a relying party's checks all pass on", r and not failed_checks(r), failed_checks(r))
    k = kept()
    check("Trek keeps one passkey, for localhost and that account",
          len(k) == 1 and k[0]["site"] == "localhost" and k[0]["account"] == "ada@example.test", k)

    # 3. Sign in.
    later("signIn()")
    ask = bar("On localhost: sign in as “ada@example.test”?")
    heights["one"] = js("innerHeight")
    check("signing in asks on the bar, naming the site and the account", ask, asks())
    control("passkey", "yes")
    r = result("signin")
    check("the page is signed in", r and r.get("ok") is True and r.get("as") == "ada@example.test", r)
    check("…and the signature verifies with the registered key (WebCrypto)",
          r and r.get("checks", {}).get("signature") is True and not failed_checks(r), r)
    check("the passkey says when it was last used", kept() and kept()[0].get("last_used"), kept())

    # 4. The site says the account has this passkey already.
    later("register('ada@example.test', { exclude: true })")
    ask = bar("Trek already has a passkey for this account")
    heights["already"] = js("innerHeight")
    check("a passkey the site excludes isn't made twice: the bar says so",
          ask and ask.get("yes") == ["OK"] and ask.get("no") == "Cancel", asks())
    control("passkey", "yes")
    r = result("register")
    check("…and the page gets InvalidStateError", r and r.get("error") == "InvalidStateError", r)
    check("…with still one passkey kept", len(kept()) == 1, kept())

    # 5. Two accounts: the bar asks which.
    later("register('bob@example.test')")
    bar("create a passkey for “bob@example.test”")
    control("passkey", "yes")
    check("a second account registers", (result("register") or {}).get("ok") is True)
    later("signIn()")
    ask = bar("Which account?")
    heights["two"] = js("innerHeight")
    check("with two passkeys the bar offers both accounts",
          ask and ask.get("yes") == ["ada@example.test", "bob@example.test"], asks())
    control("passkey", "yes 1")
    r = result("signin")
    check("…and signs in as the one chosen", r and r.get("ok") is True and r.get("as") == "bob@example.test", r)
    later("signIn({ named: 'ada@example.test' })")
    ask = bar("sign in as “ada@example.test”")
    check("a sign-in that names a credential offers only that one", ask and ask.get("yes") == ["Continue"], asks())

    # 6. The user says no: on the bar, and at Touch ID.
    control("passkey", "no")
    r = result("signin")
    check("Cancel on the bar is NotAllowedError", r and r.get("error") == "NotAllowedError", r)
    cancel_message = (r or {}).get("message")
    control("passkey-uv", "deny")
    later("signIn({ named: 'ada@example.test' })")
    bar("sign in as “ada@example.test”")
    control("passkey", "yes")
    r = result("signin")
    control("passkey-uv", "allow")
    check("a refused verification (Touch ID's stand-in) is NotAllowedError", r and r.get("error") == "NotAllowedError", r)

    # 7. A page can't name another site.
    for rp in ("example.com", "com", "ocalhost"):
        later(f"signIn({{ rpId: '{rp}', key: 'other' }})")
        r = result("other", 20)
        check(f"a sign-in claiming {rp} is SecurityError, with nothing on the bar",
              r and r.get("error") == "SecurityError" and asks() == [], (r, asks()))
    later("register('eve', { rpId: 'example.com', key: 'other' })")
    r = result("other", 20)
    check("so is registering for another site", r and r.get("error") == "SecurityError", r)

    # 8. The page takes a request back, and autofill's kind waits quietly.
    later("signInThenAbort(4000)")
    check("a request the page will take back shows first", bar("Which account?", 20), asks())
    r = result("signin", 30)
    check("…then the page aborts it: AbortError", r and r.get("error") == "AbortError", r)
    check("…and the bar goes", wait("bar gone", lambda: asks() == [], 15) is True, asks())
    later("signInThenAbort(600000, { mediation: 'conditional' })")
    offer = wait("offer", lambda: next((a for a in asks() if a.get("stage") == "offer"), None), 20)
    check("autofill's request is offered by the toolbar's key, with nothing on the bar",
          offer and offer.get("headline") is None, asks())
    check("…and the page is the size it was (it can't tell an offer from none)",
          js("innerHeight") == heights["none"], (js("innerHeight"), heights))
    control("passkey", "offer")
    ask = bar("Which account?", 20)
    check("pressing the key opens it on the bar, with Not Now", ask and ask.get("no") == "Not Now" and ask.get("conditional"), ask)
    control("passkey", "no")
    offer = wait("offer", lambda: next((a for a in asks() if a.get("stage") == "offer"), None), 15)
    check("Not Now puts it back on the key and leaves the page waiting", offer and js("window.__pk.signin.pending") is True, asks())
    later("window.__pk.controller.abort()")
    check("…until the page ends it", (result("signin", 20) or {}).get("error") == "AbortError")
    check("…and the key goes with it", wait("gone", lambda: asks() == [], 15) is True, asks())

    # 9. A page going away ends its request.
    later("signIn()")
    bar("Which account?", 20)
    load()
    check("a new page ends the old one's request", wait("gone", lambda: asks() == [], 15) is True, asks())

    # 10. What WebKit itself answers (a frame: Trek answers the page only), which is what every
    # page got before.
    js("window.__pk.features = null, features(frameWindow()), true")
    f = wait("frame", lambda: js("window.__pk.features"))
    check("a frame has WebKit's own answer: no platform authenticator",
          f and f.get("uvpaa") is False and f.get("conditional") is False, f)
    later("register('frame@example.test', { where: frameWindow(), key: 'frame' })")
    r = result("frame", 30)
    check("…and WebKit refuses its request (NotAllowedError), as it refused every page's",
          r and r.get("error") == "NotAllowedError" and asks() == [], (r, asks()))

    # 11. An address that is a number has no passkeys.
    load(f"http://127.0.0.1:{PORT}/passkeys.html")
    later("register('ip@example.test', { key: 'ip' })")
    r = result("ip", 20)
    check("a page on an IP address is refused (SecurityError)", r and r.get("error") == "SecurityError", r)

    # 12. Deleting, as Settings does.
    control("passkey-delete", "localhost")
    check("deleted passkeys are gone from Trek", kept() == [], kept())
    load()
    later("signIn()")
    ask = bar("Trek has no passkey for this site")
    heights["nothing"] = js("innerHeight")
    check("with none kept, a sign-in says so and how to get one", ask and "add a passkey" in (ask.get("note") or ""), asks())
    control("passkey", "yes")
    r = result("signin")
    check("…and the page gets NotAllowedError", r and r.get("error") == "NotAllowedError", r)
    check("…in the same words as a Cancel (a page can't tell which it was)",
          r and r.get("message") == cancel_message and cancel_message, (r, cancel_message))
    shown = {k: v for k, v in heights.items() if k != "none"}
    check("every bar leaves the page the same height, whatever Trek keeps",
          len(set(shown.values())) == 1 and min(shown.values()) < heights["none"], heights)
    later("signIn({ mediation: 'conditional', key: 'quiet' })")
    quiet = wait("quiet", lambda: next((a for a in asks() if a.get("stage") == "quiet"), None), 15)
    check("autofill's request with nothing to offer shows nothing, as with something",
          quiet and js("innerHeight") == heights["none"], (asks(), js("innerHeight")))

    # 12b. A page that opens its document again (a popup written into) still reaches Trek, and
    # one with no origin of its own (sandboxed) has no passkeys.
    load()
    js("document.open(), document.write('<title>Opened again</title><p>again</p>'), document.close(), true")
    later("signIn({ key: 'reopened' })")
    ask = bar("Trek has no passkey for this site", 20)
    check("a document opened again can still ask", ask, (asks(), js("window.__pk.reopened")))
    if ask:
        control("passkey", "yes")
    load(f"{SITE}/sandboxed")
    later("register('sandboxed@example.test', { key: 'sandboxed' })")
    r = result("sandboxed", 20)
    check("a sandboxed page, with no origin of its own, is refused with nothing on the bar",
          r and r.get("error") in ("SecurityError", "NotAllowedError") and asks() == [] and kept() == [], (r, asks(), kept()))
    load()

    # 13. An agent driving the Browser can't use a passkey by itself: its thread's card asks.
    agent()

    # 14. Off in Settings: new tabs get WebKit's answer back.
    control("passkeys", "off")
    js("window.open(location.href, '_blank') && true")
    tab = wait("tab", lambda: (lambda s: s["active"] if len(s["all_tabs"]) > 1 else None)(state()), 30)
    wait("page", lambda: js("typeof features") == "function")
    js("window.__pk.features = null, features(), true")
    f = wait("features", lambda: js("window.__pk.features"))
    check("with Passkeys off, a new tab is as WebKit leaves it", tab and f and f.get("uvpaa") is False, f)
    control("passkeys", "on")
    # Leave the Browser as it was found: the tab made for this closes itself.
    js("setTimeout(() => window.close(), 0), true")
    wait("closed", lambda: len(state()["all_tabs"]) == 1, 20)


def agent():
    project = os.path.join(os.path.dirname(os.path.dirname(dev_status()["runtime_dir"])), "trek-passkeys-check")
    os.makedirs(project, exist_ok=True)
    later("register('agent@example.test')")
    bar("create a passkey for “agent@example.test”")
    control("passkey", "yes")
    result("register")
    # The "saved" toast sits over the thread's lower right, where the card's buttons will be:
    # let it go first.
    time.sleep(9)
    dev("project", project)
    dev("send", "--no-wait", f"mock:browser {PAGE}?auto=signin wait=signed_in_as")
    card = wait("card", lambda: next((a for a in asks() if a.get("stage") == "card"), None), 90)
    check("a sign-in a page starts while an agent drives goes to the thread's approval card", card, asks())
    check("…and the bar can only call it off", card and card.get("yes") == [] and card.get("no") == "Cancel", card)
    r = js("window.__pk.signin")
    check("…with the page still waiting", isinstance(r, dict) and r.get("pending") is True, r)
    dev("control", "click", "allow")
    r = result("signin", 60)
    check("allowed on the card, the page is signed in", r and r.get("ok") is True and r.get("as") == "agent@example.test", r)
    dev("control", "--timeout", "120", "wait", "idle", "60000")
    # And refused: the page gets nothing.
    dev("send", "--no-wait", f"mock:browser {PAGE}?auto=signin wait=not_signed_in")
    card = wait("card", lambda: next((a for a in asks() if a.get("stage") == "card"), None), 90)
    check("the next one asks again (nothing is remembered)", card, asks())
    dev("control", "click", "deny")
    r = result("signin", 60)
    check("denied on the card, the page gets NotAllowedError", r and r.get("error") == "NotAllowedError", r)


def dev_status():
    import json
    return json.loads(dev("status"))


if __name__ == "__main__":
    main()
