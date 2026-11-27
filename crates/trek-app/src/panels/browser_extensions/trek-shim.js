// Added by Trek: Chrome APIs WebKit lacks, made from what WebKit has or answered by Trek itself.
// (crates/trek-app/src/panels/browser_extensions/compat.rs explains each part.)
(() => {
  const api = globalThis.chrome || globalThis.browser;
  if (!api || !api.runtime || globalThis.__trekShim) return;
  try { Object.defineProperty(globalThis, '__trekShim', { value: 1 }); } catch {}
  // Trek's own channel: WebKit hands a native message for this name to Trek, which answers it
  // (never to a program on the Mac).
  const BRIDGE = 'dev.trek.browser';
  const event = () => {
    const listeners = new Set();
    return {
      addListener: (f) => { listeners.add(f); },
      removeListener: (f) => { listeners.delete(f); },
      hasListener: (f) => listeners.has(f),
      hasListeners: () => listeners.size > 0,
      dispatch: (...args) => {
        for (const f of [...listeners]) {
          try { f(...args); } catch (e) { console.error(e); }
        }
      },
    };
  };
  const define = (object, key, value) => {
    try {
      Object.defineProperty(object, key, { value, configurable: true, enumerable: true, writable: true });
    } catch (e) {
      failed.push(`${key}: ${e && e.message}`);
    }
  };
  const failed = [];
  const missing = (key) => !(key in api) || api[key] === undefined;
  // A function that answers both ways Chrome's do: a promise, or the callback as last argument.
  const either = (f) => function (...args) {
    const callback = typeof args[args.length - 1] === 'function' ? args.pop() : null;
    const promise = Promise.resolve().then(() => f(...args));
    if (!callback) return promise;
    promise.then((value) => callback(value), (e) => { console.error(e); callback(undefined); });
    return undefined;
  };
  const native = api.runtime.sendNativeMessage ? api.runtime.sendNativeMessage.bind(api.runtime) : null;
  const call = (method, args) => {
    if (!native) return Promise.reject(new Error(`Trek can't answer ${method} here.`));
    return Promise.resolve(native(BRIDGE, { method, args: args === undefined ? null : args })).then((reply) => {
      if (reply && reply.error) throw new Error(reply.error);
      return reply ? reply.value : undefined;
    });
  };
  // Events Trek sends (a notification clicked): one port to Trek, opened when something listens.
  const events = {};
  let port = null;
  const listen = (name) => {
    const e = event();
    const add = e.addListener;
    e.addListener = (f) => { add(f); connect(); };
    events[name] = e;
    return e;
  };
  const connect = () => {
    if (port || !api.runtime.connectNative) return;
    try {
      port = api.runtime.connectNative(BRIDGE);
    } catch (e) { console.error(e); return; }
    // Trek only sends on it: nothing is posted (a port WebKit hasn't finished opening refuses a
    // message, which would land in the extension's own call that added a listener).
    try {
      port.onMessage.addListener((m) => {
        if (m && m.event && events[m.event]) events[m.event].dispatch(...(m.args || []));
      });
      port.onDisconnect.addListener(() => { port = null; });
    } catch (e) { port = null; }
  };

  // An extension's own errors in Trek's log (WebKit keeps only the uncaught ones): what stops
  // an extension is often only said on its console.
  if (native && typeof console !== 'undefined' && console.error) {
    const original = console.error.bind(console);
    let left = 200;
    const text = (v) => {
      if (v instanceof Error) return `${v.name}: ${v.message}${v.stack ? ' @ ' + String(v.stack).split('\n').slice(0, 6).join(' | ') : ''}`;
      if (typeof v === 'string') return v;
      try { return JSON.stringify(v); } catch { return String(v); }
    };
    const relay = (line) => {
      if (left-- > 0) {
        call('log.error', { text: String(line).slice(0, 2000), where: typeof document === 'undefined' ? 'background' : location.pathname }).catch(() => {});
      }
    };
    console.error = (...args) => {
      original(...args);
      relay(args.map(text).join(' '));
    };
    for (const level of ['warn']) {
      const o = console[level] && console[level].bind(console);
      if (o) console[level] = (...args) => { o(...args); relay(`[${level}] ` + args.map(text).join(' ')); };
    }
    if (typeof addEventListener === 'function') {
      addEventListener('error', (e) => relay(`Uncaught ${e.message} (${e.filename}:${e.lineno})`));
      addEventListener('unhandledrejection', (e) => relay(`Unhandled rejection: ${text(e.reason)}${e.reason && e.reason.stack ? ' ' + String(e.reason.stack).slice(0, 600) : ''}`));
    }
  }
  // WebKit keeps a namespace's object (`chrome.scripting`, `chrome.webNavigation`…) only while
  // something holds it: one nobody holds is made again on next use, without what was added to
  // it. So the shim holds each object it adds to (`held`). (Putting a proxy in place of
  // `chrome` instead broke WebKit's ports: `runtime.connect` found no listener behind it.)
  const held = [];
  const patchers = {};
  const patched = new WeakSet();
  const patch = (key, object) => {
    const f = patchers[key];
    if (!f || !object || typeof object !== 'object' || patched.has(object)) return;
    patched.add(object);
    held.push(object);
    try { f(object); } catch (e) { failed.push(`${key}: ${e && e.message}`); }
  };

  // No administrator's policy sets anything here: an empty managed store.
  const managed = {
    get: either(() => ({})),
    getBytesInUse: either(() => 0),
    getKeys: either(() => []),
    onChanged: event(),
  };
  patchers.storage = (storage) => { if (!storage.managed) define(storage, 'managed', managed); };

  // WebKit takes these worlds; only the constants are missing.
  const worlds = Object.freeze({ ISOLATED: 'ISOLATED', MAIN: 'MAIN' });
  patchers.scripting = (scripting) => {
    if (!scripting.ExecutionWorld) define(scripting, 'ExecutionWorld', worlds);
  };

  // webNavigation's events WebKit lacks: in-page navigations of a tab's main frame (its address
  // changing while it isn't loading), a page opening a new tab (a tab with an opener), and tabs
  // replaced (Trek never swaps one tab's page for another's, so that one never fires).
  const history = event();
  const fragment = event();
  const created = event();
  const replaced = event();
  patchers.webNavigation = (nav) => {
    if (!nav.onHistoryStateUpdated) define(nav, 'onHistoryStateUpdated', history);
    if (!nav.onReferenceFragmentUpdated) define(nav, 'onReferenceFragmentUpdated', fragment);
    if (!nav.onCreatedNavigationTarget) define(nav, 'onCreatedNavigationTarget', created);
    if (!nav.onTabReplaced) define(nav, 'onTabReplaced', replaced);
  };
  if (api.webNavigation && api.tabs && api.tabs.onUpdated) {
    const seen = new Map();
    const strip = (url) => String(url || '').split('#')[0];
    api.tabs.onUpdated.addListener((tabId, change, tab) => {
      const url = change.url || (tab && tab.url) || '';
      if (change.status === 'loading') { seen.set(tabId, { url, loading: true }); return; }
      if (change.status === 'complete') { seen.set(tabId, { url, loading: false }); return; }
      if (!change.url) return;
      const before = seen.get(tabId);
      const loading = tab && tab.status === 'loading';
      seen.set(tabId, { url, loading: !!loading });
      if (loading || !before || before.loading || before.url === url) return;
      const details = {
        tabId, url, frameId: 0, parentFrameId: -1, processId: -1,
        timeStamp: Date.now(), transitionType: 'link', transitionQualifiers: [],
      };
      (strip(before.url) === strip(url) ? fragment : history).dispatch(details);
    });
    if (api.tabs.onRemoved) api.tabs.onRemoved.addListener((tabId) => seen.delete(tabId));
    if (api.tabs.onCreated) {
      api.tabs.onCreated.addListener((tab) => {
        if (!tab || tab.openerTabId === undefined || tab.openerTabId < 0) return;
        created.dispatch({
          sourceTabId: tab.openerTabId, sourceProcessId: -1, sourceFrameId: 0,
          tabId: tab.id, url: tab.pendingUrl || tab.url || '', timeStamp: Date.now(),
        });
      });
    }
  }

  // webRequest's option names. WebKit reports requests to extensions (it can't hold or change
  // one: blocking goes through declarativeNetRequest), and has no constants to name the options.
  const c = (o) => Object.freeze(o);
  const requestConstants = {
    OnBeforeRequestOptions: c({ BLOCKING: 'blocking', REQUEST_BODY: 'requestBody', EXTRA_HEADERS: 'extraHeaders' }),
    OnBeforeSendHeadersOptions: c({ REQUEST_HEADERS: 'requestHeaders', BLOCKING: 'blocking', EXTRA_HEADERS: 'extraHeaders' }),
    OnSendHeadersOptions: c({ REQUEST_HEADERS: 'requestHeaders', EXTRA_HEADERS: 'extraHeaders' }),
    OnHeadersReceivedOptions: c({ BLOCKING: 'blocking', RESPONSE_HEADERS: 'responseHeaders', EXTRA_HEADERS: 'extraHeaders' }),
    OnAuthRequiredOptions: c({ RESPONSE_HEADERS: 'responseHeaders', BLOCKING: 'blocking', ASYNC_BLOCKING: 'asyncBlocking', EXTRA_HEADERS: 'extraHeaders' }),
    OnResponseStartedOptions: c({ RESPONSE_HEADERS: 'responseHeaders', EXTRA_HEADERS: 'extraHeaders' }),
    OnBeforeRedirectOptions: c({ RESPONSE_HEADERS: 'responseHeaders', EXTRA_HEADERS: 'extraHeaders' }),
    OnCompletedOptions: c({ RESPONSE_HEADERS: 'responseHeaders', EXTRA_HEADERS: 'extraHeaders' }),
    OnErrorOccurredOptions: c({ EXTRA_HEADERS: 'extraHeaders' }),
    ResourceType: c({
      MAIN_FRAME: 'main_frame', SUB_FRAME: 'sub_frame', STYLESHEET: 'stylesheet', SCRIPT: 'script',
      IMAGE: 'image', FONT: 'font', OBJECT: 'object', XMLHTTPREQUEST: 'xmlhttprequest', PING: 'ping',
      CSP_REPORT: 'csp_report', MEDIA: 'media', WEBSOCKET: 'websocket', WEBBUNDLE: 'webbundle', OTHER: 'other',
    }),
    MAX_HANDLER_BEHAVIOR_CHANGED_CALLS_PER_10_MINUTES: 20,
  };
  const behaviorChanged = either(() => undefined);
  patchers.webRequest = (wr) => {
    for (const [key, value] of Object.entries(requestConstants)) if (!(key in wr)) define(wr, key, value);
    if (!wr.handlerBehaviorChanged) define(wr, 'handlerBehaviorChanged', behaviorChanged);
  };

  // The badge's text colour: WebKit draws the badge itself, in its own colours, so the colour
  // asked for is kept (and read back) but not drawn. Whether the button is on the toolbar: it is
  // (Trek shows the first two; the rest are a click away on the Extensions page).
  const colours = new Map();
  const setColour = either((d) => { colours.set((d && d.tabId) ?? -1, d && d.color); });
  const getColour = either((d) => colours.get((d && d.tabId) ?? -1) || [255, 255, 255, 255]);
  const userSettings = either(() => ({ isOnToolbar: true }));
  patchers.action = patchers.browserAction = (action) => {
    if (!action.setBadgeTextColor) define(action, 'setBadgeTextColor', setColour);
    if (!action.getBadgeTextColor) define(action, 'getBadgeTextColor', getColour);
    if (!action.getUserSettings) define(action, 'getUserSettings', userSettings);
  };

  // declarativeNetRequest's rules: WebKit refuses a whole batch for one rule whose regular
  // expression it can't run (its rule engine takes fewer forms than Chrome's RE2). Those rules
  // are left out, said on the console, and the rest are added, as Chrome would add them all.
  const withoutUnrunnable = (original) => {
    const wrapped = function (options, callback) {
      const self = this;
      const run = async () => {
        const opts = { ...(options || {}) };
        const rules = Array.isArray(opts.addRules) ? [...opts.addRules] : null;
        const dropped = [];
        for (;;) {
          try {
            const value = await original.call(self, rules ? { ...opts, addRules: rules } : opts);
            if (dropped.length) console.warn(`Trek: WebKit can't run ${dropped.length} of these rules' regular expressions; left out: ${dropped.join(', ')}`);
            return value;
          } catch (e) {
            const m = /rule at index (\d+)/.exec(String(e && e.message));
            if (!rules || !m || !/regular expression|regexFilter/i.test(e.message) || !rules[+m[1]] || dropped.length > 5000) throw e;
            dropped.push(rules[+m[1]].id);
            rules.splice(+m[1], 1);
          }
        }
      };
      const promise = run();
      if (typeof callback !== 'function') return promise;
      promise.then(() => callback(), (e) => { console.error(e); callback(); });
      return undefined;
    };
    Object.defineProperty(wrapped, '__trek', { value: true });
    return wrapped;
  };
  patchers.declarativeNetRequest = (dnr) => {
    for (const name of ['updateDynamicRules', 'updateSessionRules']) {
      const original = dnr[name];
      if (typeof original === 'function' && !original.__trek) define(dnr, name, withoutUnrunnable(original));
    }
  };
  // A message's sender names its origin in Chrome (`sender.origin`); WebKit's names only its
  // URL. Extensions that check where a message came from (Bitwarden drops a port with no
  // origin) get it, made from the URL: the sender's scheme and host.
  const wrappedListeners = new WeakMap();
  const patchedEvents = new WeakSet();
  const originOf = (url) => {
    const m = /^([a-z][a-z0-9+.-]*:)\/\/([^/?#]*)/i.exec(String(url || ''));
    return m ? `${m[1]}//${m[2]}` : undefined;
  };
  const withOrigin = (sender) => {
    if (sender && typeof sender === 'object' && !sender.origin && sender.url) {
      const origin = originOf(sender.url);
      if (origin) {
        try { Object.defineProperty(sender, 'origin', { value: origin, enumerable: true, configurable: true }); } catch {}
      }
    }
    return sender;
  };
  const senderAt = { onMessage: 1, onMessageExternal: 1, onConnect: -1, onConnectExternal: -1 };
  // WebKit reports the request that loads an iframe's document as `main_frame` (Chrome:
  // `sub_frame`), with a parent frame no other event names. An extension that keeps a tab's
  // state by its main frame (Privacy Badger) takes each as the tab going to a new page. Such a
  // request is a `sub_frame` here, of the tab's top frame when its parent isn't one it knows.
  const knownFrames = new Map();
  const frameFix = (details) => {
    if (!details || typeof details !== 'object' || details.tabId === undefined) return details;
    let frames = knownFrames.get(details.tabId);
    if (!frames) { frames = new Set([0]); knownFrames.set(details.tabId, frames); }
    if (details.type === 'main_frame' && details.frameId !== 0) {
      const fixed = { ...details, type: 'sub_frame' };
      if (!frames.has(details.parentFrameId)) fixed.parentFrameId = 0;
      frames.add(details.frameId);
      return fixed;
    }
    if (details.type === 'main_frame') knownFrames.set(details.tabId, new Set([0]));
    else if (details.frameId !== undefined) frames.add(details.frameId);
    return details;
  };
  const patchEvent = (target, name, fix) => {
    const e = target[name];
    if (!e || typeof e.addListener !== 'function' || patchedEvents.has(e)) return;
    patchedEvents.add(e);
    const add = e.addListener.bind(e);
    const remove = e.removeListener ? e.removeListener.bind(e) : null;
    const has = e.hasListener ? e.hasListener.bind(e) : null;
    const wrap = (f) => {
      if (typeof f !== 'function') return f;
      let w = wrappedListeners.get(f);
      if (!w) {
        w = function (...args) {
          if (fix) args[0] = fix(args[0]);
          else if (senderAt[name] === 1) withOrigin(args[1]);
          else if (args[0] && args[0].sender) withOrigin(args[0].sender);
          return f.apply(this, args);
        };
        wrappedListeners.set(f, w);
      }
      return w;
    };
    try {
      define(e, 'addListener', (f, ...rest) => add(wrap(f), ...rest));
      if (remove) define(e, 'removeListener', (f) => remove(wrappedListeners.get(f) || f));
      if (has) define(e, 'hasListener', (f) => has(wrappedListeners.get(f) || f));
    } catch {}
  };
  patchers.runtime = (runtime) => {
    for (const name of Object.keys(senderAt)) patchEvent(runtime, name);
  };
  const requestEvents = ['onBeforeRequest', 'onBeforeSendHeaders', 'onSendHeaders', 'onHeadersReceived',
    'onAuthRequired', 'onResponseStarted', 'onBeforeRedirect', 'onCompleted', 'onErrorOccurred'];
  const patchWebRequest = patchers.webRequest;
  patchers.webRequest = (wr) => {
    patchWebRequest(wr);
    for (const name of requestEvents) patchEvent(wr, name, frameFix);
  };
  for (const key of Object.keys(patchers)) patch(key, api[key]);

  // chrome.notifications: Trek's own banners (Trek posts them; a click comes back here).
  if (missing('notifications')) {
    const onClicked = listen('notifications.onClicked');
    const onButtonClicked = listen('notifications.onButtonClicked');
    const onClosed = listen('notifications.onClosed');
    let next = 1;
    const plain = (options) => {
      const o = options || {};
      return {
        type: o.type, title: o.title, message: o.message, contextMessage: o.contextMessage,
        buttons: (o.buttons || []).map((b) => ({ title: String((b && b.title) || '') })),
        requireInteraction: !!o.requireInteraction, silent: !!o.silent, priority: o.priority,
        items: (o.items || []).map((i) => ({ title: String(i.title || ''), message: String(i.message || '') })),
        progress: o.progress,
      };
    };
    define(api, 'notifications', {
      create: either((id, options) => {
        if (typeof id === 'object' && id !== null) { options = id; id = undefined; }
        const name = id || `trek-${Date.now()}-${next++}`;
        if (Object.keys(events).some((k) => k.startsWith('notifications.') && events[k].hasListeners())) connect();
        return call('notifications.create', { id: name, options: plain(options) }).then(() => name);
      }),
      update: either((id, options) => call('notifications.update', { id, options: plain(options) })),
      clear: either((id) => call('notifications.clear', { id })),
      getAll: either(() => call('notifications.getAll')),
      getPermissionLevel: either(() => 'granted'),
      onClicked, onButtonClicked, onClosed,
      onShowSettings: event(),
      onPermissionLevelChanged: event(),
      TemplateType: Object.freeze({ BASIC: 'basic', IMAGE: 'image', LIST: 'list', PROGRESS: 'progress' }),
      PermissionLevel: Object.freeze({ GRANTED: 'granted', DENIED: 'denied' }),
    });
  }

  // chrome.offscreen: a hidden page of the extension's own, kept by Trek.
  if (missing('offscreen')) {
    define(api, 'offscreen', {
      createDocument: either((p) => call('offscreen.create', { url: new URL(p.url, location.href).href, reasons: p.reasons || [] })),
      closeDocument: either(() => call('offscreen.close')),
      hasDocument: either(() => call('offscreen.has')),
      Reason: Object.freeze({
        TESTING: 'TESTING', AUDIO_PLAYBACK: 'AUDIO_PLAYBACK', IFRAME_SCRIPTING: 'IFRAME_SCRIPTING',
        DOM_SCRAPING: 'DOM_SCRAPING', BLOBS: 'BLOBS', DOM_PARSER: 'DOM_PARSER', USER_MEDIA: 'USER_MEDIA',
        DISPLAY_MEDIA: 'DISPLAY_MEDIA', WEB_RTC: 'WEB_RTC', CLIPBOARD: 'CLIPBOARD',
        LOCAL_STORAGE: 'LOCAL_STORAGE', WORKERS: 'WORKERS', BATTERY_STATUS: 'BATTERY_STATUS',
        MATCH_MEDIA: 'MATCH_MEDIA', GEOLOCATION: 'GEOLOCATION',
      }),
    });
  }
  if (!api.runtime.getContexts) {
    define(api.runtime, 'getContexts', either((filter) => call('runtime.getContexts', filter || {})));
  }
  if (api.runtime && !api.runtime.ContextType) {
    define(api.runtime, 'ContextType', Object.freeze({
      TAB: 'TAB', POPUP: 'POPUP', BACKGROUND: 'BACKGROUND', OFFSCREEN_DOCUMENT: 'OFFSCREEN_DOCUMENT', SIDE_PANEL: 'SIDE_PANEL', DEVELOPER_TOOLS: 'DEVELOPER_TOOLS',
    }));
  }

  // chrome.identity: a sign-in window Trek opens, which ends at the extension's redirect address.
  if (missing('identity')) {
    const redirect = (path) => `https://${api.runtime.id}.chromiumapp.org/${String(path || '').replace(/^\//, '')}`;
    define(api, 'identity', {
      getRedirectURL: (path) => redirect(path),
      launchWebAuthFlow: either((d) => call('identity.launchWebAuthFlow', { url: d.url, interactive: d.interactive !== false, redirect: redirect('') })),
      getAuthToken: either(() => { throw new Error("Google's sign-in for extensions (identity.getAuthToken) belongs to Chrome and isn't in Trek."); }),
      getProfileUserInfo: either(() => ({ email: '', id: '' })),
      getAccounts: either(() => []),
      removeCachedAuthToken: either(() => undefined),
      clearAllCachedAuthTokens: either(() => undefined),
      onSignInChanged: event(),
    });
  }

  // chrome.idle: the Mac's own idle time and lock.
  if (missing('idle')) {
    const onStateChanged = event();
    let interval = 60;
    let last = 'active';
    let timer = null;
    const watch = () => {
      if (timer) return;
      timer = setInterval(() => {
        if (!onStateChanged.hasListeners()) { clearInterval(timer); timer = null; return; }
        call('idle.query', { seconds: interval }).then((state) => {
          if (state !== last) { last = state; onStateChanged.dispatch(state); }
        }, () => {});
      }, 10000);
    };
    const add = onStateChanged.addListener;
    onStateChanged.addListener = (f) => { add(f); watch(); };
    define(api, 'idle', {
      queryState: either((seconds) => call('idle.query', { seconds: Math.max(15, Number(seconds) || 60) })),
      setDetectionInterval: (seconds) => { interval = Math.max(15, Number(seconds) || 60); },
      getAutoLockDelay: either(() => 0),
      onStateChanged,
      IdleState: Object.freeze({ ACTIVE: 'active', IDLE: 'idle', LOCKED: 'locked' }),
    });
  }

  // chrome.downloads: the file goes to Downloads, as the Browser's own downloads do.
  if (missing('downloads')) {
    const onCreated = event();
    const onChanged = event();
    const items = new Map();
    const b64 = (buffer) => {
      const bytes = new Uint8Array(buffer);
      let s = '';
      for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
      return btoa(s);
    };
    define(api, 'downloads', {
      download: either(async (o) => {
        const response = await fetch(o.url, { method: o.method || 'GET', body: o.body, credentials: 'include' });
        if (!response.ok && response.status) throw new Error(`The download failed (${response.status}).`);
        const data = b64(await response.arrayBuffer());
        const name = o.filename || decodeURIComponent((new URL(o.url, location.href).pathname.split('/').pop() || 'download'));
        const saved = await call('downloads.save', { name, data });
        const item = { id: saved.id, url: o.url, filename: saved.path, state: 'complete', bytesReceived: saved.size, totalBytes: saved.size, fileSize: saved.size, exists: true, mime: response.headers.get('content-type') || '' };
        items.set(item.id, item);
        onCreated.dispatch(item);
        onChanged.dispatch({ id: item.id, state: { previous: 'in_progress', current: 'complete' } });
        return item.id;
      }),
      search: either((q) => [...items.values()].filter((i) => !q || q.id === undefined || q.id === i.id)),
      erase: either((q) => { const ids = [...items.keys()].filter((id) => !q || q.id === undefined || q.id === id); ids.forEach((id) => items.delete(id)); return ids; }),
      open: either((id) => call('downloads.show', { path: (items.get(id) || {}).filename })),
      show: either((id) => call('downloads.show', { path: (items.get(id) || {}).filename })),
      showDefaultFolder: () => { call('downloads.show', { path: '' }); },
      cancel: either(() => undefined), pause: either(() => undefined), resume: either(() => undefined),
      setShelfEnabled: () => {}, setUiOptions: either(() => undefined),
      onCreated, onChanged, onErased: event(), onDeterminingFilename: event(),
    });
  }

  // chrome.privacy: Trek's Browser lets no extension change these settings, and says so.
  if (missing('privacy')) {
    const setting = (value) => ({
      get: either(() => ({ value, levelOfControl: 'not_controllable' })),
      set: either(() => { throw new Error("Trek's Browser doesn't let extensions change this setting."); }),
      clear: either(() => undefined),
      onChange: event(),
    });
    define(api, 'privacy', {
      network: { networkPredictionEnabled: setting(false), webRTCIPHandlingPolicy: setting('default') },
      services: {
        alternateErrorPagesEnabled: setting(false), autofillEnabled: setting(false),
        autofillAddressEnabled: setting(false), autofillCreditCardEnabled: setting(false),
        passwordSavingEnabled: setting(false), safeBrowsingEnabled: setting(false),
        safeBrowsingExtendedReportingEnabled: setting(false), searchSuggestEnabled: setting(false),
        spellingServiceEnabled: setting(false), translationServiceEnabled: setting(false),
      },
      websites: {
        hyperlinkAuditingEnabled: setting(true), referrersEnabled: setting(true),
        thirdPartyCookiesAllowed: setting(false), doNotTrackEnabled: setting(false),
        protectedContentEnabled: setting(true), topicsEnabled: setting(false),
        fledgeEnabled: setting(false), adMeasurementEnabled: setting(false),
      },
      IPHandlingPolicy: Object.freeze({
        DEFAULT: 'default', DEFAULT_PUBLIC_AND_PRIVATE_INTERFACES: 'default_public_and_private_interfaces',
        DEFAULT_PUBLIC_INTERFACE_ONLY: 'default_public_interface_only', DISABLE_NON_PROXIED_UDP: 'disable_non_proxied_udp',
      }),
    });
  }

  // chrome.management: what an extension may learn of itself.
  if (missing('management')) {
    const self = () => {
      const m = api.runtime.getManifest();
      return {
        id: api.runtime.id, name: m.name, shortName: m.short_name || m.name, description: m.description || '',
        version: m.version, enabled: true, mayDisable: true, installType: 'normal', type: 'extension',
        offlineEnabled: !!m.offline_enabled, optionsUrl: m.options_page || (m.options_ui && m.options_ui.page) || '',
        homepageUrl: m.homepage_url || '', hostPermissions: m.host_permissions || [], permissions: m.permissions || [],
      };
    };
    define(api, 'management', {
      getSelf: either(() => self()),
      getAll: either(() => [self()]),
      get: either((id) => { if (id === api.runtime.id) return self(); throw new Error('Trek tells an extension only of itself.'); }),
      onInstalled: event(), onUninstalled: event(), onEnabled: event(), onDisabled: event(),
    });
  }

  // chrome.sidePanel: the panel's page opens in a window of its own.
  if (missing('sidePanel')) {
    let path = (() => { try { const m = api.runtime.getManifest(); return (m.side_panel && m.side_panel.default_path) || ''; } catch { return ''; } })();
    let behavior = { openPanelOnActionClick: false };
    define(api, 'sidePanel', {
      setOptions: either((o) => { if (o && o.path && !o.tabId) path = o.path; }),
      getOptions: either(() => ({ path, enabled: !!path })),
      setPanelBehavior: either((b) => { behavior = { ...behavior, ...(b || {}) }; }),
      getPanelBehavior: either(() => behavior),
      open: either(() => call('sidePanel.open', { url: path ? new URL(path, location.href).href : '' })),
      onOpened: event(), onClosed: event(),
    });
  }

  // chrome.commands: the shortcuts the manifest names (Trek binds none of them).
  if (missing('commands')) {
    define(api, 'commands', {
      getAll: either(() => {
        const c = (api.runtime.getManifest().commands) || {};
        return Object.entries(c).map(([name, v]) => ({ name, description: (v && v.description) || '', shortcut: '' }));
      }),
      onCommand: event(),
    });
  }
  // chrome.devtools, for an extension's DevTools page (React Developer Tools' Components and
  // Profiler): WebKit's inspector takes no extension panels, so Trek opens a page of its own
  // (`trek-devtools.html`, see `compat`) that runs the extension's DevTools page hidden inside it
  // with `?trek_tab=<the inspected tab>`, and shows each panel it creates as a tab of that page.
  // The panel's page is a frame of the same origin, so the DevTools page reaches its window as it
  // does in Chrome (`panel.onShown(window)`). The inspected page is reached through `scripting`.
  const inspected = typeof location !== 'undefined' && new URLSearchParams(location.search).get('trek_tab');
  if (inspected && !api.devtools && typeof document !== 'undefined') {
    const tabId = Number(inspected);
    const host = window.parent !== window ? window.parent : window;
    const dark = typeof matchMedia === 'function' && matchMedia('(prefers-color-scheme: dark)').matches;
    const panels = [];
    const select = (panel) => {
      for (const p of panels) {
        const on = p === panel;
        p.frame.style.display = on ? 'block' : 'none';
        p.button.setAttribute('aria-selected', String(on));
        if (on !== p.shown) {
          p.shown = on;
          const fire = () => (on ? p.api.onShown : p.api.onHidden).dispatch(p.frame.contentWindow);
          if (on && !p.loaded) p.pending = fire; else fire();
        }
      }
    };
    const navigated = event();
    if (api.tabs && api.tabs.onUpdated) {
      api.tabs.onUpdated.addListener((id, change) => {
        if (id === tabId && change.status === 'loading' && change.url) navigated.dispatch(change.url);
      });
    }
    const evaluate = (expression, options, callback) => {
      if (typeof options === 'function') { callback = options; options = {}; }
      const done = typeof callback === 'function' ? callback : () => {};
      api.scripting.executeScript({
        target: { tabId, allFrames: false },
        world: 'MAIN',
        func: (source) => {
          try { return { value: (0, eval)(source) }; } catch (e) { return { error: String(e) }; }
        },
        args: [String(expression)],
      }).then((results) => {
        const r = results && results[0] && results[0].result;
        if (!r) done(undefined, { isError: true, code: 'E_NORESULT', description: 'The page gave no answer.' });
        else if ('error' in r) done(undefined, { isException: true, value: r.error });
        else done(r.value, undefined);
      }, (e) => done(undefined, { isError: true, code: 'E_FAILED', description: String(e && e.message || e) }));
    };
    define(api, 'devtools', {
      inspectedWindow: {
        tabId,
        eval: evaluate,
        reload: (options) => { api.tabs.reload(tabId, { bypassCache: !!(options && options.ignoreCache) }); },
        getResources: (callback) => { if (typeof callback === 'function') callback([]); },
        onResourceAdded: event(),
        onResourceContentCommitted: event(),
      },
      network: {
        onNavigated: navigated,
        onRequestFinished: event(),
        getHAR: (callback) => { if (typeof callback === 'function') callback({ entries: [] }); },
      },
      panels: {
        themeName: dark ? 'dark' : 'default',
        onThemeChanged: event(),
        setThemeChangeHandler: () => {},
        setOpenResourceHandler: () => {},
        openResource: () => {},
        elements: { onSelectionChanged: event() },
        sources: { onSelectionChanged: event() },
        create: (title, icon, page, callback) => {
          const tabs = host.document.getElementById('trek-devtools-tabs');
          const area = host.document.getElementById('trek-devtools-panels');
          const panel = { shown: false, loaded: false, pending: null };
          panel.api = { onShown: event(), onHidden: event(), onSearch: event(), createStatusBarButton: () => ({ onClicked: event(), update: () => {} }), show: () => select(panel) };
          panel.button = host.document.createElement('button');
          panel.button.textContent = title;
          panel.button.addEventListener('click', () => select(panel));
          panel.frame = host.document.createElement('iframe');
          panel.frame.src = new URL(page, location.href).href;
          panel.frame.addEventListener('load', () => {
            panel.loaded = true;
            if (panel.pending) { const f = panel.pending; panel.pending = null; f(); }
          });
          if (tabs && area) { tabs.appendChild(panel.button); area.appendChild(panel.frame); }
          panels.push(panel);
          panel.frame.style.display = 'none';
          if (typeof callback === 'function') setTimeout(() => callback(panel.api), 0);
          // The first panel shows once its creator has listened for that (as a user's first click).
          if (panels.length === 1) setTimeout(() => select(panel), 50);
          return panel.api;
        },
      },
    });
    host.__trekDevtoolsPanels = panels;
  }

  // What the shim couldn't make, in Trek's log (an extension that still stops says why).
  if (failed.length) call('log.shim', { where: typeof document === 'undefined' ? 'background' : location.pathname, failed }).catch(() => {});
})();
