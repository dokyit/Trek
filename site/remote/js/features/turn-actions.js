// A turn's and a message's actions, as the iPhone's rows under them (`MessageActions.swift`,
// `ConfirmSheet.swift`): under a turn's end, copy its answer, undo it, retry it (on another of the
// agent's models too) and fork from it; under a message of yours, edit it, rewind to it, fork from
// just before it and copy it. The latest turn's and message's are out as small icon buttons;
// earlier ones keep theirs behind one "…", so the transcript stays quiet. Undo, retry and rewind
// ask first, as the Mac does, with the files they'd put back where the computer can say
// (`turn_preview`). What's taken back waits in the composer, and a fork opens.

import { useEffect, useState } from '../../vendor/hooks.mjs';
import { html, Icon, Menu, Sheet, Spinner, clock, took } from '../ui.js';
import * as app from '../model.js';
import { registerTurnActions } from '../routes.js';
import { Layer } from './thread-extras.js';

// ---------------------------------------------------------------------------------------------
// Which items take them

/**
 * The items that end a turn, in order: each `turn_end`, or for a turn that stopped without one, the
 * error, limit or notice it stopped with (a notice only once the turn had done something). A
 * message of the user's starts a turn, unless it's an aside (a command Trek answered itself). The
 * stretch still running (`open`) has no end yet. As the iPhone's `Block.turnEnds`.
 */
export function turnEnds(items, open) {
  const ends = [];
  let ended = false;
  let worked = false;
  let stop = null;
  const close = () => {
    if (!ended && stop) ends.push(stop);
    ended = false;
    worked = false;
    stop = null;
  };
  for (const it of items || []) {
    switch (it.kind) {
      case 'user':
        if (!it.aside) close();
        break;
      case 'turn_end':
        ends.push(it.id);
        ended = true;
        break;
      case 'error':
      case 'limit':
        stop = it.id;
        break;
      case 'notice':
        if (worked) stop = it.id;
        break;
      case 'reasoning':
      case 'tool':
      case 'assistant':
      case 'approval':
      case 'question':
      case 'plan':
        worked = true;
        stop = null;
        break;
      default:
        break;
    }
  }
  if (!open) close();
  return ends;
}

/** What a turn answered, for Copy: its answers' text between the message that started it and its end. */
export function responseText(items, endId) {
  const list = items || [];
  const end = list.findIndex((it) => it.id === endId);
  const parts = [];
  for (let i = end - 1; i >= 0; i -= 1) {
    if (list[i].kind === 'user') break;
    if (list[i].kind === 'assistant' && list[i].text) parts.unshift(list[i].text);
  }
  return parts.join('\n\n');
}

/** The models a turn can be retried with: the thread's agent's others. */
export function retryModels(row, agents) {
  const agent = (agents || []).find((a) => a.key === row?.agent?.key);
  return (agent?.models || []).filter((m) => m.id !== row?.model);
}

/** What the confirmation says, as the iPhone's. */
export function confirmWords({ action, model, edit }) {
  switch (action) {
    case 'undo':
      return { title: 'Undo this turn?', message: 'Your message and everything after it go; the message waits in the composer.', verb: 'Undo' };
    case 'retry':
      return { title: model ? `Retry with ${model.label}?` : 'Retry this turn?', message: 'The answer goes, and your message is sent again.', verb: 'Retry' };
    default:
      return edit
        ? { title: 'Edit this message?', message: 'Back to before this message, to change it and send it again.', verb: 'Edit' }
        : { title: 'Rewind to this message?', message: 'Back to before this message; it waits in the composer.', verb: 'Rewind' };
  }
}

/** What restoring does to a file, as the Mac's confirmation says it. */
export function previewVerb(change) {
  switch (change) {
    case 'revert':
      return 'revert';
    case 'delete':
      return 'delete';
    case 'bring_back':
      return 'bring back';
    case 'kept':
      return 'left as is (newer)';
    default:
      return 'left as is';
  }
}

/** The most files a confirmation lists; the rest are counted. */
const LISTED = 8;

/** The ends of a transcript, worked out once per change to it (every row with actions asks). */
const worked = new WeakMap();

function endsOf(t, open) {
  let known = worked.get(t.items);
  if (!known || known.open !== open) {
    const ends = turnEnds(t.items, open);
    const lastUser = [...t.items].reverse().find((it) => it.kind === 'user')?.id ?? null;
    known = { open, set: new Set(ends), lastTurn: ends[ends.length - 1] ?? null, lastUser };
    worked.set(t.items, known);
  }
  return known;
}

// ---------------------------------------------------------------------------------------------
// The row

async function copyText(text, said) {
  try {
    await navigator.clipboard.writeText(text);
    app.toast(said);
  } catch {
    app.toast('Couldn’t copy', 'negative');
  }
}

/** The keyboard, after Edit put the message back. */
function focusComposer() {
  requestAnimationFrame(() => document.querySelector('.composer-box textarea')?.focus());
}

function Act({ icon, label, onClick, disabled, narrow }) {
  return html`<button type="button" class=${`te-act ${narrow ? 'narrow' : ''}`} aria-label=${label} title=${label} disabled=${disabled} onClick=${onClick}><${Icon} name=${icon} size="s" /></button>`;
}

function TurnActions({ item, threadId }) {
  const [menu, setMenu] = useState(null);
  const [asked, setAsked] = useState(null);
  const t = app.state.transcripts.get(threadId);
  // Only the thread's own transcript: a sub-agent's steps drawn elsewhere take none.
  if (!t?.index?.has(item.id)) return null;
  const row = app.row(threadId);
  const busy = row?.run_state === 'working';
  const ends = endsOf(t, busy);
  const user = item.kind === 'user';
  if (!user && !ends.set.has(item.id)) return null;
  const latest = item.id === (user ? ends.lastUser : ends.lastTurn);
  const models = user ? [] : retryModels(row, app.state.agents);

  const fork = async () => {
    try {
      const ack = await app.ask({ type: 'turn_action', thread_id: threadId, item_id: item.id, action: 'fork' }, { timeout: 60_000 });
      if (!ack.thread_id) return app.toast('Forked');
      // Forked from a message: it waits in the new thread's composer.
      if (ack.text) app.setDraft(ack.thread_id, ack.text);
      app.go({ screen: 'thread', threadId: ack.thread_id });
    } catch (err) {
      app.toast(err.message, 'negative');
    }
    return undefined;
  };
  const run = async ({ action, model, edit }, restore) => {
    try {
      const ack = await app.ask(
        { type: 'turn_action', thread_id: threadId, item_id: item.id, action, ...(model ? { model: model.id } : {}), restore_files: restore },
        { timeout: 60_000 },
      );
      if ((action === 'undo' || action === 'rewind') && typeof ack.text === 'string') {
        app.setDraft(threadId, ack.text);
        if (edit) focusComposer();
      }
    } catch (err) {
      app.toast(err.message, 'negative');
    }
  };
  const copyResponse = () => {
    const text = responseText(t.items, item.id);
    if (!text) return app.toast('Nothing to copy in this turn');
    return copyText(text, 'Copied the response');
  };
  const copyMessage = () => copyText(item.text || '', 'Copied');

  const more = user
    ? [
        { label: 'Edit', icon: 'pencil', hidden: busy, onClick: () => setAsked({ action: 'rewind', edit: true }) },
        { label: 'Rewind to here', icon: 'rewind', hidden: busy, onClick: () => setAsked({ action: 'rewind' }) },
        { label: 'Fork from here', icon: 'fork', onClick: fork },
      ]
    : [
        { label: 'Undo this turn', icon: 'undo', hidden: busy, onClick: () => setAsked({ action: 'undo' }) },
        { label: 'Retry', icon: 'refresh', hidden: busy, onClick: () => setAsked({ action: 'retry' }) },
        { label: 'Retry with…', icon: 'bot', hidden: busy || models.length === 0, keep: true, onClick: () => setMenu('retry') },
        { label: 'Fork from here', icon: 'fork', onClick: fork },
      ];
  const layers = html`${menu === 'more' && html`<${Layer}><${Menu} items=${more} onClose=${() => setMenu(null)} /><//>`}
  ${menu === 'retry' &&
  html`<${Layer}><${Menu} title="Retry with" items=${models.map((m) => ({ label: m.label, icon: 'bot', onClick: () => setAsked({ action: 'retry', model: m }) }))} onClose=${() => setMenu(null)} /><//>`}
  ${asked && html`<${Layer}><${Confirm} asked=${asked} threadId=${threadId} itemId=${item.id} agent=${row?.agent?.name} onClose=${() => setAsked(null)} onRun=${(restore) => run(asked, restore)} /><//>`}`;

  if (user) {
    const at = item.at ? html`<span class="te-when">${clock(item.at)}</span>` : null;
    const copy = html`<${Act} icon="copy" label="Copy message" onClick=${copyMessage} />`;
    // Not a prompt (a command Trek answered, a typed answer): nothing to go back to.
    if (item.aside) return html`<div class="te-acts user">${at}${copy}</div>`;
    return html`<div class="te-acts user">
      ${at}
      ${latest
        ? html`<${Act} icon="pencil" label="Edit" disabled=${busy} onClick=${() => setAsked({ action: 'rewind', edit: true })} />
            <${Act} icon="rewind" label="Rewind to here" disabled=${busy} onClick=${() => setAsked({ action: 'rewind' })} />
            <${Act} icon="fork" label="Fork from here" onClick=${fork} />`
        : html`<${Act} icon="te-dots" label="Message actions" onClick=${() => setMenu('more')} />`}
      ${copy}${layers}
    </div>`;
  }

  // A turn's end: as its line says it (how long it worked, when), its actions at the line's end.
  const end = item.kind === 'turn_end';
  const verified = end ? item.verified : null;
  return html`<div class=${`te-acts ${end ? 'end' : 'stop'}`}>
    <${Act} icon="copy" label="Copy response" onClick=${copyResponse} />
    ${end && html`<span class="te-when">${item.took_secs != null ? `Worked ${took(item.took_secs)}` : 'Done'}${item.at ? ` · ${clock(item.at)}` : ''}</span>`}
    ${verified && html`<span class=${`badge ${verified.passed ? 'done' : 'failed'}`}>${verified.passed ? 'Verified' : 'Verification failed'}</span>`}
    <span class="te-fill"></span>
    ${latest
      ? html`<${Act} icon="undo" label="Undo this turn" disabled=${busy} onClick=${() => setAsked({ action: 'undo' })} />
          <${Act} icon="refresh" label="Retry" disabled=${busy} onClick=${() => setAsked({ action: 'retry' })} />
          ${models.length > 0 && html`<${Act} icon="down" label="Retry with another model" narrow disabled=${busy} onClick=${() => setMenu('retry')} />`}
          <${Act} icon="fork" label="Fork from here" onClick=${fork} />`
      : html`<${Act} icon="te-dots" label="Turn actions" onClick=${() => setMenu('more')} />`}
    ${layers}
  </div>`;
}

// ---------------------------------------------------------------------------------------------
// The confirmation

/**
 * Undo, retry or rewind, asked first as the Mac's confirmation card asks: what leaves the
 * conversation, and the files restoring would put back, each with what happens to it, under
 * "Also restore files". A computer that can't say which (no `turn_preview`) is asked plainly.
 */
function Confirm({ asked, threadId, itemId, agent, onClose, onRun }) {
  const previewing = app.has('turn_preview');
  const [preview, setPreview] = useState(null);
  const [failed, setFailed] = useState(null);
  const [restore, setRestore] = useState(true);
  useEffect(() => {
    if (!previewing) return undefined;
    let live = true;
    app.ask({ type: 'turn_preview', thread_id: threadId, item_id: itemId, action: asked.action }).then(
      (p) => {
        if (!live) return;
        setPreview(p);
        setRestore((p.files || []).length > 0);
      },
      (err) => live && setFailed(err.message),
    );
    return () => {
      live = false;
    };
  }, []);
  const words = confirmWords(asked);
  const message = preview?.recap ? `${words.message} ${agent || 'The agent'} can’t take its own session back to this point, so it continues in a new one with a recap.` : words.message;
  const go = (keep) => {
    onClose();
    onRun(keep);
  };
  return html`<${Sheet} title=${words.title} onClose=${onClose}>
    <p class="te-ask-msg">${message}</p>
    ${previewing
      ? html`<${Files} preview=${preview} failed=${failed} restore=${restore} setRestore=${setRestore} />
          <div class="te-ask-btns">
            <button class="btn" type="button" onClick=${onClose}>Cancel</button>
            <button class="btn danger" type="button" disabled=${!preview && !failed} onClick=${() => go(restore && (preview?.files || []).length > 0)}>${words.verb}</button>
          </div>`
      : html`<div class="te-ask-col">
          <button class="btn danger block" type="button" onClick=${() => go(true)}>${words.verb} and restore files</button>
          <button class="btn block" type="button" onClick=${() => go(false)}>${words.verb}, keep files</button>
          <button class="btn ghost block" type="button" onClick=${onClose}>Cancel</button>
        </div>`}
  <//>`;
}

function Files({ preview, failed, restore, setRestore }) {
  if (failed) return html`<p class="te-note">${failed}</p>`;
  if (!preview) return html`<p class="te-note te-checking"><${Spinner} />Checking which files changed…</p>`;
  if (!preview.files) return html`<p class="te-note">${preview.unavailable || 'Trek has no checkpoint for this point: the files stay as they are.'}</p>`;
  if (!preview.files.length) return html`<p class="te-note">Nothing to put back: the files these turns changed are as they were.</p>`;
  return html`<div class="te-files">
    <button type="button" class="te-check" role="checkbox" aria-checked=${restore ? 'true' : 'false'} onClick=${() => setRestore(!restore)}>
      <${Icon} name=${restore ? 'te-box-check' : 'te-box'} />Also restore files
    </button>
    <div class=${`te-file-list ${restore ? '' : 'off'}`}>
      ${preview.files.slice(0, LISTED).map((f) => html`<div class="te-file" key=${f.path}><span class="te-path">${f.path}</span><span class="te-verb">${previewVerb(f.change)}</span></div>`)}
      ${preview.files.length > LISTED && html`<div class="te-file te-verb">and ${preview.files.length - LISTED} more</div>`}
    </div>
  </div>`;
}

registerTurnActions(TurnActions);
