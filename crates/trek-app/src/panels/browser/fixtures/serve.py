#!/usr/bin/env python3
"""Serves the Browser's sign-in test pages (see browser.md, "Popups and sign-in"), on loopback.

    python3 serve.py [port]

Pages: /signin.html (a site with a "Sign in with Google"-shaped button), /idp.html (the identity
provider's popup), /tabs.html (target=_blank, window.open() written into, mailto:, a redirect to a
download, a page that refuses frames, dialogs). /transform takes the popup's form POST, as
accounts.google.com/gsi/transform does with response_mode=form_post, and hands the credential to
window.opener. /ua says whether the request's user agent is Safari's own shape.

The opener is served on 127.0.0.1 and the popup on localhost: two origins, as a site and Google are.
"""

import html
import json
import os
import sys
import urllib.parse
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer

HERE = os.path.dirname(os.path.abspath(__file__))


def safari_shaped(ua):
    """Safari's user agent, and nothing that marks an embedded view (Google turns those away)."""
    return (
        "Version/" in ua
        and ua.rstrip().endswith("Safari/605.1.15")
        and "AppleWebKit/605.1.15" in ua
        and not any(t in ua for t in ("wv)", " wv", "Electron", "Trek", "Mobile/"))
    )


class Handler(SimpleHTTPRequestHandler):
    def __init__(self, *a, **k):
        super().__init__(*a, directory=HERE, **k)

    def log_message(self, *a):
        pass

    def send_page(self, body, status=200, headers=()):
        data = body.encode()
        self.send_response(status)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        for k, v in headers:
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        path = urllib.parse.urlparse(self.path).path
        if path == "/ua":
            ua = self.headers.get("User-Agent", "")
            body = json.dumps({"ua": ua, "safari": safari_shaped(ua)}).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Access-Control-Allow-Origin", "*")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        if path == "/file":
            # Where a redirect lands: a file the browser can't show, so it downloads.
            body = b"Trek download check\n"
            self.send_response(200)
            self.send_header("Content-Type", "application/octet-stream")
            self.send_header("Content-Disposition", 'attachment; filename="trek-download-check.txt"')
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        if path == "/download":
            self.send_response(302)
            self.send_header("Location", "/file")
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        if path == "/no-frames":
            # Refuses to be framed; loads fine as the page itself.
            self.send_page(
                "<!doctype html><title>No frames</title><h1 id=ok>Loaded top level</h1>",
                headers=[
                    ("X-Frame-Options", "DENY"),
                    ("Content-Security-Policy", "frame-ancestors 'none'"),
                ],
            )
            return
        if path == "/to-app":
            # A sign-in that ends by sending the browser to an app's scheme.
            self.send_response(302)
            self.send_header("Location", "trek-test-app://signed-in?code=1")
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        return super().do_GET()

    def do_POST(self):
        path = urllib.parse.urlparse(self.path).path
        if path != "/transform":
            self.send_page("not found", 404)
            return
        size = int(self.headers.get("Content-Length") or 0)
        form = urllib.parse.parse_qs(self.rfile.read(size).decode())
        credential = form.get("credential", [""])[0]
        origin = form.get("origin", ["*"])[0]
        # What Google's transform page does: hand the credential to the page that opened the
        # popup, then close. With no window.opener it has nowhere to go and stays, blank.
        self.send_page(
            f"""<!doctype html><title>transform</title><body>
<p id=stuck hidden>No window.opener: the sign-in can't reach the page that started it.</p>
<script>
  const msg = {{ type: 'credential', credential: {json.dumps(credential)} }};
  if (window.opener) {{
    window.opener.postMessage(msg, {json.dumps(origin)});
    window.close();
  }} else {{
    document.getElementById('stuck').hidden = false;
  }}
</script>""",
            # Google sends COOP report-only here: an enforced policy would cut the popup off
            # from its opener, as it should.
            headers=[("Cross-Origin-Opener-Policy-Report-Only", "same-origin")],
        )


def main():
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 47615
    server = ThreadingHTTPServer(("127.0.0.1", port), Handler)
    print(f"http://127.0.0.1:{port}/signin.html", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
