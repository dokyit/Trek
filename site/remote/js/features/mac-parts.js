// Pieces the computer's own screens share (features/mac*.js): the screen's frame, "Asking your
// computer…", a sheet that asks before something can't be taken back, an agent's mark, a limit's
// bar, what the computer last answered (so a screen opened again shows it at once), and asking
// again once the connection is back.

import { useEffect, useRef } from '../../vendor/hooks.mjs';
import { html, Icon, IconButton, Sheet, Spinner, addIcons } from '../ui.js';
import { ConnectionBanner } from '../thread.js';
import * as app from '../model.js';
import { usedTone } from './mac-text.js';

// Lucide (ISC licence), as ui.js's own.
addIcons({
  'mac-circle-x': '<circle cx="12" cy="12" r="10"/><path d="m15 9-6 6"/><path d="m9 9 6 6"/>',
  'mac-circle-check': '<circle cx="12" cy="12" r="10"/><path d="m9 12 2 2 4-4"/>',
  'mac-inbox': '<polyline points="22 12 16 12 14 15 10 15 8 12 2 12"/><path d="M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z"/>',
  'mac-lock': '<rect width="18" height="11" x="3" y="11" rx="2" ry="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/>',
  'mac-users': '<path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M22 21v-2a4 4 0 0 0-3-3.87"/><path d="M16 3.13a4 4 0 0 1 0 7.75"/>',
  'mac-monitor-cog': '<path d="M12 17v4"/><path d="m14.305 7.53.923-.382"/><path d="m15.228 4.852-.923-.383"/><path d="m16.852 3.228-.383-.924"/><path d="m16.852 8.772-.383.923"/><path d="m19.148 3.228.383-.924"/><path d="m19.53 9.696-.382-.924"/><path d="m20.772 4.852.924-.383"/><path d="m20.772 7.148.924.383"/><path d="M22 13v2a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h7"/><path d="M8 21h8"/><circle cx="18" cy="6" r="3"/>',
  'mac-bell-ring': '<path d="M10.268 21a2 2 0 0 0 3.464 0"/><path d="M22 8c0-2.3-.8-4.3-2-6"/><path d="M3.262 15.326A1 1 0 0 0 4 17h16a1 1 0 0 0 .74-1.673C19.41 13.956 18 12.499 18 8A6 6 0 0 0 6 8c0 4.499-1.411 5.956-2.738 7.326"/><path d="M4 2C2.8 3.7 2 5.7 2 8"/>',
});

/** A screen of the computer's: Back, its title (and a line under it), what it offers top right, the connection's banner, its body. */
export function Frame({ title, sub, actions, children, id }) {
  return html`<div class="screen mac-screen" id=${id}>
    <header class="bar">
      <${IconButton} icon="back" label="Back" onClick=${() => app.back()} />
      <div class="bar-title"><h1>${title}</h1>${sub && html`<div class="sub">${sub}</div>`}</div>
      ${actions}
    </header>
    <${ConnectionBanner} />
    <div class="scroll mac-scroll">${children}</div>
  </div>`;
}

/** "Asking your computer…": the screen waits for its first answer. */
export function Asking({ text = 'Asking your computer…' }) {
  return html`<div class="mac-asking" role="status"><${Spinner} /><span>${text}</span></div>`;
}

/** A section's title, with a count beside it (none at zero). */
export function Title({ text, count }) {
  return html`<h2 class="mac-title">${text}${count > 0 && html` <span>${count}</span>`}</h2>`;
}

/** What a group of settings or rows is for, under it. */
export function Foot({ children }) {
  return html`<p class="mac-foot">${children}</p>`;
}

/** Asks before something that can't be taken back, in the computer's words: Cancel, or `action`. */
export function Confirm({ title, text, action, danger = false, onConfirm, onClose }) {
  return html`<${Sheet} title=${title} onClose=${onClose}>
    <p class="mac-confirm">${text}</p>
    <div class="row-btns mac-sheet-btns">
      <button class="btn" type="button" onClick=${onClose}>Cancel</button>
      <button class=${`btn ${danger ? 'danger' : 'primary'}`} type="button" onClick=${() => {
        onClose();
        onConfirm();
      }}>${action}</button>
    </div>
  <//>`;
}

/** An agent's mark: a neutral glyph with its name (the site keeps no agents' logos). */
export function AgentMark({ agent }) {
  return html`<span class="mac-agent" title=${agent?.name || ''} aria-hidden="true"><${Icon} name="bot" size="xs" /></span>`;
}

/** How much of a limit is used, in the Mac's colours (red from 90%, amber from 70%). */
export function LimitBar({ percent }) {
  const p = Math.min(Math.max(Number(percent) || 0, 0), 100);
  return html`<div class="mac-limit-bar" aria-hidden="true"><span class=${usedTone(p)} style=${{ width: `${p}%`, opacity: p > 0 ? 1 : 0 }}></span></div>`;
}

// ---------------------------------------------------------------------------------------------
// What each computer last answered, for this visit: a screen opened again shows it at once and
// asks again behind it.

const kept = new Map();

function keyFor(what) {
  return `${app.state.machine?.hostId || ''}\u0000${what}`;
}

export function remember(what, value) {
  kept.set(keyFor(what), value);
  return value;
}

export function recall(what) {
  return kept.get(keyFor(what)) ?? null;
}

/**
 * Runs `load` once the computer is connected, and again when `deps` change while it is (and
 * after a reconnect). Returns whether it's connected.
 */
export function useOnline(load, deps = []) {
  const online = app.useApp().status === 'online';
  const latest = useRef(load);
  latest.current = load;
  useEffect(() => {
    if (online) latest.current();
  }, [online, ...deps]);
  return online;
}

/** False once the component is gone: an answer that comes after is dropped. */
export function useMounted() {
  const mounted = useRef(true);
  useEffect(() => () => {
    mounted.current = false;
  }, []);
  return mounted;
}
