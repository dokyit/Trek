// Browser use: holding the answer of a script the agent runs in a page (`browser_eval`, see
// agent.rs). Unlike Trek's own commands (agent.js), the agent's script is for the page's own
// world: it's there to read the page's variables. So this runs there too, and everything it
// keeps is the page's to change: its answer is what the page says of itself, and no decision of
// Trek's rests on it. One expression: `.hold(id, promise)` keeps what the promise settles to,
// `.take(id)` hands it over, or says it's still waiting.
(() => {
  const KEY = '__trekEval';
  let results = null;
  try {
    const kept = Object.getOwnPropertyDescriptor(window, KEY);
    if (kept && kept.value && typeof kept.value === 'object') results = kept.value;
  } catch (_) {}
  if (!results) {
    results = {};
    try { Object.defineProperty(window, KEY, { value: results, configurable: true, enumerable: false, writable: true }); } catch (_) {}
  }
  const safe = (v) => {
    const seen = new WeakSet();
    const out = JSON.stringify(v, (k, x) => {
      if (typeof x === 'bigint') return x.toString() + 'n';
      if (typeof x === 'function') return '[function ' + (x.name || 'anonymous') + ']';
      if (typeof x === 'symbol') return x.toString();
      if (x === undefined) return null;
      if (x && typeof x === 'object') {
        if (x.nodeType === 1) return '[element ' + String(x.tagName).toLowerCase() + (x.id ? '#' + x.id : '') + ']';
        if (x.nodeType) return '[node ' + x.nodeName + ']';
        if (x === window) return '[window]';
        if (seen.has(x)) return '[circular]';
        seen.add(x);
        if (x instanceof Map) return Object.fromEntries(x);
        if (x instanceof Set) return Array.from(x);
        if (x instanceof Error) return { error: x.message, stack: String(x.stack || '').slice(0, 1000) };
      }
      return x;
    });
    return out === undefined ? 'undefined' : out;
  };
  return {
    hold(id, promise) {
      results[id] = null;
      Promise.resolve(promise).then(
        (v) => { results[id] = { value: safe(v) }; },
        (e) => { results[id] = { error: String((e && e.message) || e) }; },
      );
      return { started: id };
    },
    take(id) {
      const r = results[id];
      if (r) delete results[id];
      return r ? r : { waiting: id in results };
    },
  };
})()
