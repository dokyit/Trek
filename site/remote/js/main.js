// The web remote's start: a pairing link opens the pairing screen (the code is taken out of the
// address at once, so it isn't kept in the history or a home-screen shortcut); otherwise the
// machine used last connects, and the screens follow the route.

import { render } from '../vendor/preact.mjs';
import { html } from './ui.js';
import * as app from './model.js';
import { parsePairing } from './link.js';
import { localNetworkPermission } from './client.js';
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
    app.state.pairing = { hostId: pairing.hostId, name: pairing.name, routes: pairing.routes, certs: pairing.certs, pairing: { code: pairing.code } };
    app.go({ screen: 'pair' }, { replace: true });
    // A browser that already lets the page on the network pairs at once; otherwise Connect asks.
    if ((await localNetworkPermission()) === 'granted') app.use(app.state.pairing);
  } else if (new URLSearchParams(location.search).has('demo')) {
    app.startDemo();
  } else if (machines.length) {
    let last = null;
    try {
      last = localStorage.getItem('trek-remote-last');
    } catch {
      // The newest, then.
    }
    app.use(machines.find((m) => m.hostId === last) || machines[0]);
    const thread = hash.match(/^#t=(.+)$/);
    app.go(thread ? { screen: 'thread', threadId: decodeURIComponent(thread[1]) } : { screen: 'threads' }, { replace: true });
  } else {
    app.go({ screen: 'welcome' }, { replace: true });
  }
  render(html`<${App} />`, document.getElementById('app'));
  if ('serviceWorker' in navigator && location.protocol === 'https:') {
    navigator.serviceWorker.register('/remote/sw.js', { scope: '/remote/' }).catch(() => {});
  }
}

start();
