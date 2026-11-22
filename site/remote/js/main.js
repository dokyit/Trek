// The web remote's start: a pairing link opens the pairing screen (the code is taken out of the
// address at once, so it isn't kept in the history or a home-screen shortcut); otherwise the
// machine used last connects, and the screens follow the route.

import { render } from '../vendor/preact.mjs';
import { html } from './ui.js';
import * as app from './model.js';
import { parsePairing } from './link.js';
import { openTarget } from './push.js';
import { apply } from './look.js';
import { ThreadScreen } from './thread.js';
import { MachinesScreen, NewThreadScreen, PairScreen, SettingsScreen, ThreadsScreen, WelcomeScreen } from './screens.js';
import { screens } from './routes.js';
import './features/index.js';

apply();

function App() {
  const state = app.useApp();
  const r = state.route;
  let screen;
  switch (r.screen) {
    case 'pair':
      screen = html`<${PairScreen} />`;
      break;
    case 'welcome':
      screen = html`<${WelcomeScreen} />`;
      break;
    case 'thread':
      screen = html`<${ThreadScreen} key=${r.threadId} threadId=${r.threadId} />`;
      break;
    case 'new':
      screen = html`<${NewThreadScreen} projectId=${r.projectId} />`;
      break;
    case 'machines':
      screen = html`<${MachinesScreen} />`;
      break;
    case 'settings':
      screen = html`<${SettingsScreen} />`;
      break;
    default: {
      const Feature = screens.get(r.screen);
      if (Feature) screen = html`<${Feature} ...${r} />`;
      else screen = state.machine ? html`<${ThreadsScreen} />` : html`<${WelcomeScreen} />`;
    }
  }
  return html`${screen}
    <div class="toasts" aria-live="polite">${state.toasts.map((t) => html`<div key=${t.id} class=${`toast ${t.tone}`}>${t.text}</div>`)}</div>`;
}

async function start() {
  const hash = location.hash;
  const pairing = parsePairing(hash);
  if (pairing) {
    try {
      history.replaceState(null, '', location.pathname + location.search);
    } catch {
      // The code stays in the address of a sandboxed page.
    }
  }
  const machines = await app.loadMachines();
  if (pairing) {
    // The link itself too: an iPhone's Safari copies it into the Home Screen app, which keeps a
    // pairing of its own (`PairScreen`).
    app.state.pairing = { hostId: pairing.hostId, name: pairing.name, routes: pairing.routes, certs: pairing.certs, pairing: { code: pairing.code }, push: pairing.push, link: location.origin + location.pathname + hash };
    // Never pairs by itself (as the iPhone app doesn't): any page can open a pairing link, one
    // naming a computer that isn't the user's among them. Connect pairs, once the user has read
    // whose it is.
    app.go({ screen: 'pair' }, { replace: true });
  } else if (new URLSearchParams(location.search).has('demo')) {
    app.startDemo();
  } else if (machines.length) {
    let last = null;
    try {
      last = localStorage.getItem('trek-remote-last');
    } catch {
      // The newest, then.
    }
    // A notification tapped opens its thread, on the computer it came from.
    const target = openTarget(hash);
    app.use(machines.find((m) => target?.host && m.hostId === target.host) || machines.find((m) => m.hostId === last) || machines[0]);
    app.go(target ? { screen: 'thread', threadId: target.thread } : { screen: 'threads' }, { replace: true });
  } else {
    app.go({ screen: 'welcome' }, { replace: true });
  }
  render(html`<${App} />`, document.getElementById('app'));
  if ('serviceWorker' in navigator && location.protocol === 'https:') {
    navigator.serviceWorker.register('/remote/sw.js', { scope: '/remote/' }).catch(() => {});
    // A notification tapped while the remote is open: to its thread (`sw.js`).
    navigator.serviceWorker.addEventListener('message', (event) => {
      const { type, thread, host } = event.data || {};
      if (type !== 'open' || !thread) return;
      const machine = host && app.state.machines.find((m) => m.hostId === host);
      if (machine && app.state.machine?.hostId !== host) app.use(machine);
      app.go({ screen: 'thread', threadId: thread });
    });
  }
}

start();
