// A transcript's items as the web remote draws them (`docs/MOBILE.md` › Transcripts): messages,
// answers in markdown, reasoning folded away, tools as one-line rows that open to their output,
// the requests (approvals, questions, plans) as cards to answer, a turn's end and changed files,
// notices, errors and limits, handoffs, links to other threads, and pull request and CI cards.

import { useState } from '../vendor/hooks.mjs';
import { html, Icon, Spinner, ago, took, plural, clock } from './ui.js';
import { Markdown } from './markdown.js';
import * as app from './model.js';
import { parts, sheets, threadParts, turnActions } from './routes.js';

const TOOL_ICON = { command: 'terminal', read: 'file', edit: 'pencil', search: 'search', web: 'globe', agent: 'bot', other: 'zap' };

/** One item; `threadId` for what its buttons send. */
export function Item({ item, threadId, last }) {
  switch (item.kind) {
    case 'user':
      return html`<${UserMessage} item=${item} threadId=${threadId} />`;
    case 'assistant':
      return html`<div class="msg-assistant"><${Markdown} text=${item.text} threadId=${threadId} />${item.streaming && html`<span class="muted"> ▍</span>`}</div>`;
    case 'reasoning':
      return html`<${Reasoning} item=${item} open=${last} />`;
    case 'tool':
      return html`<${Tool} item=${item} threadId=${threadId} /><${Parts} kind="tool" item=${item} threadId=${threadId} />`;
    case 'approval':
      return html`<${Approval} item=${item} threadId=${threadId} />`;
    case 'question':
      return html`<${Questions} item=${item} threadId=${threadId} />`;
    case 'plan':
      return html`<${Plan} item=${item} threadId=${threadId} />`;
    case 'turn_end':
      return html`<${TurnEnd} item=${item} threadId=${threadId} />`;
    // A turn that stopped with one of these ends with it: its actions (undo, retry) go after it.
    case 'notice':
      return html`<div class="notice">${item.text}</div><${Actions} item=${item} threadId=${threadId} />`;
    case 'error':
      return html`<div class="notice error">${item.text}</div><${Actions} item=${item} threadId=${threadId} />`;
    case 'limit':
      return html`<div class="notice limit">${item.text}${item.resets_at ? ` · resets ${clock(item.resets_at)}` : ''}</div><${Actions} item=${item} threadId=${threadId} />`;
    case 'handoff':
      return html`<div class="notice">Handed off from ${item.from_agent?.name || item.from} to ${item.to_agent?.name || item.to}</div>`;
    case 'thread_link':
      return html`<${ThreadLink} item=${item} />`;
    case 'changes':
      return html`<${Changes} item=${item} />`;
    case 'card':
      return html`<${WorkCard} card=${item.card || {}} threadId=${threadId} />`;
    default: {
      const Kind = threadParts.itemKinds.get(item.kind);
      return Kind ? html`<${Kind} item=${item} threadId=${threadId} last=${last} />` : null;
    }
  }
}

/** What areas draw under a built-in item (a message's photos, a watched video's frames). */
function Parts({ kind, item, threadId }) {
  return (threadParts.itemParts.get(kind) || []).map((P, i) => html`<${P} key=${i} item=${item} threadId=${threadId} />`);
}

function Actions({ item, threadId }) {
  return turnActions.map((A, i) => html`<${A} key=${i} item=${item} threadId=${threadId} />`);
}

function UserMessage({ item, threadId }) {
  // What the media area draws under the bubble isn't said in it again.
  const extras = [];
  if (item.images && !app.has('attachment')) extras.push(plural(item.images, 'image'));
  if (item.videos?.length && !app.has('media')) extras.push(plural(item.videos.length, 'video'));
  if (item.files?.length) extras.push(plural(item.files.length, 'file'));
  const split = parts.get('split-mentions');
  const text = split ? split(item.text || '').text : item.text;
  return html`<div class="msg-user">${text}${extras.length > 0 && html`<div class="attach"><${Icon} name="clip" size="xs" /> ${extras.join(' · ')}</div>`}</div><${Parts} kind="user" item=${item} threadId=${threadId} /><${Actions} item=${item} threadId=${threadId} />`;
}

function Reasoning({ item, open }) {
  const [shown, setShown] = useState(false);
  const show = shown || (open && item.streaming);
  return html`<div class="reasoning">
    <button type="button" aria-expanded=${show ? 'true' : 'false'} onClick=${() => setShown(!shown)}><${Icon} name="idea" size="s" />Thinking<${Icon} name=${show ? 'down' : 'forward'} size="xs" /></button>
    ${show && html`<${Markdown} text=${item.text} />`}
  </div>`;
}

function Tool({ item, threadId }) {
  const [open, setOpen] = useState(false);
  const [sheet, setSheet] = useState(false);
  const status = item.status || 'done';
  const counts = item.added != null || item.removed != null;
  // A tool call that stands for a sub-agent opens it: Trek's own as a thread, the agent's own in a sheet.
  const sub = item.sub_agent;
  const SubSheet = sheets.get('sub-agent');
  const openSub = sub && (sub.thread_id ? () => app.go({ screen: 'thread', threadId: sub.thread_id }) : SubSheet ? () => setSheet(true) : null);
  return html`<div class=${`tool ${status}`}>
    <button type="button" aria-expanded=${open ? 'true' : 'false'} onClick=${() => setOpen(!open)}>
      ${status === 'running' ? html`<${Spinner} />` : html`<${Icon} name=${TOOL_ICON[item.tool] || 'zap'} size="s" />`}
      <span class="what">${item.title || item.tool}</span>
      <span class="detail">${item.detail}</span>
      ${counts && html`<span class="counts"><span class="add">+${item.added || 0}</span> <span class="del">−${item.removed || 0}</span></span>`}
      ${status === 'failed' && html`<span class="badge failed">Failed</span>`}
      ${status === 'denied' && html`<span class="badge failed">Denied</span>`}
    </button>
    ${open && html`<pre>${item.detail && item.detail.length > 60 ? `${item.detail}\n\n` : ''}${item.output || (status === 'running' ? 'Running…' : 'No output.')}</pre>`}
    ${openSub && html`<button type="button" class="btn small ghost" style=${{ margin: '0 6px 6px' }} onClick=${openSub}><${Icon} name="bot" size="s" />Open the sub-agent</button>`}
    ${sheet && SubSheet && html`<${SubSheet} threadId=${threadId} callId=${item.call_id} link=${sub} onClose=${() => setSheet(false)} />`}
  </div>`;
}

// ---------------------------------------------------------------------------------------------
// Requests

const DESTRUCTIVE = /\brm\s+-[a-z]*[rf]|\bgit\s+(push\s+(-f\b|--force)|reset\s+--hard|clean\s+-[a-z]*f)|\bdrop\s+(table|database)\b|\bmkfs\b|\bdd\s+if=|\bshutdown\b|\bformat\s+[a-z]:|Remove-Item\b.*-Recurse/i;

async function answer(threadId, requestId, response, setBusy) {
  setBusy(true);
  try {
    await app.ask({ type: 'answer', thread_id: threadId, request_id: requestId, response });
  } catch (err) {
    app.toast(err.message, 'negative');
  } finally {
    setBusy(false);
  }
}

const STATE_WORDS = {
  allowed: ['Allowed', 'done'],
  allowed_for_session: ['Allowed for this session', 'done'],
  denied: ['Denied', 'failed'],
  resolved: ['Moved on', ''],
  answered: ['Answered', 'done'],
  approved: ['Approved', 'done'],
  rejected: ['Sent back', 'failed'],
};

function Resolved({ state }) {
  const [text, tone] = STATE_WORDS[state] || [state, ''];
  return html`<span class=${`badge ${tone}`}>${text}</span>`;
}

export function Approval({ item, threadId }) {
  const [busy, setBusy] = useState(false);
  const [writing, setWriting] = useState(false);
  const pending = item.state === 'pending';
  const warning = DESTRUCTIVE.test(item.detail || '');
  // Not for a request a rule of the user's always asks about: the computer refuses that.
  const session = pending && !item.asked_by && app.state.macSettings?.session_approvals === true;
  // "Always allow…" writes a rule, which only the computer's setting lets a phone do.
  const AlwaysAllow = sheets.get('always-allow');
  const rule = pending && !!item.always_allow && !item.asked_by && !!AlwaysAllow && app.has('rules') && app.state.macSettings?.phone_rules === true;
  const forSession = () => {
    const agent = app.row(threadId)?.agent?.name || 'the agent';
    if (confirm(`Let ${agent} do this kind of thing without asking again this session?`)) answer(threadId, item.request_id, { kind: 'approval', decision: 'allow_for_session' }, setBusy);
  };
  return html`<div class=${`card ${pending ? 'ask' : ''}`}>
    <div class="card-head"><${Icon} name="shield" size="s" /><span class="grow">${item.title || 'Approval'}</span>${!pending && html`<${Resolved} state=${item.state} />`}</div>
    ${item.detail && html`<pre class="detail">${item.detail}</pre>`}
    ${warning && pending && html`<p class="muted" style=${{ margin: '0 0 10px', fontSize: '13px' }}><${Icon} name="alert" size="xs" /> This could delete or overwrite things. Read it before you allow it.</p>`}
    ${item.asked_by && pending && html`<p class="muted" style=${{ margin: '0 0 10px', fontSize: '13px' }}>Your rule asks about this every time: <span class="mono">${item.asked_by}</span></p>`}
    ${pending &&
    html`<div class="row-btns">
      <button class="btn" type="button" disabled=${busy} onClick=${() => answer(threadId, item.request_id, { kind: 'approval', decision: 'deny' }, setBusy)}>Deny</button>
      <button class=${`btn ${warning ? '' : 'primary'}`} type="button" disabled=${busy} onClick=${() => answer(threadId, item.request_id, { kind: 'approval', decision: 'allow' }, setBusy)}>Allow</button>
      ${session && html`<button class="btn ghost small" type="button" disabled=${busy} onClick=${forSession}>Allow for this session…</button>`}
      ${rule && html`<button class="btn ghost small" type="button" disabled=${busy} onClick=${() => setWriting(true)}>Always allow…</button>`}
    </div>`}
    ${writing && AlwaysAllow && html`<${AlwaysAllow} threadId=${threadId} item=${item} onClose=${() => setWriting(false)} />`}
  </div>`;
}

export function Questions({ item, threadId }) {
  const questions = item.questions || [];
  const pending = item.state === 'pending';
  const [picked, setPicked] = useState(() => questions.map(() => []));
  const [other, setOther] = useState(() => questions.map(() => ''));
  const [busy, setBusy] = useState(false);
  const toggle = (qi, label, multi) => {
    const next = picked.map((p, i) => (i !== qi ? p : multi ? (p.includes(label) ? p.filter((l) => l !== label) : [...p, label]) : [label]));
    setPicked(next);
  };
  const ready = questions.every((q, i) => picked[i].length > 0 || other[i].trim());
  const send = () => {
    const answers = questions.map((q, i) => ({
      question: q.question,
      answer: [...picked[i], ...(other[i].trim() ? [other[i].trim()] : [])].join(', '),
    }));
    answer(threadId, item.request_id, { kind: 'questions', answers }, setBusy);
  };
  return html`<div class=${`card ${pending ? 'ask' : ''}`}>
    <div class="card-head"><${Icon} name="question" size="s" /><span class="grow">${questions.length > 1 ? `${questions.length} questions` : 'A question'}</span>${!pending && html`<${Resolved} state=${item.state} />`}</div>
    ${questions.map(
      (q, qi) => html`<div style=${{ marginBottom: '10px' }}>
        ${q.header && html`<div class="badge" style=${{ marginBottom: '6px' }}>${q.header}</div>`}
        <div style=${{ fontWeight: 600, marginBottom: '8px', color: 'var(--fg-strong)' }}>${q.question}</div>
        ${pending
          ? html`${(q.options || []).map(
                (o) => html`<button type="button" class="option" aria-pressed=${picked[qi].includes(o.label) ? 'true' : 'false'} onClick=${() => toggle(qi, o.label, q.multi)}>
                  <${Icon} name=${picked[qi].includes(o.label) ? 'check' : q.multi ? 'plus' : 'forward'} size="s" />
                  <span><div class="label">${o.label}</div>${o.description && html`<div class="desc">${o.description}</div>`}</span>
                </button>`,
              )}
              <input class="input" type=${q.secret ? 'password' : 'text'} placeholder=${q.options?.length ? 'Or say something else' : 'Your answer'} value=${other[qi]} onInput=${(e) => setOther(other.map((v, i) => (i === qi ? e.target.value : v)))} />`
          : html`<div class="muted">${(item.answers || []).find((a) => a.question === q.question)?.answer || ''}</div>`}
      </div>`,
    )}
    ${pending && html`<button class="btn primary block" type="button" disabled=${!ready || busy} onClick=${send}>Answer</button>`}
  </div>`;
}

export function Plan({ item, threadId }) {
  const pending = item.state === 'pending';
  const [feedback, setFeedback] = useState('');
  const [busy, setBusy] = useState(false);
  return html`<div class=${`card ${pending ? 'plan' : ''}`}>
    <div class="card-head"><${Icon} name="plan" size="s" /><span class="grow">Plan</span>${!pending && html`<${Resolved} state=${item.state} />`}</div>
    <${Markdown} text=${item.markdown} threadId=${threadId} />
    ${pending &&
    html`<div style=${{ marginTop: '10px' }}>
      <textarea class="input" rows="2" placeholder="What to change (to send it back)" value=${feedback} onInput=${(e) => setFeedback(e.target.value)}></textarea>
      <div class="row-btns" style=${{ marginTop: '8px' }}>
        <button class="btn" type="button" disabled=${busy} onClick=${() => answer(threadId, item.request_id, { kind: 'plan', approve: false, feedback: feedback.trim() || null }, setBusy)}>Send back</button>
        <button class="btn primary" type="button" disabled=${busy} onClick=${() => answer(threadId, item.request_id, { kind: 'plan', approve: true, feedback: null }, setBusy)}>Approve</button>
      </div>
    </div>`}
  </div>`;
}

// ---------------------------------------------------------------------------------------------
// The rest

function TurnEnd({ item, threadId }) {
  const verified = item.verified;
  return html`<${Actions} item=${item} threadId=${threadId} /><div class="turn-end">
    <span>${item.took_secs != null ? `Worked ${took(item.took_secs)}` : 'Done'}${item.at ? ` · ${clock(item.at)}` : ''}</span>
    ${verified && html`<span class=${`badge ${verified.passed ? 'done' : 'failed'}`}>${verified.passed ? 'Verified' : 'Verification failed'}</span>`}
  </div>`;
}

function Changes({ item }) {
  const [open, setOpen] = useState(false);
  const files = item.files || [];
  return html`<div class="tool">
    <button type="button" aria-expanded=${open ? 'true' : 'false'} onClick=${() => setOpen(!open)}>
      <${Icon} name="diff" size="s" /><span class="what">${plural(files.length, 'file')} changed</span><span class="detail"></span>
      <span class="counts"><span class="add">+${item.added || 0}</span> <span class="del">−${item.removed || 0}</span></span>
    </button>
    ${open &&
    html`<div class="files" style=${{ padding: '4px 10px 8px', borderTop: '1px solid var(--border-soft)' }}>
      ${files.map(
        (f) => html`<div class="file">
          <span class="path">${f.status === 'renamed' && f.from ? `${f.from} → ${f.path}` : f.path}</span>
          ${f.binary ? html`<span class="muted">binary</span>` : html`<span class="mono"><span class="add">+${f.added}</span> <span class="del">−${f.removed}</span></span>`}
          ${f.status !== 'modified' && html`<span class="badge">${f.status}</span>`}
        </div>`,
      )}
    </div>`}
  </div>`;
}

const LINK_WORDS = { started: 'Started', continued_in: 'Continued in', started_by: 'Started by', continued_from: 'Continued from' };

function ThreadLink({ item }) {
  const row = app.row(item.thread_id);
  const title = row?.title || item.title || 'a thread';
  const open = item.open !== false;
  return html`<button type="button" class="tool" style=${{ textAlign: 'left' }} disabled=${!open} onClick=${() => open && app.go({ screen: 'thread', threadId: item.thread_id })}>
    <span style=${{ display: 'flex', gap: '8px', alignItems: 'center', padding: '8px 10px' }}>
      <${Icon} name="link" size="s" /><span class="what">${LINK_WORDS[item.link] || 'Thread'}</span><span class="detail" style=${{ fontFamily: 'var(--font)' }}>${title}</span>
      ${open ? html`<${Icon} name="forward" size="s" />` : html`<span class="badge">Gone</span>`}
    </span>
  </button>`;
}

const PHASE_TONE = { merged: 'done', published: 'done', passed: 'done', ready: 'done', failed: 'failed', conflicted: 'approval', closed: '', checking: 'working', merging: 'working', releasing: 'working', creating: 'working', open: 'sky' };

function WorkCard({ card, threadId }) {
  const [busy, setBusy] = useState(false);
  const act = async (action) => {
    setBusy(true);
    try {
      await app.ask({ type: 'card_action', thread_id: threadId, key: card.key, action });
    } catch (err) {
      app.toast(err.message, 'negative');
    } finally {
      setBusy(false);
    }
  };
  const kind = card.kind === 'pr' ? `Pull request${card.number ? ` #${card.number}` : ''}` : card.kind === 'run' ? card.title || 'Workflow run' : card.kind === 'release' ? 'Release' : 'Card';
  const checks = card.checks || card.stages || card.jobs || [];
  return html`<div class="card">
    <div class="card-head"><${Icon} name=${card.kind === 'pr' ? 'branch' : 'play'} size="s" /><span class="grow">${kind}</span>${card.phase && html`<span class=${`badge ${PHASE_TONE[card.phase] || ''}`}>${card.phase}</span>`}</div>
    ${card.kind === 'pr' && card.title && html`<div style=${{ fontWeight: 600, marginBottom: '4px' }}>${card.title}</div>`}
    ${card.head && html`<div class="muted mono" style=${{ fontSize: '12px', marginBottom: '6px' }}>${card.head}${card.base ? ` → ${card.base}` : ''}${card.added != null ? ` · +${card.added} −${card.removed || 0}` : ''}</div>`}
    ${checks.length > 0 &&
    html`<div class="files">${checks.map(
      (c) => html`<div class="file"><span class="path" style=${{ fontFamily: 'var(--font)' }}>${c.name || c.label}</span><span class=${`badge ${c.state === 'pass' ? 'done' : c.state === 'fail' ? 'failed' : c.state === 'running' ? 'working' : ''}`}>${c.state}</span></div>`,
    )}</div>`}
    ${card.conflicts?.length > 0 && html`<div class="muted" style=${{ fontSize: '13px', marginTop: '6px' }}>Conflicts in ${card.conflicts.join(', ')}</div>`}
    <div class="row-btns" style=${{ marginTop: '10px' }}>
      ${card.url && html`<a class="btn small" href=${card.url} target="_blank" rel="noopener noreferrer"><${Icon} name="external" size="s" />Open</a>`}
      ${(card.actions || []).includes('merge') && html`<button class="btn small primary" type="button" disabled=${busy} onClick=${() => act('merge')}>Merge</button>`}
      ${(card.actions || []).includes('resolve') && html`<button class="btn small" type="button" disabled=${busy} onClick=${() => act('resolve')}>Resolve conflicts</button>`}
      ${(card.actions || []).includes('rerun') && html`<button class="btn small" type="button" disabled=${busy} onClick=${() => act('rerun')}>Re-run</button>`}
    </div>
  </div>`;
}

export { ago };
