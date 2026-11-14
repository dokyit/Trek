// "Match my computer": the remote drawn in the Trek theme the computer has on (its own, or the
// user's or an agent's theme file), from the snapshot's `look` (docs/MOBILE.md, "The Mac's
// look"), as the iPhone's Settings › Appearance › Match your Mac does (Looks.swift). The theme's
// colours for Night or Paper, whichever the remote is in, become the remote's colour tokens (what
// it leaves out stays Trek's); its interface and conversation faces, its text sizes (gently: the
// computer's points are for a desk) and its corners follow. The background art stays on the
// computer: the remote has none to show. On by default, as on the iPhone; kept on this phone.
//
// Nothing from the look reaches a style unchecked: colours must be `#RRGGBB[AA]`, fonts are names
// mapped to the remote's own stacks, numbers are clamped to the Mac's ranges.

import { useState } from '../../vendor/hooks.mjs';
import { Switch, html } from '../ui.js';
import * as app from '../model.js';
import { registerSettingsSection } from '../routes.js';

const SETTING = 'trek-remote-match-computer';
/** The last look the computer sent, so the remote opens in it before the computer says again. */
const KEPT = 'trek-remote-computer-look';

/** Trek's own sizes on the Mac (points), and the remote's for them (px). */
const TREK_UI = 14;
const TREK_CONVERSATION = 14.5;
const TEXT_PX = 15;
const CONVO_PX = 15.5;
const RADII = { '--radius': 12, '--radius-s': 8, '--radius-l': 18 };

const SERIF = "ui-serif, 'Iowan Old Style', 'Palatino Linotype', Palatino, Georgia, 'Noto Serif', 'Times New Roman', serif";
const FACES = { mono: 'var(--mono)', serif: SERIF };

/** Every token a look may set, so turning it off (or a new look) clears the last one. */
const TOKENS = ['--bg', '--sidebar', '--surface', '--raised', '--border', '--border-soft', '--hover', '--fg', '--fg-strong', '--muted', '--accent', '--on-accent', '--link', '--font', '--look-convo', '--text', '--convo', ...Object.keys(RADII)];

/** `#RRGGBB` or `#RRGGBBAA` as `[r, g, b, a]` (0–255, alpha 0–1); anything else isn't a colour. */
export function colour(text) {
  const m = /^\s*#([0-9a-f]{6})([0-9a-f]{2})?\s*$/i.exec(typeof text === 'string' ? text : '');
  if (!m) return null;
  const n = parseInt(m[1], 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255, m[2] ? parseInt(m[2], 16) / 255 : 1];
}

const hex = ([r, g, b], a = 1) => `#${[r, g, b].map((v) => v.toString(16).padStart(2, '0')).join('')}${a < 1 ? Math.round(a * 255).toString(16).padStart(2, '0') : ''}`;

/** WCAG contrast between two colours. */
function contrast(a, b) {
  const lum = ([r, g, b2]) => {
    const c = [r, g, b2].map((v) => {
      const s = v / 255;
      return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
    });
    return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
  };
  const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p);
  return (x + 0.05) / (y + 0.05);
}

const clamp = (v, lo, hi, fallback) => (typeof v === 'number' && Number.isFinite(v) ? Math.min(Math.max(v, lo), hi) : fallback);
const round = (v) => Math.round(v * 100) / 100;

/** The tokens `look` sets for Night (`dark`) or Paper, as the Mac makes its kit theme from a
 * theme's colours (themes.rs): the window the ground, the accent the links' colour too, with text
 * on it in `on_accent` (else white or near-black, whichever reads). Only what the look sets. */
export function lookStyle(look, dark) {
  const theme = look?.theme;
  const out = {};
  if (!theme || typeof theme !== 'object') return out;
  const palette = (dark ? theme.dark : theme.light) || {};
  const pick = (key) => colour(palette[key] ?? palette[key.replace(/_(.)/g, (_, c) => c.toUpperCase())]);
  const set = (key, names, opaque) => {
    const c = pick(key);
    if (c) for (const name of names) out[name] = hex(c, opaque ? 1 : c[3]);
    return c;
  };
  const window = set('window', ['--bg'], true);
  set('sidebar', ['--sidebar'], true);
  const surface = set('surface', ['--surface'], false);
  set('raised', ['--raised'], false);
  const border = set('border', ['--border'], false);
  const text = set('text', ['--fg', '--fg-strong'], false);
  set('muted', ['--muted'], false);
  const accent = set('accent', ['--accent', '--link'], true);
  if (!set('on_accent', ['--on-accent'], true) && accent) out['--on-accent'] = contrast(accent, [255, 255, 255]) >= 3 ? '#ffffff' : '#111111';
  // The quieter rules and the pressed rows, from the look's own colours.
  if (border || window) out['--border-soft'] = 'color-mix(in srgb, var(--border) 65%, var(--bg))';
  if (surface || text) out['--hover'] = 'color-mix(in srgb, var(--fg) 6%, var(--surface))';
  if (FACES[theme.ui_font]) out['--font'] = FACES[theme.ui_font];
  if (FACES[theme.conversation_font]) out['--look-convo'] = FACES[theme.conversation_font];
  // The Mac's points scaled to the remote's sizes, halfway: a phone's text is already sized for it.
  const gently = (size, trek, px) => `${round(px * (1 + (size / trek - 1) / 2))}px`;
  if (typeof theme.ui_size === 'number') out['--text'] = gently(clamp(theme.ui_size, 11, 18, TREK_UI), TREK_UI, TEXT_PX);
  if (typeof theme.conversation_size === 'number') out['--convo'] = gently(clamp(theme.conversation_size, 12, 22, TREK_CONVERSATION), TREK_CONVERSATION, CONVO_PX);
  const corners = clamp(theme.corners, 0, 2, 1);
  if (corners !== 1) for (const [name, px] of Object.entries(RADII)) out[name] = `${round(px * corners)}px`;
  return out;
}

// ---------------------------------------------------------------------------------------------
// On the page

function matching() {
  try {
    return localStorage.getItem(SETTING) !== 'off';
  } catch {
    return true;
  }
}

function setMatching(on) {
  try {
    if (on) localStorage.removeItem(SETTING);
    else localStorage.setItem(SETTING, 'off');
  } catch {
    // Chosen for this visit.
  }
}

let look = null;
try {
  look = JSON.parse(localStorage.getItem(KEPT) || 'null');
} catch {
  // Trek's own until the computer says.
}

/** Draw the remote in `next` (none: Trek's own), or in the look it has when not given. */
export function applyLook(next = look) {
  const root = document.documentElement;
  for (const name of TOKENS) root.style.removeProperty(name);
  const vars = matching() ? lookStyle(next, root.dataset.scheme !== 'light') : {};
  for (const [name, value] of Object.entries(vars)) root.style.setProperty(name, value);
  if (vars['--look-convo']) root.dataset.lookConvo = '';
  else delete root.dataset.lookConvo;
  // The browser's own bar in the ground's colour; Trek's own is look.js's Night or Paper.
  const meta = document.querySelector('meta[name="theme-color"]');
  meta?.setAttribute('content', vars['--bg'] || (root.dataset.scheme === 'light' ? '#ffffff' : '#0a0a0b'));
}

/** The computer's look, from its snapshot: none is Trek's own (an older computer sends none). */
function took(next) {
  look = next && typeof next === 'object' ? next : null;
  try {
    if (look) localStorage.setItem(KEPT, JSON.stringify(look));
    else localStorage.removeItem(KEPT);
  } catch {
    // Not kept: the next visit waits for the computer.
  }
  applyLook();
}

if (typeof document !== 'undefined') {
  applyLook();
  app.onMessage((msg) => {
    if (msg.type === 'snapshot') took(app.state.look);
  });
  // Night and Paper have their own colours in a look: follow the remote's switch between them.
  new MutationObserver(() => applyLook()).observe(document.documentElement, { attributes: true, attributeFilter: ['data-scheme'] });
}

function MatchComputer() {
  const state = app.useApp();
  const [on, setOn] = useState(matching);
  const name = state.machine?.name || 'your computer';
  const shown = state.look || look;
  const detail = shown?.theme?.name ? `${shown.theme.name} on ${name}` : `Trek’s own look until ${name} says which it has on`;
  return html`<div class="group">
    <div class="item">
      <div class="grow">Match my computer<div class="sub">${detail}</div></div>
      <${Switch}
        on=${on}
        label="Match my computer"
        onChange=${(v) => {
          setOn(v);
          setMatching(v);
          applyLook();
        }}
      />
    </div>
  </div>`;
}

registerSettingsSection(MatchComputer);
