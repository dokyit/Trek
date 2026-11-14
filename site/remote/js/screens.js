// The web remote's other screens: the thread list, the first screen (nothing paired yet), pairing
// from a link, a new thread, the paired machines, and Settings.

import { useEffect, useState } from '../vendor/hooks.mjs';
import { html, Icon, IconButton, Menu, Sheet, Spinner, Switch, Segmented, ProjectBadge, ago } from './ui.js';
import { ConnectionBanner, Mark, attached } from './thread.js';
import * as app from './model.js';
import { supported } from './wire.js';
import { setScheme, scheme } from './look.js';
import { composerPreviews, composerTools, settingsSections, threadParts } from './routes.js';

// ---------------------------------------------------------------------------------------------
// Threads

function rowLine(t) {
  if (t.needs) return { text: t.needs.text || 'Needs you', tone: t.needs.kind === 'failed' ? 'failed' : 'needs' };
  if (t.run_state === 'failed') return { text: 'Failed', tone: 'failed' };
  if (t.run_state === 'working') return { text: t.activity || 'Working…', tone: '' };
  if (t.busy_in_background) return { text: 'Waiting on what it started', tone: '' };
  if (t.background?.length) return { text: `Running ${t.background.join(', ')}`, tone: '' };
  return null;
}

function dotFor(t) {
  if (t.needs || t.run_state === 'needs-you') return 'connecting';
  if (t.run_state === 'failed') return 'failed';
  if (t.run_state === 'working' || t.busy_in_background) return 'working';
  if (t.background?.length) return 'sky';
  return null;
}

export function ThreadRow({ t }) {
  const line = rowLine(t);
  return html`<button type="button" class=${`thread-row ${t.unseen ? 'unseen' : ''}`} onClick=${() => app.go({ screen: 'thread', threadId: t.id })}>
    <${ProjectBadge} project=${t.project} dot=${dotFor(t)} />
    <div class="thread-main">
      <div class="thread-top"><span class="thread-title">${t.title || 'New thread'}</span><span class="thread-time">${ago(t.updated_at)}</span></div>
      ${line && html`<div class=${`thread-line ${line.tone}`}>${line.text}</div>`}
      <div class="thread-meta">
        <span class="grow">${t.project?.name || 'Chat'}</span>
        ${t.branch && html`<span>· <${Icon} name="branch" size="xs" /> ${t.branch}</span>`}
        ${(t.model_label || t.agent?.name) && html`<span class="grow">· <${Mark} agent=${t.agent} size=${12} /> ${t.model_label || t.agent?.name}</span>`}
        ${t.additions || t.deletions ? html`<span class="mono"><span class="add">+${t.additions}</span> <span class="del">−${t.deletions}</span></span>` : null}
        ${t.sub_agents?.length > 0 && html`<span class="badge">${t.sub_agents.length} sub-agent${t.sub_agents.length === 1 ? '' : 's'}</span>`}
      </div>
    </div>
  </button>`;
}

export function ThreadsScreen() {
  const state = app.useApp();
  const [menu, setMenu] = useState(false);
  const [query, setQuery] = useState(null);
  const all = [...state.threads.values()];
  const shown = query ? all.filter((t) => `${t.title} ${t.project?.name || ''} ${t.branch || ''}`.toLowerCase().includes(query.toLowerCase())) : all;
  const groups = app.sections(shown);
  const name = state.machine?.name || 'Trek';
  return html`<div class="screen">
    <header class="bar">
      ${query == null
        ? html`<div class="bar-title" style=${{ paddingLeft: '12px' }}>
              <h1>${name}</h1>
              <div class="sub"><span class=${`dot ${state.status}`}></span>${statusWords(state)}</div>
            </div>
            <${IconButton} icon="search" label="Search threads" onClick=${() => setQuery('')} />
            <${IconButton} icon="more" label="Menu" onClick=${() => setMenu(true)} />`
        : html`<${IconButton} icon="back" label="Close search" onClick=${() => setQuery(null)} />
            <input class="input" style=${{ flex: 1, minHeight: '38px' }} type="search" placeholder="Search threads" value=${query} onInput=${(e) => setQuery(e.target.value)} autofocus />`}
    </header>
    <${ConnectionBanner} />
    <div class="scroll" style=${{ paddingBottom: '96px' }}>
      ${!state.haveSnapshot && state.status !== 'online' && state.threads.size === 0
        ? html`<div class="empty">${state.status === 'connecting' ? html`<${Spinner} />` : 'Nothing here until Trek answers.'}</div>`
        : groups.length === 0
          ? html`<div class="empty">${query ? 'No thread matches.' : 'No threads yet. Start one with New thread.'}</div>`
          : groups.map(
              (g) => html`<section key=${g.key}>
                <h2 class="section-title">${g.title} <span>${g.threads.length}</span></h2>
                ${g.threads.map((t) => html`<${ThreadRow} key=${t.id} t=${t} />`)}
              </section>`,
            )}
    </div>
    ${state.status === 'online' && state.projects.length > 0 && html`<button class="fab" type="button" onClick=${() => app.go({ screen: 'new' })}><${Icon} name="plus" />New thread</button>`}
    ${menu &&
    html`<${Menu}
      title=${name}
      onClose=${() => setMenu(false)}
      items=${[
        ...threadParts.listMenuItems.flatMap((fn) => fn({ state, has: app.has }) || []),
        { label: 'Machines', icon: 'monitor', detail: String(state.machines.length), onClick: () => app.go({ screen: 'machines' }) },
        { label: 'Settings', icon: 'settings', onClick: () => app.go({ screen: 'settings' }) },
        { label: 'Reconnect', icon: 'refresh', hidden: state.status === 'online', onClick: () => app.reconnect({ gesture: true }) },
      ]}
    />`}
  </div>`;
}

function statusWords(state) {
  switch (state.status) {
    case 'online':
      return state.host?.version ? `Connected · Trek ${state.host.version}` : 'Connected';
    case 'connecting':
      return 'Connecting…';
    case 'permission':
      return 'Waiting for permission';
    case 'unauthorized':
      return 'Not paired any more';
    default:
      return 'Offline';
  }
}

// ---------------------------------------------------------------------------------------------
// Nothing paired, and pairing from a link

export function WelcomeScreen() {
  return html`<div class="screen"><div class="scroll"><div class="hero">
    <img class="mark" src="/favicon.svg" alt="" />
    <h1>Trek, from your phone</h1>
    <p>Follow every thread on your computer, answer approvals, questions and plans, and start new work while your agents keep running there.</p>
    <ol class="steps">
      <li><span>On your computer, open Trek’s <b>Settings › Phone & Machines</b> and turn on <b>Let your phone connect</b>.</span></li>
      <li><span>Choose <b>Show pairing code</b>, then <b>A phone’s browser</b>.</span></li>
      <li><span>Scan the code with this phone’s camera. It opens this page and pairs.</span></li>
    </ol>
    <p>This phone connects straight to your computer, on your Wi-Fi or tailnet. Nothing goes through trekdesktop.com.</p>
    ${!supported() && html`<div class="banner bad" style=${{ margin: '16px 0 0' }}><${Icon} name="alert" /><span class="grow">This browser can’t make the connection Trek needs (WebTransport). Use Chrome, Edge or Samsung Internet.</span></div>`}
    <p style=${{ marginTop: '18px' }}><button class="btn ghost" type="button" onClick=${() => app.startDemo()}><${Icon} name="play" size="s" />Explore the demo</button></p>
  </div></div></div>`;
}

export function PairScreen() {
  const state = app.useApp();
  const pairing = state.pairing;
  const status = state.machine === pairing ? state.status : 'idle';
  useEffect(() => {
    if (status === 'online') app.go({ screen: 'threads' }, { replace: true });
  }, [status]);
  if (!pairing) return html`<${WelcomeScreen} />`;
  const connect = () => app.use(pairing, { gesture: true });
  const failed = {
    pairing_failed: 'That code didn’t work: it was used already, or it’s more than a few minutes old. Show a new code on your computer and scan it again.',
    offline: `Couldn’t reach ${pairing.name}${state.detail ? ` (${state.detail})` : ''}. Is this phone on the same Wi-Fi as it, or on the same tailnet?`,
    permission: state.detail === 'denied' ? 'This browser is blocking the page from your local network. Allow “Local network access” for this site in the browser’s site settings, then try again.' : null,
    unsupported: 'This browser can’t make the connection Trek needs (WebTransport). Open the link in Chrome, Edge or Samsung Internet.',
  }[status];
  return html`<div class="screen"><div class="scroll"><div class="hero">
    <img class="mark" src="/favicon.svg" alt="" />
    <h1>Pair with ${pairing.name}</h1>
    <p>This phone will connect straight to Trek on ${pairing.name}, on your network. The browser may ask to look for devices on your local network: allow it.</p>
    ${failed && html`<div class="banner bad" style=${{ margin: '0 0 16px' }}><${Icon} name="alert" /><span class="grow">${failed}</span></div>`}
    <button class="btn primary block" id="pair-connect" type="button" disabled=${status === 'connecting'} onClick=${connect}>
      ${status === 'connecting' ? html`<${Spinner} /> Connecting…` : 'Connect'}
    </button>
    <p class="muted" style=${{ marginTop: '14px', fontSize: '13px' }}>Addresses: ${pairing.routes.join(', ')}</p>
  </div></div></div>`;
}

// ---------------------------------------------------------------------------------------------
// A new thread

export function NewThreadScreen({ projectId }) {
  const state = app.useApp();
  const projects = state.projects;
  const [project, setProject] = useState(projectId || projects[0]?.id || '');
  const [agentKey, setAgentKey] = useState(() => {
    try {
      return localStorage.getItem('trek-remote-agent') || state.agents[0]?.key || '';
    } catch {
      return state.agents[0]?.key || '';
    }
  });
  const agent = state.agents.find((a) => a.key === agentKey) || state.agents[0];
  const [model, setModel] = useState(null);
  const [worktree, setWorktree] = useState(false);
  const [plan, setPlan] = useState(false);
  const [text, setText] = useState('');
  // What goes with it (the composer's tools add them): photos, and videos and files sent up first.
  const [attachments, setAttachments] = useState([]);
  // Start tapped while its files were still going up: the ones it waits for.
  const [after, setAfter] = useState(null);
  const [busy, setBusy] = useState(false);
  const chosenModel = model || agent?.default_model || agent?.models?.[0]?.id || null;
  const isRepo = projects.find((p) => p.id === project)?.is_repo;
  const online = state.status === 'online';
  const said = text.trim();

  /** What goes with attachments started with no words, as the iPhone words it. */
  const caption = () => {
    const photos = attachments.filter((a) => a.kind === 'image').length;
    const videos = attachments.filter((a) => a.kind === 'file' && a.video).length;
    const files = attachments.filter((a) => a.kind === 'file' && !a.video).length;
    const total = photos + videos + files;
    if (photos === total) return total > 1 ? `Here are ${total} photos.` : 'Here’s a photo.';
    if (videos === total) return total > 1 ? `Here are ${total} videos.` : 'Here’s a video.';
    if (files === total) return total > 1 ? `Here are ${total} files.` : 'Here’s a file.';
    return `Here are ${total} attachments.`;
  };

  const start = async () => {
    if ((!said && attachments.length === 0) || !agent || !project) return;
    setBusy(true);
    try {
      try {
        localStorage.setItem('trek-remote-agent', agent.key);
      } catch {
        // Not remembered.
      }
      const ack = await app.ask({ type: 'new_thread', project_id: project, agent: agent.key, model: chosenModel, text: said || caption(), worktree: worktree && !!isRepo, plan, ...attached(attachments) }, { timeout: 120_000 });
      if (ack.thread_id) app.go({ screen: 'thread', threadId: ack.thread_id }, { replace: true });
      else app.back();
    } catch (err) {
      app.toast(err.message, 'negative');
    } finally {
      setBusy(false);
    }
  };

  // It starts once the files it waits for are in; one that doesn't go up (its toast says why) leaves it to you.
  useEffect(() => {
    if (!after) return;
    const left = attachments.filter((a) => after.includes(a.id));
    if (left.length < after.length) setAfter(null);
    else if (left.every((a) => !a.pending)) {
      setAfter(null);
      start();
    }
  }, [attachments, after]);

  const tapStart = () => {
    const pending = attachments.filter((a) => a.pending).map((a) => a.id);
    if (pending.length) setAfter(pending);
    else start();
  };

  return html`<div class="screen">
    <header class="bar">
      <${IconButton} icon="x" label="Cancel" onClick=${() => app.back()} />
      <div class="bar-title"><h1>New thread</h1></div>
      <button class="btn small primary" type="button" style=${{ marginRight: '8px' }} aria-label=${after ? 'Starting once its files are in' : 'Start'} disabled=${(!said && attachments.length === 0) || busy || !!after || !online} onClick=${tapStart}>${busy || after ? html`<${Spinner} />` : 'Start'}</button>
    </header>
    <${ConnectionBanner} />
    <div class="scroll"><div class="pad">
      <label class="field"><span>Project</span>
        <select class="input" value=${project} onChange=${(e) => setProject(e.target.value)}>
          ${projects.map((p) => html`<option value=${p.id}>${p.name}${p.branch ? ` (${p.branch})` : ''}</option>`)}
        </select>
      </label>
      <label class="field"><span>Agent</span>
        <select class="input" value=${agent?.key} onChange=${(e) => {
          setAgentKey(e.target.value);
          setModel(null);
        }}>
          ${state.agents.map((a) => html`<option value=${a.key}>${a.name}</option>`)}
        </select>
      </label>
      ${agent?.models?.length > 0 &&
      html`<label class="field"><span>Model</span>
        <select class="input" value=${chosenModel} onChange=${(e) => setModel(e.target.value)}>
          ${agent.models.map((m) => html`<option value=${m.id}>${m.label}</option>`)}
        </select>
      </label>`}
      ${isRepo && html`<div class="item" style=${{ padding: '6px 0', border: 0 }}><div class="grow">In a new worktree<div class="sub">Its own branch and folder, so it doesn’t touch what you have checked out.</div></div><${Switch} on=${worktree} onChange=${setWorktree} label="In a new worktree" /></div>`}
      <div class="item" style=${{ padding: '6px 0 14px', border: 0 }}><div class="grow">Plan first<div class="sub">The agent plans, and waits for you to approve it.</div></div><${Switch} on=${plan} onChange=${setPlan} label="Plan first" /></div>
      <div class="field"><span>What should it do?</span>
        <div class="new-compose">
          ${composerPreviews.map((Preview, i) => html`<${Preview} key=${i} attachments=${attachments} setAttachments=${setAttachments} />`)}
          <textarea class="input" rows="6" value=${text} onInput=${(e) => setText(e.target.value)} placeholder="Describe the task" aria-label="What should it do?" autofocus></textarea>
          <div class="new-compose-tools">
            ${composerTools.map((Tool, i) => html`<${Tool} key=${i} threadId=${null} text=${text} setText=${setText} attachments=${attachments} setAttachments=${setAttachments} disabled=${!online} />`)}
          </div>
        </div>
      </div>
    </div></div>
  </div>`;
}

// ---------------------------------------------------------------------------------------------
// Machines

export function MachinesScreen() {
  const state = app.useApp();
  const [menu, setMenu] = useState(null);
  return html`<div class="screen">
    <header class="bar"><${IconButton} icon="back" label="Back" onClick=${() => app.back()} /><div class="bar-title"><h1>Machines</h1></div></header>
    <div class="scroll">
      <h2 class="group-title">Paired with this phone</h2>
      <div class="group">
        ${state.machines.map((m) => {
          const current = state.machine?.hostId === m.hostId;
          return html`<button type="button" class="item" onClick=${() => (current ? app.go({ screen: 'threads' }) : (app.use(m, { gesture: true }), app.go({ screen: 'threads' })))}>
            <${Icon} name="monitor" />
            <div class="grow"><div>${m.name}</div><div class="sub">${current ? statusWords(state) : m.lastConnectedAt ? `Last connected ${ago(m.lastConnectedAt)} ago` : 'Paired'} · ${(m.routes || []).join(', ')}</div></div>
            <span class="icon-btn" role="button" aria-label=${`More for ${m.name}`} onClick=${(e) => {
              e.stopPropagation();
              setMenu(m);
            }}><${Icon} name="more" /></span>
          </button>`;
        })}
      </div>
      <p class="muted pad">To add another computer, show its pairing code in Trek’s Settings › Phone & Machines (choose “A phone’s browser”) and scan it with this phone.</p>
    </div>
    ${menu &&
    html`<${Menu}
      title=${menu.name}
      onClose=${() => setMenu(null)}
      items=${[
        {
          label: 'Forget this computer',
          icon: 'trash',
          danger: true,
          onClick: () => {
            if (confirm(`Forget ${menu.name}? To use it from this phone again, pair it again. (Unpair this phone in Trek’s Settings › Phone to stop it there too.)`)) app.forget(menu.hostId);
          },
        },
      ]}
    />`}
  </div>`;
}

// ---------------------------------------------------------------------------------------------
// Settings

export function SettingsScreen() {
  const state = app.useApp();
  const [look, setLook] = useState(scheme());
  return html`<div class="screen">
    <header class="bar"><${IconButton} icon="back" label="Back" onClick=${() => app.back()} /><div class="bar-title"><h1>Settings</h1></div></header>
    <div class="scroll">
      <h2 class="group-title">Appearance</h2>
      <div class="group"><div class="item"><div class="grow">Theme</div>
        <${Segmented} label="Theme" options=${[['system', 'System'], ['dark', 'Night'], ['light', 'Paper']]} value=${look} onChange=${(v) => {
          setLook(v);
          setScheme(v);
        }} />
      </div></div>
      ${settingsSections.map((Section, i) => html`<${Section} key=${i} />`)}
      <h2 class="group-title">This phone</h2>
      <div class="group">
        <div class="item"><div class="grow">Connected to<div class="sub">${state.machine ? `${state.machine.name}${state.route ? ` at ${state.machine.routes?.[0] || ''}` : ''}` : 'Nothing'}</div></div></div>
        <div class="item"><div class="grow">Add to the home screen<div class="sub">In the browser’s menu, choose Add to Home screen (or Install app): Trek opens like an app, full screen.</div></div></div>
      </div>
      <h2 class="group-title">About</h2>
      <div class="group">
        <div class="item"><div class="grow">Trek’s web remote<div class="sub">It connects straight to Trek on your computer over WebTransport, its certificate pinned from the pairing code. What it shows comes from there; this site sees none of it.</div></div></div>
        <a class="item" href="https://github.com/dokyit/Trek" target="_blank" rel="noopener noreferrer"><div class="grow">Trek on GitHub</div><${Icon} name="external" size="s" /></a>
      </div>
    </div>
  </div>`;
}

export { Sheet };
