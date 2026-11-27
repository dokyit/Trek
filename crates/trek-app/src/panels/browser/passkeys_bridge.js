// Trek's passkeys, the bridge: runs in a content world of its own (the page's scripts can't see
// or change anything here) and is the only thing that can reach Trek's native handler, which
// exists in this world alone. It passes a request from the page's side (`passkeys_page.js`) to
// Trek as text and the answer back, and nothing else: one kind of message, of bounded size.
//
// What it forwards is still the page's own request, so Trek takes nothing in it on trust: the
// origin is WebKit's, read natively from the frame the message came from.
(() => {
  'use strict';
  if (window.top !== window) return;
  const handler = window.webkit && window.webkit.messageHandlers && window.webkit.messageHandlers.trekPasskeys;
  if (!handler) return;
  const ASK = 'trek-passkeys:ask';
  const ANSWER = 'trek-passkeys:answer';
  const MAX = 262144;
  const KINDS = ['create', 'get', 'cancel'];
  const cancelled = { error: { name: 'NotAllowedError', message: 'The request was cancelled.' } };
  const onAsk = (event) => {
    const detail = event.detail;
    if (typeof detail !== 'string' || detail.length > MAX) return;
    let message;
    let text;
    try {
      message = JSON.parse(detail);
      // Written out again here, in this world: what goes to Trek is well-formed text whatever
      // the page put in its own (half a surrogate pair comes out as an escape).
      text = JSON.stringify(message);
    } catch (_) { return; }
    if (!message || typeof message.id !== 'string' || message.id.length > 80) return;
    if (!KINDS.includes(message.kind) || typeof text !== 'string' || text.length > MAX) return;
    const answer = (reply) => {
      document.dispatchEvent(new CustomEvent(ANSWER, {
        detail: JSON.stringify({ id: message.id, ok: reply.ok, error: reply.error }),
      }));
    };
    handler.postMessage(text).then(
      (reply) => {
        let parsed;
        try { parsed = JSON.parse(reply); } catch (_) { parsed = null; }
        answer(parsed && typeof parsed === 'object' ? parsed : cancelled);
      },
      () => answer(cancelled),
    );
  };
  document.addEventListener(ASK, onAsk);
  // A page that opens its document again (`document.open()`, a popup written into) takes every
  // listener off it, this one included. Listening again is free while it's still there (the
  // same listener is never added twice), so each change of the document's root does it.
  new MutationObserver(() => document.addEventListener(ASK, onAsk))
    .observe(document, { childList: true });
})();
