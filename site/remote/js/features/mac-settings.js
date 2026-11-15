// The computer's settings a phone may see and change (`settings`, `set_settings`: an allowlist),
// grouped and worded as the iPhone's Trek settings for a machine: new threads, the composer, the
// inbox, interruptions, hand-holding and the permission rules, what this phone may grant (the
// computer's say alone), failing checks, the computer's own alerts, notifications to this phone
// through ntfy, and its theme. A change shows at once and goes to the computer, whose answer
// replaces it, or puts it back with the computer's words when it refuses. Also the section the
// remote's Settings screen shows for the computer.

import { useEffect, useRef, useState } from '../../vendor/hooks.mjs';
import { html, Icon, Segmented, Sheet, Switch, ProjectBadge, copy } from '../ui.js';
import * as app from '../model.js';
import { Frame, Asking, Confirm, Foot, useOnline, useMounted, recall, remember } from './mac-parts.js';
import { ACCESS, accessLabel, applySettings, effortLabel, effortsFor, httpsAddress, isNtfySh, newThreadsLine, platformWord, systemThemeLabel } from './mac-text.js';

const SETTLE = [
  [0, 'Never'],
  [1, '1 day'],
  [3, '3 days'],
  [7, '1 week'],
];
const ALERTS = [
  ['banner_and_sound', 'Banner and sound'],
  ['banner', 'Banner'],
  ['sound', 'Sound'],
  ['off', 'Off'],
];
const WHEN = [
  ['away', 'When I’m away'],
  ['always', 'Always'],
];
/** ntfy's Android app opens its subscribe link; elsewhere the topic is copied and pasted. */
const ANDROID = typeof navigator !== 'undefined' && /Android/i.test(navigator.userAgent);

/** The settings as the computer last gave them: the connection's first read, or this screen's. */
function known() {
  const s = app.state.macSettings;
  return s?.default_agent ? s : recall('settings') || s;
}

export function ComputerSettingsScreen() {
  const state = app.useApp();
  const [s, setS] = useState(known);
  const [platform, setPlatform] = useState(() => recall('platform'));
  const [picking, setPicking] = useState(false);
  const confirmed = useRef(s);
  const mounted = useMounted();

  // The answer is the computer's word: kept, shown, and given to the approval cards (which read
  // `session_approvals` and `phone_rules` from it).
  const take = (reply) => {
    confirmed.current = reply;
    remember('settings', reply);
    app.state.macSettings = reply;
    app.changed();
    if (mounted.current) setS(reply);
  };

  useOnline(() => {
    app
      .ask({ type: 'settings' })
      .then(take)
      .catch((err) => mounted.current && app.toast(err.message, 'negative'));
    if (app.has('machines') && !recall('platform')) {
      app
        .ask({ type: 'machine_info' })
        .then((info) => {
          remember('platform', info.platform);
          if (mounted.current) setPlatform(info.platform);
        })
        .catch(() => {});
    }
  });

  /** Change some settings: shown at once, then as the computer answers (or put back, saying why). */
  const change = async (fields, done) => {
    setS((now) => (now ? applySettings(now, fields) : now));
    try {
      take(await app.ask({ type: 'set_settings', ...fields }));
      done?.();
    } catch (err) {
      if (mounted.current) setS(confirmed.current);
      app.toast(err.message, 'negative');
    }
  };

  const word = platformWord(platform);
  const sub = [state.machine?.name, word, state.host?.version ? `Trek ${state.host.version}` : null].filter(Boolean).join(' · ');
  return html`<${Frame} title="Computer settings" sub=${sub} id="mac-settings">
    ${!s
      ? html`<${Asking} />`
      : html`
          <${NewThreads} s=${s} change=${change} />
          ${s.follow_up &&
          html`<h2 class="group-title">Composer</h2>
            <div class="group">
              <div class="item mac-stack">
                <span>Messages sent while an agent works</span>
                <${Segmented} label="Messages sent while an agent works" options=${[['steer', 'Steer'], ['queue', 'Queue']]} value=${s.follow_up} onChange=${(v) => change({ follow_up: v })} />
              </div>
            </div>
            <${Foot}>Steer slips your message in at the agent’s next step. Queue holds it until the turn ends. For messages sent on your computer; here, you pick for each one in the composer.<//>`}
          ${s.auto_settle_days != null &&
          html`<h2 class="group-title">Inbox</h2>
            <div class="group">
              <${Choice}
                label="Settle finished threads"
                value=${String(s.auto_settle_days)}
                options=${[...SETTLE, ...(SETTLE.some(([d]) => d === s.auto_settle_days) ? [] : [[s.auto_settle_days, `${s.auto_settle_days} days`]])].map(([d, label]) => [String(d), label])}
                onChange=${(v) => change({ auto_settle_days: Number(v) })}
              />
            </div>
            <${Foot}>Read threads leave the inbox after this long. Threads waiting on you never settle on their own.<//>`}
          ${(s.resume_interrupted != null || s.continue_on_other_account != null) &&
          html`<h2 class="group-title">Interruptions</h2>
            <div class="group">
              ${s.resume_interrupted != null &&
              html`<${Toggle} label="Resume interrupted threads" sub="When Trek opens, threads a quit or crash cut off carry on by themselves" on=${s.resume_interrupted} onChange=${(v) => change({ resume_interrupted: v })} />`}
              ${s.continue_on_other_account != null &&
              html`<${Toggle} label="Continue on another account" sub="At a usage limit, carry on with another account of the same agent that has room" on=${s.continue_on_other_account} onChange=${(v) => change({ continue_on_other_account: v })} />`}
            </div>
            <${Foot}>Off, your computer offers these and waits for you.<//>`}
          ${(s.default_access || app.has('rules')) &&
          html`<h2 class="group-title">Permissions</h2>
            <div class="group">
              ${s.default_access &&
              html`<button type="button" class="item" onClick=${() => setPicking(true)}>
                <span class="grow">Hand-holding</span><span class="muted">${accessLabel(s.default_access)}</span><${Icon} name="forward" size="s" />
              </button>`}
              ${app.has('rules') &&
              html`<button type="button" class="item" onClick=${() => app.go({ screen: 'permission-rules' })}>
                <span class="grow">Permission rules</span>${s.phone_rules !== true && html`<span class="muted">Read only</span>`}<${Icon} name="forward" size="s" />
              </button>`}
            </div>
            ${s.default_access && html`<${Foot}>${s.full_access ? 'What new threads start with. Change it for any thread from its composer.' : 'What new threads start with. Full access is locked: only your computer can unlock it, in Trek’s Settings › Permissions.'}<//>`}`}
          <h2 class="group-title">Approvals from this phone</h2>
          <div class="group">
            <${Granted} label="Allow for a session" sub="Answer a request for the rest of its session" on=${s.session_approvals === true} />
            ${s.phone_rules != null && html`<${Granted} label="Write permission rules" sub="Add and remove rules, and Always allow from a request" on=${s.phone_rules === true} />`}
          </div>
          <${Foot}>Only your computer changes these, in Trek’s Settings › Phone: anything that outlasts one request is its to grant, since it can’t tell who’s holding this phone.<//>
          <${FailingChecks} s=${s} change=${change} />
          ${s.notifications &&
          html`<h2 class="group-title">Notifications on your computer</h2>
            <div class="group"><${Choice} label="Alerts" value=${s.notifications} options=${ALERTS} onChange=${(v) => change({ notifications: v })} /></div>
            <${Foot}>When a turn finishes or an agent is waiting for your approval.<//>`}
          ${s.push && html`<${Push} push=${s.push} change=${change} />`}
          ${s.theme &&
          html`<h2 class="group-title">Appearance on your computer</h2>
            <div class="group">
              <div class="item mac-stack">
                <${Segmented} label="Theme on your computer" options=${[['system', systemThemeLabel(platform)], ['night', 'Night'], ['paper', 'Paper']]} value=${s.theme} onChange=${(v) => change({ theme: v })} />
              </div>
            </div>
            <${Foot}>This phone’s own theme is under Appearance in Settings.<//>`}
        `}
    ${picking && s && html`<${AccessSheet} current=${s.default_access} unlocked=${!!s.full_access} onPick=${(v) => change({ default_access: v })} onClose=${() => setPicking(false)} />`}
  <//>`;
}

/** New threads' agent, model and effort. */
function NewThreads({ s, change }) {
  const agents = app.state.agents;
  if (!s.default_agent) return null;
  const agent = agents.find((a) => a.key === s.default_agent);
  const models = agent?.models || [];
  const efforts = effortsFor(agents, s.default_agent, s.default_model);
  const agentOptions = [...agents.map((a) => [a.key, a.name]), ...(agent ? [] : [[s.default_agent, s.default_agent]])];
  const modelOptions = [['', 'Agent’s default'], ...models.map((m) => [m.id, m.label]), ...(s.default_model && !models.some((m) => m.id === s.default_model) ? [[s.default_model, s.default_model]] : [])];
  const effortOptions = [...efforts.map((e) => [e, effortLabel(e)]), ...(s.default_effort && !efforts.includes(s.default_effort) ? [[s.default_effort, effortLabel(s.default_effort)]] : [])];
  return html`<h2 class="group-title">New threads</h2>
    <div class="group">
      <${Choice} label="Agent" value=${s.default_agent} options=${agentOptions} onChange=${(v) => change({ default_agent: v })} />
      <${Choice} label="Model" value=${s.default_model || ''} options=${modelOptions} onChange=${(v) => change({ default_model: v })} />
      ${efforts.length > 0 && html`<${Choice} label="Reasoning effort" value=${s.default_effort} options=${effortOptions} onChange=${(v) => change({ default_effort: v })} />`}
    </div>
    <${Foot}>New threads on your computer and here start with this agent and model. Leave the model on the agent’s default to follow whatever it recommends.<//>`;
}

/** The projects whose failing checks go back to their threads by themselves (repositories only). */
function FailingChecks({ s, change }) {
  const repos = app.state.projects.filter((p) => p.is_repo);
  if (!Array.isArray(s.send_failed_checks) || repos.length === 0) return null;
  return html`<h2 class="group-title">Send failing checks to the agent</h2>
    <div class="group">
      ${repos.map(
        (p) => html`<div class="item" key=${p.id}>
          <span class="mac-badge-s"><${ProjectBadge} project=${p} /></span>
          <span class="grow">${p.name}</span>
          <${Switch} on=${s.send_failed_checks.includes(p.id)} label=${`Send ${p.name}’s failing checks to the agent`} onChange=${(on) => change({ send_failed_checks: { project_id: p.id, on } })} />
        </div>`,
      )}
    </div>
    <${Foot}>When a pull request a thread opened fails CI, its checks and the end of their logs go back to that thread without asking, up to 2 tries a pull request. Off, the thread’s bar offers Send to agent.<//>`;
}

/** Notifications on this phone through ntfy: on or off, when, the topic to subscribe to, and the server. */
function Push({ push, change }) {
  const [server, setServer] = useState(push.server || '');
  const [renewing, setRenewing] = useState(false);
  useEffect(() => setServer(push.server || ''), [push.server]);
  const value = server.trim();
  const edited = value !== (push.server || '');
  const ok = httpsAddress(value);
  const save = () => {
    if (edited && ok) change({ push_server: value });
  };
  // A Trek that sends notifications to this page itself (`web_push`); one from before them sends
  // them through ntfy alone, and is worded as it was.
  const direct = app.has('web_push');
  const test = () => change({ push_test: true }, () => app.toast(direct ? 'Sent. It should arrive in a moment.' : 'Sent. It should arrive through ntfy in a moment.'));
  return html`<h2 class="group-title">Notifications on this phone</h2>
    <div class="group">
      <${Toggle} label="Tell my phone when a thread needs me" sub=${direct ? 'Also when one finishes or fails. This phone gets them once its own Notifications are on, in Settings.' : 'Also when one finishes or fails, through the free ntfy app'} on=${push.enabled} onChange=${(v) => change({ push: v })} />
      ${push.enabled && html`<${Choice} label="Send them" value=${push.when} options=${WHEN} onChange=${(v) => change({ push_when: v })} />`}
    </div>
    ${push.enabled && html`<${Foot}>Away means no keyboard or mouse on your computer for two minutes, or its screen locked.<//>`}
    ${push.enabled &&
    push.topic &&
    html`<h2 class="group-title">${direct ? 'Also through the ntfy app (optional)' : 'Set up ntfy'}</h2>
      <div class="group">
        <div class="item mac-stack">
          <span class="muted mac-small">Topic</span>
          <code class="mac-topic">${push.topic}</code>
          <div class="row-btns">
            ${ANDROID && push.subscribe_url
              ? html`<a class="btn small primary" href=${push.subscribe_url}><${Icon} name="bell" size="s" />Subscribe in ntfy</a>`
              : push.topic_url && html`<a class="btn small" href=${push.topic_url} target="_blank" rel="noopener noreferrer"><${Icon} name="external" size="s" />Open in ntfy</a>`}
            <button class="btn small" type="button" onClick=${() => copy(push.topic, app.toast)}><${Icon} name="copy" size="s" />Copy topic</button>
            <button class="btn small" type="button" onClick=${test}><${Icon} name="mac-bell-ring" size="s" />Send a test</button>
          </div>
        </div>
        <div class="item">
          <ol class="mac-steps">
            ${ANDROID ? html`<li>Get the free ntfy app, from Google Play or F-Droid.</li><li>Tap Subscribe in ntfy. Or copy the topic, then in ntfy tap + and paste it.</li>` : html`<li>Copy the topic.</li><li>Open ntfy and tap +.</li><li>Paste the topic and tap Subscribe.</li>`}
            ${!isNtfySh(push.server) && html`<li>Turn on “Use another server” and enter ${push.server}.</li>`}
          </ol>
        </div>
      </div>
      <${Foot}>Keep the topic to yourself: anyone who has it can read what’s sent.<//>
      <h2 class="group-title">ntfy server</h2>
      <div class="group">
        <div class="item mac-server">
          <input
            class="input"
            type="url"
            inputmode="url"
            autocapitalize="off"
            spellcheck="false"
            aria-label="ntfy server"
            placeholder="https://ntfy.sh"
            value=${server}
            onInput=${(e) => setServer(e.target.value)}
            onKeyDown=${(e) => e.key === 'Enter' && save()}
          />
          ${edited && html`<button class="btn small primary" type="button" disabled=${!ok} onClick=${save}>Save</button>`}
        </div>
        ${edited && value && !ok && html`<p class="mac-warn mac-inset" role="alert">Your computer takes an https:// address from a phone.</p>`}
        <button type="button" class="item mac-danger" onClick=${() => setRenewing(true)}><${Icon} name="refresh" size="s" /><span class="grow">New topic</span></button>
      </div>
      <${Foot}>The free ntfy.sh, or your own server.<//>
      ${renewing &&
      html`<${Confirm}
        title="Make a new topic?"
        text="The old topic stops getting notifications. Subscribe to the new one in ntfy."
        action="New topic"
        danger=${true}
        onConfirm=${() => change({ new_push_topic: true })}
        onClose=${() => setRenewing(false)}
      />`}`}`;
}

/** The hand-holding levels, each with what it means, the current one ticked; Full access only once the computer has unlocked it. */
function AccessSheet({ current, unlocked, onPick, onClose }) {
  return html`<${Sheet} title="Hand-holding" onClose=${onClose}>
    ${ACCESS.filter(([v]) => v !== 'full-access' || unlocked || current === 'full-access').map(
      ([v, label, help]) => html`<button type="button" class="option" aria-pressed=${v === current ? 'true' : 'false'} onClick=${() => {
        onClose();
        if (v !== current) onPick(v);
      }}>
        <${Icon} name=${v === current ? 'check' : 'shield'} size="s" /><span><div class="label">${label}</div><div class="desc">${help}</div></span>
      </button>`,
    )}
    <p class="mac-note">Change it for any thread from its composer.</p>
  <//>`;
}

/** A setting with a few choices: its words, and the choice in a menu. */
function Choice({ label, sub, value, options, onChange }) {
  return html`<label class="item mac-choice">
    <span class="grow">${label}${sub && html`<span class="sub">${sub}</span>`}</span>
    <select class="mac-select" value=${value} onChange=${(e) => onChange(e.target.value)}>
      ${options.map(([v, text]) => html`<option value=${v}>${text}</option>`)}
    </select>
  </label>`;
}

function Toggle({ label, sub, on, onChange }) {
  return html`<div class="item">
    <div class="grow">${label}${sub && html`<div class="sub">${sub}</div>`}</div>
    <${Switch} on=${!!on} label=${label} onChange=${onChange} />
  </div>`;
}

/** What only the computer grants this phone: shown, never changed from here. */
function Granted({ label, sub, on }) {
  return html`<div class="item">
    <div class="grow">${label}<div class="sub">${sub}</div></div>
    <span class=${on ? 'done' : 'muted'}>${on ? 'On' : 'Off'}</span>
    <${Icon} name="mac-lock" size="xs" label="Changed on your computer" />
  </div>`;
}

/** Settings › the computer: its settings, permission rules and plan usage, one tap away. */
export function ComputerSection() {
  const state = app.useApp();
  if (!state.machine) return null;
  const line = newThreadsLine(state.macSettings, state.agents);
  const rulesOpen = state.macSettings?.phone_rules === true;
  return html`<h2 class="group-title">${state.machine.name}</h2>
    <div class="group">
      <button type="button" class="item" onClick=${() => app.go({ screen: 'computer-settings' })}>
        <${Icon} name="mac-monitor-cog" />
        <div class="grow">Computer settings<div class="sub">${line || 'New threads, the composer, the inbox, hand-holding, alerts and theme'}</div></div>
        <${Icon} name="forward" size="s" />
      </button>
      ${state.features.has('rules') &&
      html`<button type="button" class="item" onClick=${() => app.go({ screen: 'permission-rules' })}>
        <${Icon} name="shield" />
        <div class="grow">Permission rules<div class="sub">What runs without asking, what asks first, what never runs</div></div>
        ${!rulesOpen && html`<span class="muted">Read only</span>`}
        <${Icon} name="forward" size="s" />
      </button>`}
      <button type="button" class="item" onClick=${() => app.go({ screen: 'usage' })}>
        <${Icon} name="gauge" />
        <div class="grow">Plan usage<div class="sub">Each plan’s limits, and when they reset</div></div>
        <${Icon} name="forward" size="s" />
      </button>
    </div>
    <p class="mac-foot">Settings that live on ${state.machine.name}: each change goes there at once.</p>`;
}
