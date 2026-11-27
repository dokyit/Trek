// The fixture's background: each command from its page (options.js) calls one API and answers
// what came of it; events the APIs send are kept in storage.local's `events`.
const record = async (entry) => {
  const { events = [] } = await chrome.storage.local.get('events');
  events.push(entry);
  await chrome.storage.local.set({ events });
};
chrome.notifications.onClicked.addListener((id) => record({ event: 'clicked', id }));
chrome.notifications.onButtonClicked.addListener((id, button) => record({ event: 'button', id, button }));
chrome.notifications.onClosed.addListener((id, byUser) => record({ event: 'closed', id, byUser }));

const commands = {
  shim: () => ({
    notifications: typeof chrome.notifications, offscreen: typeof chrome.offscreen,
    identity: typeof chrome.identity, idle: typeof chrome.idle, downloads: typeof chrome.downloads,
    managed: typeof chrome.storage.managed, world: chrome.scripting.ExecutionWorld && chrome.scripting.ExecutionWorld.MAIN,
    history: typeof chrome.webNavigation.onHistoryStateUpdated,
  }),
  notify: () => chrome.notifications.create('n1', {
    type: 'basic', iconUrl: 'icon.png', title: 'Fixture says hi', message: 'From the background',
    buttons: [{ title: 'Yes' }, { title: 'No' }],
  }),
  all: () => chrome.notifications.getAll(),
  clear: () => chrome.notifications.clear('n1'),
  events: async () => (await chrome.storage.local.get('events')).events || [],
  offscreen: async () => {
    if (!(await chrome.offscreen.hasDocument())) {
      await chrome.offscreen.createDocument({ url: 'offscreen.html', reasons: ['DOM_PARSER'], justification: 'parse' });
    }
    // The offscreen page parses HTML (a DOMParser, which a worker hasn't) and answers.
    for (let i = 0; i < 50; i++) {
      try {
        const answer = await chrome.runtime.sendMessage({ to: 'offscreen', html: '<p id=x>parsed</p>' });
        if (answer) return answer;
      } catch {}
      await new Promise((r) => setTimeout(r, 100));
    }
    return 'no answer';
  },
  idle: () => chrome.idle.queryState(60),
  native: () => chrome.runtime.sendNativeMessage('com.trek.fixture_echo', { ping: 1 }),
  port: () => new Promise((resolve) => {
    const port = chrome.runtime.connectNative('com.trek.fixture_echo');
    port.onMessage.addListener((m) => { resolve(m); port.disconnect(); });
    port.onDisconnect.addListener(() => resolve({ disconnected: chrome.runtime.lastError ? chrome.runtime.lastError.message : true }));
    port.postMessage({ port: 2 });
  }),
  auth: (arg) => chrome.identity.launchWebAuthFlow({ url: arg, interactive: true }),
  redirect: () => chrome.identity.getRedirectURL('cb'),
  download: () => chrome.downloads.download({ url: 'data:text/plain,hello%20from%20the%20fixture', filename: 'trek-fixture.txt' }),
  tab: (arg) => chrome.tabs.create({ url: arg }).then((tab) => ({ id: tab && tab.id, url: tab && (tab.pendingUrl || tab.url) })),
};
chrome.runtime.onMessage.addListener((message, sender, reply) => {
  if (!message || !message.command || !commands[message.command]) return false;
  Promise.resolve()
    .then(() => commands[message.command](message.arg))
    .then((value) => reply({ ok: value === undefined ? null : value, origin: sender.origin }), (e) => reply({ error: String(e && e.message || e) }));
  return true;
});
