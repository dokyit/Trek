// The computer's permission rules (feature `rules`), as its Settings › Permissions page has them:
// the list in order, a tester that says which rule decides a call at each level, and adding and
// removing a rule. Reading and trying them is always open; writing one is the computer's to allow
// (its Settings › Phone, `phone_rules`), since a rule lasts until it's removed. Until it does, the
// list is read-only and says where to turn it on. Also an approval card's "Always allow…".

import { useEffect, useState } from '../../vendor/hooks.mjs';
import { html, Icon, Menu, Segmented, Sheet, Spinner } from '../ui.js';
import * as app from '../model.js';
import { Frame, Asking, Confirm, Foot, useOnline, useMounted, recall, remember } from './mac-parts.js';
import { ACCESS, RULE_KINDS, VERDICTS, decidedBy, kindLabel, outcomeLook, phrase, ruleShown, verdictLabel } from './mac-text.js';

const KINDS = Object.entries(RULE_KINDS).map(([kind, k]) => [kind, k.label]);
const TONE = { deny: 'failed', ask: 'approval', allow: 'done' };

export function RulesScreen() {
  const state = app.useApp();
  const [list, setList] = useState(() => recall('rules'));
  const [kind, setKind] = useState('command');
  const [verdict, setVerdict] = useState('deny');
  const [pattern, setPattern] = useState('');
  const [busy, setBusy] = useState(false);
  const [allowing, setAllowing] = useState(null);
  const [picked, setPicked] = useState(null);
  const [testKind, setTestKind] = useState('command');
  const [tried, setTried] = useState('');
  const [test, setTest] = useState(null);
  const mounted = useMounted();
  const online = state.status === 'online';

  const take = (reply) => {
    remember('rules', reply);
    if (mounted.current) setList(reply);
  };
  useOnline(() =>
    app
      .ask({ type: 'rules' })
      .then(take)
      .catch((err) => mounted.current && app.toast(err.message, 'negative')),
  );

  // Tried as it's typed, once it settles a moment.
  useEffect(() => {
    const text = tried.trim();
    if (!text || !online) return undefined;
    const timer = setTimeout(() => {
      app
        .ask({ type: 'rule_test', kind: testKind, text })
        .then((t) => mounted.current && setTest(t))
        .catch((err) => mounted.current && app.toast(err.message, 'negative'));
    }, 250);
    return () => clearTimeout(timer);
  }, [testKind, tried, online]);

  /** Add or remove a rule; the computer answers with the list as it is after. */
  const write = async (msg, after) => {
    setBusy(true);
    try {
      take(await app.ask({ type: 'rule_action', ...msg }));
      after?.();
    } catch (err) {
      app.toast(err.message, 'negative');
    } finally {
      if (mounted.current) setBusy(false);
    }
  };
  const add = () => {
    const p = pattern.trim();
    if (!p || busy) return;
    const rule = { kind, pattern: p, action: verdict };
    // An allow lasts until it's removed: asked about first, as the iPhone asks for Face ID.
    if (verdict === 'allow') setAllowing(rule);
    else write({ action: 'add', rule }, () => setPattern(''));
  };

  const editable = !!list?.editable;
  const rules = list?.rules || [];
  const shownTest = test && test.text === tried.trim() && test.kind === testKind ? test : null;
  return html`<${Frame} title="Permission rules" sub=${state.machine?.name} id="mac-rules">
    <h2 class="group-title">Rules</h2>
    ${!list
      ? html`<${Asking} />`
      : html`<div class="group">
          ${rules.length === 0 && html`<div class="item muted">No rules yet. The thread’s level decides what asks.</div>`}
          ${rules.map((rule, i) => {
            const body = html`<span class=${`mac-verdict ${TONE[rule.action] || ''}`}>${verdictLabel(rule.action)}</span>
              <span class="grow"><span class="mac-mono mac-pattern">${rule.pattern}</span><span class="sub">${kindLabel(rule.kind)}</span></span>`;
            return editable
              ? html`<button type="button" class="item mac-rule" key=${i} aria-label=${ruleShown(rule)} onClick=${() => setPicked(i)}>${body}<${Icon} name="more" size="s" /></button>`
              : html`<div class="item mac-rule" key=${i} aria-label=${ruleShown(rule)}>${body}</div>`;
          })}
        </div>`}
    <${Foot}>A deny holds at every level. An ask puts a card up even at Full access. An allow lets it through without asking. Deny is tried first, then ask, then allow.<//>

    ${list &&
    (editable
      ? html`<h2 class="group-title">New rule</h2>
          <div class="group">
            <label class="item mac-choice"><span class="grow">Looks at</span>
              <select class="mac-select" value=${kind} onChange=${(e) => setKind(e.target.value)}>${KINDS.map(([v, label]) => html`<option value=${v}>${label}</option>`)}</select>
            </label>
            <div class="item mac-stack">
              <span>Then</span>
              <${Segmented} label="Then" options=${VERDICTS} value=${verdict} onChange=${setVerdict} />
            </div>
            <div class="item mac-stack">
              <input
                class="input mac-mono"
                type="text"
                autocapitalize="off"
                autocomplete="off"
                spellcheck="false"
                enterkeyhint="done"
                aria-label="Pattern"
                placeholder=${RULE_KINDS[kind].hint}
                value=${pattern}
                onInput=${(e) => setPattern(e.target.value)}
                onKeyDown=${(e) => e.key === 'Enter' && add()}
              />
              <button class="btn primary block" type="button" disabled=${!pattern.trim() || busy || !online} onClick=${add}>${busy ? html`<${Spinner} />` : 'Add rule'}</button>
            </div>
          </div>
          <${Foot}>${verdict === 'allow' ? 'An allow lasts until it’s removed.' : 'It applies from the next request, in sessions already running.'}<//>`
      : html`<div class="banner mac-locked"><${Icon} name="mac-lock" size="s" /><span class="grow">Adding and removing rules from this phone is off on your computer. Turn it on there in Trek’s Settings › Phone, or change them on the computer.</span></div>`)}

    <h2 class="group-title">Try a call</h2>
    <div class="group">
      <label class="item mac-choice"><span class="grow">Try</span>
        <select class="mac-select" value=${testKind} onChange=${(e) => setTestKind(e.target.value)}>${KINDS.map(([v, label]) => html`<option value=${v}>${label}</option>`)}</select>
      </label>
      <div class="item mac-stack">
        <input
          class="input mac-mono"
          type="text"
          autocapitalize="off"
          autocomplete="off"
          spellcheck="false"
          aria-label="Call to try"
          placeholder=${RULE_KINDS[testKind].test}
          value=${tried}
          onInput=${(e) => setTried(e.target.value)}
        />
        ${shownTest && html`<p class="mac-decided" aria-live="polite">${decidedBy(shownTest)}</p>`}
      </div>
      ${shownTest &&
      (shownTest.levels || []).map((l) => {
        const [word, tone] = outcomeLook(l.outcome);
        return html`<div class="item mac-level" key=${l.level}><span class="grow">${ACCESS.find(([v]) => v === l.level)?.[1] || l.level}</span><b class=${tone}>${word}</b></div>`;
      })}
    </div>
    <${Foot}>What happens to it at each level, with your rules, as your computer’s tester says.<//>

    ${picked != null &&
    rules[picked] &&
    html`<${Menu}
      title=${ruleShown(rules[picked])}
      onClose=${() => setPicked(null)}
      items=${[{ label: 'Remove rule', icon: 'trash', danger: true, onClick: () => write({ action: 'remove', index: picked, rule: rules[picked] }) }]}
    />`}
    ${allowing &&
    html`<${Confirm}
      title="Allow without asking?"
      text=${`Allow ${kindLabel(allowing.kind).toLowerCase()} “${allowing.pattern}” without asking, until the rule is removed.`}
      action="Add rule"
      onConfirm=${() => write({ action: 'add', rule: allowing }, () => setPattern(''))}
      onClose=${() => setAllowing(null)}
    />`}
  <//>`;
}

/**
 * An approval card's "Always allow…": the rule it would write (`item.always_allow`), ready to
 * shorten, then "Add rule and allow" once you say so (the iPhone asks for Face ID there). The
 * computer checks the rule would let this card's call through, and says why not when it wouldn't.
 * Offered where the computer serves `rules`, lets this phone write them (`phone_rules`), and has a
 * draft for the card (none where a rule of yours asks about it every time).
 */
export function AlwaysAllowSheet({ threadId, item, onClose }) {
  const draft = item.always_allow || { kind: 'command', pattern: '' };
  const [pattern, setPattern] = useState(draft.pattern || '');
  const [sure, setSure] = useState(false);
  const [busy, setBusy] = useState(false);
  const mounted = useMounted();
  const agent = app.row(threadId)?.agent?.name || 'The agent';
  const what = phrase(item.title);
  const p = pattern.trim();
  const add = async () => {
    setBusy(true);
    try {
      await app.ask({ type: 'always_allow', thread_id: threadId, request_id: item.request_id, pattern: p });
      app.toast('Rule added. It runs without asking from now on.');
      onClose();
    } catch (err) {
      app.toast(err.message, 'negative');
      if (mounted.current) setSure(false);
    } finally {
      if (mounted.current) setBusy(false);
    }
  };
  return html`<${Sheet} title="Always allow" onClose=${onClose}>
    <div class="field">
      <span>${agent} wants to ${what}</span>
      ${item.detail && html`<pre class="mac-detail">${item.detail}</pre>`}
    </div>
    <label class="field">
      <span>Allow ${kindLabel(draft.kind).toLowerCase()}</span>
      <textarea
        class="input mac-mono"
        rows="2"
        autocapitalize="off"
        spellcheck="false"
        placeholder=${RULE_KINDS[draft.kind]?.hint || ''}
        value=${pattern}
        onInput=${(e) => {
          setPattern(e.target.value);
          setSure(false);
        }}
      ></textarea>
    </label>
    <p class="mac-note">It runs without asking from now on. Your computer’s Settings › Permissions lists it, and it can be removed there or from Permission rules here.</p>
    ${sure
      ? html`<p class="mac-sure">Let ${agent} ${what} without asking, until the rule is removed?</p>
          <div class="row-btns mac-sheet-btns">
            <button class="btn" type="button" disabled=${busy} onClick=${() => setSure(false)}>Back</button>
            <button class="btn primary" type="button" disabled=${busy} onClick=${add}>${busy ? html`<${Spinner} />` : 'Allow'}</button>
          </div>`
      : html`<div class="row-btns mac-sheet-btns">
          <button class="btn" type="button" onClick=${onClose}>Cancel</button>
          <button class="btn primary" type="button" disabled=${!p} onClick=${() => setSure(true)}>Add rule and allow</button>
        </div>`}
  <//>`;
}
