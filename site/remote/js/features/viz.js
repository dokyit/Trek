// Answers drawn beyond plain Markdown, as the Mac and the iPhone draw them: ```trek-viz blocks
// become charts and diagrams (viz-spec.js reads them, viz-card.js draws them), and TeX math is
// typeset (viz-math.js).
//
// The transcript draws an answer's Markdown again each time it changes (an answer streaming in),
// and this runs after each drawing, so it does little each time: a block already drawn for the
// same JSON gets its card back (the same element, with what was picked on it) rather than being
// read and drawn again, and a formula already set is copied from the one set before.

import * as routes from '../routes.js';
import { parse, parsePartial, unwrapInnerFence } from './viz-spec.js';
import { card, pending, refused } from './viz-card.js';
import { markdownMath, typesetMath } from './viz-math.js';

/** The marked extension that hands formulas to `typesetMath` whole (see viz-math.js). */
export { markdownMath };

/** Cards drawn, by whether their block was still open and its JSON: each a few elements, reused
 * once the transcript's redraw has let go of them. Blocks still being written keep only their
 * latest few. */
const kept = { closed: new Map(), open: new Map() };
const KEEP = { closed: 80, open: 4 };

function take(which, key) {
  const list = kept[which].get(key);
  const node = list?.find((n) => !n.isConnected);
  if (node) {
    kept[which].delete(key);
    kept[which].set(key, list);
  }
  return node || null;
}

function keep(which, key, node) {
  const map = kept[which];
  const list = map.get(key) || [];
  list.push(node);
  map.delete(key);
  map.set(key, list.slice(-3));
  while (map.size > KEEP[which]) map.delete(map.keys().next().value);
}

/** Nothing in `root` comes after `node`: where a fence still open runs to. */
function lastBlock(node, root) {
  for (let n = node; n && n !== root; n = n.parentNode) {
    for (let s = n.nextSibling; s; s = s.nextSibling) {
      if (s.nodeType === 1 || (s.nodeType === 3 && s.textContent.trim())) return false;
    }
  }
  return true;
}

/** The answer is still being written: the transcript's cursor follows it. */
function streaming(root) {
  const next = root.nextElementSibling;
  return !!next && next.textContent.includes('▍');
}

/** What a chart has to lay out in: the answer's width less the card's edges and padding. */
const chartWidth = (root) => Math.max((root.clientWidth || 360) - 30, 200);

/** A block's card, its skeleton, or the note saying why it can't be drawn. `open`: its fence is
 * still being written; `ended`: it's the last thing in an answer that's finished. */
function build(json, open, ended, width) {
  if (!open) {
    let viz;
    try {
      viz = parse(json);
    } catch (err) {
      // An answer that ended with its last block still open: as the iPhone says it.
      const cut = ended && !parsePartial(json).complete;
      return refused(cut ? 'the answer ended before the block was finished' : err.reason || String(err.message || err));
    }
    return card(viz, { width });
  }
  const partial = parsePartial(json);
  if (partial.drawable) return card(partial.drawable, { drawing: !partial.complete, width });
  return pending(partial.kind, partial.title);
}

/** Every ```trek-viz block in `root` (marked's `pre > code.language-trek-viz`) as its card, or
 * left as code under a note saying why it can't be drawn. */
export function drawVisualizations(root, ctx = {}) {
  const codes = root.querySelectorAll('pre > code.language-trek-viz');
  if (!codes.length) return;
  const live = ctx.live ?? streaming(root);
  const width = chartWidth(root);
  for (const code of codes) {
    const pre = code.parentElement;
    if (pre.dataset.viz) continue;
    const text = code.textContent;
    const json = unwrapInnerFence(text);
    // A fence still open runs to the end of the answer, so only the last block can be one.
    const atEnd = lastBlock(pre, root);
    const open = live && atEnd;
    const which = open ? 'open' : 'closed';
    const key = `${!live && atEnd ? 'end' : ''}\n${json}`;
    let node = take(which, key);
    try {
      if (node) {
        node.classList.remove('enter');
        node.vizFit?.(width);
      } else {
        node = build(json, open, !live && atEnd, width);
        keep(which, key, node);
      }
    } catch {
      // A block this can't draw stays as code rather than costing the answer its drawing.
      continue;
    }
    if (node.classList.contains('viz-refused')) {
      pre.dataset.viz = 'refused';
      pre.before(node);
      continue;
    }
    // The closing line of a ```json fence inside the block left an empty block after it.
    const next = pre.nextElementSibling;
    if (json !== text && next?.tagName === 'PRE' && !next.textContent.trim()) next.remove();
    pre.replaceWith(node);
  }
}

routes.registerMarkdownEnhancer((root, ctx) => {
  drawVisualizations(root, ctx);
  typesetMath(root);
});

// Formulas have to be taken out of the Markdown before marked reads it (viz-math.js says why), so
// math needs the transcript's marked to use `markdownMath`: through `registerMarkdownExtension`
// where routes.js has it. Without it an answer's math shows as written, as before.
routes.registerMarkdownExtension?.(markdownMath);
