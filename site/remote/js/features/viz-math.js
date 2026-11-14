// Math in answers, typeset as the Mac and the iPhone do: `$…$` and `\(…\)` in a line of text,
// `$$…$$` and `\[…\]` centred on their own (set inline in a table cell or a heading).
//
// Formulas are found in the Markdown source before marked reads it, by the Mac's own rules
// (crates/trek-math/src/detect.rs, byte for byte as the iPhone's MathDetect.swift): marked would
// take `\[` for an escaped bracket, `\\` for an escaped backslash and `a_1 … b_1` for emphasis,
// and nothing after it could tell what was written. So `markdownMath`, a marked extension, hides
// each formula behind a placeholder while marked reads the text (`preprocess`) and puts it back
// as a `.md-math` span holding its TeX (`postprocess`); `typesetMath`, run on the drawn answer,
// sets each span with KaTeX, loaded the first time an answer has math.
//
// Dollars are also money and shell variables, so a single `$` only opens math the way Pandoc has
// it: not followed by a space, and closed by a `$` that isn't preceded by a space or followed by a
// digit ("costs $5 and $10" stays prose). Code (fenced blocks and code spans) is never math, and
// an escaped `\$` is a dollar. A formula still being written shows as written until it closes;
// one KaTeX can't set shows its source, as code.

const INLINE_MAX = 2000;
/** Bigger "formulas" are more likely a mistake than math: shown as written. */
const TEX_MAX = 20_000;

const DELIMS = {
  dollar: ['$', '$'],
  dollars: ['$$', '$$'],
  paren: ['\\(', '\\)'],
  bracket: ['\\[', '\\]'],
};

export const isDisplay = (delim) => delim === 'dollars' || delim === 'bracket';

const isSpace = (c) => c === ' ' || c === '\t' || c === '\n' || c === '\f' || c === '\r';

/** The length of the character at `i`, in UTF-16 units. */
function charLen(s, i) {
  const c = s.charCodeAt(i);
  return c >= 0xd800 && c < 0xdc00 && i + 1 < s.length ? 2 : 1;
}

function runOf(s, i, c) {
  let n = 0;
  while (s[i + n] === c) n++;
  return n;
}

/** The line after the line ending at `nl` is blank (or a quote's empty line), or there is none. */
function blankLineAfter(s, nl) {
  for (let k = nl + 1; k < s.length && s[k] !== '\n'; k++) {
    if (!(s[k] === ' ' || s[k] === '\t' || s[k] === '\r' || s[k] === '>')) return false;
  }
  return true;
}

/** A line that starts a list item or heading: inline math can't run into one. */
function startsBlock(line) {
  let l = line.replace(/^[ \t\n\f\r]+/, '');
  if (l.startsWith('>')) l = l.slice(1).replace(/^[ \t\n\f\r]+/, '');
  if (/^[-*+] /.test(l) || l.startsWith('#')) return true;
  return /^\d+[.)] /.test(l);
}

/** Where the code span opened by `run` backticks at `i` ends: past a run of exactly as many,
 * before a blank line or `limit` (a fence). */
function codeSpanEnd(s, i, run, limit) {
  let j = i + run;
  while (j < limit) {
    if (s[j] === '`') {
      const n = runOf(s, j, '`');
      if (n === run) return j + n;
      j += n;
    } else if (s[j] === '\n' && blankLineAfter(s, j)) {
      return null;
    } else {
      j += 1;
    }
  }
  return null;
}

/** A line without its quote markers, list marker and indent. */
function fenceBody(line) {
  let s = line.trimStart();
  while (s.startsWith('>')) s = s.slice(1).trimStart();
  for (const marker of ['- ', '* ', '+ ']) if (s.startsWith(marker)) return s.slice(2).trimStart();
  const m = /^\d+[.)] /.exec(s);
  return m ? s.slice(m[0].length).trimStart() : s;
}

function fenceMarker(body) {
  const c = body[0];
  if (c !== '`' && c !== '~') return null;
  const n = runOf(body, 0, c);
  return n >= 3 ? { c, n } : null;
}

/** The fenced code blocks in `s`, as `[start, end)`; one never closed runs to the end. */
function fences(s) {
  const out = [];
  let open = null;
  let offset = 0;
  while (offset < s.length) {
    const start = offset;
    const nl = s.indexOf('\n', start);
    offset = nl < 0 ? s.length : nl + 1;
    const body = fenceBody(s.slice(start, offset));
    const m = fenceMarker(body);
    if (!open) {
      // A backtick fence's info string has no backticks.
      if (m && !(m.c === '`' && body.slice(m.n).includes('`'))) open = { start, ...m };
    } else if (m && m.c === open.c && m.n >= open.n && body.slice(m.n).trim() === '') {
      out.push([open.start, offset]);
      open = null;
    }
  }
  if (open) out.push([open.start, s.length]);
  return out;
}

/** The formula `delim` opened at `i` and hasn't closed, to the end of the text. */
function openRegion(s, i, delim) {
  return { start: i, end: Math.max(s.trimEnd().length, i + DELIMS[delim][0].length), delim, closed: false };
}

/** The `$` at `j` closes the one before `start`: something between, no space before it, no digit
 * after it ("$5 and $10"), and a letter after it only after a letter, digit or bracket (`$n$th`,
 * not `$PATH:$HOME`). */
function dollarCloses(s, start, j) {
  if (j === start) return false;
  const before = s[j - 1];
  if (isSpace(before)) return false;
  const c = s[j + 1];
  if (c === undefined) return true;
  if (c >= '0' && c <= '9') return false;
  if (/[A-Za-z_]/.test(c)) return /[A-Za-z0-9})\]']/.test(before);
  return true;
}

/** The formula `delim` opens at `i`, if it closes, or (the text ending first, an answer still
 * being written) the open one running to its end. `$` never stays open: it's as often money as
 * math. `dead.at` is where the scan gave up: nothing opened before it can close either. */
function closing(s, i, delim, limit, dead) {
  const [open, close] = DELIMS[delim];
  const start = i + open.length;
  if (delim === 'dollar' && (start >= s.length || isSpace(s[start]))) return null;
  const inline = !isDisplay(delim);
  let j = start;
  while (j < limit) {
    if (inline && j - start > INLINE_MAX) return null;
    const c = s[j];
    if (c === '\\') {
      if (close[0] === '\\' && s.startsWith(close, j)) {
        if (s.slice(start, j).trim() === '') return null;
        return { start: i, end: j + 2, delim, closed: true };
      }
      j += 1 + (j + 1 < s.length ? charLen(s, j + 1) : 0);
    } else if (c === '$' && (delim === 'dollar' || delim === 'dollars')) {
      if (delim === 'dollars') {
        if (s[j + 1] === '$') {
          if (s.slice(start, j).trim() === '') return null;
          return { start: i, end: j + 2, delim, closed: true };
        }
        j += 1;
        continue;
      }
      // A single dollar closes, or it was never math.
      return dollarCloses(s, start, j) ? { start: i, end: j + 1, delim, closed: true } : null;
    } else if (c === '\n') {
      if (blankLineAfter(s, j)) {
        dead.at = j;
        // A paragraph can't hold a formula past its end; at the end of the text, it's still
        // being written.
        const restBlank = /^[ \t\n\f\r]*$/.test(s.slice(j));
        return restBlank && delim !== 'dollar' && limit === s.length ? openRegion(s, i, delim) : null;
      }
      if (inline) {
        const nl = s.indexOf('\n', j + 1);
        if (startsBlock(s.slice(j + 1, nl < 0 ? s.length : nl))) return null;
      }
      j += 1;
    } else {
      j += charLen(s, j);
    }
  }
  dead.at = limit;
  return delim !== 'dollar' && limit === s.length ? openRegion(s, i, delim) : null;
}

/** Every formula in `s`, in order: `{ start, end, delim, closed }`, delimiters included. */
export function mathRegions(s) {
  const code = fences(s);
  let f = 0;
  const out = [];
  let i = 0;
  // `\[` openers before this can't close: an earlier one scanned to the end of its paragraph.
  let deadBracket = 0;
  while (i < s.length) {
    while (f < code.length && code[f][1] <= i) f++;
    if (f < code.length && code[f][0] <= i) {
      i = code[f][1];
      continue;
    }
    const nextFence = f < code.length ? code[f][0] : s.length;
    const c = s[i];
    if (c === '\\') {
      const delim = s[i + 1] === '(' ? 'paren' : s[i + 1] === '[' ? 'bracket' : null;
      if (delim === 'bracket' && i < deadBracket) {
        i += 2;
      } else if (delim) {
        const dead = { at: 0 };
        const region = closing(s, i, delim, nextFence, dead);
        if (region) {
          i = region.end;
          out.push(region);
        } else {
          if (delim === 'bracket') deadBracket = Math.max(deadBracket, dead.at);
          i += 2;
        }
      } else {
        // An escape: whatever it escapes is literal.
        i += 1 + (i + 1 < s.length ? charLen(s, i + 1) : 0);
      }
    } else if (c === '`') {
      const run = runOf(s, i, '`');
      i = codeSpanEnd(s, i, run, nextFence) ?? i + run;
    } else if (c === '$') {
      const delim = s[i + 1] === '$' ? 'dollars' : 'dollar';
      const region = closing(s, i, delim, nextFence, { at: 0 });
      if (region) {
        i = region.end;
        out.push(region);
      } else {
        i += DELIMS[delim][0].length;
      }
    } else {
      i += charLen(s, i);
    }
  }
  return out;
}

function unquote(line) {
  for (;;) {
    const trimmed = line.trimStart();
    if (!trimmed.startsWith('>')) return line;
    const rest = trimmed.slice(1);
    line = rest.startsWith(' ') ? rest.slice(1) : rest;
  }
}

/** The TeX between a region's delimiters. Lines of a formula in a block quote lose their `>`. */
export function regionTex(region, s) {
  const [open, close] = DELIMS[region.delim];
  let inner = s.slice(region.start, region.end);
  if (inner.startsWith(open)) inner = inner.slice(open.length);
  if (region.closed && inner.endsWith(close)) inner = inner.slice(0, -close.length);
  const lineStart = region.start > 0 ? s.lastIndexOf('\n', region.start - 1) + 1 : 0;
  if (!s.slice(lineStart, region.start).trimStart().startsWith('>') || !inner.includes('\n')) return inner;
  return inner
    .split('\n')
    .map((line, i) => (i === 0 ? line : unquote(line)))
    .join('\n');
}

// ---------------------------------------------------------------------------------------------
// The marked extension

const PLACEHOLDER = 0xf0000;
/** A placeholder: a character from the supplementary private use area, which nothing writes and
 * Markdown leaves alone (the iPhone's). */
const PLACEHOLDERS = /[\u{F0000}-\u{FFFFD}]/gu;
const ENCODED = /%F3%B[0-F]%[89AB][0-9A-F]%[89AB][0-9A-F]/gi;

const escapeHtml = (s) => s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);

/** The formulas hidden from the text marked is reading now (`parse` is synchronous). */
let reading = null;

/** `markdown` with each formula behind a placeholder, marked to read. */
export function hideMath(markdown) {
  reading = null;
  if (!markdown.includes('$') && !markdown.includes('\\(') && !markdown.includes('\\[')) return markdown;
  const regions = mathRegions(markdown);
  if (!regions.length) return markdown;
  let out = '';
  let at = 0;
  regions.forEach((r, n) => {
    out += markdown.slice(at, r.start) + String.fromCodePoint(PLACEHOLDER + n);
    at = r.end;
  });
  reading = { source: markdown, regions: regions.slice(0, 0xfffd) };
  return out + markdown.slice(at);
}

/** The formula behind placeholder `n`, as HTML for where it landed. */
function formulaHtml(job, n, where) {
  const r = job.regions[n];
  if (!r) return '';
  const source = job.source.slice(r.start, r.end);
  // In a tag (a link's title, a picture's alt text) or in code, it's text as written.
  if (where === 'tag' || where === 'code' || !r.closed) return escapeHtml(source);
  const tex = regionTex(r, job.source);
  if (tex.length > TEX_MAX) return escapeHtml(source);
  return `<span class="md-math" data-delim="${r.delim}">${escapeHtml(tex)}</span>`;
}

/** marked's HTML with the formulas back: TeX in `.md-math` spans for `typesetMath`. */
export function restoreMath(html) {
  const job = reading;
  reading = null;
  if (!job) return html;
  const at = (ph) => ph.codePointAt(0) - PLACEHOLDER;
  // A display formula alone in its paragraph becomes a block of its own.
  html = html.replace(/<p>([\u{F0000}-\u{FFFFD}])<\/p>/gu, (whole, ph) => {
    const r = job.regions[at(ph)];
    return r && r.closed && isDisplay(r.delim) ? `<div class="md-math-block">${formulaHtml(job, at(ph), 'text')}</div>` : whole;
  });
  let out = '';
  let code = 0;
  let i = 0;
  while (i < html.length) {
    const lt = html.indexOf('<', i);
    const text = html.slice(i, lt < 0 ? html.length : lt);
    out += text.replace(PLACEHOLDERS, (ph) => formulaHtml(job, at(ph), code > 0 ? 'code' : 'text'));
    if (lt < 0) break;
    const gt = html.indexOf('>', lt);
    const tag = html.slice(lt, gt < 0 ? html.length : gt + 1);
    const name = /^<(\/?)(code|pre)\b/i.exec(tag);
    if (name) code = Math.max(0, code + (name[1] ? -1 : 1));
    // Inside a tag: an attribute's text, or a link's address (which marked percent-encodes).
    out += tag
      .replace(PLACEHOLDERS, (ph) => formulaHtml(job, at(ph), 'tag'))
      .replace(ENCODED, (hex) => {
        const ph = decodeURIComponent(hex);
        const r = job.regions[at(ph)];
        return r ? escapeHtml(encodeURI(job.source.slice(r.start, r.end)).replace(/%25/g, '%')) : '';
      });
    i = gt < 0 ? html.length : gt + 1;
  }
  return out;
}

/** The marked extension, for `marked.use(markdownMath)`: formulas reach `typesetMath` whole. */
export const markdownMath = { hooks: { preprocess: hideMath, postprocess: restoreMath } };

// ---------------------------------------------------------------------------------------------
// Typesetting

let katex = null;
let loading = null;
const waiting = new Set();
/** Set formulas by display and TeX: an answer streaming in draws its formulas again and again. */
const made = new Map();
const KEEP = 600;

/** KaTeX and its styles, fetched the first time an answer has math (pages without pay nothing). */
function load() {
  if (loading) return loading;
  const css = new Promise((resolve) => {
    const link = document.createElement('link');
    link.rel = 'stylesheet';
    link.href = new URL('../../vendor/katex/katex.min.css', import.meta.url).href;
    link.addEventListener('load', resolve, { once: true });
    // Without its styles a formula still reads, if not well: set it anyway.
    link.addEventListener('error', resolve, { once: true });
    document.head.append(link);
  });
  loading = Promise.all([import('../../vendor/katex/katex.mjs'), css]).then(
    ([module]) => {
      katex = module.default;
      const now = [...waiting];
      waiting.clear();
      for (const el of now) {
        if (!el.isConnected) continue;
        try {
          setOne(el);
        } catch {
          // Left as its TeX.
        }
      }
    },
    () => {
      // Not fetched (offline before it was ever kept): the formulas stay as written.
      waiting.clear();
      loading = null;
    },
  );
  return loading;
}

/** A formula KaTeX can't set: its source in a code chip, as `code` in an answer looks. */
function sourceChip(tex, delim, display) {
  const [open, close] = DELIMS[delim];
  const code = document.createElement('code');
  code.className = display ? 'md-math-source block' : 'md-math-source';
  // Word joiners keep the delimiters with the formula when the line breaks.
  code.textContent = `${open}⁠${tex}⁠${close}`;
  return code;
}

function build(tex, delim, display) {
  const holder = document.createElement('span');
  try {
    katex.render(tex, holder, { displayMode: display, throwOnError: true, strict: 'ignore', trust: false, output: 'htmlAndMathml', maxExpand: 1000 });
    return holder;
  } catch {
    holder.replaceChildren(sourceChip(tex, delim, display));
    return holder;
  }
}

function setOne(el) {
  if (el.dataset.set) return;
  const delim = DELIMS[el.dataset.delim] ? el.dataset.delim : 'dollar';
  const tex = el.textContent;
  // Display math sits in a line where a block can't go: a table cell or a heading.
  const display = isDisplay(delim) && !el.closest('td, th, h1, h2, h3, h4, h5, h6');
  const key = `${display ? 'D' : 'I'}${delim}\n${tex}`;
  let node = made.get(key);
  if (!node) {
    node = build(tex, delim, display);
    made.set(key, node);
    if (made.size > KEEP) made.delete(made.keys().next().value);
  }
  // KaTeX's MathML, hidden from sight, is what a screen reader reads.
  el.replaceChildren(...node.cloneNode(true).childNodes);
  el.dataset.set = display ? 'display' : 'inline';
}

/** Typeset the formulas `restoreMath` left in `root`. Idempotent: a formula already set is left. */
export function typesetMath(root) {
  const spans = root.querySelectorAll('.md-math:not([data-set])');
  if (!spans.length) return;
  if (katex) {
    for (const el of spans) {
      try {
        setOne(el);
      } catch {
        // Left as its TeX.
      }
    }
    return;
  }
  for (const el of spans) waiting.add(el);
  load();
}
