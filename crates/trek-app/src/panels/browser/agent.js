// Browser use: what Trek runs in a page for an agent (see agent.rs). Installed once per
// document as `window.__trekAgent`; each tool call then runs one of its commands and gets JSON
// back. Elements the agent can act on get a stable ref (`data-trek-ref="e12"`), kept while the
// element stays on the page. Events are dispatched as a person's would arrive: pointer and mouse
// events around a click, key events around typing, and text inserted through the editing
// commands, so frameworks that track input values (React, Vue) see the change.
(() => {
  const VERSION = 1;
  if (window.__trekAgent && window.__trekAgent.version === VERSION) return;

  const INTERACTIVE = new Set(['link', 'button', 'textbox', 'searchbox', 'checkbox', 'radio', 'combobox', 'listbox', 'option', 'slider', 'spinbutton', 'switch', 'tab', 'menuitem', 'menuitemcheckbox', 'menuitemradio', 'treeitem', 'gridcell']);
  // Roles that keep their place in the outline (the rest flatten into their parent).
  const STRUCTURE = new Set(['heading', 'navigation', 'main', 'banner', 'contentinfo', 'complementary', 'form', 'search', 'region', 'list', 'listitem', 'table', 'row', 'cell', 'columnheader', 'rowheader', 'dialog', 'alertdialog', 'alert', 'status', 'group', 'tablist', 'tabpanel', 'menu', 'menubar', 'tree', 'grid', 'article', 'img', 'iframe', 'paragraph', 'progressbar', 'meter', 'separator', 'figure', 'blockquote', 'code']);
  // Roles whose name comes from their own text.
  // (Containers' text shows as their children instead.)
  const NAMED_BY_CONTENT = new Set(['link', 'button', 'heading', 'option', 'tab', 'menuitem', 'menuitemcheckbox', 'menuitemradio', 'treeitem', 'checkbox', 'radio', 'switch', 'clickable']);
  // Roles read whole: their insides aren't walked.
  const LEAVES = new Set(['link', 'button', 'option', 'tab', 'menuitem', 'menuitemcheckbox', 'menuitemradio', 'checkbox', 'radio', 'switch', 'textbox', 'searchbox', 'heading', 'img', 'slider', 'spinbutton', 'progressbar', 'meter', 'separator']);
  const SKIP_TAGS = new Set(['SCRIPT', 'STYLE', 'NOSCRIPT', 'TEMPLATE', 'META', 'LINK', 'HEAD', 'TITLE', 'BASE']);
  const TEXT_TYPES = new Set(['text', 'email', 'password', 'search', 'tel', 'url', 'number', '', 'date', 'datetime-local', 'month', 'time', 'week']);

  const A = { version: VERSION, n: 0, refs: new Map(), results: {}, flashTimer: 0 };

  const clip = (s, n) => {
    s = String(s == null ? '' : s).replace(/\s+/g, ' ').trim();
    return s.length > n ? s.slice(0, n - 1) + '…' : s;
  };
  const isEl = (n) => n && n.nodeType === 1;
  const tag = (el) => el.tagName;
  const ours = (el) => typeof el.id === 'string' && el.id.startsWith('__trek');

  // ---- roles and names -------------------------------------------------------------------

  const inputRole = (el) => {
    const t = (el.getAttribute('type') || 'text').toLowerCase();
    switch (t) {
      case 'button': case 'submit': case 'reset': case 'image': return 'button';
      case 'checkbox': return 'checkbox';
      case 'radio': return 'radio';
      case 'range': return 'slider';
      case 'number': return 'spinbutton';
      case 'search': return 'searchbox';
      case 'hidden': return null;
      case 'file': return 'button';
      case 'color': return 'button';
      default: return el.getAttribute('list') ? 'combobox' : 'textbox';
    }
  };

  const roleOf = (el) => {
    const explicit = (el.getAttribute('role') || '').trim().split(/\s+/)[0];
    if (explicit && explicit !== 'presentation' && explicit !== 'none') return explicit;
    if (explicit === 'presentation' || explicit === 'none') return null;
    switch (tag(el)) {
      case 'A': case 'AREA': return el.hasAttribute('href') ? 'link' : null;
      case 'BUTTON': case 'SUMMARY': return 'button';
      case 'INPUT': return inputRole(el);
      case 'SELECT': return el.multiple || el.size > 1 ? 'listbox' : 'combobox';
      case 'TEXTAREA': return 'textbox';
      case 'OPTION': return 'option';
      case 'H1': case 'H2': case 'H3': case 'H4': case 'H5': case 'H6': return 'heading';
      case 'NAV': return 'navigation';
      case 'MAIN': return 'main';
      case 'HEADER': return el.closest('article, aside, main, nav, section') ? null : 'banner';
      case 'FOOTER': return el.closest('article, aside, main, nav, section') ? null : 'contentinfo';
      case 'ASIDE': return 'complementary';
      case 'FORM': return 'form';
      case 'SEARCH': return 'search';
      case 'SECTION': return el.hasAttribute('aria-label') || el.hasAttribute('aria-labelledby') ? 'region' : null;
      case 'ARTICLE': return 'article';
      case 'UL': case 'OL': case 'MENU': return 'list';
      case 'LI': return 'listitem';
      case 'TABLE': return 'table';
      case 'TR': return 'row';
      case 'TD': return 'cell';
      case 'TH': return el.getAttribute('scope') === 'row' ? 'rowheader' : 'columnheader';
      case 'IMG': return el.getAttribute('alt') === '' ? null : 'img';
      case 'SVG': return el.getAttribute('aria-label') || el.querySelector(':scope > title') ? 'img' : null;
      case 'DIALOG': return 'dialog';
      case 'FIELDSET': case 'DETAILS': return 'group';
      case 'IFRAME': case 'FRAME': return 'iframe';
      case 'P': return 'paragraph';
      case 'PROGRESS': return 'progressbar';
      case 'METER': return 'meter';
      case 'HR': return 'separator';
      case 'FIGURE': return 'figure';
      case 'BLOCKQUOTE': return 'blockquote';
      case 'PRE': return 'code';
    }
    if (el.isContentEditable && (!el.parentElement || !el.parentElement.isContentEditable)) return 'textbox';
    return null;
  };

  const textOf = (el) => {
    try { return el.innerText != null ? el.innerText : el.textContent; } catch (_) { return el.textContent; }
  };

  const byIds = (el, attr) => {
    const ids = (el.getAttribute(attr) || '').trim();
    if (!ids) return '';
    const root = el.getRootNode();
    return ids.split(/\s+/).map((id) => {
      const t = (root.getElementById ? root.getElementById(id) : null) || el.ownerDocument.getElementById(id);
      return t ? textOf(t) : '';
    }).join(' ');
  };

  const nameOf = (el, role) => {
    let n = byIds(el, 'aria-labelledby') || el.getAttribute('aria-label') || '';
    if (!n.trim()) {
      const t = tag(el);
      if (t === 'INPUT' || t === 'TEXTAREA' || t === 'SELECT') {
        const type = (el.getAttribute('type') || '').toLowerCase();
        if (t === 'INPUT' && ['button', 'submit', 'reset'].includes(type)) n = el.value || (type === 'submit' ? 'Submit' : type === 'reset' ? 'Reset' : '');
        else if (t === 'INPUT' && type === 'image') n = el.alt || 'Submit';
        if (!n && el.labels && el.labels.length) n = Array.from(el.labels).map((l) => textOf(l)).join(' ');
        if (!n) n = el.getAttribute('placeholder') || el.getAttribute('aria-placeholder') || el.title || '';
      } else if (t === 'IMG' || t === 'AREA') {
        n = el.getAttribute('alt') || el.title || '';
      } else if (t === 'FIELDSET') {
        const legend = el.querySelector(':scope > legend');
        n = legend ? textOf(legend) : '';
      } else if (t === 'TABLE') {
        n = el.caption ? textOf(el.caption) : '';
      } else if (t === 'FIGURE') {
        const cap = el.querySelector(':scope > figcaption');
        n = cap ? textOf(cap) : '';
      } else if (t === 'SVG' || t === 'svg') {
        const title = el.querySelector(':scope > title');
        n = title ? title.textContent : '';
      } else if (t === 'IFRAME') {
        n = el.title || el.getAttribute('name') || '';
      }
      if (!n.trim() && NAMED_BY_CONTENT.has(role)) {
        n = textOf(el) || '';
        if (!n.trim()) {
          const img = el.querySelector('img[alt], svg[aria-label], [aria-label]');
          if (img) n = img.getAttribute('alt') || img.getAttribute('aria-label') || '';
        }
      }
      if (!n.trim()) n = el.title || '';
    }
    return clip(n, 120);
  };

  // ---- visibility --------------------------------------------------------------------------

  const styleOf = (el) => {
    try { return el.ownerDocument.defaultView.getComputedStyle(el); } catch (_) { return null; }
  };

  // Whether `el` is shown at all. A page laid out at no size (the Browser tool off screen) still
  // has its elements; only styles decide then.
  const shown = (el, st) => {
    if (el.hidden && tag(el) !== 'DIALOG') return false;
    if (el.getAttribute('aria-hidden') === 'true') return false;
    if (tag(el) === 'DIALOG' && !el.open) return false;
    if (!st) return true;
    if (st.display === 'none' || st.visibility === 'hidden' || st.visibility === 'collapse') return false;
    if (st.display === 'contents') return true;
    const view = el.ownerDocument.defaultView;
    if (view.innerWidth > 0 && view.innerHeight > 0) {
      const r = el.getBoundingClientRect();
      if (r.width === 0 && r.height === 0 && st.overflow !== 'visible') return false;
      if (r.width === 0 && r.height === 0 && !el.firstElementChild && !(el.textContent || '').trim() && !['INPUT', 'IMG'].includes(tag(el))) return false;
    }
    return true;
  };

  // ---- refs --------------------------------------------------------------------------------

  const refOf = (el) => {
    let r = el.getAttribute('data-trek-ref');
    if (r) {
      const known = A.refs.get(r);
      const other = known && known.deref();
      // A copy of an element carries its ref along (cloneNode): it gets one of its own.
      if (!other || other === el || !other.isConnected) {
        A.refs.set(r, new WeakRef(el));
        return r;
      }
    }
    r = 'e' + (++A.n);
    el.setAttribute('data-trek-ref', r);
    A.refs.set(r, new WeakRef(el));
    return r;
  };

  // Every document and shadow root on the page Trek can reach: the page's, open shadow roots and
  // same-origin frames.
  const roots = () => {
    const out = [];
    const visit = (root) => {
      out.push(root);
      const all = root.querySelectorAll('*');
      for (const el of all) {
        if (el.shadowRoot) visit(el.shadowRoot);
        if (tag(el) === 'IFRAME' || tag(el) === 'FRAME') {
          let doc = null;
          try { doc = el.contentDocument; } catch (_) {}
          if (doc && doc.documentElement) visit(doc);
        }
      }
    };
    visit(document);
    return out;
  };

  const find = (ref) => {
    ref = String(ref || '').trim().replace(/^\[?ref=/, '').replace(/\]$/, '');
    const known = A.refs.get(ref);
    const el = known && known.deref();
    if (el && el.isConnected) return el;
    const sel = '[data-trek-ref="' + ref.replace(/["\\]/g, '') + '"]';
    for (const root of roots()) {
      const hit = root.querySelector(sel);
      if (hit) { A.refs.set(ref, new WeakRef(hit)); return hit; }
    }
    return null;
  };

  const missing = (ref) => ({ error: 'No element with ref ' + ref + ' on the page any more. Take a new browser_snapshot: the page may have changed.' });

  // ---- geometry and the highlight ---------------------------------------------------------

  // `el`'s box in the top page's viewport (through the frames it's in).
  const topRect = (el) => {
    const r = el.getBoundingClientRect();
    let x = r.left, y = r.top;
    let win = el.ownerDocument.defaultView;
    while (win && win !== window && win.frameElement) {
      const f = win.frameElement.getBoundingClientRect();
      x += f.left; y += f.top;
      win = win.parent;
    }
    return { x, y, width: r.width, height: r.height };
  };

  const flash = (el, label) => {
    try {
      const r = topRect(el);
      let box = document.getElementById('__trek-agent-flash');
      if (!box) {
        box = document.createElement('div');
        box.id = '__trek-agent-flash';
        box.setAttribute('aria-hidden', 'true');
        box.style.cssText = 'position:fixed;pointer-events:none;z-index:2147483647;border:2px solid #FF6A3D;background:rgba(255,106,61,.14);border-radius:4px;box-shadow:0 0 0 4px rgba(255,106,61,.18);transition:opacity 260ms ease-out;';
        const t = document.createElement('div');
        t.style.cssText = 'position:absolute;left:-2px;top:-22px;background:#FF6A3D;color:#fff;font:600 11px -apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;padding:2px 6px;border-radius:4px;white-space:nowrap;';
        box.appendChild(t);
        document.documentElement.appendChild(box);
      }
      Object.assign(box.style, { left: r.x - 2 + 'px', top: r.y - 2 + 'px', width: r.width + 4 + 'px', height: r.height + 4 + 'px', opacity: '1', display: 'block' });
      box.firstChild.textContent = 'Agent · ' + label;
      box.firstChild.style.top = r.y > 26 ? '-22px' : r.height + 6 + 'px';
      clearTimeout(A.flashTimer);
      A.flashTimer = setTimeout(() => {
        box.style.opacity = '0';
        A.flashTimer = setTimeout(() => { box.style.display = 'none'; }, 300);
      }, 1100);
    } catch (_) {}
  };

  const reveal = (el) => {
    try { el.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' }); } catch (_) {
      try { el.scrollIntoView(); } catch (_) {}
    }
  };

  const describe = (el) => {
    const role = roleOf(el) || tag(el).toLowerCase();
    const name = nameOf(el, role);
    return name ? role + ' "' + name + '"' : role;
  };

  const deepActive = () => {
    let a = document.activeElement;
    for (;;) {
      if (a && a.shadowRoot && a.shadowRoot.activeElement) { a = a.shadowRoot.activeElement; continue; }
      if (a && (tag(a) === 'IFRAME' || tag(a) === 'FRAME')) {
        let doc = null;
        try { doc = a.contentDocument; } catch (_) {}
        if (doc && doc.activeElement && doc.activeElement !== doc.body) { a = doc.activeElement; continue; }
      }
      return a;
    }
  };

  // ---- the snapshot ------------------------------------------------------------------------

  const valueOf = (el, role) => {
    const t = tag(el);
    if (t === 'INPUT') {
      const type = (el.getAttribute('type') || 'text').toLowerCase();
      if (['checkbox', 'radio', 'button', 'submit', 'reset', 'image', 'hidden'].includes(type)) return undefined;
      if (type === 'password') return el.value ? '•'.repeat(Math.min(el.value.length, 12)) : '';
      if (type === 'file') return el.files && el.files.length ? Array.from(el.files).map((f) => f.name).join(', ') : '';
      return clip(el.value, 200);
    }
    if (t === 'TEXTAREA') return clip(el.value, 300);
    if (t === 'SELECT') return clip(Array.from(el.selectedOptions).map((o) => o.label || o.text).join(', '), 120);
    if (t === 'PROGRESS' || t === 'METER') return String(el.value);
    if (role === 'textbox' && el.isContentEditable) return clip(textOf(el), 300);
    const now = el.getAttribute('aria-valuetext') || el.getAttribute('aria-valuenow');
    return now == null ? undefined : clip(now, 60);
  };

  const states = (el, role, o) => {
    const t = tag(el);
    const attr = (k) => el.getAttribute(k);
    if (t === 'INPUT' && (el.type === 'checkbox' || el.type === 'radio')) {
      o.checked = el.indeterminate ? 'mixed' : el.checked;
    } else if (attr('aria-checked') != null) {
      o.checked = attr('aria-checked') === 'mixed' ? 'mixed' : attr('aria-checked') === 'true';
    }
    if (attr('aria-pressed') != null) o.pressed = attr('aria-pressed') === 'true';
    if (el.disabled || attr('aria-disabled') === 'true') o.disabled = true;
    if (attr('aria-expanded') != null) o.expanded = attr('aria-expanded') === 'true';
    else if (t === 'DETAILS') o.expanded = el.open;
    else if (t === 'SUMMARY' && el.parentElement && tag(el.parentElement) === 'DETAILS') o.expanded = el.parentElement.open;
    if (t === 'OPTION') { if (el.selected) o.selected = true; }
    else if (attr('aria-selected') === 'true') o.selected = true;
    if (el.required || attr('aria-required') === 'true') o.required = true;
    if (el.readOnly || attr('aria-readonly') === 'true') o.readonly = true;
    if (attr('aria-invalid') === 'true') o.invalid = true;
    if (attr('aria-current') && attr('aria-current') !== 'false') o.current = true;
    if (role === 'heading') {
      const m = /^H([1-6])$/.exec(t);
      o.level = m ? +m[1] : +(attr('aria-level') || 2);
    }
    if (t === 'INPUT') {
      const type = (attr('type') || 'text').toLowerCase();
      if (!['text', 'checkbox', 'radio', 'button', 'submit'].includes(type)) o.type = type;
    }
  };

  const clickable = (el, st) => {
    if (el.hasAttribute('onclick')) return true;
    const tabindex = el.getAttribute('tabindex');
    if (tabindex != null && +tabindex >= 0 && tag(el) !== 'BODY') return true;
    if (st && st.cursor === 'pointer') {
      const p = el.parentElement;
      const ps = p ? styleOf(p) : null;
      return !(ps && ps.cursor === 'pointer');
    }
    return false;
  };

  // Children as the page draws them: a shadow root's in place of the light ones (with slotted
  // nodes where its slots are), a same-origin frame's document.
  const childrenOf = (el) => {
    if (el.shadowRoot) return Array.from(el.shadowRoot.childNodes);
    if (tag(el) === 'SLOT') {
      const assigned = el.assignedNodes({ flatten: true });
      return assigned.length ? assigned : Array.from(el.childNodes);
    }
    return Array.from(el.childNodes);
  };

  A.snapshot = (opts) => {
    opts = opts || {};
    const interactiveOnly = !!opts.interactive_only;
    const max = Math.max(50, Math.min(+opts.max_nodes || 1500, 5000));
    let count = 0;
    let truncated = false;
    const active = deepActive();
    // Text that names a control (its <label>, what aria-labelledby points at) is in the control's
    // line already.
    const labelling = new Set();
    for (const root of roots()) {
      for (const c of root.querySelectorAll('[aria-labelledby]')) {
        for (const id of (c.getAttribute('aria-labelledby') || '').split(/\s+/)) {
          const t = id && (root.getElementById ? root.getElementById(id) : null);
          if (t && !t.contains(c)) labelling.add(t);
        }
      }
    }
    const names = (el) => labelling.has(el) || (tag(el) === 'LABEL' && !!el.control);

    const walk = (node, out, depth) => {
      if (count >= max) { truncated = true; return; }
      if (node.nodeType === 3) {
        if (interactiveOnly) return;
        const s = clip(node.textContent, 300);
        if (s) {
          // Runs of text in one parent read as one line.
          if (typeof out[out.length - 1] === 'string') out[out.length - 1] = clip(out[out.length - 1] + ' ' + s, 600);
          else { out.push(s); count++; }
        }
        return;
      }
      if (!isEl(node) || SKIP_TAGS.has(tag(node)) || ours(node)) return;
      const el = node;
      const st = styleOf(el);
      if (!shown(el, st)) return;
      const role = roleOf(el);
      const editable = role === 'textbox' && el.isContentEditable;
      const interactive = INTERACTIVE.has(role) || editable || (!role && clickable(el, st));
      if (interactive || (role && (STRUCTURE.has(role) || role === 'combobox'))) {
        if (interactiveOnly && !interactive && role !== 'heading' && role !== 'dialog' && role !== 'iframe') {
          for (const c of childrenOf(el)) walk(c, out, depth);
          return;
        }
        const o = { role: role || 'clickable' };
        const name = o.role === 'clickable' && el.querySelector('a[href], button, input, select, textarea, [role]') ? clip(el.getAttribute('aria-label') || el.title || '', 120) : nameOf(el, o.role);
        if (name) o.name = name;
        if (interactive || role === 'iframe') o.ref = refOf(el);
        const v = valueOf(el, role);
        if (v !== undefined && v !== '') o.value = v;
        states(el, role, o);
        if (el === active) o.focused = true;
        if (role === 'link') {
          const href = el.getAttribute('href') || '';
          if (href && !href.startsWith('javascript:')) o.href = el.href;
        }
        count++;
        out.push(o);
        // A link or button reads as its name; its insides aren't walked again (but something
        // that's only clickable may hold controls of its own).
        if (o.role === 'clickable' && !el.querySelector('a[href], button, input, select, textarea, [role], [tabindex], [onclick]')) return;
        if (LEAVES.has(role)) return;
        if (tag(el) === 'SELECT') {
          const opts2 = Array.from(el.options).slice(0, 30);
          o.children = opts2.map((op) => {
            const c = { role: 'option', name: clip(op.label || op.text, 80) };
            if (op.selected) c.selected = true;
            if (op.disabled) c.disabled = true;
            return c;
          });
          if (el.options.length > 30) o.children.push('… ' + (el.options.length - 30) + ' more options');
          return;
        }
        if (tag(el) === 'IFRAME' || tag(el) === 'FRAME') {
          let doc = null;
          try { doc = el.contentDocument; } catch (_) {}
          if (doc && doc.body) {
            o.children = [];
            walk(doc.body, o.children, depth + 1);
          } else {
            o.name = (o.name ? o.name + ' — ' : '') + 'another site\'s frame (' + clip(el.src, 80) + '): its insides can\'t be read';
          }
          return;
        }
        o.children = [];
        for (const c of childrenOf(el)) walk(c, o.children, depth + 1);
        if (!o.children.length) delete o.children;
        // An empty container (a status line with nothing in it yet) says nothing.
        if (!interactive && !o.name && !o.children && !['separator', 'img', 'iframe', 'progressbar', 'meter', 'dialog'].includes(o.role)) {
          out.pop();
          count--;
        }
        return;
      }
      // A label's own words are its control's name: only what else it holds is walked.
      const label = names(el);
      for (const c of childrenOf(el)) if (!label || c.nodeType === 1) walk(c, out, depth);
    };

    const body = [];
    if (document.body) walk(document.body, body, 0);
    const se = document.scrollingElement || document.documentElement;
    return {
      title: document.title,
      url: location.href,
      scroll: { x: Math.round(scrollX), y: Math.round(scrollY), width: innerWidth, height: innerHeight, page_width: se.scrollWidth, page_height: se.scrollHeight },
      focused: active && active !== document.body ? describe(active) : null,
      nodes: body,
      truncated,
    };
  };

  // ---- reading -----------------------------------------------------------------------------

  A.text = () => {
    const main = document.querySelector('main, [role=main], article') || document.body;
    let t = main ? textOf(main) : '';
    t = String(t || '').replace(/[ \t]+\n/g, '\n').replace(/\n{3,}/g, '\n\n').trim();
    return { title: document.title, url: location.href, text: t.length > 60000 ? t.slice(0, 60000) + '\n… (cut at 60,000 characters)' : t };
  };

  A.state = () => {
    const se = document.scrollingElement || document.documentElement;
    return {
      title: document.title,
      url: location.href,
      ready: document.readyState,
      x: Math.round(scrollX), y: Math.round(scrollY),
      width: innerWidth, height: innerHeight,
      page_width: se ? se.scrollWidth : 0, page_height: se ? se.scrollHeight : 0,
      dpr: devicePixelRatio,
    };
  };

  // What an action on `ref` would do, before it's done: for Trek's approvals.
  A.probe = (ref, key) => {
    const el = ref ? find(ref) : deepActive();
    if (!el) return ref ? missing(ref) : { describe: 'the page', password: false, page: location.href };
    const t = tag(el);
    const type = (el.getAttribute('type') || '').toLowerCase();
    const auto = (el.getAttribute('autocomplete') || '').toLowerCase();
    const out = {
      describe: describe(el),
      password: t === 'INPUT' && (type === 'password' || auto.includes('password')),
      page: location.href,
      disabled: !!(el.disabled || el.getAttribute('aria-disabled') === 'true'),
    };
    const link = el.closest('a[href], area[href]');
    if (link) {
      out.href = link.href;
      if (link.hasAttribute('download')) out.download = link.href;
    }
    // Does acting on it submit a form, and where to?
    const form = el.form || el.closest('form');
    let submits = false;
    if (form) {
      if (key === 'Enter') submits = t === 'INPUT' && TEXT_TYPES.has(type);
      else if (t === 'BUTTON') submits = (type || 'submit') === 'submit';
      else if (t === 'INPUT') submits = type === 'submit' || type === 'image';
    }
    if (submits) {
      const action = (t !== 'FORM' && el.getAttribute('formaction')) ? el.formAction : form.action;
      out.submits = { action: action || location.href, method: (form.method || 'get').toLowerCase() };
    }
    return out;
  };

  // ---- acting ------------------------------------------------------------------------------

  const mouse = (el, type, x, y, extra) => {
    const view = el.ownerDocument.defaultView;
    const init = Object.assign({ bubbles: true, cancelable: true, composed: true, clientX: x, clientY: y, screenX: x, screenY: y, view, button: 0, buttons: 0, detail: 1 }, extra || {});
    const pointer = type.startsWith('pointer');
    const E = pointer ? (view.PointerEvent || view.MouseEvent) : view.MouseEvent;
    if (pointer) Object.assign(init, { pointerId: 1, pointerType: 'mouse', isPrimary: true, width: 1, height: 1, pressure: init.buttons ? 0.5 : 0 });
    if (type === 'mouseenter' || type === 'mouseleave' || type === 'pointerenter' || type === 'pointerleave') init.bubbles = false;
    return el.dispatchEvent(new E(type, init));
  };

  const center = (el) => {
    const r = el.getBoundingClientRect();
    return [r.left + r.width / 2, r.top + r.height / 2];
  };

  // What's drawn over `el`'s middle, when it isn't `el` (or inside it): a click would land there.
  const coveredBy = (el) => {
    const [x, y] = center(el);
    const view = el.ownerDocument.defaultView;
    if (!(x >= 0 && y >= 0 && x < view.innerWidth && y < view.innerHeight)) return null;
    let hit = el.getRootNode().elementFromPoint ? el.getRootNode().elementFromPoint(x, y) : el.ownerDocument.elementFromPoint(x, y);
    if (!hit || hit === el || el.contains(hit) || hit.contains(el) || ours(hit)) return null;
    // A label for the control counts as the control.
    if (tag(hit) === 'LABEL' && hit.control === el) return null;
    return describe(hit);
  };

  A.hover = (ref) => {
    const el = find(ref);
    if (!el) return missing(ref);
    reveal(el);
    flash(el, 'Hover');
    const [x, y] = center(el);
    for (const t of ['pointerover', 'pointerenter', 'mouseover', 'mouseenter', 'pointermove', 'mousemove']) mouse(el, t, x, y);
    return { did: 'Hovered over ' + describe(el) };
  };

  A.click = (ref) => {
    let el = find(ref);
    if (!el) return missing(ref);
    if (tag(el) === 'OPTION' && el.parentElement) return A.select(refOf(el.closest('select') || el.parentElement), el.value);
    const what = describe(el);
    if (el.disabled || el.getAttribute('aria-disabled') === 'true') return { error: what + ' is disabled: nothing happens when it\'s clicked.' };
    reveal(el);
    flash(el, 'Click');
    const covered = coveredBy(el);
    const [x, y] = center(el);
    for (const t of ['pointerover', 'pointerenter', 'mouseover', 'mouseenter', 'pointermove', 'mousemove']) mouse(el, t, x, y);
    const down = mouse(el, 'pointerdown', x, y, { buttons: 1 });
    let ok = down && mouse(el, 'mousedown', x, y, { buttons: 1 });
    if (ok) {
      const focusable = el.closest('a[href], button, input, select, textarea, summary, [tabindex], [contenteditable=""], [contenteditable=true]');
      try { if (focusable) focusable.focus({ preventScroll: true }); } catch (_) {}
    }
    mouse(el, 'pointerup', x, y);
    mouse(el, 'mouseup', x, y);
    // The click itself, with the element's own default action (a link, a checkbox, submit).
    el.click();
    const out = { did: 'Clicked ' + what };
    if (covered) out.note = 'It was under ' + covered + ', which may have taken the click on screen.';
    return out;
  };

  const nativeSetter = (el) => {
    const proto = tag(el) === 'TEXTAREA' ? el.ownerDocument.defaultView.HTMLTextAreaElement.prototype : tag(el) === 'SELECT' ? el.ownerDocument.defaultView.HTMLSelectElement.prototype : el.ownerDocument.defaultView.HTMLInputElement.prototype;
    const d = Object.getOwnPropertyDescriptor(proto, 'value');
    return d && d.set ? (v) => d.set.call(el, v) : (v) => { el.value = v; };
  };

  const editableOf = (el) => {
    const t = tag(el);
    if (t === 'TEXTAREA') return 'field';
    if (t === 'INPUT' && TEXT_TYPES.has((el.getAttribute('type') || '').toLowerCase())) return 'field';
    if (el.isContentEditable) return 'rich';
    return null;
  };

  const keyCodeOf = (key) => {
    const named = { Enter: 13, Tab: 9, Escape: 27, Backspace: 8, ' ': 32, ArrowLeft: 37, ArrowUp: 38, ArrowRight: 39, ArrowDown: 40, Delete: 46, PageUp: 33, PageDown: 34, Home: 36, End: 35, Shift: 16, Control: 17, Alt: 18, Meta: 91 };
    if (key in named) return named[key];
    if (key.length === 1) {
      const c = key.toUpperCase().charCodeAt(0);
      if ((c >= 65 && c <= 90) || (c >= 48 && c <= 57)) return c;
      return key.charCodeAt(0);
    }
    if (/^F\d+$/.test(key)) return 111 + +key.slice(1);
    return 0;
  };

  const codeOf = (key) => {
    if (key.length === 1) {
      if (/[a-z]/i.test(key)) return 'Key' + key.toUpperCase();
      if (/[0-9]/.test(key)) return 'Digit' + key;
      if (key === ' ') return 'Space';
      return '';
    }
    return key;
  };

  const keyEvent = (el, type, k) => {
    const view = el.ownerDocument.defaultView;
    const e = new view.KeyboardEvent(type, { key: k.key, code: k.code || codeOf(k.key), bubbles: true, cancelable: true, composed: true, view, ctrlKey: !!k.ctrl, shiftKey: !!k.shift, altKey: !!k.alt, metaKey: !!k.meta });
    const code = k.keyCode || keyCodeOf(k.key);
    try {
      Object.defineProperty(e, 'keyCode', { get: () => (type === 'keypress' && k.key.length === 1 ? k.key.charCodeAt(0) : code) });
      Object.defineProperty(e, 'which', { get: () => (type === 'keypress' && k.key.length === 1 ? k.key.charCodeAt(0) : code) });
      Object.defineProperty(e, 'charCode', { get: () => (type === 'keypress' && k.key.length === 1 ? k.key.charCodeAt(0) : 0) });
    } catch (_) {}
    return el.dispatchEvent(e);
  };

  // Put `text` where the caret is in `el`, as typing would: the editing command fires the
  // beforeinput and input events a browser fires for typing. Where it can't (a field type that
  // doesn't take it), the value is set through the element's own setter and the input event
  // sent by hand.
  const insert = (el, text, kind) => {
    const before = kind === 'field' ? el.value : textOf(el);
    let done = false;
    try { done = el.ownerDocument.execCommand('insertText', false, text); } catch (_) {}
    const after = kind === 'field' ? el.value : textOf(el);
    if (done && after !== before) return;
    // At its maxlength a field takes nothing more, typed or not.
    if (done && kind === 'field' && el.maxLength >= 0 && el.value.length >= el.maxLength) return;
    if (kind === 'field') {
      let start = el.value.length, end = el.value.length;
      try { if (el.selectionStart != null) { start = el.selectionStart; end = el.selectionEnd; } } catch (_) {}
      const v = el.value.slice(0, start) + text + el.value.slice(end);
      nativeSetter(el)(v);
      try { el.setSelectionRange(start + text.length, start + text.length); } catch (_) {}
    } else {
      el.textContent = before + text;
    }
    const view = el.ownerDocument.defaultView;
    el.dispatchEvent(new view.InputEvent('input', { bubbles: true, composed: true, inputType: 'insertText', data: text }));
  };

  const selectAll = (el, kind) => {
    if (kind === 'field') {
      try { el.select(); } catch (_) {}
      try { el.setSelectionRange(0, el.value.length); } catch (_) {}
    } else {
      const sel = el.ownerDocument.getSelection();
      const range = el.ownerDocument.createRange();
      range.selectNodeContents(el);
      sel.removeAllRanges();
      sel.addRange(range);
    }
  };

  const caretToEnd = (el, kind) => {
    if (kind === 'field') {
      try { el.setSelectionRange(el.value.length, el.value.length); } catch (_) {}
    } else {
      const sel = el.ownerDocument.getSelection();
      const range = el.ownerDocument.createRange();
      range.selectNodeContents(el);
      range.collapse(false);
      sel.removeAllRanges();
      sel.addRange(range);
    }
  };

  const deleteSelection = (el, kind) => {
    let done = false;
    try { done = el.ownerDocument.execCommand('delete', false); } catch (_) {}
    if (done) return;
    if (kind === 'field') {
      nativeSetter(el)('');
      const view = el.ownerDocument.defaultView;
      el.dispatchEvent(new view.InputEvent('input', { bubbles: true, composed: true, inputType: 'deleteContentBackward' }));
    } else {
      el.textContent = '';
    }
  };

  A.type = (ref, text, clear) => {
    const el = find(ref);
    if (!el) return missing(ref);
    if (tag(el) === 'SELECT') return { error: describe(el) + ' is a drop-down: choose with browser_select.' };
    const kind = editableOf(el);
    if (!kind) return { error: describe(el) + ' doesn\'t take text. Type into a text field, a text area or an editable area.' };
    if (el.disabled || el.readOnly) return { error: describe(el) + ' is ' + (el.disabled ? 'disabled' : 'read-only') + '.' };
    reveal(el);
    flash(el, 'Type');
    try { el.focus({ preventScroll: true }); } catch (_) {}
    if (clear !== false) {
      selectAll(el, kind);
      if ((kind === 'field' ? el.value : textOf(el)) !== '') deleteSelection(el, kind);
    } else {
      caretToEnd(el, kind);
    }
    text = String(text);
    // Key by key for ordinary lengths (autocomplete and masks listen to the keys), in one go
    // past that.
    const chunks = text.length <= 400 ? Array.from(text) : [text];
    for (const ch of chunks) {
      if (ch === '\n' && kind === 'field' && tag(el) === 'INPUT') continue;
      const k = { key: ch.length === 1 ? (ch === '\n' ? 'Enter' : ch) : 'Unidentified' };
      const go = keyEvent(el, 'keydown', k);
      if (go) {
        if (ch.length === 1) keyEvent(el, 'keypress', k);
        if (ch === '\n' && kind === 'rich') {
          try { el.ownerDocument.execCommand('insertParagraph', false); } catch (_) {}
        } else {
          insert(el, ch, kind);
        }
      }
      keyEvent(el, 'keyup', k);
    }
    const view = el.ownerDocument.defaultView;
    if (kind === 'field') el.dispatchEvent(new view.Event('change', { bubbles: true }));
    const value = kind === 'field' ? el.value : textOf(el);
    const shownValue = tag(el) === 'INPUT' && el.type === 'password' ? '•'.repeat(Math.min(value.length, 12)) : clip(value, 200);
    return { did: 'Typed into ' + describe(el), value: shownValue };
  };

  // The form `el` would submit with Enter: by its default button, as a browser does.
  const submitFrom = (el) => {
    const form = el.form || el.closest('form');
    if (!form) return false;
    const button = Array.from(form.elements).find((b) => (tag(b) === 'BUTTON' && (b.getAttribute('type') || 'submit').toLowerCase() === 'submit') || (tag(b) === 'INPUT' && ['submit', 'image'].includes((b.type || '').toLowerCase())));
    if (button) {
      if (!button.disabled) button.click();
      return true;
    }
    try { form.requestSubmit(); } catch (_) { form.submit(); }
    return true;
  };

  const tabbables = (doc) => Array.from(doc.querySelectorAll('a[href], button, input, select, textarea, summary, iframe, [tabindex], [contenteditable=""], [contenteditable=true]')).filter((e) => {
    if (e.disabled || (e.getAttribute('tabindex') != null && +e.getAttribute('tabindex') < 0)) return false;
    if (tag(e) === 'INPUT' && e.type === 'hidden') return false;
    return shown(e, styleOf(e));
  });

  const scrollPage = (dy, dx) => {
    const se = document.scrollingElement || document.documentElement;
    if (dy && se.scrollHeight <= innerHeight + 1 || dx && se.scrollWidth <= innerWidth + 1) {
      const box = scrollerUnder(dy !== 0);
      if (box) { box.scrollBy({ left: dx || 0, top: dy || 0, behavior: 'instant' }); return box; }
    }
    window.scrollBy({ left: dx || 0, top: dy || 0, behavior: 'instant' });
    return null;
  };

  // The biggest panel on screen that scrolls that way: pages that keep the window still and
  // scroll a box inside it.
  const scrollerUnder = (vertical) => {
    let best = null, area = 0;
    for (const el of document.querySelectorAll('*')) {
      const can = vertical ? el.scrollHeight > el.clientHeight + 1 : el.scrollWidth > el.clientWidth + 1;
      if (!can) continue;
      const st = styleOf(el);
      const o = vertical ? st.overflowY : st.overflowX;
      if (o !== 'auto' && o !== 'scroll' && o !== 'overlay') continue;
      const r = el.getBoundingClientRect();
      const a = r.width * r.height;
      if (a > area) { area = a; best = el; }
    }
    return best;
  };

  A.press = (key, ref) => {
    // `key` arrives parsed by Trek: {key, code, keyCode, ctrl, shift, alt, meta, label}.
    let el = ref ? find(ref) : deepActive();
    if (ref && !el) return missing(ref);
    if (!el || el === document.documentElement) el = document.body;
    if (ref) { reveal(el); try { el.focus({ preventScroll: true }); } catch (_) {} }
    if (el !== document.body) flash(el, key.label || key.key);
    const kind = editableOf(el);
    const go = keyEvent(el, 'keydown', key);
    let did = '';
    const plain = !key.ctrl && !key.meta && !key.alt;
    if (go) {
      const k = key.key;
      if (k.length === 1 && plain) {
        keyEvent(el, 'keypress', key);
        if (kind) insert(el, k, kind);
        else if (k === ' ' && el.matches && el.matches('button, summary, [role=button], input[type=checkbox], input[type=radio], [role=checkbox], [role=switch]')) { el.click(); did = ' (it clicked ' + describe(el) + ')'; }
        else if (k === ' ') scrollPage(innerHeight * 0.85);
      } else if (k === 'Enter' && plain) {
        keyEvent(el, 'keypress', key);
        if (tag(el) === 'TEXTAREA' || kind === 'rich') {
          try { el.ownerDocument.execCommand(tag(el) === 'TEXTAREA' ? 'insertLineBreak' : 'insertParagraph', false); } catch (_) {}
        } else if (kind === 'field') {
          if (submitFrom(el)) did = ' (it submitted the form)';
        } else if (el.matches && el.matches('a[href], button, summary, [role=button], [role=link], [role=menuitem], [role=tab], [role=option]')) {
          el.click(); did = ' (it clicked ' + describe(el) + ')';
        }
      } else if (k === 'Tab' && !key.ctrl && !key.meta) {
        const list = tabbables(el.ownerDocument);
        const at = list.indexOf(el);
        const next = list.length ? list[(at + (key.shift ? -1 : 1) + list.length) % list.length] : null;
        if (next) { next.focus(); did = ' (focus moved to ' + describe(next) + ')'; }
      } else if ((k === 'Backspace' || k === 'Delete') && kind) {
        let done = false;
        try { done = el.ownerDocument.execCommand(k === 'Backspace' ? 'delete' : 'forwardDelete', false); } catch (_) {}
        if (!done && kind === 'field') {
          const s = el.selectionStart, e = el.selectionEnd;
          let v = el.value;
          if (s !== e) v = v.slice(0, s) + v.slice(e);
          else if (k === 'Backspace' && s > 0) v = v.slice(0, s - 1) + v.slice(s);
          else if (k === 'Delete') v = v.slice(0, s) + v.slice(s + 1);
          nativeSetter(el)(v);
          el.dispatchEvent(new el.ownerDocument.defaultView.InputEvent('input', { bubbles: true, inputType: k === 'Backspace' ? 'deleteContentBackward' : 'deleteContentForward' }));
        }
      } else if ((key.ctrl || key.meta) && k.toLowerCase() === 'a' && kind) {
        selectAll(el, kind);
      } else if (tag(el) === 'SELECT' && (k === 'ArrowDown' || k === 'ArrowUp')) {
        const i = Math.max(0, Math.min(el.options.length - 1, el.selectedIndex + (k === 'ArrowDown' ? 1 : -1)));
        if (i !== el.selectedIndex) {
          el.selectedIndex = i;
          el.dispatchEvent(new Event('input', { bubbles: true }));
          el.dispatchEvent(new Event('change', { bubbles: true }));
        }
      } else if (!kind && plain) {
        const page = innerHeight * 0.85;
        const by = { ArrowDown: [0, 40], ArrowUp: [0, -40], ArrowRight: [40, 0], ArrowLeft: [-40, 0], PageDown: [0, page], PageUp: [0, -page] }[k];
        if (by) scrollPage(by[1], by[0]);
        else if (k === 'End') window.scrollTo({ top: 1e9, behavior: 'instant' });
        else if (k === 'Home') window.scrollTo({ top: 0, behavior: 'instant' });
      }
    }
    keyEvent(el, 'keyup', key);
    return { did: 'Pressed ' + (key.label || key.key) + (el === document.body ? ' in the page' : ' in ' + describe(el)) + did + (go ? '' : ' (the page handled it itself)') };
  };

  const norm = (s) => String(s || '').replace(/\s+/g, ' ').trim().toLowerCase();

  A.select = (ref, option) => {
    let el = find(ref);
    if (!el) return missing(ref);
    if (tag(el) === 'OPTION') el = el.closest('select') || el;
    if (tag(el) !== 'SELECT') return { custom: true, describe: describe(el) };
    const want = norm(option);
    const opts = Array.from(el.options);
    const hit = opts.find((o) => o.value === option) || opts.find((o) => norm(o.label || o.text) === want) || opts.find((o) => norm(o.value) === want) || opts.find((o) => norm(o.label || o.text).includes(want));
    if (!hit) return { error: 'No option "' + option + '" in ' + describe(el) + '. Its options: ' + opts.slice(0, 30).map((o) => '"' + clip(o.label || o.text, 40) + '"').join(', ') };
    if (hit.disabled) return { error: 'The option "' + clip(hit.label || hit.text, 60) + '" is disabled.' };
    reveal(el);
    flash(el, 'Choose');
    try { el.focus({ preventScroll: true }); } catch (_) {}
    if (el.multiple) hit.selected = true; else nativeSetter(el)(hit.value);
    if (!hit.selected) hit.selected = true;
    el.dispatchEvent(new Event('input', { bubbles: true }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
    return { did: 'Chose "' + clip(hit.label || hit.text, 80) + '" in ' + describe(el) };
  };

  // An option of an ARIA listbox on screen whose text is `option`: its ref (a custom select's
  // options appear only once it's open).
  A.option = (option) => {
    const want = norm(option);
    let partial = null;
    for (const root of roots()) {
      for (const o of root.querySelectorAll('[role=option], [role=menuitem], [role=menuitemradio], [role=treeitem]')) {
        if (!shown(o, styleOf(o))) continue;
        const name = norm(nameOf(o, 'option'));
        if (name === want || norm(o.getAttribute('data-value')) === want) return { ref: refOf(o) };
        if (!partial && name.includes(want)) partial = o;
      }
    }
    return partial ? { ref: refOf(partial) } : { none: true };
  };

  A.scroll = (dir, amount, ref) => {
    if (ref) {
      const el = find(ref);
      if (!el) return missing(ref);
      reveal(el);
      flash(el, 'Scroll');
      return Object.assign({ did: 'Scrolled ' + describe(el) + ' into view' }, A.state());
    }
    const h = amount || innerHeight * 0.85, w = amount || innerWidth * 0.85;
    const by = { down: [0, h], up: [0, -h], right: [w, 0], left: [-w, 0] }[dir || 'down'];
    if (!by) return { error: 'Scroll up, down, left or right.' };
    const box = scrollPage(by[1], by[0]);
    const s = A.state();
    if (box) {
      return Object.assign(s, { did: 'Scrolled ' + describe(box) + ' ' + dir + ' (the window itself doesn\'t scroll)', box_y: Math.round(box.scrollTop), box_height: box.scrollHeight });
    }
    return Object.assign(s, { did: 'Scrolled ' + (dir || 'down') });
  };

  A.scrollTo = (y) => { window.scrollTo({ left: scrollX, top: y, behavior: 'instant' }); return A.state(); };

  A.check = (text, selector, gone) => {
    let found = false;
    if (selector) {
      for (const root of roots()) {
        let hit = null;
        try { hit = root.querySelector(selector); } catch (e) { return { error: 'That isn\'t a CSS selector: ' + e.message }; }
        if (hit && shown(hit, styleOf(hit))) { found = true; break; }
      }
    } else {
      const want = String(text);
      for (const root of roots()) {
        const body = root.body || root;
        const t = body && (body.innerText != null ? body.innerText : body.textContent);
        if (t && t.includes(want)) { found = true; break; }
        // Field values aren't in the text.
        for (const f of (root.querySelectorAll ? root.querySelectorAll('input, textarea') : [])) {
          if (f.type !== 'password' && f.value && f.value.includes(want)) { found = true; break; }
        }
        if (found) break;
      }
    }
    return { met: gone ? !found : found };
  };

  // ---- scripts the agent runs (browser_eval) ----------------------------------------------

  const safe = (v) => {
    const seen = new WeakSet();
    const out = JSON.stringify(v, (k, x) => {
      if (typeof x === 'bigint') return x.toString() + 'n';
      if (typeof x === 'function') return '[function ' + (x.name || 'anonymous') + ']';
      if (typeof x === 'symbol') return x.toString();
      if (x === undefined) return null;
      if (x && typeof x === 'object') {
        if (isEl(x)) return '[element ' + describe(x) + ']';
        if (x.nodeType) return '[node ' + x.nodeName + ']';
        if (x === window) return '[window]';
        if (seen.has(x)) return '[circular]';
        seen.add(x);
        if (x instanceof Map) return Object.fromEntries(x);
        if (x instanceof Set) return Array.from(x);
        if (x instanceof Error) return { error: x.message, stack: String(x.stack || '').slice(0, 1000) };
      }
      return x;
    });
    return out === undefined ? 'undefined' : out;
  };

  A.hold = (id, promise) => {
    A.results[id] = null;
    Promise.resolve(promise).then(
      (v) => { A.results[id] = { value: safe(v) }; },
      (e) => { A.results[id] = { error: String((e && e.message) || e) }; },
    );
    return { started: id };
  };

  A.take = (id) => {
    const r = A.results[id];
    if (r) delete A.results[id];
    return r ? r : { waiting: id in A.results };
  };

  try {
    Object.defineProperty(window, '__trekAgent', { value: A, configurable: true, enumerable: false, writable: true });
  } catch (_) {
    window.__trekAgent = A;
  }
})();
