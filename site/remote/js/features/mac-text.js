// Words and sums for the computer's own screens (features/mac*.js): how much of a limit is left and
// when it resets, a search hit's matched words, the settings as a change leaves them, where a
// chart's bars and axis labels go, and the labels Trek gives levels, rules and outcomes. Plain
// functions with no imports, so Node's tests run them (script/site/test/mac.test.mjs).

// ---------------------------------------------------------------------------------------------
// Usage

/** The percent of a limit left when `percent` of it is used, rounded and never below 0. */
export function left(percent) {
  return Math.round(Math.max(0, 100 - (Number(percent) || 0)));
}

/** A limit's colour by how much of it is used, as the Mac's: red from 90%, amber from 70%. */
export function usedTone(percent) {
  return percent >= 90 ? 'failed' : percent >= 70 ? 'approval' : '';
}

/** The most used of any limit the providers show, 0–100. */
export function peak(usage) {
  let most = 0;
  for (const p of usage?.providers || []) for (const l of p.limits || []) most = Math.max(most, Number(l.percent) || 0);
  return most;
}

/** The limit closest to running out. */
export function tightest(provider) {
  let worst = null;
  for (const l of provider?.limits || []) if (!worst || l.percent > worst.percent) worst = l;
  return worst;
}

/** When a limit resets or a credit lapses, as the Mac's Usage card says it: "in 42m", "in 2h 10m", "Fri 9:41 AM (in 4d)". */
export function until(ms, now = Date.now(), locale = undefined) {
  const s = Math.max(0, Math.round((ms - now) / 1000));
  if (s < 60) return 'in under a minute';
  if (s < 3600) return `in ${Math.floor(s / 60)}m`;
  if (s < 86400) return `in ${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;
  const when = new Date(ms).toLocaleString(locale, { weekday: 'short', hour: 'numeric', minute: '2-digit' });
  return `${when} (in ${Math.floor(s / 86400)}d)`;
}

/** "now", "5m", "3h", "2d", "1w": how long ago, as the iPhone's "As of …" says it. */
export function shortAgo(ms, now = Date.now()) {
  const s = Math.max(0, (now - ms) / 1000);
  if (s < 60) return 'now';
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  if (s < 7 * 86400) return `${Math.floor(s / 86400)}d`;
  return `${Math.floor(s / (7 * 86400))}w`;
}

/** "As of just now", "As of 40m ago": when a provider's figures kept from an earlier run were read. */
export function asOf(ms, now = Date.now()) {
  const ago = shortAgo(ms, now);
  return ago === 'now' ? 'As of just now' : `As of ${ago} ago`;
}

// ---------------------------------------------------------------------------------------------
// Basecamp

/** A review row's icon and colour: what it waits on, or how it ended. */
export function reviewLook(row) {
  switch (row?.status) {
    case 'needs_you': {
      const icon = { Question: 'question', 'Plan to review': 'plan', Approval: 'shield', 'Sub-agent needs you': 'shield' }[row.label] || 'alert';
      return { icon, tone: 'approval' };
    }
    case 'failed':
      return { icon: 'mac-circle-x', tone: 'failed' };
    case 'paused':
      return { icon: 'clock', tone: 'approval' };
    default:
      return { icon: 'mac-circle-check', tone: 'done' };
  }
}

/**
 * Where `n` bars go across `width` pixels: each one's slot, the gap between slots (a hairline
 * when there are many), and a bar's width (24 at most, so a week's seven stay bars, not blocks).
 */
export function barLayout(n, width) {
  if (!(n > 0) || !(width > 0)) return { slot: 0, gap: 0, bar: 0 };
  const gap = n > 60 ? 1 : n > 40 ? 2 : 3;
  const slot = Math.max(0, (width - gap * (n - 1)) / n);
  return { slot, gap, bar: Math.max(1, Math.min(slot, 24)) };
}

/** The left edge of bar `i`, centred in its slot. */
export function barX(i, layout) {
  return i * (layout.slot + layout.gap) + (layout.slot - layout.bar) / 2;
}

/** The bar under `x` (pixels from the chart's left edge). */
export function barAt(x, n, width) {
  if (!(n > 0) || !(width > 0)) return null;
  return Math.min(Math.max(Math.floor((x / width) * n), 0), n - 1);
}

/** A bar as an SVG path: rounded at its top (4 at most), square on the baseline. */
export function barPath(x, w, h, base) {
  const r = Math.max(0, Math.min(4, w / 2, h));
  const y = base - h;
  const n = (v) => Math.round(v * 100) / 100;
  return `M${n(x)} ${n(base)}V${n(y + r)}Q${n(x)} ${n(y)} ${n(x + r)} ${n(y)}H${n(x + w - r)}Q${n(x + w)} ${n(y)} ${n(x + w)} ${n(y + r)}V${n(base)}Z`;
}

/**
 * The axis labels that fit: each centred under its bar and kept inside the edges; one that would
 * run into the one before it (a narrow phone, a long label) is left out. `measure(text)` gives a
 * label's width in pixels.
 */
export function tickLayout(ticks, n, layout, width, measure) {
  const out = [];
  let taken = -Infinity;
  for (const t of ticks || []) {
    if (!(t.bar >= 0 && t.bar < n)) continue;
    const w = measure(t.label);
    const centre = t.bar * (layout.slot + layout.gap) + layout.slot / 2;
    const x = Math.min(Math.max(centre, w / 2), width - w / 2);
    if (x - w / 2 < taken + 6) continue;
    taken = x + w / 2;
    out.push({ label: t.label, x });
  }
  return out;
}

// ---------------------------------------------------------------------------------------------
// Search

/** What the phone itself has that matches `query` (titles, projects, agents, branches), newest first; every thread for no query. */
export function localMatches(threads, query) {
  const q = String(query || '').trim().toLowerCase();
  const sorted = [...threads].sort((a, b) => (b.updated_at || 0) - (a.updated_at || 0));
  if (!q) return sorted;
  return sorted.filter((t) => [t.title, t.project?.name, t.agent?.name, t.branch].some((s) => typeof s === 'string' && s.toLowerCase().includes(q)));
}

/**
 * The computer's results while they answer what's typed (or text it extends, or that extends it:
 * typing on shows the wider answer until the narrower one comes), then what only the phone matched.
 */
export function mergeResults({ query, found, foundFor, local }) {
  const mine = local.map((thread) => ({ thread }));
  const q = String(query || '').trim().toLowerCase();
  const f = String(foundFor || '').toLowerCase();
  if (!q || !f || !(q.startsWith(f) || f.startsWith(q))) return mine;
  const ids = new Set(found.map((r) => r.thread?.id));
  return [...found.filter((r) => r.thread?.id), ...mine.filter((r) => !ids.has(r.thread.id))];
}

/**
 * A search hit's excerpt in pieces, `[{ text, hit }]`, the matched words marked. `ranges` are
 * `[start, end)` in Unicode scalar values, as the Mac counts them; one that doesn't fit is ignored.
 */
export function highlight(text, ranges) {
  const chars = Array.from(String(text || ''));
  const marks = new Array(chars.length).fill(false);
  for (const r of ranges || []) {
    if (!Array.isArray(r) || r.length !== 2) continue;
    const [a, b] = r;
    if (!(Number.isInteger(a) && Number.isInteger(b) && a >= 0 && a < b && b <= chars.length)) continue;
    for (let i = a; i < b; i++) marks[i] = true;
  }
  const out = [];
  chars.forEach((c, i) => {
    const last = out[out.length - 1];
    if (last && last.hit === marks[i]) last.text += c;
    else out.push({ text: c, hit: marks[i] });
  });
  return out;
}

// ---------------------------------------------------------------------------------------------
// Settings

/** Hand-holding levels, as the Mac names them: `[value, label, what it means]`. */
export const ACCESS = [
  ['supervised', 'Supervised', 'Asks before every edit and command'],
  ['auto-accept-edits', 'Auto-accept edits', 'Applies file edits; asks before commands'],
  ['auto', 'Auto', 'Works on its own; checks before risky actions'],
  ['full-access', 'Full access', 'No prompts and no sandbox'],
];

export function accessLabel(value) {
  return ACCESS.find(([v]) => v === value)?.[1] || value || '';
}

/** "Extra high" for `xhigh`, else the effort with a capital. */
export function effortLabel(effort) {
  if (!effort) return '';
  return effort === 'xhigh' ? 'Extra high' : effort[0].toUpperCase() + effort.slice(1);
}

/** The efforts the agent's model offers (its default model when none is picked). */
export function effortsFor(agents, agentKey, modelId) {
  const agent = (agents || []).find((a) => a.key === agentKey);
  const id = modelId || agent?.default_model;
  return agent?.models?.find((m) => m.id === id)?.efforts || [];
}

/** "Claude Code · Opus 5.5 · Auto-accept edits": what new threads start with; null before the settings come. */
export function newThreadsLine(settings, agents) {
  if (!settings?.default_agent) return null;
  const agent = (agents || []).find((a) => a.key === settings.default_agent);
  const modelId = settings.default_model || agent?.default_model;
  const model = modelId ? agent?.models?.find((m) => m.id === modelId)?.label || (settings.default_model ? modelId : null) : null;
  return [agent?.name || settings.default_agent, model, settings.default_access ? accessLabel(settings.default_access) : null].filter(Boolean).join(' · ');
}

/**
 * The settings with `change` made (a `set_settings` message's fields), to show at once while the
 * computer answers: another agent goes back to its default model, `""` is the agent's default,
 * `push*` change the push settings, a project's failing checks join or leave the list.
 */
export function applySettings(settings, change) {
  const out = { ...settings, push: settings.push ? { ...settings.push } : settings.push };
  for (const [key, value] of Object.entries(change)) {
    switch (key) {
      case 'type':
      case 'new_push_topic':
      case 'push_test':
        break;
      case 'default_agent':
        if (value !== settings.default_agent) {
          out.default_agent = value;
          out.default_model = null;
        }
        break;
      case 'default_model':
        out.default_model = value === '' ? null : value;
        break;
      case 'push':
        if (out.push) out.push.enabled = value;
        break;
      case 'push_when':
        if (out.push) out.push.when = value;
        break;
      case 'push_server':
        if (out.push) out.push.server = value;
        break;
      case 'send_failed_checks':
        if (Array.isArray(out.send_failed_checks) && value?.project_id) {
          const on = out.send_failed_checks.filter((id) => id !== value.project_id);
          if (value.on) on.push(value.project_id);
          out.send_failed_checks = on;
        }
        break;
      default:
        out[key] = value;
    }
  }
  return out;
}

/** An ntfy server a phone may set: an `https://` address with a host (the computer refuses anything else). */
export function httpsAddress(text) {
  try {
    const url = new URL(String(text || '').trim());
    return url.protocol === 'https:' && !!url.hostname;
  } catch {
    return false;
  }
}

/** Whether `server` is ntfy's own (no "Use another server" step then). */
export function isNtfySh(server) {
  const s = String(server || '').trim().toLowerCase().replace(/\/+$/, '');
  return s === 'https://ntfy.sh' || s === 'ntfy.sh';
}

/** The computer's "System" theme, named for what it follows. */
export function systemThemeLabel(platform) {
  return { macos: 'Match macOS', windows: 'Match Windows' }[String(platform || '').toLowerCase()] || 'System';
}

/** "Windows", "macOS", "Linux"; null before the machine has said. */
export function platformWord(platform) {
  return { macos: 'macOS', windows: 'Windows', linux: 'Linux' }[String(platform || '').toLowerCase()] || null;
}

// ---------------------------------------------------------------------------------------------
// Permission rules

/** What a rule looks at, as the Mac's Settings › Permissions names it, with the fields' hints. */
export const RULE_KINDS = {
  command: { label: 'Command', hint: 'A command, like rm or git push', test: 'A command to try, like rm -rf build; echo done' },
  edit: { label: 'File edit', hint: 'A file or folder, like ~/.ssh/** or *.lock', test: 'A path to try, like src/main.rs' },
  fetch: { label: 'Web fetch', hint: 'An address, like example.com', test: 'An address to try, like https://example.com/x' },
  tool: { label: 'Tool', hint: 'A tool’s name, like WebSearch or mcp__github__*', test: 'A tool’s name to try, like mcp__github__create_issue' },
};

export const VERDICTS = [
  ['deny', 'Deny'],
  ['ask', 'Ask'],
  ['allow', 'Allow'],
];

export function kindLabel(kind) {
  return RULE_KINDS[kind]?.label || 'Tool';
}

export function verdictLabel(action) {
  return VERDICTS.find(([v]) => v === action)?.[1] || action || '';
}

/** "Deny command “rm”", as the Mac shows a rule. */
export function ruleShown(rule) {
  return `${verdictLabel(rule.action)} ${kindLabel(rule.kind).toLowerCase()} “${rule.pattern}”`;
}

/** How a call fares at a level: its word and colour. */
export function outcomeLook(outcome) {
  return { refused: ['Refused', 'failed'], asks: ['Asks', 'approval'], runs: ['Runs', 'done'] }[outcome] || ['?', ''];
}

/** What the tester says when no rule decides. */
export function decidedBy(test) {
  return test?.decided_by || 'No rule decides this: the thread’s level does.';
}

/** "Run command" → "run a command": a request's title as the end of "Claude Code wants to …". */
export function phrase(title) {
  const t = String(title || '').toLowerCase();
  if (t.startsWith('run') || t.startsWith('bash') || t.startsWith('exec')) return 'run a command';
  if (t.startsWith('edit') || t.startsWith('write') || t.startsWith('apply')) return 'change files';
  if (t.startsWith('fetch') || t.startsWith('web')) return 'go online';
  if (!t) return 'do something';
  return `use ${title}`;
}
