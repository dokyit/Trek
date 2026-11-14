// Search, as the iPhone has it: the threads this phone has filter as you type, and once the typing
// pauses the computer searches as its sidebar does (feature `search`): titles, then what was said,
// the message that matched under its thread with its words marked. Every thread it finds opens,
// settled long ago or a sub-agent's: one the list doesn't hold is kept as an `unlisted` row (as
// the iPhone keeps it), so it opens with its title, and the computer's own row follows it there.

import { useEffect, useRef, useState } from '../../vendor/hooks.mjs';
import { html, Icon, IconButton, Spinner } from '../ui.js';
import * as app from '../model.js';
import { ThreadRow } from '../screens.js';
import { ConnectionBanner } from '../thread.js';
import { highlight, localMatches, mergeResults } from './mac-text.js';

/** How long the typing pauses before the computer is asked. */
const PAUSE = 300;

export function SearchScreen() {
  const state = app.useApp();
  const [query, setQuery] = useState('');
  const [found, setFound] = useState([]);
  const [foundFor, setFoundFor] = useState('');
  const [searching, setSearching] = useState(false);
  const field = useRef(null);
  const typed = useRef('');
  const onComputer = app.has('search');
  const online = state.status === 'online';
  const trimmed = query.trim();
  typed.current = trimmed;

  useEffect(() => {
    field.current?.focus();
  }, []);

  useEffect(() => {
    if (!onComputer || !online || trimmed.length < 2) {
      setSearching(false);
      if (!trimmed) {
        setFound([]);
        setFoundFor('');
      }
      return undefined;
    }
    const timer = setTimeout(async () => {
      setSearching(true);
      try {
        const reply = await app.ask({ type: 'search', query: trimmed, limit: 60 });
        if (typed.current !== trimmed) return;
        setFound(reply.results || []);
        setFoundFor(trimmed);
      } catch (err) {
        if (typed.current === trimmed) app.toast(err.message, 'negative');
      } finally {
        if (typed.current === trimmed) setSearching(false);
      }
    }, PAUSE);
    return () => clearTimeout(timer);
  }, [trimmed, onComputer, online]);

  const results = mergeResults({ query: trimmed, found, foundFor, local: localMatches(state.threads.values(), trimmed) });
  const name = state.machine?.name || 'your computer';
  return html`<div class="screen mac-screen" id="mac-search">
    <header class="bar">
      <${IconButton} icon="back" label="Back" onClick=${() => app.back()} />
      <input
        ref=${field}
        class="input mac-search-field"
        type="search"
        enterkeyhint="search"
        aria-label="Search"
        placeholder=${onComputer ? 'Threads and messages' : 'Threads, projects, agents, branches'}
        value=${query}
        onInput=${(e) => setQuery(e.target.value)}
      />
    </header>
    <${ConnectionBanner} />
    <div class="scroll mac-scroll">
      ${searching && html`<div class="mac-searching" role="status"><${Spinner} /><span>Searching messages on ${name}…</span></div>`}
      ${results.map((r) => html`<${Hit} key=${r.thread.id} result=${r} />`)}
      ${results.length === 0 &&
      !searching &&
      html`<div class="empty">
        ${trimmed ? html`<p class="mac-empty-title">No results for “${trimmed}”</p><p>Check the spelling or try a new search.</p>` : 'No threads yet.'}
      </div>`}
    </div>
  </div>`;
}

/** A thread found: its row, then whose sub-agent it is and the message that matched, its words marked. */
function Hit({ result }) {
  const found = result.thread;
  const row = app.row(found.id) || found;
  const parent = found.parent_id ? app.row(found.parent_id) : null;
  // A thread the list doesn't hold is kept as the computer's unlisted rows are, before it opens.
  const keep = () => {
    if (!app.state.threads.has(found.id) && !app.state.unlisted.has(found.id)) app.receive({ type: 'unlisted', thread: found });
  };
  const open = () => app.go({ screen: 'thread', threadId: found.id });
  return html`<div class="mac-hit" onClickCapture=${keep}>
    <${ThreadRow} t=${row} />
    ${(result.snippet || found.parent_id) &&
    html`<div class="mac-hit-line" onClick=${open}>
      ${found.parent_id && html`<div class="mac-hit-parent"><${Icon} name="mac-users" size="xs" /><span class="mac-ellipsis">${parent ? `Sub-agent of ${parent.title}` : 'Sub-agent'}</span></div>`}
      ${result.snippet && html`<div class="mac-hit-snippet">${highlight(result.snippet, result.ranges).map((p) => (p.hit ? html`<mark>${p.text}</mark>` : p.text))}</div>`}
    </div>`}
  </div>`;
}
