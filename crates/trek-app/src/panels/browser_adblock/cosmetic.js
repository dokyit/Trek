/*trek-adblock*/
// Trek's ad blocker in a page (browser_adblock/page.rs), in a content world of its own: what the
// filter lists hide on this site, given before the page's first script (`__trekAdblock`), and
// what the lists' generic rules hide, asked of Trek as the page's classes and ids appear. Also
// the procedural filters (`:has-text()`, `:upward()`, `:remove()`…), which CSS can't say.
(() => {
  const data = globalThis.__trekAdblock;
  if (!data || globalThis.__trekAdblockRan) return;
  globalThis.__trekAdblockRan = true;
  const ask = (message) => {
    const h = globalThis.webkit && webkit.messageHandlers && webkit.messageHandlers.trekAdblock;
    if (!h) return Promise.reject(new Error('no handler'));
    return h.postMessage(JSON.stringify(message)).then((answer) => JSON.parse(answer));
  };

  // ---- Hiding: one style element, added to as selectors come.
  let sheet = null;
  const hidden = new Set();
  const pending = [];
  const flush = () => {
    if (!pending.length) return;
    const root = document.documentElement || document.head;
    if (!root) return;
    if (!sheet) {
      sheet = document.createElement('style');
      sheet.setAttribute('data-trek', '');
      root.appendChild(sheet);
    }
    // In rules of 200 selectors (each checked by `hide`: one WebKit can't parse would void its
    // whole rule).
    const text = [];
    for (let i = 0; i < pending.length; i += 200) {
      text.push(`${pending.slice(i, i + 200).join(',\n')}\n{display:none!important}`);
    }
    sheet.textContent += text.join('\n') + '\n';
    pending.length = 0;
  };
  const hide = (selectors) => {
    for (const s of selectors) {
      if (!s || hidden.has(s)) continue;
      try { document.createDocumentFragment().querySelector(s); } catch { continue; }
      hidden.add(s);
      pending.push(s);
    }
    flush();
  };
  hide(data.hide || []);
  if (!sheet) document.addEventListener('readystatechange', flush, { once: true });

  // ---- Generic rules: the page's classes and ids, asked of Trek in batches.
  const seen = new Set();
  let batch = { classes: new Set(), ids: new Set() };
  let timer = 0;
  const send = () => {
    timer = 0;
    const classes = [...batch.classes];
    const ids = [...batch.ids];
    batch = { classes: new Set(), ids: new Set() };
    if (!classes.length && !ids.length) return;
    ask({ kind: 'generic', classes, ids, exceptions: data.exceptions || [] })
      .then((selectors) => { if (Array.isArray(selectors)) hide(selectors); }, () => {});
  };
  const note = (el) => {
    if (!el || el.nodeType !== 1) return;
    const id = el.id;
    if (id && !seen.has('#' + id)) { seen.add('#' + id); batch.ids.add(id); }
    const list = el.classList;
    if (list) {
      for (const c of list) {
        if (!seen.has('.' + c)) { seen.add('.' + c); batch.classes.add(c); }
      }
    }
    if (!timer) timer = setTimeout(send, 50);
  };
  const scan = (root) => {
    if (!root || !root.querySelectorAll) return;
    note(root);
    for (const el of root.querySelectorAll('[id],[class]')) note(el);
  };

  // ---- Procedural filters.
  const textRegex = (arg) => {
    const m = /^\/(.*)\/([imsu]*)$/.exec(arg);
    if (m) { try { return new RegExp(m[1], m[2]); } catch { return null; } }
    return null;
  };
  const matcher = (arg) => {
    const re = textRegex(arg);
    return re ? (s) => re.test(s) : (s) => s.includes(arg);
  };
  const cssMatcher = (arg, pseudo) => {
    const at = arg.indexOf(':');
    if (at < 0) return () => false;
    const name = arg.slice(0, at).trim();
    const test = matcher(arg.slice(at + 1).trim());
    return (el) => test(getComputedStyle(el, pseudo).getPropertyValue(name));
  };
  const attrMatcher = (arg) => {
    const m = /^\s*([^=]+?)\s*(?:=\s*"?([\s\S]*?)"?)?\s*$/.exec(arg);
    if (!m) return () => false;
    const name = m[1].replace(/^"|"$/g, '');
    const nameRe = textRegex(name);
    const value = m[2] === undefined ? null : matcher(m[2]);
    return (el) => {
      for (const a of el.attributes) {
        const named = nameRe ? nameRe.test(a.name) : a.name === name;
        if (named && (!value || value(a.value))) return true;
      }
      return false;
    };
  };
  const step = (elements, op) => {
    const out = [];
    const push = (e) => { if (e && !out.includes(e)) out.push(e); };
    switch (op.type) {
      case 'css-selector': {
        for (const root of elements) {
          try {
            if (root === document) { for (const e of document.querySelectorAll(op.arg)) push(e); }
            else {
              // A selector after an operator is relative (`> .x`, `+ .y`) or a descendant one.
              const rel = /^\s*[>+~]/.test(op.arg) ? `:scope ${op.arg}` : op.arg;
              for (const e of root.querySelectorAll(rel)) push(e);
            }
          } catch {}
        }
        return out;
      }
      case 'has-text': { const t = matcher(op.arg); return elements.filter((e) => t(e.textContent || '')); }
      case 'min-text-length': { const n = Number(op.arg) || 0; return elements.filter((e) => (e.textContent || '').length >= n); }
      case 'matches-attr': { const t = attrMatcher(op.arg); return elements.filter((e) => t(e)); }
      case 'matches-css': { const t = cssMatcher(op.arg, null); return elements.filter((e) => t(e)); }
      case 'matches-css-before': { const t = cssMatcher(op.arg, '::before'); return elements.filter((e) => t(e)); }
      case 'matches-css-after': { const t = cssMatcher(op.arg, '::after'); return elements.filter((e) => t(e)); }
      case 'matches-path': { const t = matcher(op.arg); return t(location.pathname + location.search) ? elements : []; }
      case 'upward': {
        const n = Number(op.arg);
        for (const e of elements) {
          if (n > 0) { let p = e; for (let i = 0; i < n && p; i++) p = p.parentElement; push(p); }
          else { try { push(e.parentElement && e.parentElement.closest(op.arg)); } catch {} }
        }
        return out;
      }
      case 'xpath': {
        for (const root of elements) {
          try {
            const r = document.evaluate(op.arg, root, null, XPathResult.ORDERED_NODE_SNAPSHOT_TYPE, null);
            for (let i = 0; i < r.snapshotLength; i++) { const n = r.snapshotItem(i); if (n.nodeType === 1) push(n); }
          } catch {}
        }
        return out;
      }
      default:
        return [];
    }
  };
  const procedural = [];
  for (const json of data.procedural || []) {
    try {
      const f = typeof json === 'string' ? JSON.parse(json) : json;
      if (Array.isArray(f.selector) && f.selector.length) procedural.push(f);
    } catch {}
  }
  const acted = new WeakMap();
  const apply = () => {
    for (const f of procedural) {
      let elements = [document];
      for (const op of f.selector) {
        elements = step(elements, op);
        if (!elements.length) break;
      }
      for (const el of elements) {
        if (el === document || el === document.documentElement || el === document.body) continue;
        const done = acted.get(el) || new Set();
        if (done.has(f)) continue;
        done.add(f);
        acted.set(el, done);
        const action = f.action;
        if (!action) el.style.setProperty('display', 'none', 'important');
        else if (action.type === 'remove') el.remove();
        else if (action.type === 'style') el.setAttribute('style', `${el.getAttribute('style') || ''};${action.arg}`);
        else if (action.type === 'remove-attr') el.removeAttribute(action.arg);
        else if (action.type === 'remove-class') el.classList.remove(action.arg);
      }
    }
  };

  // ---- Watching the page.
  let queued = false;
  const later = () => {
    if (queued) return;
    queued = true;
    setTimeout(() => { queued = false; if (procedural.length) apply(); }, 100);
  };
  const start = () => {
    flush();
    if (!data.generichide) scan(document.documentElement);
    if (procedural.length) apply();
    new MutationObserver((records) => {
      for (const r of records) {
        if (r.type === 'attributes') { if (!data.generichide) note(r.target); continue; }
        for (const n of r.addedNodes) if (n.nodeType === 1 && !data.generichide) scan(n);
      }
      later();
    }).observe(document.documentElement, { childList: true, subtree: true, attributes: true, attributeFilter: ['class', 'id'] });
  };
  if (document.documentElement) start();
  else document.addEventListener('readystatechange', start, { once: true });

  // ---- The element picker (⋯ › Block an element…): Trek calls this, the user clicks.
  globalThis.__trekAdblockPick = () => new Promise((resolve) => {
    const box = document.createElement('div');
    box.style.cssText = 'position:fixed;z-index:2147483647;pointer-events:none;border:2px solid #e8590c;background:rgba(232,89,12,.15);border-radius:3px;transition:all .05s';
    const tip = document.createElement('div');
    tip.style.cssText = 'position:fixed;z-index:2147483647;left:50%;top:12px;transform:translateX(-50%);font:13px -apple-system,sans-serif;background:#1d1d1f;color:#fff;padding:6px 12px;border-radius:6px;pointer-events:none';
    tip.textContent = 'Click what to block · Esc to stop';
    document.documentElement.append(box, tip);
    let target = null;
    const move = (e) => {
      target = e.target;
      const r = target.getBoundingClientRect();
      Object.assign(box.style, { left: r.left + 'px', top: r.top + 'px', width: r.width + 'px', height: r.height + 'px' });
    };
    const stop = (value) => {
      removeEventListener('mousemove', move, true);
      removeEventListener('click', click, true);
      removeEventListener('keydown', key, true);
      box.remove();
      tip.remove();
      resolve(value);
    };
    const click = (e) => {
      e.preventDefault();
      e.stopPropagation();
      const el = target || e.target;
      stop(el ? selectorFor(el) : null);
    };
    const key = (e) => { if (e.key === 'Escape') { e.preventDefault(); stop(null); } };
    addEventListener('mousemove', move, true);
    addEventListener('click', click, true);
    addEventListener('keydown', key, true);
  });
  // A selector for `el` that names it alone: its id, else a path of tags and classes.
  const selectorFor = (el) => {
    const esc = (s) => (globalThis.CSS && CSS.escape ? CSS.escape(s) : s);
    const parts = [];
    for (let e = el; e && e.nodeType === 1 && e !== document.documentElement; e = e.parentElement) {
      if (e.id && !/\d{3,}/.test(e.id)) { parts.unshift(`#${esc(e.id)}`); break; }
      let part = e.localName;
      const classes = [...e.classList].filter((c) => !/\d{3,}/.test(c)).slice(0, 3);
      if (classes.length) part += classes.map((c) => `.${esc(c)}`).join('');
      const parent = e.parentElement;
      if (parent) {
        const same = [...parent.children].filter((c) => c.localName === e.localName);
        if (same.length > 1 && !classes.length) part += `:nth-of-type(${same.indexOf(e) + 1})`;
      }
      parts.unshift(part);
      try { if (document.querySelectorAll(parts.join(' > ')).length === 1) break; } catch {}
    }
    return parts.join(' > ');
  };
  globalThis.__trekAdblockHide = (selector) => { hide([selector]); };
})();
