// Screens and sheets that features add (`features/*.js`): each registers its screen by name, and
// `main.js` shows it for `{ screen: name, ... }` routes. Thread screens add their parts the same
// way (`registerThreadPart`): strips over the composer, extra menu items, item kinds.

export const screens = new Map();

/** A screen for `app.go({ screen: name, ... })`; it gets the route as its props. */
export function registerScreen(name, component) {
  screens.set(name, component);
}

/** Parts a thread screen draws, by place: `strips` (over the composer), `itemKinds` (a transcript item kind → component). */
export const threadParts = { strips: [], itemKinds: new Map(), menuItems: [], listMenuItems: [] };

/** A strip over the composer: `component({ row, threadId })`, drawn in the order registered. */
export function registerStrip(component) {
  threadParts.strips.push(component);
}

/** A transcript item kind this remote didn't draw before: `component({ item, threadId, last })`. */
export function registerItemKind(kind, component) {
  threadParts.itemKinds.set(kind, component);
}

/** Thread menu items: `fn({ row, threadId, has })` → `[{ label, icon, onClick, hidden }]`. */
export function registerMenuItems(fn) {
  threadParts.menuItems.push(fn);
}

/** The thread list's menu items: `fn({ state, has })` → `[{ label, icon, onClick, hidden }]`. */
export function registerListMenuItems(fn) {
  threadParts.listMenuItems.push(fn);
}

/**
 * Buttons in the composer (attach a photo or a file, dictate): `component({ threadId, text,
 * setText, attachments, setAttachments, disabled })`. `attachments` are what goes with the next
 * message: `{ kind: 'image', mime, data, name }` (base64 JPEG or PNG) or `{ kind: 'file', id, name }`
 * (an upload's id, from `upload_part`).
 */
export const composerTools = [];
export function registerComposerTool(component) {
  composerTools.push(component);
}

/** Over the composer's box, what's attached: `component({ attachments, setAttachments })`. */
export const composerPreviews = [];
export function registerComposerPreview(component) {
  composerPreviews.push(component);
}

/** A turn's or a message's actions (undo, retry, fork, rewind): `component({ item, threadId })`. */
export const turnActions = [];
export function registerTurnActions(component) {
  turnActions.push(component);
}

/** What the transcript's markdown becomes once drawn: `fn(element, { threadId })` after each render (math, charts, pictures). */
export const markdownEnhancers = [];
export function registerMarkdownEnhancer(fn) {
  markdownEnhancers.push(fn);
}
