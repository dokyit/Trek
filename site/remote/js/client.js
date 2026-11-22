// The web remote's connection to one paired machine: it dials every address the machine answers
// on together and keeps the first that opens (as the iPhone app does), says hello with its token
// (or pairs with the code from a pairing link), and from then on sends requests, matches replies
// to them by id, and hands everything else the machine says to `onMessage`. It comes back by
// itself when the connection drops, sooner when the page is looked at again or the network
// returns.

import { openWire, supported } from './wire.js';
import { liveHashes, mayReplace, sessionUrl } from './link.js';
import * as store from './store.js';

export const PROTOCOL = 1;
export const APP_VERSION = 'web-1';

/** How long a request waits for its answer. */
const ANSWER_IN = 30_000;
/** Between pings while the page shows: a connection that stopped answering is found this soon. */
const PING_EVERY = 25_000;
const PONG_IN = 10_000;
const BACKOFF = [1000, 2000, 4000, 8000, 15000, 30000];

export class ClientError extends Error {
  constructor(code, message) {
    super(message || code);
    this.code = code;
  }
}

/**
 * The state of the connection, as the page shows it:
 * - `connecting`: dialing.
 * - `online`: said hello and was welcomed.
 * - `offline`: no address answered (asleep, another network, Trek closed); it tries again.
 * - `permission`: the browser needs a tap before it may look on the local network (Chrome's Local
 *   Network Access); `connect({ gesture: true })` from a tap.
 * - `unauthorized`: the machine doesn't know this phone any more (unpaired there): pair again.
 * - `pairing_failed`: the code was refused. `pairing_conflict`: the computer that answered says
 *   it's one this phone keeps already, without its certificates (`mayReplace`): not kept.
 * - `unsupported`: this browser has no WebTransport.
 */
export class Client {
  /**
   * @param {object} machine the stored machine (`store.saveMachine`), or a pairing in progress:
   *   `{ hostId, name, routes, certs, token? , pairing?: { code } }`
   * @param {{ onMessage: (msg) => void, onStatus: (status, detail) => void, onMachine: (machine) => void }} hooks
   */
  constructor(machine, hooks) {
    this.machine = machine;
    this.hooks = hooks;
    this.status = 'idle';
    this.detail = '';
    this.wire = null;
    this.next = 1;
    /** id → { resolve, reject, timer } */
    this.waiting = new Map();
    this.attempt = 0;
    this.retryTimer = null;
    this.pingTimer = null;
    this.stopped = false;
    this.host = null;
    this.features = new Set();
    this.onVisible = () => {
      if (document.visibilityState !== 'visible' || this.stopped) return;
      if (this.status === 'online') this.check();
      else if (this.status === 'offline') this.connect();
    };
    this.onOnline = () => {
      if (!this.stopped && this.status !== 'online' && this.status !== 'connecting') this.connect();
    };
    if (typeof document !== 'undefined') document.addEventListener('visibilitychange', this.onVisible);
    if (typeof window !== 'undefined') window.addEventListener('online', this.onOnline);
  }

  setStatus(status, detail = '') {
    this.status = status;
    this.detail = detail;
    this.hooks.onStatus?.(status, detail);
  }

  /** Connect now (again). `gesture`: called from a tap, so the browser may ask for permission. */
  async connect({ gesture = false } = {}) {
    if (this.stopped) return;
    if (!supported()) {
      this.setStatus('unsupported');
      return;
    }
    if (this.status === 'connecting') return;
    clearTimeout(this.retryTimer);
    if (!gesture && (await localNetworkPermission()) === 'prompt') {
      this.setStatus('permission');
      return;
    }
    this.setStatus('connecting');
    let wire;
    try {
      wire = await this.dial();
    } catch (err) {
      if (this.stopped) return;
      const denied = (await localNetworkPermission()) === 'denied';
      if (denied) {
        this.setStatus('permission', 'denied');
        return;
      }
      this.setStatus('offline', err.message);
      this.retry();
      return;
    }
    if (this.stopped) {
      wire.close();
      return;
    }
    this.wire = wire;
    wire.onmessage = (text) => this.heard(text);
    wire.onclose = (why) => this.lost(wire, why);
    try {
      await this.greet();
    } catch (err) {
      wire.onclose = null;
      wire.close();
      if (this.wire === wire) this.wire = null;
      if (err.code === 'unauthorized') {
        this.setStatus('unauthorized', err.message);
      } else if (err.code === 'pairing_failed' || err.code === 'pairing_conflict') {
        this.setStatus(err.code, err.message);
      } else if (err.code === 'unsupported_protocol') {
        this.setStatus('unsupported_protocol', err.message);
      } else {
        this.setStatus('offline', err.message);
        this.retry();
      }
      return;
    }
    this.attempt = 0;
    this.setStatus('online');
    this.keepChecking();
  }

  /** Every address at once; the first wire that opens is kept and the rest let go. */
  async dial() {
    const hashes = liveHashes(this.machine.certs || []).map((c) => c.hash);
    if (hashes.length === 0) throw new Error('no certificate to trust: pair again');
    const routes = this.machine.routes || [];
    const urls = routes.map((r) => sessionUrl(r)).filter(Boolean);
    if (urls.length === 0) throw new Error('no address for this computer');
    let won = null;
    const tries = urls.map((url) =>
      openWire(url, hashes).then((wire) => {
        if (won) {
          wire.close();
          return wire;
        }
        won = wire;
        this.route = routes[urls.indexOf(url)];
        return wire;
      }),
    );
    try {
      return await Promise.any(tries);
    } catch (all) {
      const reasons = (all.errors || []).map((e) => e.message).filter(Boolean);
      throw new Error(reasons[0] || 'no address answered');
    }
  }

  /** `hello` with the token, or `pair` with the code; then the machine as it now is. */
  async greet() {
    const m = this.machine;
    const deviceId = await store.deviceId();
    let reply;
    if (m.pairing) {
      reply = await this.ask({
        type: 'pair',
        protocol: PROTOCOL,
        code: m.pairing.code,
        device_id: deviceId,
        device_name: await store.deviceName(),
        app_version: APP_VERSION,
        platform: 'web',
      });
      if (reply.type !== 'paired') throw new ClientError('bad_reply', `the computer answered ${reply.type}`);
      // A computer kept here already under the id this one gives keeps its place unless this is
      // it (`mayReplace`): a link can name any computer's id.
      const id = reply.host?.id || m.hostId;
      const kept = id ? await store.machine(id) : null;
      if (kept && !mayReplace(kept, m.certs)) {
        throw new ClientError('pairing_conflict', `This phone is paired with ${kept.name} already, and the computer that answered isn’t it. If it’s the same computer, forget it under Machines, then pair again.`);
      }
      m.token = reply.token;
      m.pairedAt = Date.now();
      delete m.pairing;
      store.keep();
    } else {
      reply = await this.ask({
        type: 'hello',
        protocol: PROTOCOL,
        device_id: deviceId,
        token: m.token,
        app_version: APP_VERSION,
        platform: 'web',
      });
      if (reply.type !== 'welcome') throw new ClientError('bad_reply', `the computer answered ${reply.type}`);
    }
    this.host = reply.host || null;
    this.features = new Set(reply.features || []);
    if (reply.host?.id) m.hostId = reply.host.id;
    if (reply.host?.name) m.name = reply.host.name;
    if (reply.host?.version) m.version = reply.host.version;
    if (Array.isArray(reply.routes) && reply.routes.length) {
      // Keep any address that answered before but isn't in the new list last: a machine on
      // another network now may still be reached where it was.
      const routes = [...reply.routes];
      for (const r of m.routes || []) if (!routes.includes(r)) routes.push(r);
      m.routes = routes.slice(0, 8);
    }
    if (Array.isArray(reply.web_certs) && reply.web_certs.length) m.certs = reply.web_certs;
    m.features = [...this.features];
    m.lastConnectedAt = Date.now();
    await store.saveMachine(m);
    this.hooks.onMachine?.(m);
  }

  heard(text) {
    let msg;
    try {
      msg = JSON.parse(text);
    } catch {
      return;
    }
    if (!msg || typeof msg !== 'object') return;
    if (msg.re != null && this.waiting.has(msg.re)) {
      const waiter = this.waiting.get(msg.re);
      this.waiting.delete(msg.re);
      clearTimeout(waiter.timer);
      if (msg.type === 'error') waiter.reject(new ClientError(msg.code, msg.message));
      else waiter.resolve(msg);
    }
    if (msg.type === 'pong') return;
    this.hooks.onMessage?.(msg);
  }

  /** Send a request and wait for its answer (`ack`, or the reply of its own); an `error` rejects. */
  ask(msg, { timeout = ANSWER_IN } = {}) {
    const wire = this.wire;
    if (!wire || wire.closed) return Promise.reject(new ClientError('offline', 'Not connected'));
    const id = String(this.next++);
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.waiting.delete(id);
        reject(new ClientError('timeout', 'The computer didn’t answer in time'));
      }, timeout);
      this.waiting.set(id, { resolve, reject, timer });
      wire.send(JSON.stringify({ ...msg, id }));
    });
  }

  /** Send a message no answer is waited for (`mark_seen`, `unsubscribe`). */
  tell(msg) {
    if (this.wire && !this.wire.closed) this.wire.send(JSON.stringify(msg));
  }

  /** A ping now: a connection that doesn't answer is dropped and made again. */
  async check() {
    try {
      await this.ask({ type: 'ping' }, { timeout: PONG_IN });
    } catch (err) {
      if (err.code === 'timeout' && this.wire) this.wire.close('no answer to a ping');
    }
  }

  keepChecking() {
    clearInterval(this.pingTimer);
    this.pingTimer = setInterval(() => {
      if (this.status === 'online' && (typeof document === 'undefined' || document.visibilityState === 'visible')) this.check();
    }, PING_EVERY);
  }

  lost(wire, why) {
    if (this.wire !== wire) return;
    this.wire = null;
    clearInterval(this.pingTimer);
    for (const [, waiter] of this.waiting) {
      clearTimeout(waiter.timer);
      waiter.reject(new ClientError('offline', 'The connection was lost'));
    }
    this.waiting.clear();
    if (this.stopped) return;
    this.setStatus('offline', why);
    this.retry();
  }

  retry() {
    clearTimeout(this.retryTimer);
    const wait = BACKOFF[Math.min(this.attempt, BACKOFF.length - 1)];
    this.attempt += 1;
    this.retryTimer = setTimeout(() => {
      if (typeof document !== 'undefined' && document.visibilityState === 'hidden') return;
      this.connect();
    }, wait);
  }

  stop() {
    this.stopped = true;
    clearTimeout(this.retryTimer);
    clearInterval(this.pingTimer);
    if (typeof document !== 'undefined') document.removeEventListener('visibilitychange', this.onVisible);
    if (typeof window !== 'undefined') window.removeEventListener('online', this.onOnline);
    if (this.wire) {
      this.wire.onclose = null;
      this.wire.close('stopped');
      this.wire = null;
    }
  }
}

/**
 * What the browser says of looking on the local network for this page (Chrome's Local Network
 * Access): `granted`, `prompt` (it will ask, from a tap), `denied`, or `granted` where it isn't
 * asked about at all.
 */
export async function localNetworkPermission() {
  if (!navigator.permissions?.query) return 'granted';
  for (const name of ['local-network', 'local-network-access']) {
    try {
      const status = await navigator.permissions.query({ name });
      return status.state;
    } catch {
      // Not a permission this browser knows.
    }
  }
  return 'granted';
}
