//! `mock:browser`: the mock uses Trek's browser tools end to end, as an agent would call them
//! (`trek-browser`, over Trek's channel), on a test site it serves itself: a sign-up form with a
//! field kept the way React keeps one, a drop-down, a checkbox, a tooltip on hover, a list of the
//! page's own, an editable area, a shadow root, a same-origin frame, a long page and a second
//! page. It checks what each step should have done and ends with a tally, so a run of it is a
//! live test of browser use (`.agents/skills/verify-trek`). `mock:browser approvals` also types a
//! password and runs a script, which ask the user first below full access. `mock:browser <url>`
//! opens that address first instead.

use super::{Session, Step};
use crate::AgentEvent;
use serde_json::{Value, json};
use std::time::Duration;

/// The test site's pages, by path.
fn page(path: &str) -> Option<&'static str> {
    Some(match path {
        "/" | "/form.html" => FORM,
        "/next.html" => NEXT,
        "/frame.html" => FRAME,
        _ => return None,
    })
}

const FORM: &str = r##"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>Trek browser check</title>
<meta name="viewport" content="width=device-width">
<style>
  :root { color-scheme: light dark; font: 15px/1.45 -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }
  body { margin: 24px; max-width: 620px; }
  label { display: block; margin: 12px 0 4px; font-weight: 600; }
  input:not([type=checkbox]), select, [contenteditable] { width: 100%; box-sizing: border-box; padding: 6px 8px; font: inherit; border: 1px solid #8884; border-radius: 6px; }
  [contenteditable] { min-height: 44px; }
  .row { display: flex; gap: 8px; align-items: center; margin-top: 12px; }
  #flavors { list-style: none; padding: 4px; margin: 4px 0; border: 1px solid #8884; border-radius: 6px; }
  #flavors li { padding: 4px 8px; cursor: pointer; }
  #spacer { height: 1400px; }
  button { font: inherit; padding: 6px 12px; border-radius: 6px; }
</style></head>
<body>
<h1>Sign up</h1>
<form id="signup" action="/done" method="post" aria-label="Sign up">
  <label for="email">Email</label>
  <input id="email" type="email" required placeholder="you@example.com">
  <label for="who">Name</label>
  <input id="who" autocomplete="off">
  <p id="hello">Hello, nobody</p>
  <label for="country">Country</label>
  <select id="country"><option value="">Choose…</option><option value="no">Norway</option><option value="se">Sweden</option></select>
  <div class="row"><input type="checkbox" id="news"><label for="news" style="margin:0">Subscribe to news</label></div>
  <label for="pw">Password</label>
  <input id="pw" type="password" autocomplete="new-password">
  <label id="notes-label">Notes</label>
  <div id="notes" contenteditable="true" aria-labelledby="notes-label"></div>
  <div class="row">
    <button type="button" id="help">Help</button>
    <span id="tip" role="tooltip" hidden>We never share your email</span>
  </div>
  <label id="flavor-label">Flavor</label>
  <div id="flavor" role="combobox" aria-labelledby="flavor-label" aria-expanded="false" aria-controls="flavors" tabindex="0">Pick a flavor</div>
  <ul id="flavors" role="listbox" hidden><li role="option">Vanilla</li><li role="option">Mint</li><li role="option">Mango</li></ul>
  <p id="flavor-out">Flavor: none</p>
  <div class="row"><button type="submit">Send</button></div>
</form>
<p id="result" role="status"></p>
<trek-card></trek-card>
<iframe src="/frame.html" title="Inner frame" style="width:100%;height:90px;border:1px solid #8884;border-radius:6px"></iframe>
<p><a href="/next.html">Next page</a></p>
<div id="spacer"></div>
<p><a id="bottom" href="/next.html#bottom">Bottom link</a></p>
<script>
  // A field kept as React keeps one: its value lives in `state`, the element's own value setter
  // is wrapped to remember what code set, and an input event carrying that value is dropped (as
  // React's value tracker does). Only real typing reaches `state`; setting `.value` doesn't.
  const who = document.getElementById('who');
  const native = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value');
  let state = '', tracked = '';
  Object.defineProperty(who, 'value', {
    configurable: true,
    get() { return native.get.call(this); },
    set(v) { tracked = String(v); native.set.call(this, v); },
  });
  who.addEventListener('input', () => {
    const v = native.get.call(who);
    if (v === tracked) return;
    tracked = v;
    state = v;
    who.value = state;
    document.getElementById('hello').textContent = 'Hello, ' + (state || 'nobody');
  });
  const help = document.getElementById('help'), tip = document.getElementById('tip');
  help.addEventListener('mouseenter', () => { tip.hidden = false; });
  help.addEventListener('mouseleave', () => { tip.hidden = true; });
  const flavor = document.getElementById('flavor'), flavors = document.getElementById('flavors');
  flavor.addEventListener('click', () => {
    flavors.hidden = !flavors.hidden;
    flavor.setAttribute('aria-expanded', String(!flavors.hidden));
  });
  flavors.addEventListener('click', (e) => {
    const o = e.target.closest('[role=option]');
    if (!o) return;
    document.getElementById('flavor-out').textContent = 'Flavor: ' + o.textContent;
    flavors.hidden = true;
    flavor.setAttribute('aria-expanded', 'false');
  });
  document.getElementById('signup').addEventListener('submit', (e) => {
    e.preventDefault();
    const v = (id) => document.getElementById(id);
    v('result').textContent = 'Submitted: ' + v('email').value + ' / ' + state + ' / ' + (v('country').value || 'none') + ' / ' + (v('news').checked ? 'subscribed' : 'not subscribed') + ' / notes: ' + v('notes').innerText.trim();
  });
  customElements.define('trek-card', class extends HTMLElement {
    connectedCallback() {
      const root = this.attachShadow({ mode: 'open' });
      root.innerHTML = '<p>Inside a shadow root</p><button>Shadow button</button>';
      root.querySelector('button').addEventListener('click', () => { root.querySelector('p').textContent = 'Shadow clicked'; });
    }
  });
</script>
</body></html>
"##;

const NEXT: &str = r##"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>Next page</title>
<style>:root { color-scheme: light dark; font: 15px -apple-system, sans-serif; } body { margin: 24px; }</style></head>
<body><h1>Next page</h1><p>You followed the link.</p>
<p><a href="/form.html">Back to the form</a></p>
<p><a href="/form.html" target="_blank">The form in a new tab</a></p></body></html>
"##;

const FRAME: &str = r##"<!doctype html>
<html><head><meta charset="utf-8"><style>:root { color-scheme: light dark; font: 14px -apple-system, sans-serif; }</style></head>
<body><p>In the frame</p><a href="/next.html" target="_top">Frame link</a></body></html>
"##;

/// Serve the test site on a port of its own, for a while (the page stays to look at after the
/// check). Its address.
async fn serve() -> std::io::Result<String> {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let path = req
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("/")
                    .split(['?', '#'])
                    .next()
                    .unwrap_or("/");
                let (status, body) = match page(path) {
                    Some(b) => ("200 OK", b),
                    None => (
                        "404 Not Found",
                        "<!doctype html><title>Not found</title>Not found",
                    ),
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes()).await;
                let _ = stream.write_all(body.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(15 * 60)).await;
        server.abort();
    });
    Ok(base)
}

/// The ref of the element whose outline line names `name` (and is `role`, when given).
pub(super) fn ref_of(outline: &str, role: Option<&str>, name: &str) -> Option<String> {
    let quoted = format!("{}", Value::from(name));
    outline
        .lines()
        .map(str::trim_start)
        .filter(|l| l.contains(&quoted))
        .filter(|l| role.is_none_or(|r| l.starts_with(&format!("- {r} "))))
        .find_map(|l| {
            let at = l.find("[ref=")? + 5;
            let end = l[at..].find(']')? + at;
            Some(l[at..end].to_string())
        })
}

/// What the check found: each step, and what went wrong with it.
#[derive(Default)]
struct Tally(Vec<(String, Option<String>)>);

impl Tally {
    fn check(&mut self, step: &str, ok: bool, why: impl Into<String>) {
        self.0.push((step.to_string(), (!ok).then(|| why.into())));
    }

    fn report(&self) -> String {
        let failed: Vec<_> = self.0.iter().filter(|(_, e)| e.is_some()).collect();
        let mut out = if failed.is_empty() {
            format!("Browser check: all {} steps passed.\n", self.0.len())
        } else {
            format!(
                "Browser check: {} of {} steps failed.\n",
                failed.len(),
                self.0.len()
            )
        };
        for (step, e) in &self.0 {
            match e {
                None => out.push_str(&format!("\n- ✓ {step}")),
                Some(e) => out.push_str(&format!("\n- ✗ {step}: {e}")),
            }
        }
        out
    }
}

/// An answer's text (a screenshot's file goes: nothing reads it here).
fn text_of(v: &Value) -> String {
    if let Some(path) = v["image"].as_str() {
        let _ = std::fs::remove_file(path);
    }
    v["text"].as_str().unwrap_or_default().to_string()
}

impl Session {
    /// One call to Trek's browser tool `method`, with its row in the transcript: its text, or
    /// why it failed.
    async fn browser_tool(
        &mut self,
        method: &str,
        detail: &str,
        params: Value,
    ) -> Step<Result<Value, String>> {
        let id = self.id("tool");
        self.tool_start(
            &id,
            &format!("mcp__{}__{method}", trek_ipc::browser::SERVER),
            detail,
        )
        .await?;
        let answer = match self.browser.clone() {
            None => Err("Trek gave this session no browser tools (Settings › Tools › Browser tools is off).".to_string()),
            Some(client) => {
                let (handle_tx, handle_rx) = std::sync::mpsc::channel();
                let m = method.to_string();
                let mut call = tokio::task::spawn_blocking(move || {
                    let mut conn = client.connect()?;
                    if let Ok(h) = conn.handle() {
                        let _ = handle_tx.send(h);
                    }
                    conn.call(&m, &params)
                });
                loop {
                    tokio::select! {
                        done = &mut call => break done.unwrap_or_else(|e| Err(e.to_string())),
                        cmd = self.commands.recv() => {
                            if let Err(stop) = self.handle_midturn(cmd) {
                                if let Ok(h) = handle_rx.try_recv() {
                                    let _ = h.shutdown();
                                }
                                return Err(stop);
                            }
                        }
                    }
                }
            }
        };
        let (output, ok) = match &answer {
            Ok(v) if v["image"].is_string() => (
                format!("{}\n[image]", v["text"].as_str().unwrap_or_default()),
                true,
            ),
            Ok(v) => (
                v["text"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| v.to_string()),
                true,
            ),
            Err(e) => (e.clone(), false),
        };
        self.emit(AgentEvent::ToolFinished { id, output, ok })
            .await?;
        Ok(answer)
    }

    /// `mock:browser`: the check (see the module's notes).
    pub(super) async fn browser_check(&mut self, text: &str) -> Step {
        let args = super::after_keyword(text, &["mock:browser"]).unwrap_or_default();
        let approvals = args.split_whitespace().any(|w| w == "approvals");
        let url = args
            .split_whitespace()
            .find(|w| w.contains("://") || w.starts_with("localhost"))
            .map(str::to_string);
        if let Some(url) = url {
            self.think("I'll open the page in Trek's browser and read it.")
                .await?;
            let opened = self
                .browser_tool("browser_open", &url, json!({"url": url}))
                .await?;
            let snap = self.browser_tool("browser_snapshot", "", json!({})).await?;
            return match (opened, snap) {
                (Ok(_), Ok(s)) => {
                    let lines = text_of(&s).lines().count();
                    self.say(&format!(
                        "I opened {url} in Trek's browser and read it: {lines} lines of outline."
                    ))
                    .await
                }
                (Err(e), _) | (_, Err(e)) => {
                    self.say(&format!("I couldn't use Trek's browser: {e}"))
                        .await
                }
            };
        }
        let base = match serve().await {
            Ok(b) => b,
            Err(e) => {
                return self
                    .say(&format!("I couldn't serve the test site: {e}"))
                    .await;
            }
        };
        self.think(
            "I'll fill in the test site's sign-up form in Trek's browser, checking each step.",
        )
        .await?;
        let mut t = Tally::default();
        let form = format!("{base}/form.html");

        // Open, and read the page.
        let r = self
            .browser_tool("browser_open", &form, json!({"url": form, "new_tab": true}))
            .await?;
        t.check(
            "open the form",
            r.as_ref()
                .is_ok_and(|v| text_of(v).contains("Trek browser check")),
            format!("{r:?}"),
        );
        if r.is_err() {
            return self.say(&t.report()).await;
        }
        let snap = self
            .browser_tool("browser_snapshot", "", json!({}))
            .await?
            .map(|v| text_of(&v))
            .unwrap_or_else(|e| e);
        let refs = |role: Option<&str>, name: &str| ref_of(&snap, role, name);
        let (email, who, country, news, help, flavor, notes, send, shadow, next) = (
            refs(Some("textbox"), "Email"),
            refs(Some("textbox"), "Name"),
            refs(Some("combobox"), "Country"),
            refs(Some("checkbox"), "Subscribe to news"),
            refs(Some("button"), "Help"),
            refs(Some("combobox"), "Flavor"),
            refs(Some("textbox"), "Notes"),
            refs(Some("button"), "Send"),
            refs(Some("button"), "Shadow button"),
            refs(Some("link"), "Next page"),
        );
        t.check(
            "the outline names the form's fields with refs",
            [&email, &who, &country, &news, &help, &flavor, &notes, &send]
                .iter()
                .all(|r| r.is_some()),
            snap.clone(),
        );
        t.check(
            "the outline reaches into the shadow root",
            shadow.is_some(),
            "no Shadow button",
        );
        t.check(
            "the outline reaches into the same-origin frame",
            snap.contains("\"Frame link\"") && snap.contains("In the frame"),
            "no Frame link",
        );
        t.check(
            "links show where they go",
            snap.contains("-> /next.html"),
            "no href",
        );
        let r0 = |r: &Option<String>| r.clone().unwrap_or_else(|| "e0".into());

        // Type, as a person would.
        let r = self
            .browser_tool(
                "browser_type",
                "Email",
                json!({"ref": r0(&email), "text": "ada@example.com"}),
            )
            .await?;
        t.check(
            "type an email",
            r.as_ref()
                .is_ok_and(|v| text_of(v).contains("ada@example.com")),
            format!("{r:?}"),
        );
        let r = self
            .browser_tool(
                "browser_type",
                "Name",
                json!({"ref": r0(&who), "text": "Ada"}),
            )
            .await?;
        t.check(
            "type into a field kept like React's",
            r.is_ok(),
            format!("{r:?}"),
        );
        let r = self
            .browser_tool(
                "browser_wait",
                "Hello, Ada",
                json!({"text": "Hello, Ada", "timeout_ms": 3000}),
            )
            .await?;
        t.check(
            "the page's own state saw the typing",
            r.is_ok(),
            format!("{r:?}"),
        );
        let r = self
            .browser_tool(
                "browser_select",
                "Country: Norway",
                json!({"ref": r0(&country), "option": "Norway"}),
            )
            .await?;
        t.check(
            "choose in a drop-down",
            r.as_ref().is_ok_and(|v| text_of(v).contains("Norway")),
            format!("{r:?}"),
        );
        let r = self
            .browser_tool(
                "browser_click",
                "Subscribe to news",
                json!({"ref": r0(&news)}),
            )
            .await?;
        t.check("tick a checkbox", r.is_ok(), format!("{r:?}"));
        let r = self
            .browser_tool(
                "browser_type",
                "Notes",
                json!({"ref": r0(&notes), "text": "Hi there"}),
            )
            .await?;
        t.check(
            "type into an editable area",
            r.as_ref().is_ok_and(|v| text_of(v).contains("Hi there")),
            format!("{r:?}"),
        );
        let r = self
            .browser_tool("browser_hover", "Help", json!({"ref": r0(&help)}))
            .await?;
        let tip = self
            .browser_tool(
                "browser_wait",
                "the tooltip",
                json!({"text": "We never share your email", "timeout_ms": 2000}),
            )
            .await?;
        t.check(
            "hover shows a tooltip",
            r.is_ok() && tip.is_ok(),
            format!("{r:?} {tip:?}"),
        );
        let r = self
            .browser_tool(
                "browser_select",
                "Flavor: Mint",
                json!({"ref": r0(&flavor), "option": "Mint"}),
            )
            .await?;
        let picked = self
            .browser_tool(
                "browser_wait",
                "Flavor: Mint",
                json!({"text": "Flavor: Mint", "timeout_ms": 2000}),
            )
            .await?;
        t.check(
            "choose in a list of the page's own",
            r.is_ok() && picked.is_ok(),
            format!("{r:?} {picked:?}"),
        );
        let r = self
            .browser_tool(
                "browser_press",
                "Tab",
                json!({"key": "Shift+Tab", "ref": r0(&email)}),
            )
            .await?;
        t.check("press a key", r.is_ok(), format!("{r:?}"));

        // Send it.
        let r = self
            .browser_tool("browser_click", "Send", json!({"ref": r0(&send)}))
            .await?;
        t.check("click the submit button", r.is_ok(), format!("{r:?}"));
        let want = "Submitted: ada@example.com / Ada / no / subscribed / notes: Hi there";
        let r = self
            .browser_tool(
                "browser_wait",
                "Submitted",
                json!({"text": want, "timeout_ms": 3000}),
            )
            .await?;
        t.check(
            "the form went with everything typed and chosen",
            r.is_ok(),
            format!("{r:?}"),
        );
        let r = self.browser_tool("browser_text", "", json!({})).await?;
        t.check(
            "read the page's text",
            r.as_ref().is_ok_and(|v| text_of(v).contains(want)),
            format!("{r:?}"),
        );

        // The rest of the page.
        let r = self
            .browser_tool(
                "browser_click",
                "Shadow button",
                json!({"ref": r0(&shadow)}),
            )
            .await?;
        let inside = self
            .browser_tool(
                "browser_wait",
                "Shadow clicked",
                json!({"text": "Shadow clicked", "timeout_ms": 2000}),
            )
            .await?;
        t.check(
            "click inside a shadow root",
            r.is_ok() && inside.is_ok(),
            format!("{r:?} {inside:?}"),
        );
        let r = self
            .browser_tool("browser_scroll", "down", json!({"direction": "down"}))
            .await?;
        t.check(
            "scroll down",
            r.as_ref()
                .is_ok_and(|v| !text_of(v).contains("Now 0 px down")),
            format!("{r:?}"),
        );
        let r = self
            .browser_tool("browser_screenshot", "", json!({}))
            .await?;
        let shot = r.as_ref().map(|v| v["image"].is_string());
        t.check("take a screenshot", shot == Ok(true), format!("{r:?}"));
        if let Ok(v) = &r {
            text_of(v);
        }
        let r = self
            .browser_tool(
                "browser_screenshot",
                "the whole page",
                json!({"full_page": true}),
            )
            .await?;
        t.check(
            "take a screenshot of the whole page",
            r.as_ref().is_ok_and(|v| text_of(v).contains("whole page")),
            format!("{r:?}"),
        );

        // Away and back.
        let r = self
            .browser_tool("browser_click", "Next page", json!({"ref": r0(&next)}))
            .await?;
        t.check(
            "follow a link",
            r.as_ref().is_ok_and(|v| text_of(v).contains("/next.html")),
            format!("{r:?}"),
        );
        let r = self.browser_tool("browser_back", "", json!({})).await?;
        t.check(
            "go back",
            r.as_ref().is_ok_and(|v| text_of(v).contains("/form.html")),
            format!("{r:?}"),
        );
        let r = self
            .browser_tool(
                "browser_open",
                "next page in a new tab",
                json!({"url": format!("{base}/next.html"), "new_tab": true}),
            )
            .await?;
        let r2 = self.browser_tool("browser_tabs", "", json!({})).await?;
        let tabs = r2.as_ref().map(text_of).unwrap_or_default();
        t.check(
            "open a second tab",
            r.is_ok() && tabs.contains("Next page") && tabs.contains("Trek browser check"),
            format!("{r:?} {tabs}"),
        );
        let first = tabs
            .lines()
            .find(|l| l.contains("Trek browser check"))
            .and_then(|l| l.trim_start_matches("- tab ").split([' ', ':']).next())
            .and_then(|n| n.parse::<u64>().ok());
        let second = tabs
            .lines()
            .find(|l| l.contains("Next page"))
            .and_then(|l| l.trim_start_matches("- tab ").split([' ', ':']).next())
            .and_then(|n| n.parse::<u64>().ok());
        let r = self
            .browser_tool(
                "browser_switch",
                "the form's tab",
                json!({"tab": first.unwrap_or(0)}),
            )
            .await?;
        t.check(
            "switch tabs",
            r.as_ref().is_ok_and(|v| text_of(v).contains("form.html")),
            format!("{r:?}"),
        );
        let r = self
            .browser_tool(
                "browser_close",
                "the second tab",
                json!({"tab": second.unwrap_or(0)}),
            )
            .await?;
        t.check("close a tab", r.is_ok(), format!("{r:?}"));
        let r = self
            .browser_tool(
                "browser_click",
                "a ref that isn't there",
                json!({"ref": "e99999"}),
            )
            .await?;
        t.check(
            "a stale ref says so",
            r.as_ref().is_err_and(|e| e.contains("snapshot")),
            format!("{r:?}"),
        );

        if approvals {
            let snap = self
                .browser_tool("browser_snapshot", "", json!({"interactive_only": true}))
                .await?
                .map(|v| text_of(&v))
                .unwrap_or_default();
            let pw = ref_of(&snap, Some("textbox"), "Password");
            let r = self
                .browser_tool(
                    "browser_type",
                    "Password",
                    json!({"ref": r0(&pw), "text": "hunter2"}),
                )
                .await?;
            t.check(
                "type a password (asks below full access)",
                r.is_ok(),
                format!("{r:?}"),
            );
            let r = self
                .browser_tool(
                    "browser_eval",
                    "document.title",
                    json!({"js": "document.title"}),
                )
                .await?;
            t.check(
                "run a script (asks below full access)",
                r.as_ref()
                    .is_ok_and(|v| text_of(v).contains("Trek browser check")),
                format!("{r:?}"),
            );
        }
        self.pause(Duration::from_millis(200)).await?;
        self.say(&t.report()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_are_found_by_role_and_name() {
        let outline = "- form \"Sign up\":\n  - textbox \"Email\" [ref=e1] [required]\n  - textbox \"Name\" [ref=e2]\n  - button \"Send\" [ref=e9]\n- link \"Send feedback\" [ref=e10] -> /f";
        assert_eq!(
            ref_of(outline, Some("textbox"), "Name").as_deref(),
            Some("e2")
        );
        assert_eq!(
            ref_of(outline, Some("button"), "Send").as_deref(),
            Some("e9")
        );
        assert_eq!(ref_of(outline, Some("link"), "Send"), None);
        assert_eq!(ref_of(outline, None, "Email").as_deref(), Some("e1"));
    }

    #[test]
    fn the_test_site_serves_its_pages() {
        assert!(page("/form.html").unwrap().contains("<trek-card>"));
        assert!(page("/").unwrap().contains("Sign up"));
        assert!(page("/next.html").unwrap().contains("Next page"));
        assert!(page("/frame.html").unwrap().contains("Frame link"));
        assert!(page("/nope").is_none());
        let mut t = Tally::default();
        t.check("a", true, "");
        t.check("b", false, "because");
        assert!(
            t.report()
                .starts_with("Browser check: 1 of 2 steps failed.")
        );
        assert!(t.report().contains("- ✗ b: because"));
    }
}
