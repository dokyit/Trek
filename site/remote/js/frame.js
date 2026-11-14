// The WebSocket protocol's framing (RFC 6455 §5), as a client speaks it, on the one WebTransport
// stream the page opens to the phone server. The server reads that stream as it reads the iPhone's
// WebSocket (`trek_remote::web`): frames the page sends are masked, the server's aren't, a message
// may come in pieces (continuation frames), and pings, pongs and a close may come between them.
//
// Plain code over bytes, with no browser API: `test/frame.test.mjs` runs it in Node.

export const OP = { CONTINUATION: 0, TEXT: 1, BINARY: 2, CLOSE: 8, PING: 9, PONG: 10 };

const encoder = new TextEncoder();

/** Four random bytes, the mask a client puts on every frame. */
export function randomMask() {
  const mask = new Uint8Array(4);
  globalThis.crypto.getRandomValues(mask);
  return mask;
}

/**
 * One whole frame (FIN set), masked as a client's must be.
 * @param {number} opcode
 * @param {Uint8Array} payload
 * @param {Uint8Array} [mask]
 */
export function encodeFrame(opcode, payload = new Uint8Array(0), mask = randomMask()) {
  const length = payload.length;
  const head = length < 126 ? 2 : length < 65536 ? 4 : 10;
  const out = new Uint8Array(head + 4 + length);
  out[0] = 0x80 | (opcode & 0x0f);
  if (length < 126) {
    out[1] = 0x80 | length;
  } else if (length < 65536) {
    out[1] = 0x80 | 126;
    out[2] = length >>> 8;
    out[3] = length & 0xff;
  } else {
    out[1] = 0x80 | 127;
    const view = new DataView(out.buffer);
    view.setUint32(2, Math.floor(length / 2 ** 32));
    view.setUint32(6, length >>> 0);
  }
  out.set(mask, head);
  const body = head + 4;
  for (let i = 0; i < length; i++) out[body + i] = payload[i] ^ mask[i & 3];
  return out;
}

/** A text message as one frame. */
export function textFrame(text, mask) {
  return encodeFrame(OP.TEXT, encoder.encode(text), mask);
}

/** A close frame: the code, and a reason of at most 123 bytes. */
export function closeFrame(code = 1000, reason = '', mask) {
  const said = encoder.encode(reason).slice(0, 123);
  const payload = new Uint8Array(2 + said.length);
  payload[0] = code >>> 8;
  payload[1] = code & 0xff;
  payload.set(said, 2);
  return encodeFrame(OP.CLOSE, payload, mask);
}

export class FrameError extends Error {}

/**
 * Reads the server's frames from the bytes as they arrive, in any pieces, and hands back whole
 * messages: `{ op: OP.TEXT, text }`, `{ op: OP.BINARY, data }`, `{ op: OP.PING, data }`,
 * `{ op: OP.PONG, data }` and `{ op: OP.CLOSE, code, reason }`.
 */
export class FrameReader {
  /** @param {number} maxMessage the largest message taken, in bytes (the server's own limit is 64 MiB). */
  constructor(maxMessage = 64 * 1024 * 1024) {
    this.maxMessage = maxMessage;
    this.buffer = new Uint8Array(64 * 1024);
    this.start = 0;
    this.end = 0;
    /** The pieces of a message that came in more than one frame. */
    this.pieces = null;
    this.piecesOp = 0;
    this.piecesLength = 0;
    this.decoder = new TextDecoder('utf-8');
  }

  /** Take `chunk` and return the messages it completed, in order. */
  push(chunk) {
    this.append(chunk);
    const out = [];
    for (;;) {
      const message = this.next();
      if (message === undefined) break;
      if (message !== null) out.push(message);
    }
    return out;
  }

  append(chunk) {
    const need = this.end - this.start + chunk.length;
    if (this.end + chunk.length > this.buffer.length) {
      if (need > this.buffer.length) {
        const grown = new Uint8Array(Math.max(need, this.buffer.length * 2));
        grown.set(this.buffer.subarray(this.start, this.end));
        this.buffer = grown;
      } else {
        this.buffer.copyWithin(0, this.start, this.end);
      }
      this.end -= this.start;
      this.start = 0;
    }
    this.buffer.set(chunk, this.end);
    this.end += chunk.length;
  }

  /** The next whole message; `null` for a frame that completed none; `undefined` when more bytes are needed. */
  next() {
    const have = this.end - this.start;
    if (have < 2) return undefined;
    const b = this.buffer;
    const at = this.start;
    const fin = (b[at] & 0x80) !== 0;
    if (b[at] & 0x70) throw new FrameError('a frame with reserved bits set');
    const op = b[at] & 0x0f;
    const masked = (b[at + 1] & 0x80) !== 0;
    let length = b[at + 1] & 0x7f;
    let head = 2;
    if (length === 126) {
      if (have < 4) return undefined;
      length = (b[at + 2] << 8) | b[at + 3];
      head = 4;
    } else if (length === 127) {
      if (have < 10) return undefined;
      const view = new DataView(b.buffer, b.byteOffset + at + 2, 8);
      length = view.getUint32(0) * 2 ** 32 + view.getUint32(4);
      head = 10;
    }
    if (masked) head += 4;
    if (length > this.maxMessage) throw new FrameError(`a frame of ${length} bytes`);
    if (have < head + length) return undefined;
    let payload = b.slice(at + head, at + head + length);
    if (masked) {
      const mask = b.subarray(at + head - 4, at + head);
      for (let i = 0; i < payload.length; i++) payload[i] ^= mask[i & 3];
    }
    this.start += head + length;
    if (this.start === this.end) this.start = this.end = 0;

    if (op >= 8) {
      if (!fin || length > 125) throw new FrameError('a control frame in pieces or over 125 bytes');
      if (op === OP.CLOSE) {
        const code = payload.length >= 2 ? (payload[0] << 8) | payload[1] : 1005;
        const reason = payload.length > 2 ? this.decoder.decode(payload.subarray(2)) : '';
        return { op, code, reason };
      }
      if (op === OP.PING || op === OP.PONG) return { op, data: payload };
      throw new FrameError(`control opcode ${op}`);
    }
    if (op === OP.CONTINUATION) {
      if (!this.pieces) throw new FrameError('a continuation with no message to continue');
    } else if (op === OP.TEXT || op === OP.BINARY) {
      if (this.pieces) throw new FrameError('a new message before the last one ended');
      if (fin) return this.message(op, payload);
      this.pieces = [];
      this.piecesOp = op;
      this.piecesLength = 0;
    } else {
      throw new FrameError(`opcode ${op}`);
    }
    this.pieces.push(payload);
    this.piecesLength += payload.length;
    if (this.piecesLength > this.maxMessage) throw new FrameError(`a message over ${this.maxMessage} bytes`);
    if (!fin) return null;
    const whole = new Uint8Array(this.piecesLength);
    let offset = 0;
    for (const piece of this.pieces) {
      whole.set(piece, offset);
      offset += piece.length;
    }
    const opcode = this.piecesOp;
    this.pieces = null;
    return this.message(opcode, whole);
  }

  message(op, payload) {
    if (op === OP.TEXT) return { op, text: this.decoder.decode(payload) };
    return { op, data: payload };
  }
}
