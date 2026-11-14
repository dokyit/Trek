// An interface mockup, as the iPhone draws one (VizMockup.swift): a tree of semantic parts in
// Trek's own controls, optionally inside a device's chrome. Pictures are placeholders by design:
// nothing is ever loaded.

import { initials, progressFraction } from './viz-spec.js';
import { el, icon, seriesColor, toneColor } from './viz-dom.js';

const kids = (node) => node.children.map(part);

function part(node) {
  const text = node.text ?? '';
  const color = toneColor(node.tone);
  switch (node.kind) {
    case 'frame':
      return chrome(node);
    case 'row':
      return el('div', { class: 'mk-row' }, kids(node));
    case 'column':
      return el('div', { class: 'mk-col' }, kids(node));
    case 'card':
      return el('div', { class: 'mk-card' }, node.text !== undefined && el('div', { class: 'mk-card-title', text }), kids(node));
    case 'list':
      return el(
        'div',
        { class: 'mk-list-wrap' },
        node.text !== undefined && el('div', { class: 'mk-list-title', text }),
        el(
          'div',
          { class: 'mk-list' },
          node.children.map((child) => el('div', { class: 'mk-list-row' }, el('div', { class: 'viz-grow' }, part(child)), child.kind === 'text' && icon('chevron', 'viz-ico xs mk-chev'))),
        ),
      );
    case 'nav':
      return el(
        'div',
        { class: 'mk-nav' },
        node.text !== undefined && el('b', { class: 'mk-nav-title', text }),
        el('span', { class: 'viz-grow' }),
        node.children.map((child) => (child.kind === 'text' ? el('span', { class: 'mk-nav-link', text: child.text ?? '' }) : part(child))),
      );
    case 'tabs': {
      const active = node.value ?? node.children[0]?.text;
      return el(
        'div',
        { class: 'mk-tabs', style: { '--c': color } },
        node.children.map((child) => el('span', { class: `mk-tab${child.text === active ? ' on' : ''}`, text: child.text ?? '' })),
      );
    }
    case 'text':
      return el('p', { class: 'mk-text', text });
    case 'metric':
      return el('div', { class: 'mk-metric' }, el('span', { text }), el('b', { style: node.tone ? { color } : null, text: node.value ?? '' }));
    case 'button': {
      const look = !node.tone || node.tone === 'accent' ? 'primary' : node.tone === 'neutral' ? 'neutral' : 'toned';
      return el('span', { class: `mk-button ${look}`, style: { '--c': color }, text });
    }
    case 'badge':
      return el('span', { class: 'mk-badge', style: { '--c': color } }, el('i'), el('span', { text }));
    case 'divider':
      return el('hr', { class: 'mk-divider' });
    case 'input': {
      const typed = node.value;
      return el(
        'div',
        { class: 'mk-input-wrap' },
        typed !== undefined && el('span', { class: 'mk-input-label', text }),
        el('div', { class: `mk-input${typed !== undefined ? ' typed' : ''}`, style: { '--c': color } }, icon('search', 'viz-ico xs'), el('span', { text: typed ?? text })),
      );
    }
    case 'toggle':
      return el('div', { class: 'mk-toggle' }, el('span', { class: 'viz-grow', text }), el('span', { class: `mk-switch${node.value === 'on' ? ' on' : ''}`, style: { '--c': color } }, el('i')));
    case 'progress': {
      const value = node.value ?? '';
      return el(
        'div',
        { class: 'mk-progress' },
        el('div', { class: 'mk-progress-head' }, el('span', { class: 'viz-grow', text }), el('span', { class: 'viz-mono muted', text: value.endsWith('%') ? value : `${value}%` })),
        el('div', { class: 'mk-progress-track' }, el('span', { style: { width: `${(progressFraction(value) ?? 0) * 100}%`, '--c': color } })),
      );
    }
    case 'avatar': {
      let h = 0;
      for (const b of new TextEncoder().encode(text)) h = (h * 31 + b) & 0xffff;
      const tint = node.tone ? color : seriesColor(h);
      return el('span', { class: 'mk-avatar', style: { '--c': tint }, text: initials(text) });
    }
    case 'image':
      return el('div', { class: `mk-image ${node.value || 'wide'}` }, icon('image', 'viz-ico'), node.text !== undefined && el('span', { text }));
    default:
      return null;
  }
}

/** A frame's chrome: a phone or tablet with its status bar, or a browser or window with its
 * traffic lights. */
function chrome(node) {
  const device = node.device || 'window';
  const content = el('div', { class: 'mk-col mk-content' }, kids(node));
  if (device === 'phone' || device === 'tablet') {
    return el(
      'div',
      { class: `mk-device ${device}` },
      el(
        'div',
        { class: 'mk-screen' },
        device === 'phone' && el('span', { class: 'mk-island' }),
        el('div', { class: 'mk-status' }, el('b', { text: '9:41' }), el('span', { class: 'viz-grow' }), icon('signal', 'viz-ico xs'), icon('wifi', 'viz-ico xs'), icon('battery', 'viz-ico xs')),
        el('div', { class: 'mk-screen-body' }, node.text !== undefined && el('div', { class: 'mk-screen-title', text: node.text }), content),
        device === 'phone' && el('span', { class: 'mk-home' }),
      ),
    );
  }
  const browser = device === 'browser';
  return el(
    'div',
    { class: `mk-window ${device}` },
    el(
      'div',
      { class: 'mk-bar' },
      el('span', { class: 'mk-lights' }, el('i'), el('i'), el('i')),
      browser
        ? [icon('backward', 'viz-ico xs muted'), el('span', { class: 'mk-url' }, icon('lock', 'viz-ico xs'), el('span', { text: node.text ?? 'app' }))]
        : el('span', { class: 'mk-window-title', text: node.text ?? '' }),
      el('span', { class: browser ? 'mk-pad s' : 'mk-pad' }),
    ),
    el('div', { class: 'mk-window-body' }, content),
  );
}

export function mockup(viz) {
  const framed = viz.nodes.some((n) => n.kind === 'frame');
  return el('div', { class: `viz-mock${framed ? ' framed viz-inset' : ' plain'}` }, viz.nodes.map(part));
}
