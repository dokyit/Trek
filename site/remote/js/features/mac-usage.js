// Plan usage as the computer's Usage card shows it: the providers it shows (picked there, three at
// most), each limit's window, how much of it is left and when it resets, and the resets Codex
// granted, which can be spent here after the computer's own warning (feature `use_reset`).

import { useEffect, useRef, useState } from '../../vendor/hooks.mjs';
import { html, Icon, IconButton, Spinner } from '../ui.js';
import * as app from '../model.js';
import { Frame, Asking, AgentMark, Confirm, LimitBar, useOnline, useMounted, recall, remember } from './mac-parts.js';
import { asOf, left, until, usedTone } from './mac-text.js';

export function UsageScreen() {
  const state = app.useApp();
  const [usage, setUsage] = useState(() => recall('usage'));
  const [asking, setAsking] = useState(false);
  const again = useRef(null);
  const mounted = useMounted();

  const load = async () => {
    clearTimeout(again.current);
    setAsking(true);
    try {
      const reply = await app.ask({ type: 'usage' });
      remember('usage', reply);
      if (!mounted.current) return;
      setUsage(reply);
      // An agent was still answering: ask again shortly for the rest, as the iPhone does.
      if (reply.loading) again.current = setTimeout(() => mounted.current && load(), 3000);
    } catch (err) {
      if (mounted.current) app.toast(err.message, 'negative');
    } finally {
      if (mounted.current) setAsking(false);
    }
  };
  useOnline(load);
  useEffect(() => () => clearTimeout(again.current), []);

  const providers = usage?.providers || [];
  const busy = asking || !!usage?.loading;
  return html`<${Frame}
    title="Usage"
    id="mac-usage"
    sub=${busy && providers.length > 0 ? 'Asking your agents…' : state.machine?.name}
    actions=${busy ? html`<span class="mac-bar-spinner"><${Spinner} /></span>` : html`<${IconButton} icon="refresh" label="Ask again" onClick=${load} disabled=${state.status !== 'online'} />`}
  >
    <div class="mac-page">
      ${!usage || (providers.length === 0 && busy)
        ? html`<${Asking} text="Asking your agents…" />`
        : providers.length === 0
          ? html`<div class="empty">
              <${Icon} name="gauge" />
              <p class="mac-empty-title">No plan usage</p>
              <p>${usage.picked ? 'No providers picked. Choose some on your computer’s Usage card.' : 'None of the agents on your computer reported a plan.'}</p>
            </div>`
          : html`
              ${providers.map((p) => html`<${Provider} key=${p.agent?.key} provider=${p} reload=${load} />`)}
              ${usage.picked != null &&
              html`<p class="mac-quiet">${usage.picked ? 'The providers you picked for the Usage card on your computer. Change them there.' : 'The providers on your computer’s Usage card, picked by Trek. Choose others there.'}</p>`}
            `}
    </div>
  <//>`;
}

/** One agent's plan: its name and plan, what's left of its tightest limit, each limit as a bar, and its resets. */
function Provider({ provider: p, reload }) {
  const limits = p.limits || [];
  const worst = limits.length ? Math.max(...limits.map((l) => Number(l.percent) || 0)) : null;
  const noLimits = p.no_limits ?? (limits.length === 0 && !p.error && !p.note);
  return html`<section class="mac-card mac-provider">
    <div class="mac-provider-head">
      <${AgentMark} agent=${p.agent} />
      <div class="grow">
        <div class="mac-provider-name">${p.agent?.name}</div>
        ${p.plan && html`<div class="muted">${p.plan}</div>`}
      </div>
      ${worst != null && html`<b class=${`mac-left ${usedTone(worst)}`} aria-label=${`${left(worst)} percent left on the tightest limit`}>${left(worst)}% left</b>`}
    </div>
    ${p.error && html`<p class="mac-warn"><${Icon} name="alert" size="xs" /> ${p.error}</p>`}
    ${p.note && html`<p class="muted">${p.note}</p>`}
    ${noLimits && html`<p class="muted">No usage limits on this plan.</p>`}
    ${limits.map(
      (l) => html`<div class="mac-limit" key=${l.label}>
        <div class="mac-limit-head">
          <span class="mac-limit-label">${l.label}</span>
          ${l.window && html`<span class="mac-window">${l.window}</span>`}
          <span class="grow"></span>
          <span class="muted mac-num">${left(l.percent)}% left</span>
        </div>
        <${LimitBar} percent=${l.percent} />
        ${l.resets_at && html`<div class="mac-small muted">Resets ${until(l.resets_at)}</div>`}
      </div>`,
    )}
    ${p.today && limits.length === 0 && html`<p class="muted">${p.today}</p>`}
    ${p.as_of && html`<div class="mac-small muted">${asOf(p.as_of)}</div>`}
    ${p.resets?.length > 0 && app.has('use_reset') && html`<${Resets} resets=${p.resets} reload=${reload} />`}
  </section>`;
}

/** "Usage limit resets · 1 available": Codex's granted resets, each with when it lapses and Use reset, behind the computer's own warning. */
function Resets({ resets, reload }) {
  const [asking, setAsking] = useState(null);
  const [using, setUsing] = useState(() => new Set());
  const mounted = useMounted();
  const use = async (credit) => {
    setUsing((s) => new Set(s).add(credit.id));
    try {
      await app.ask({ type: 'use_reset', credit_id: credit.id }, { timeout: 60_000 });
      app.toast('Usage limits reset');
      reload();
    } catch (err) {
      app.toast(err.message, 'negative');
    } finally {
      if (mounted.current)
        setUsing((s) => {
          const next = new Set(s);
          next.delete(credit.id);
          return next;
        });
    }
  };
  return html`<div class="mac-resets">
    <div class="mac-limit-head"><b class="grow">Usage limit resets</b><b class="done">${resets.length} available</b></div>
    ${resets.map(
      (r) => html`<div class="mac-reset" key=${r.id}>
        <div class="grow">
          <div>${r.title}</div>
          ${r.expires_at && html`<div class="mac-small muted">Expires ${until(r.expires_at)}</div>`}
        </div>
        <button class="btn small" type="button" disabled=${using.has(r.id)} onClick=${() => setAsking(r)}>${using.has(r.id) ? html`<${Spinner} />` : 'Use reset'}</button>
      </div>`,
    )}
    ${asking &&
    html`<${Confirm}
      title=${`Use “${asking.title}”?`}
      text="Your rate limits reset right away. The credit is spent and can’t be returned."
      action="Use reset"
      danger=${true}
      onConfirm=${() => use(asking)}
      onClose=${() => setAsking(null)}
    />`}
  </div>`;
}
