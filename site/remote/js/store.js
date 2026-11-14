// What the web remote keeps on the phone: the machines it paired with (each one's name, addresses,
// token and certificate hashes) and this browser's device id. In IndexedDB, this page's alone;
// where IndexedDB can't be opened (a private window that refuses it), in memory for the visit.

const DB = 'trek-remote';
const VERSION = 1;

let opening = null;
const memory = { machines: new Map(), meta: new Map() };

function open() {
  if (!opening) {
    opening = new Promise((resolve) => {
      let request;
      try {
        request = indexedDB.open(DB, VERSION);
      } catch {
        resolve(null);
        return;
      }
      request.onupgradeneeded = () => {
        const db = request.result;
        if (!db.objectStoreNames.contains('machines')) db.createObjectStore('machines', { keyPath: 'hostId' });
        if (!db.objectStoreNames.contains('meta')) db.createObjectStore('meta');
      };
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => resolve(null);
      request.onblocked = () => resolve(null);
    });
  }
  return opening;
}

function done(request) {
  return new Promise((resolve, reject) => {
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

async function tx(store, mode, work) {
  const db = await open();
  if (!db) return work(null);
  return work(db.transaction(store, mode).objectStore(store));
}

export async function machines() {
  return tx('machines', 'readonly', async (s) => {
    const all = s ? await done(s.getAll()) : [...memory.machines.values()];
    return all.sort((a, b) => (b.lastConnectedAt || b.pairedAt || 0) - (a.lastConnectedAt || a.pairedAt || 0));
  });
}

export async function machine(hostId) {
  return tx('machines', 'readonly', async (s) => (s ? (await done(s.get(hostId))) || null : memory.machines.get(hostId) || null));
}

export async function saveMachine(record) {
  return tx('machines', 'readwrite', async (s) => {
    if (s) await done(s.put(record));
    else memory.machines.set(record.hostId, record);
    return record;
  });
}

export async function forgetMachine(hostId) {
  return tx('machines', 'readwrite', async (s) => {
    if (s) await done(s.delete(hostId));
    else memory.machines.delete(hostId);
  });
}

export async function meta(key) {
  return tx('meta', 'readonly', async (s) => (s ? done(s.get(key)) : memory.meta.get(key)));
}

export async function setMeta(key, value) {
  return tx('meta', 'readwrite', async (s) => {
    if (s) await done(s.put(value, key));
    else memory.meta.set(key, value);
  });
}

/** This browser's lasting device id, made the first time. */
export async function deviceId() {
  let id = await meta('device-id');
  if (!id) {
    id = globalThis.crypto.randomUUID ? globalThis.crypto.randomUUID() : fallbackId();
    await setMeta('device-id', id);
  }
  return id;
}

function fallbackId() {
  const bytes = new Uint8Array(16);
  globalThis.crypto.getRandomValues(bytes);
  return [...bytes].map((b) => b.toString(16).padStart(2, '0')).join('');
}

/** Ask the browser to keep what's stored here through storage pressure (granted to installed apps). */
export async function keep() {
  try {
    if (navigator.storage?.persist && !(await navigator.storage.persisted())) await navigator.storage.persist();
  } catch {
    // Not offered: kept as long as the browser keeps it.
  }
}

/** What the phone is called on the computer's list of paired devices. */
export async function deviceName() {
  let model = '';
  try {
    const data = await navigator.userAgentData?.getHighEntropyValues?.(['model', 'platform']);
    model = (data?.model || '').trim();
  } catch {
    // Not offered.
  }
  const ua = navigator.userAgent || '';
  if (!model) {
    const android = ua.match(/Android [\d.]+; ([^;)]+?)(?: Build|\))/);
    if (android && android[1] !== 'K') model = android[1].trim();
  }
  const browser = /EdgA?\//.test(ua) ? 'Edge' : /Firefox\//.test(ua) ? 'Firefox' : /SamsungBrowser\//.test(ua) ? 'Samsung Internet' : /Chrome\//.test(ua) ? 'Chrome' : /Safari\//.test(ua) ? 'Safari' : 'a browser';
  const os = /Android/.test(ua) ? 'Android' : /iPhone|iPad/.test(ua) ? 'iPhone' : /Windows/.test(ua) ? 'Windows' : /Mac OS/.test(ua) ? 'Mac' : /Linux/.test(ua) ? 'Linux' : '';
  if (model) return `${model} (${browser})`;
  if (os === 'Android' || os === 'iPhone') return `${os} phone (${browser})`;
  return os ? `${browser} on ${os}` : `Web remote (${browser})`;
}
