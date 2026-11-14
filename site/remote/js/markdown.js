// An agent's markdown as HTML, made safe: marked (GitHub's flavour) and then DOMPurify, which
// keeps what an answer is drawn with and drops scripts, handlers, frames and forms. Links open in
// the browser, outside the remote. Answers are drawn again as they stream, so the last few are
// kept by their text.

import { Marked } from '../vendor/marked.mjs';
import DOMPurify from '../vendor/purify.mjs';
import { h } from '../vendor/preact.mjs';
import { useLayoutEffect, useRef } from '../vendor/hooks.mjs';
import { html } from './ui.js';
import { markdownEnhancers } from './routes.js';

const marked = new Marked({ gfm: true, breaks: false });

DOMPurify.addHook('afterSanitizeAttributes', (node) => {
  if (node.tagName === 'A') {
    node.setAttribute('target', '_blank');
    node.setAttribute('rel', 'noopener noreferrer');
  }
  // Pictures come from the computer (`media`), not from wherever an answer points.
  if (node.tagName === 'IMG') {
    const src = node.getAttribute('src') || '';
    if (!/^(data:image\/|blob:)/.test(src)) {
      node.setAttribute('data-src', src);
      node.removeAttribute('src');
    }
  }
});

const PURIFY = {
  FORBID_TAGS: ['style', 'form', 'input', 'button', 'textarea', 'select', 'iframe', 'object', 'embed', 'video', 'audio', 'source'],
  FORBID_ATTR: ['srcset'],
};

const cache = new Map();
const KEEP = 300;

export function toHtml(text) {
  const source = text || '';
  let out = cache.get(source);
  if (out === undefined) {
    try {
      out = DOMPurify.sanitize(marked.parse(source), PURIFY);
    } catch {
      out = DOMPurify.sanitize(escape(source));
    }
    cache.set(source, out);
    if (cache.size > KEEP) cache.delete(cache.keys().next().value);
  }
  return out;
}

function escape(text) {
  return text.replace(/[&<>]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' })[c]);
}

export function Markdown({ text, class: cls = '', threadId }) {
  const ref = useRef(null);
  const markup = toHtml(text);
  useLayoutEffect(() => {
    if (ref.current) for (const enhance of markdownEnhancers) enhance(ref.current, { threadId });
  }, [markup]);
  return h('div', { ref, class: `md ${cls}`, dangerouslySetInnerHTML: { __html: markup } });
}

export function Plain({ text }) {
  return html`<span>${text}</span>`;
}
