//! A CRX3 package's signatures, checked as Chrome checks them before it installs an update
//! (`components/crx_file/crx_verifier.cc`): the header (a `CrxFileHeader` protocol buffer) holds
//! key proofs (a public key and its signature, RSA or ECDSA with SHA-256) and the signed data,
//! whose `crx_id` is the extension's id; each proof signs `"CRX3 SignedData\0"`, the signed data's
//! length (4 bytes, little-endian), the signed data, then the zip archive. A package passes only
//! when every proof's signature is good, one proof's key is the one the id is made from (the
//! developer's), and, from the Chrome Web Store, one is the store's own publisher key.

use sha2::Digest as _;

/// The SHA-256 of the Chrome Web Store's publisher key (Chromium's `kPublisherKeyHash`), which
/// signs every package the store serves.
pub const STORE_PUBLISHER_KEY_HASH: [u8; 32] = [
    0x61, 0xf7, 0xf2, 0xa6, 0xbf, 0xcf, 0x74, 0xcd, 0x0b, 0xc1, 0xfe, 0x24, 0x97, 0xcc, 0x9b, 0x04,
    0x25, 0x4c, 0x65, 0x8f, 0x79, 0xf2, 0x14, 0x53, 0x92, 0x86, 0x7e, 0xa8, 0x36, 0x63, 0x67, 0xcf,
];

const SIGNED_PREFIX: &[u8] = b"CRX3 SignedData\x00";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Rsa,
    Ecdsa,
}

struct Proof<'a> {
    kind: Kind,
    key: &'a [u8],
    signature: &'a [u8],
}

fn varint(buf: &[u8], at: &mut usize) -> Result<u64, String> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let b = *buf.get(*at).ok_or("The package's header is cut short.")?;
        *at += 1;
        value |= u64::from(b & 0x7f) << shift;
        if b < 0x80 {
            return Ok(value);
        }
    }
    Err("The package's header is damaged.".into())
}

/// A protocol buffer's fields: (number, bytes) for length-delimited ones (others are skipped).
fn fields(buf: &[u8]) -> Result<Vec<(u64, &[u8])>, String> {
    let mut out = vec![];
    let mut at = 0;
    while at < buf.len() {
        let key = varint(buf, &mut at)?;
        match key & 7 {
            0 => {
                varint(buf, &mut at)?;
            }
            1 => at += 8,
            5 => at += 4,
            2 => {
                let len = varint(buf, &mut at)? as usize;
                let end = at.checked_add(len).filter(|e| *e <= buf.len());
                let end = end.ok_or("The package's header is damaged.")?;
                out.push((key >> 3, &buf[at..end]));
                at = end;
            }
            _ => return Err("The package's header is damaged.".into()),
        }
    }
    if at > buf.len() {
        return Err("The package's header is cut short.".into());
    }
    Ok(out)
}

/// The id a key makes: its SHA-256's first 16 bytes, each nibble a letter `a`..`p`.
pub fn id_of_key(key: &[u8]) -> String {
    id_of_bytes(&sha2::Sha256::digest(key)[..16])
}

fn id_of_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .flat_map(|b| [b >> 4, b & 0xf])
        .map(|n| (b'a' + n) as char)
        .collect()
}

/// The key inside a SubjectPublicKeyInfo (DER): the BIT STRING after the algorithm.
fn spki_key(spki: &[u8]) -> Option<&[u8]> {
    fn element(buf: &[u8], at: usize) -> Option<(u8, usize, usize)> {
        let tag = *buf.get(at)?;
        let first = *buf.get(at + 1)? as usize;
        let (len, start) = if first < 0x80 {
            (first, at + 2)
        } else {
            let n = first & 0x7f;
            if n == 0 || n > 4 {
                return None;
            }
            let mut len = 0usize;
            for i in 0..n {
                len = (len << 8) | *buf.get(at + 2 + i)? as usize;
            }
            (len, at + 2 + n)
        };
        (start + len <= buf.len()).then_some((tag, start, start + len))
    }
    let (tag, start, end) = element(spki, 0)?;
    if tag != 0x30 || end != spki.len() {
        return None;
    }
    let (alg, _, alg_end) = element(spki, start)?;
    if alg != 0x30 {
        return None;
    }
    let (bits, bits_start, bits_end) = element(spki, alg_end)?;
    if bits != 0x03 || bits_end != end || spki.get(bits_start) != Some(&0) {
        return None;
    }
    Some(&spki[bits_start + 1..bits_end])
}

fn verify(proof: &Proof, message: &[u8]) -> bool {
    let Some(key) = spki_key(proof.key) else {
        return false;
    };
    let algorithm: &dyn ring::signature::VerificationAlgorithm = match proof.kind {
        Kind::Rsa => &ring::signature::RSA_PKCS1_2048_8192_SHA256,
        Kind::Ecdsa => &ring::signature::ECDSA_P256_SHA256_ASN1,
    };
    ring::signature::UnparsedPublicKey::new(algorithm, key)
        .verify(message, proof.signature)
        .is_ok()
}

/// The zip archive of CRX3 `bytes`, once its signatures check out for extension `id` (and, with
/// `from_store`, the Chrome Web Store's publisher key signed it too); why not, when they don't.
pub fn verified<'a>(bytes: &'a [u8], id: &str, from_store: bool) -> Result<&'a [u8], String> {
    if bytes.get(..4) != Some(b"Cr24".as_slice()) {
        return Err("It isn't an extension package.".into());
    }
    let word = |at: usize| -> Result<usize, String> {
        bytes
            .get(at..at + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
            .ok_or_else(|| "The package is cut short.".to_string())
    };
    if word(4)? != 3 {
        return Err("Only CRX3 packages are taken.".into());
    }
    let header_len = word(8)?;
    let header = bytes
        .get(12..12 + header_len)
        .ok_or("The package is cut short.")?;
    let archive = &bytes[12 + header_len..];
    let mut proofs = vec![];
    let mut signed_data: Option<&[u8]> = None;
    for (number, value) in fields(header)? {
        let kind = match number {
            2 => Kind::Rsa,
            3 => Kind::Ecdsa,
            10000 => {
                signed_data = Some(value);
                continue;
            }
            _ => continue,
        };
        let parts = fields(value)?;
        let get = |n: u64| parts.iter().find(|(f, _)| *f == n).map(|(_, v)| *v);
        let (Some(key), Some(signature)) = (get(1), get(2)) else {
            return Err("A key proof is incomplete.".into());
        };
        proofs.push(Proof {
            kind,
            key,
            signature,
        });
    }
    let signed_data = signed_data.ok_or("The package says no id.")?;
    let crx_id = fields(signed_data)?
        .into_iter()
        .find(|(f, _)| *f == 1)
        .map(|(_, v)| v)
        .filter(|v| v.len() == 16)
        .ok_or("The package says no id.")?;
    let signed_id = id_of_bytes(crx_id);
    if signed_id != id {
        return Err(format!(
            "The package is for another extension ({signed_id})."
        ));
    }
    if proofs.is_empty() {
        return Err("The package isn't signed.".into());
    }
    let mut message =
        Vec::with_capacity(SIGNED_PREFIX.len() + 4 + signed_data.len() + archive.len());
    message.extend_from_slice(SIGNED_PREFIX);
    message.extend_from_slice(&(signed_data.len() as u32).to_le_bytes());
    message.extend_from_slice(signed_data);
    message.extend_from_slice(archive);
    if !proofs.iter().all(|p| verify(p, &message)) {
        return Err("A signature on the package doesn't check out.".into());
    }
    if !proofs
        .iter()
        .any(|p| sha2::Sha256::digest(p.key)[..16] == *crx_id)
    {
        return Err("The package isn't signed with the extension's own key.".into());
    }
    if from_store
        && !proofs
            .iter()
            .any(|p| sha2::Sha256::digest(p.key)[..] == STORE_PUBLISHER_KEY_HASH)
    {
        return Err("The package isn't signed by the Chrome Web Store.".into());
    }
    if !archive.starts_with(b"PK\x03\x04") {
        return Err("The package's archive is damaged.".into());
    }
    Ok(archive)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::rand::SystemRandom;
    use ring::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair as _};

    /// A P-256 public key as a SubjectPublicKeyInfo.
    fn spki(point: &[u8]) -> Vec<u8> {
        let mut out = hex("3059301306072a8648ce3d020106082a8648ce3d030107034200");
        out.extend_from_slice(point);
        out
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    fn field(number: u64, value: &[u8]) -> Vec<u8> {
        let mut out = vec![];
        let mut put = |mut v: u64| loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                break;
            }
            out.push(b | 0x80);
        };
        put((number << 3) | 2);
        put(value.len() as u64);
        out.extend_from_slice(value);
        out
    }

    /// A CRX3 signed by fresh ECDSA keys (`signers`), for the id of the first.
    fn package(signers: usize, tamper: bool) -> (Vec<u8>, String) {
        let rng = SystemRandom::new();
        let keys: Vec<EcdsaKeyPair> = (0..signers)
            .map(|_| {
                let pkcs8 =
                    EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
                EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng)
                    .unwrap()
            })
            .collect();
        let first = spki(keys[0].public_key().as_ref());
        let crx_id = &sha2::Sha256::digest(&first)[..16];
        let signed_data = field(1, crx_id);
        let archive = b"PK\x03\x04 the archive".to_vec();
        let mut message = SIGNED_PREFIX.to_vec();
        message.extend_from_slice(&(signed_data.len() as u32).to_le_bytes());
        message.extend_from_slice(&signed_data);
        message.extend_from_slice(&archive);
        let mut header = vec![];
        for k in &keys {
            let sig = k.sign(&rng, &message).unwrap();
            let mut proof = field(1, &spki(k.public_key().as_ref()));
            proof.extend(field(2, sig.as_ref()));
            header.extend(field(3, &proof));
        }
        header.extend(field(10000, &signed_data));
        let mut out = b"Cr24".to_vec();
        out.extend_from_slice(&3u32.to_le_bytes());
        out.extend_from_slice(&(header.len() as u32).to_le_bytes());
        out.extend(header);
        let mut body = archive;
        if tamper {
            body.push(b'!');
        }
        out.extend(body);
        (out, id_of_key(&first))
    }

    /// Real packages, when given (`TREK_CRX_SAMPLES=<file>:<id>:<1 for the Chrome Web
    /// Store>,…`): checked the way Trek checks the store's.
    #[test]
    #[ignore = "needs real store packages in TREK_CRX_SAMPLES"]
    fn real_store_packages_pass() {
        let samples = std::env::var("TREK_CRX_SAMPLES").unwrap_or_default();
        assert!(!samples.is_empty());
        for sample in samples.split(',') {
            let mut parts = sample.rsplitn(3, ':');
            let (store, id, file) = (
                parts.next().unwrap(),
                parts.next().unwrap(),
                parts.next().unwrap(),
            );
            let bytes = std::fs::read(file).unwrap();
            assert!(verified(&bytes, id, store == "1").is_ok(), "{file}");
            // One byte changed in its archive, and it's refused.
            let mut bad = bytes.clone();
            let last = bad.len() - 1;
            bad[last] ^= 1;
            assert!(verified(&bad, id, store == "1").is_err(), "{file}");
        }
    }

    #[test]
    fn a_package_signed_for_the_extension_passes() {
        let (crx, id) = package(2, false);
        assert!(verified(&crx, &id, false).unwrap().starts_with(b"PK"));
    }

    #[test]
    fn a_changed_archive_another_id_or_no_store_key_is_refused() {
        let (crx, id) = package(1, true);
        assert!(
            verified(&crx, &id, false)
                .unwrap_err()
                .contains("signature")
        );
        let (crx, _) = package(1, false);
        let other = "a".repeat(32);
        assert!(
            verified(&crx, &other, false)
                .unwrap_err()
                .contains("another extension")
        );
        let (crx, id) = package(1, false);
        assert!(
            verified(&crx, &id, true)
                .unwrap_err()
                .contains("Chrome Web Store")
        );
        assert!(verified(b"PK\x03\x04", &id, false).is_err());
        assert!(verified(b"Cr24\x02\0\0\0", &id, false).is_err());
    }
}
