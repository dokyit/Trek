// Settings › Notifications: banners on this phone when a thread needs you, finishes or fails,
// sent by the computer itself through the browser's push service (`../push.js`, feature
// `web_push`). Permission is asked from the switch, as browsers require. On an iPhone or iPad only
// a Home Screen app gets notifications, so Safari is told how to add it instead.

import { useEffect, useState } from '../../vendor/hooks.mjs';
import { html, Icon, Spinner, Switch } from '../ui.js';
import * as app from '../model.js';
import { registerSettingsSection } from '../routes.js';
import { asksComputer, environment, madeWith, notifyRow, pushSupport, serverKey, subscriptionFor } from '../push.js';

async function registration() {
  return navigator.serviceWorker.ready;
}

/** Ask for permission (from the tap), subscribe with the computer's key and hand it over. */
async function turnOn(key) {
  const permission = await Notification.requestPermission();
  if (permission !== 'granted') {
    throw new Error(permission === 'denied' ? 'Notifications are blocked for this site. Allow them in the browser’s site settings.' : 'Notifications weren’t allowed.');
  }
  const reg = await registration();
  let sub = await reg.pushManager.getSubscription();
  // A browser keeps one subscription, made with one computer's key: another's is replaced.
  if (sub && !madeWith(sub, key)) {
    await sub.unsubscribe().catch(() => {});
    sub = null;
  }
  sub = sub || (await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: serverKey(key) }));
  return app.ask({ type: 'web_push', subscribe: subscriptionFor(sub) });
}

async function turnOff(key) {
  const answer = await app.ask({ type: 'web_push', unsubscribe: true });
  try {
    const sub = await (await registration()).pushManager.getSubscription();
    if (sub && madeWith(sub, key)) await sub.unsubscribe();
  } catch {
    // The computer has stopped sending: that's what counts.
  }
  return answer;
}

/** Whether this browser still holds a subscription made with `key`. */
async function holds(key) {
  try {
    const sub = await (await registration()).pushManager.getSubscription();
    return !!sub && madeWith(sub, key);
  } catch {
    return false;
  }
}

export function NotificationsSection() {
  const state = app.useApp();
  const online = state.status === 'online';
  const served = state.features.has('web_push');
  const [push, setPush] = useState(null); // the computer's answer: { key, subscribed, enabled }
  const [here, setHere] = useState(false); // this browser still has the subscription
  const [busy, setBusy] = useState(false);
  const [support, setSupport] = useState(() => pushSupport(environment()));

  useEffect(() => {
    if (!asksComputer({ online, features: state.features })) return undefined;
    let live = true;
    app
      .ask({ type: 'web_push' })
      .then(async (answer) => {
        if (!live) return;
        setPush(answer);
        setHere(await holds(answer.key));
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [online, served, state.machine?.hostId]);

  if (!state.machine || state.machine.demo) return null;
  const name = state.machine.name;
  const on = !!push?.subscribed && here;

  const toggle = async (want) => {
    if (!push?.key || busy) return;
    setBusy(true);
    try {
      const answer = want ? await turnOn(push.key) : await turnOff(push.key);
      setPush(answer);
      setHere(want);
      app.toast(want ? `${name} will tell this phone when a thread needs you` : 'Notifications are off for this phone');
    } catch (err) {
      app.toast(err.message, 'negative');
    } finally {
      setSupport(pushSupport(environment()));
      setBusy(false);
    }
  };

  const row = notifyRow({ served, support, online, push, here });
  // A Trek from before notifications on the phone: nothing here, as before.
  if (!row) return null;
  const words = {
    install: ['Add Trek to your Home Screen first', 'On an iPhone or iPad only a Home Screen app gets notifications. In Safari, tap Share, then Add to Home Screen, open Trek from there and pair it again: it keeps its own pairing.'],
    unsupported: ['Not in this browser', 'It can’t show notifications from a page. Chrome, Edge and Samsung Internet can.'],
    asking: ['Notifications', `Asking ${name}…`],
    offline: ['Notifications', `Connect to ${name} to turn them on.`],
    denied: ['Notifications', 'Blocked for this site. Allow notifications in the browser’s site settings, then come back.'],
    'off-there': ['Notifications', `Off on ${name} for now: turn on “Tell my phone when a thread needs me” in its Settings › Phone & Machines.`],
    on: ['Notifications', `When a thread needs you, finishes or fails, and you’re away from ${name} (as its settings say).`],
    off: ['Notifications', `${name} sends them itself, straight to this phone’s browser: no account, no other app.`],
  }[row];
  const control = ['on', 'off', 'off-there'].includes(row)
    ? busy
      ? html`<${Spinner} />`
      : html`<${Switch} id="notify-switch" on=${on} label="Notifications on this phone" onChange=${(v) => (online ? toggle(v) : null)} />`
    : row === 'asking'
      ? html`<${Spinner} />`
      : null;
  return html`<h2 class="group-title">This phone’s notifications</h2>
    <div class="group">
      <div class="item" id=${`notify-${row}`}><${Icon} name="bell" /><div class="grow">${words[0]}<div class="sub">${words[1]}</div></div>${control}</div>
    </div>
    ${support === 'ready' && state.machines.length > 1 && html`<p class="mac-foot">A browser takes notifications from one computer at a time: turning them on here stops them from the others.</p>`}`;
}

registerSettingsSection(NotificationsSection);
