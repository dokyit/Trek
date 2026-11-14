// One connection to Trek's phone server, as a page can make it: a WebTransport session to
// `https://<address>:<port>/trek`, the server's self-signed certificate pinned by its SHA-256
// (`serverCertificateHashes`, the hashes from the pairing link and `web_certs`), and one
// bidirectional stream on it that carries the WebSocket protocol's frames (`frame.js`).

import { FrameReader, OP, closeFrame, encodeFrame, textFrame } from './frame.js';
import { fromBase64 } from './link.js';

/** Whether this browser can reach Trek at all. */
export function supported() {
  return typeof globalThis.WebTransport === 'function';
}

/**
 * Open a wire to `url`, pinning `hashes` (base64 SHA-256s of certificates). Resolves once the
 * stream is open; rejects when the session can't be made in `timeoutMs`.
 */
export async function openWire(url, hashes, { timeoutMs = 9000 } = {}) {
  const transport = new WebTransport(url, {
    serverCertificateHashes: hashes.map((hash) => ({ algorithm: 'sha-256', value: fromBase64(hash) })),
  });
  let timer;
  const timeout = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error('no answer in time')), timeoutMs);
  });
  try {
    await Promise.race([transport.ready, timeout]);
    const stream = await Promise.race([transport.createBidirectionalStream(), timeout]);
    return new Wire(transport, stream);
  } catch (err) {
    try {
      transport.close();
    } catch {
      // Not open yet: nothing to close.
    }
    throw err;
  } finally {
    clearTimeout(timer);
  }
}

export class Wire {
  constructor(transport, stream) {
    this.transport = transport;
    this.writer = stream.writable.getWriter();
    this.reader = stream.readable.getReader();
    this.frames = new FrameReader();
    /** @type {(text: string) => void} */
    this.onmessage = null;
    /** @type {(why: string) => void} */
    this.onclose = null;
    this.closed = false;
    this.readLoop();
    transport.closed.then(
      () => this.finish('the connection closed'),
      (err) => this.finish(describe(err, 'the connection was lost')),
    );
  }

  async readLoop() {
    try {
      for (;;) {
        const { value, done } = await this.reader.read();
        if (done) break;
        for (const message of this.frames.push(value)) {
          if (message.op === OP.TEXT) {
            this.onmessage?.(message.text);
          } else if (message.op === OP.PING) {
            this.write(encodeFrame(OP.PONG, message.data));
          } else if (message.op === OP.CLOSE) {
            this.write(closeFrame(message.code === 1005 ? 1000 : message.code));
            this.finish(message.reason || 'the computer closed the connection');
            return;
          }
        }
      }
      this.finish('the computer ended the connection');
    } catch (err) {
      this.finish(describe(err, 'the connection was lost'));
    }
  }

  write(bytes) {
    if (this.closed) return;
    this.writer.write(bytes).catch((err) => this.finish(describe(err, 'couldn’t send')));
  }

  /** Send one message (a JSON text). */
  send(text) {
    this.write(textFrame(text));
  }

  close(why = 'closed') {
    if (this.closed) return;
    try {
      this.write(closeFrame(1000));
    } catch {
      // Closing anyway.
    }
    this.finish(why);
  }

  finish(why) {
    if (this.closed) return;
    this.closed = true;
    try {
      this.transport.close();
    } catch {
      // Already closed.
    }
    this.onclose?.(why);
  }
}

function describe(err, fallback) {
  if (!err) return fallback;
  return err.message || String(err) || fallback;
}
