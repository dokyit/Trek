//! The bytes a relying party checks: `clientDataJSON`, the authenticator data with its flags and
//! counter, the public key as a COSE key, and the attestation object around them.

use crate::cbor::Value;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest as _, Sha256};

/// Trek's authenticator, as relying parties may name it (an AAGUID of its own, made once).
pub const AAGUID: [u8; 16] = [
    0x94, 0x7e, 0xb9, 0xe3, 0xba, 0x36, 0x46, 0x30, 0xb8, 0x70, 0xc9, 0x09, 0x29, 0x1d, 0xdd, 0x40,
];

/// COSE's name for ECDSA with P-256 and SHA-256, the one algorithm Trek's keys sign with.
pub const ES256: i64 = -7;

/// Authenticator data flags (WebAuthn 6.1).
pub mod flags {
    /// User present.
    pub const UP: u8 = 0x01;
    /// User verified (Touch ID, or the computer's password).
    pub const UV: u8 = 0x04;
    /// Backup eligible: never set, Trek's keys stay on this computer.
    pub const BE: u8 = 0x08;
    /// Backed up: never set.
    pub const BS: u8 = 0x10;
    /// Attested credential data follows (a new passkey).
    pub const AT: u8 = 0x40;
    /// Extension data follows: never set.
    pub const ED: u8 = 0x80;
}

pub fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// base64url as pages send it: without padding, though padding is let through.
pub fn unb64(text: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(text.trim_end_matches('=')).ok()
}

pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// What a ceremony is for, as `clientDataJSON`'s `type` says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ceremony {
    Create,
    Get,
}

/// `clientDataJSON`, byte for byte as WebAuthn's serialization writes it (5.8.1.1): relying
/// parties that don't parse JSON compare it as text, so the order and the escaping are fixed.
/// `origin` is the page's own (`Origin::serialize`); Trek answers no frame of another origin, so
/// `crossOrigin` is always false.
pub fn client_data_json(ceremony: Ceremony, challenge: &[u8], origin: &str) -> Vec<u8> {
    let mut out = String::from("{\"type\":");
    ccd_string(
        match ceremony {
            Ceremony::Create => "webauthn.create",
            Ceremony::Get => "webauthn.get",
        },
        &mut out,
    );
    out.push_str(",\"challenge\":");
    ccd_string(&b64(challenge), &mut out);
    out.push_str(",\"origin\":");
    ccd_string(origin, &mut out);
    out.push_str(",\"crossOrigin\":false}");
    out.into_bytes()
}

/// The specification's `CCDToString`: a JSON string with only `"` and `\` escaped by name and
/// everything below a space as `\u00xx`.
fn ccd_string(text: &str, out: &mut String) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// A P-256 public key as a COSE key (RFC 9053; WebAuthn 6.5.1.1): `point` is the uncompressed
/// point, `04 || x || y`.
pub fn cose_key(point: &[u8; 65]) -> Vec<u8> {
    Value::Map(vec![
        // kty: EC2
        (Value::Int(1), Value::Int(2)),
        // alg: ES256
        (Value::Int(3), Value::Int(ES256)),
        // crv: P-256
        (Value::Int(-1), Value::Int(1)),
        (Value::Int(-2), Value::Bytes(point[1..33].to_vec())),
        (Value::Int(-3), Value::Bytes(point[33..].to_vec())),
    ])
    .encode()
}

/// The same key as a DER `SubjectPublicKeyInfo`, which `getPublicKey()` hands a page.
pub fn spki(point: &[u8; 65]) -> Vec<u8> {
    // SEQUENCE { SEQUENCE { id-ecPublicKey, prime256v1 }, BIT STRING { point } }
    const HEAD: [u8; 26] = [
        0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08,
        0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
    ];
    let mut out = HEAD.to_vec();
    out.extend_from_slice(point);
    out
}

/// Authenticator data (WebAuthn 6.1): the hash of the relying party id, the flags, the signature
/// counter, and for a new passkey its id and public key. The counter is always zero: Trek keeps
/// none, as synced passkeys don't, so a profile restored from a backup can never look like a
/// cloned key to a site.
pub fn authenticator_data(rp_id: &str, flags: u8, attested: Option<(&[u8], &[u8; 65])>) -> Vec<u8> {
    let mut out = sha256(rp_id.as_bytes()).to_vec();
    let at = if attested.is_some() { flags::AT } else { 0 };
    out.push((flags & !flags::AT) | at);
    out.extend_from_slice(&0u32.to_be_bytes());
    if let Some((id, point)) = attested {
        out.extend_from_slice(&AAGUID);
        out.extend_from_slice(&(id.len() as u16).to_be_bytes());
        out.extend_from_slice(id);
        out.extend_from_slice(&cose_key(point));
    }
    out
}

/// The attestation object for a new passkey, with no attestation statement (`fmt: "none"`):
/// Trek has no manufacturer's certificate to vouch with, and sites that take passkeys don't ask
/// for one.
pub fn attestation_object(auth_data: &[u8]) -> Vec<u8> {
    Value::Map(vec![
        (Value::text("fmt"), Value::text("none")),
        (Value::text("attStmt"), Value::Map(vec![])),
        (Value::text("authData"), Value::Bytes(auth_data.to_vec())),
    ])
    .encode()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// The key of WebAuthn's own COSE example (6.5.1.1, "Examples of credentialPublicKey Values
    /// Encoded in COSE_Key Format", the ES256 one).
    const X: &str = "65eda5a12577c2bae829437fe338701a10aaa375e1bb5b5de108de439c08551d";
    const Y: &str = "1e52ed75701163f7f9e40ddf9f341b3dc9ba860af7e0ca7ca7e9eecd0084d19c";

    pub fn example_point() -> [u8; 65] {
        let mut point = [4u8; 65];
        point[1..33].copy_from_slice(&unhex(X));
        point[33..].copy_from_slice(&unhex(Y));
        point
    }

    #[test]
    fn the_specifications_example_key_encodes_to_its_bytes() {
        // A5 / 01 02 / 03 26 / 20 01 / 21 58 20 x / 22 58 20 y
        let want = format!("a5010203262001215820{X}225820{Y}");
        assert_eq!(hex(&cose_key(&example_point())), want);
        // And it is a point on the curve, which a made-up example wouldn't be.
        assert!(p256::ecdsa::VerifyingKey::from_sec1_bytes(&example_point()).is_ok());
    }

    #[test]
    fn the_public_key_info_is_what_a_der_reader_expects() {
        let der = spki(&example_point());
        assert_eq!(der.len(), 91);
        assert_eq!(
            hex(&der[..26]),
            "3059301306072a8648ce3d020106082a8648ce3d030107034200"
        );
        assert_eq!(&der[26..], &example_point());
    }

    #[test]
    fn client_data_is_written_exactly_as_the_specification_serializes_it() {
        let challenge = unhex("00010203fffefd");
        assert_eq!(
            String::from_utf8(client_data_json(
                Ceremony::Create,
                &challenge,
                "https://example.com"
            ))
            .unwrap(),
            r#"{"type":"webauthn.create","challenge":"AAECA__-_Q","origin":"https://example.com","crossOrigin":false}"#
        );
        assert_eq!(
            String::from_utf8(client_data_json(
                Ceremony::Get,
                b"hello",
                "http://localhost:3000"
            ))
            .unwrap(),
            r#"{"type":"webauthn.get","challenge":"aGVsbG8","origin":"http://localhost:3000","crossOrigin":false}"#
        );
        // It is JSON, whatever the origin holds, and it reads back as the same values.
        let odd = String::from_utf8(client_data_json(
            Ceremony::Get,
            b"x",
            "a\"b\\c\u{1}\u{1f}d\u{e9}",
        ))
        .unwrap();
        assert!(odd.contains(r#""origin":"a\"b\\c\u0001\u001fdé""#), "{odd}");
        let parsed: serde_json::Value = serde_json::from_str(&odd).unwrap();
        assert_eq!(parsed["origin"], "a\"b\\c\u{1}\u{1f}d\u{e9}");
        assert_eq!(parsed["crossOrigin"], false);
    }

    #[test]
    fn base64url_has_no_padding_and_reads_either_way() {
        assert_eq!(b64(&[0xfb, 0xff]), "-_8");
        assert_eq!(unb64("-_8"), Some(vec![0xfb, 0xff]));
        assert_eq!(unb64("-_8="), Some(vec![0xfb, 0xff]));
        assert_eq!(unb64(""), Some(vec![]));
        // The other alphabet isn't base64url.
        assert_eq!(unb64("+/8"), None);
        assert_eq!(unb64("not base64!"), None);
    }

    #[test]
    fn the_relying_party_hash_is_the_sha_256_of_its_id() {
        for (rp, want) in [
            (
                "example.com",
                "a379a6f6eeafb9a55e378c118034e2751e682fab9f2d30ab13d2125586ce1947",
            ),
            (
                "localhost",
                "49960de5880e8c687434170f6476605b8fe4aeb9a28632c7995cf3ba831d9763",
            ),
            (
                "webauthn.io",
                "74a6ea9213c99c2f74b22492b320cf40262a94c1a950a0397f29250b60841ef0",
            ),
        ] {
            let data = authenticator_data(rp, flags::UP | flags::UV, None);
            assert_eq!(hex(&data[..32]), want, "{rp}");
        }
    }

    #[test]
    fn an_assertions_data_is_37_bytes_hash_flags_and_a_zero_counter() {
        let data = authenticator_data("example.com", flags::UP | flags::UV, None);
        assert_eq!(data.len(), 37);
        assert_eq!(data[32], 0x05, "user present and verified, nothing else");
        assert_eq!(&data[33..], &[0, 0, 0, 0]);
        // Presence alone, and AT can't be asked for without a key to attest.
        assert_eq!(authenticator_data("example.com", flags::UP, None)[32], 0x01);
        assert_eq!(
            authenticator_data("example.com", flags::UP | flags::AT, None)[32],
            0x01
        );
    }

    #[test]
    fn a_new_passkeys_data_carries_its_id_and_key() {
        let id = [7u8; 32];
        let point = example_point();
        let data = authenticator_data("example.com", flags::UP | flags::UV, Some((&id, &point)));
        assert_eq!(
            data[32], 0x45,
            "UP, UV and AT; not backed up, no extensions"
        );
        assert_eq!(data[32] & (flags::BE | flags::BS | flags::ED), 0);
        assert_eq!(&data[33..37], &[0, 0, 0, 0]);
        assert_eq!(&data[37..53], &AAGUID);
        assert_eq!(&data[53..55], &[0, 32]);
        assert_eq!(&data[55..87], &id);
        // The key is the rest, and exactly one CBOR item.
        let (key, used) = Value::decode(&data[87..]).unwrap();
        assert_eq!(used, data.len() - 87);
        assert_eq!(key.get(&Value::Int(1)), Some(&Value::Int(2)), "EC2");
        assert_eq!(key.get(&Value::Int(3)), Some(&Value::Int(-7)), "ES256");
        assert_eq!(key.get(&Value::Int(-1)), Some(&Value::Int(1)), "P-256");
        assert_eq!(
            key.get(&Value::Int(-2)),
            Some(&Value::Bytes(point[1..33].to_vec()))
        );
        assert_eq!(
            key.get(&Value::Int(-3)),
            Some(&Value::Bytes(point[33..].to_vec()))
        );
    }

    #[test]
    fn the_attestation_object_says_none_and_wraps_the_data() {
        let data = authenticator_data("example.com", flags::UP | flags::UV, None);
        let object = attestation_object(&data);
        // The start every "none" attestation has, as a browser writes it.
        assert!(
            hex(&object).starts_with("a363666d74646e6f6e656761747453746d74a0686175746844617461"),
            "{}",
            hex(&object)
        );
        assert!(b64(&object).starts_with("o2NmbXRkbm9uZWdhdHRTdG10oGhhdXRoRGF0YV"));
        let (value, used) = Value::decode(&object).unwrap();
        assert_eq!(used, object.len());
        assert_eq!(value.get(&Value::text("fmt")), Some(&Value::text("none")));
        assert_eq!(
            value.get(&Value::text("attStmt")),
            Some(&Value::Map(vec![]))
        );
        assert_eq!(
            value.get(&Value::text("authData")),
            Some(&Value::Bytes(data))
        );
    }
}
