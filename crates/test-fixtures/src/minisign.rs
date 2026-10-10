//! Test signing keys in minisign's format, for updater end-to-end runs (`script/update-e2e.ps1`)
//! where the release key isn't at hand: Trek built with `TREK_UPDATE_PUBKEY=<the key line>`
//! trusts releases signed here. The secret half is a plain seed: test use only.
//!
//! A minisign public key is base64 of `Ed` + key id (8 bytes) + Ed25519 key (32). A signature
//! file is `ED` + key id + Ed25519 over BLAKE2b-512 of the file (minisign's prehashed form, the
//! one `verify_stream` reads), then the trusted comment and a global signature over the first
//! signature followed by the comment.

use base64::Engine as _;
use ring::rand::SecureRandom as _;
use ring::signature::{Ed25519KeyPair, KeyPair as _};
use std::path::Path;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// Write `<dir>/test.pub` (minisign) and `<dir>/test.seed` (key id and seed, hex); returns the
/// public key line.
pub fn keygen(dir: &Path) -> std::io::Result<String> {
    let mut secret = [0u8; 40];
    ring::rand::SystemRandom::new().fill(&mut secret).map_err(|_| std::io::Error::other("no randomness"))?;
    let (id, seed) = secret.split_at(8);
    let pair = Ed25519KeyPair::from_seed_unchecked(seed).map_err(|_| std::io::Error::other("bad seed"))?;
    let line = B64.encode([b"Ed".as_slice(), id, pair.public_key().as_ref()].concat());
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("test.pub"), format!("untrusted comment: Trek update test key\n{line}\n"))?;
    std::fs::write(dir.join("test.seed"), format!("{}\n", hex(&secret)))?;
    Ok(line)
}

/// Sign `file` with the key in `seed_file` (from `keygen`) into `<file>.minisig`; returns the
/// signature file's text.
pub fn sign(seed_file: &Path, file: &Path, trusted_comment: &str) -> std::io::Result<String> {
    let text = std::fs::read_to_string(seed_file)?;
    let secret = unhex(text.trim()).filter(|s| s.len() == 40).ok_or_else(|| std::io::Error::other("not a key from minisign-keygen"))?;
    let (id, seed) = secret.split_at(8);
    let pair = Ed25519KeyPair::from_seed_unchecked(seed).map_err(|_| std::io::Error::other("bad seed"))?;
    let mut hash = Blake2b::new();
    hash.update(&std::fs::read(file)?);
    let signature = pair.sign(&hash.finalize());
    let global = pair.sign(&[signature.as_ref(), trusted_comment.as_bytes()].concat());
    let out = format!(
        "untrusted comment: signature from Trek's update test key\n{}\ntrusted comment: {trusted_comment}\n{}\n",
        B64.encode([b"ED".as_slice(), id, signature.as_ref()].concat()),
        B64.encode(global.as_ref())
    );
    let mut name = file.as_os_str().to_owned();
    name.push(".minisig");
    std::fs::write(name, &out)?;
    Ok(out)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    (s.len() % 2 == 0).then_some(())?;
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

/// BLAKE2b with a 64-byte digest and no key (RFC 7693).
pub struct Blake2b {
    h: [u64; 8],
    t: u128,
    buf: [u8; 128],
    len: usize,
}

const IV: [u64; 8] = [
    0x6a09e667f3bcc908,
    0xbb67ae8584caa73b,
    0x3c6ef372fe94f82b,
    0xa54ff53a5f1d36f1,
    0x510e527fade682d1,
    0x9b05688c2b3e6c1f,
    0x1f83d9abfb41bd6b,
    0x5be0cd19137e2179,
];

const SIGMA: [[usize; 16]; 10] = [
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
    [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
    [11, 8, 12, 0, 5, 2, 15, 13, 10, 14, 3, 6, 7, 1, 9, 4],
    [7, 9, 3, 1, 13, 12, 11, 14, 2, 6, 5, 10, 4, 0, 15, 8],
    [9, 0, 5, 7, 2, 4, 10, 15, 14, 1, 11, 12, 6, 8, 3, 13],
    [2, 12, 6, 10, 0, 11, 8, 3, 4, 13, 7, 5, 15, 14, 1, 9],
    [12, 5, 1, 15, 14, 13, 4, 10, 0, 7, 6, 3, 9, 2, 8, 11],
    [13, 11, 7, 14, 12, 1, 3, 9, 5, 0, 15, 4, 8, 6, 2, 10],
    [6, 15, 14, 9, 11, 3, 0, 8, 12, 2, 13, 7, 1, 4, 10, 5],
    [10, 2, 8, 4, 7, 6, 1, 5, 15, 11, 9, 14, 3, 12, 13, 0],
];

impl Default for Blake2b {
    fn default() -> Self {
        Self::new()
    }
}

impl Blake2b {
    pub fn new() -> Self {
        let mut h = IV;
        // Parameter block: digest length 64, no key, fanout and depth 1.
        h[0] ^= 0x0101_0000 ^ 64;
        Blake2b { h, t: 0, buf: [0; 128], len: 0 }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        while !data.is_empty() {
            // The last block is compressed in `finalize`, flagged as last: only a full buffer
            // with more to come is compressed here.
            if self.len == 128 {
                self.t += 128;
                let block = self.buf;
                self.compress(&block, false);
                self.len = 0;
            }
            let take = (128 - self.len).min(data.len());
            self.buf[self.len..self.len + take].copy_from_slice(&data[..take]);
            self.len += take;
            data = &data[take..];
        }
    }

    pub fn finalize(mut self) -> [u8; 64] {
        self.t += self.len as u128;
        self.buf[self.len..].fill(0);
        let block = self.buf;
        self.compress(&block, true);
        let mut out = [0u8; 64];
        for (chunk, word) in out.chunks_mut(8).zip(self.h) {
            chunk.copy_from_slice(&word.to_le_bytes());
        }
        out
    }

    fn compress(&mut self, block: &[u8; 128], last: bool) {
        let m: [u64; 16] = std::array::from_fn(|i| u64::from_le_bytes(block[i * 8..i * 8 + 8].try_into().unwrap()));
        let mut v = [0u64; 16];
        v[..8].copy_from_slice(&self.h);
        v[8..].copy_from_slice(&IV);
        v[12] ^= self.t as u64;
        v[13] ^= (self.t >> 64) as u64;
        if last {
            v[14] = !v[14];
        }
        let g = |v: &mut [u64; 16], a: usize, b: usize, c: usize, d: usize, x: u64, y: u64| {
            v[a] = v[a].wrapping_add(v[b]).wrapping_add(x);
            v[d] = (v[d] ^ v[a]).rotate_right(32);
            v[c] = v[c].wrapping_add(v[d]);
            v[b] = (v[b] ^ v[c]).rotate_right(24);
            v[a] = v[a].wrapping_add(v[b]).wrapping_add(y);
            v[d] = (v[d] ^ v[a]).rotate_right(16);
            v[c] = v[c].wrapping_add(v[d]);
            v[b] = (v[b] ^ v[c]).rotate_right(63);
        };
        for round in 0..12 {
            let s = &SIGMA[round % 10];
            g(&mut v, 0, 4, 8, 12, m[s[0]], m[s[1]]);
            g(&mut v, 1, 5, 9, 13, m[s[2]], m[s[3]]);
            g(&mut v, 2, 6, 10, 14, m[s[4]], m[s[5]]);
            g(&mut v, 3, 7, 11, 15, m[s[6]], m[s[7]]);
            g(&mut v, 0, 5, 10, 15, m[s[8]], m[s[9]]);
            g(&mut v, 1, 6, 11, 12, m[s[10]], m[s[11]]);
            g(&mut v, 2, 7, 8, 13, m[s[12]], m[s[13]]);
            g(&mut v, 3, 4, 9, 14, m[s[14]], m[s[15]]);
        }
        for i in 0..8 {
            self.h[i] ^= v[i] ^ v[i + 8];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(data: &[u8]) -> String {
        let mut h = Blake2b::new();
        h.update(data);
        hex(&h.finalize())
    }

    #[test]
    fn blake2b_matches_the_reference_vectors() {
        assert_eq!(digest(b""), "786a02f742015903c6c6fd852552d272912f4740e15847618a86e217f71f5419d25e1031afee585313896444934eb04b903a685b1448b755d56f701afe9be2ce");
        assert_eq!(digest(b"abc"), "ba80a53f981c4d0d6a2797b69f12f6e94c212f14685ac4b74b12bb6fdbffa2d17d87c5392aab792dc252d5de4533cc9518d38aa8dbf1925ab92386edd4009923");
        // Across block boundaries, fed whole or in pieces.
        let long: Vec<u8> = (0..1000u32).map(|i| i as u8).collect();
        let mut pieces = Blake2b::new();
        for chunk in long.chunks(37) {
            pieces.update(chunk);
        }
        assert_eq!(hex(&pieces.finalize()), digest(&long));
        assert_ne!(digest(&long[..128]), digest(&long[..129]));
    }

    #[test]
    fn signatures_verify_as_trek_verifies_them() {
        let dir = std::env::temp_dir().join(format!("trek-fixture-minisign-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let line = keygen(&dir).unwrap();
        let file = dir.join("Trek-0.4.1.zip");
        std::fs::write(&file, vec![7u8; 300]).unwrap();
        let sig = sign(&dir.join("test.seed"), &file, "Trek 0.4.1 windows-x86_64").unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("Trek-0.4.1.zip.minisig")).unwrap(), sig);

        let key = minisign_verify::PublicKey::from_base64(&line).unwrap();
        let signature = minisign_verify::Signature::decode(&sig).unwrap();
        assert_eq!(signature.trusted_comment(), "Trek 0.4.1 windows-x86_64");
        // Streamed, as Trek's download checks it.
        let mut stream = key.verify_stream(&signature).unwrap();
        stream.update(&[7u8; 100]);
        stream.update(&[7u8; 200]);
        stream.finalize().unwrap();
        let mut tampered = key.verify_stream(&signature).unwrap();
        tampered.update(&[8u8; 300]);
        assert!(tampered.finalize().is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
