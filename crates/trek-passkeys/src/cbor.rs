//! The little of CBOR an authenticator writes (RFC 8949), in CTAP2's canonical form: integers in
//! their shortest encoding, definite lengths, and a map's keys sorted by their encoded bytes,
//! shorter first. A relying party hashes and compares these bytes, so there is one right answer.
//! The reader exists for the tests, which take what was written apart again.

/// A CBOR data item, as far as WebAuthn needs them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// Major types 0 and 1.
    Int(i64),
    Bytes(Vec<u8>),
    Text(String),
    Array(Vec<Value>),
    /// Written with its keys in canonical order, whatever order they're given in.
    Map(Vec<(Value, Value)>),
    Bool(bool),
}

impl Value {
    pub fn text(s: &str) -> Value {
        Value::Text(s.to_string())
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut Vec<u8>) {
        match self {
            Value::Int(n) if *n >= 0 => head(0, *n as u64, out),
            // -1 - n, which for a negative i64 is its bits inverted.
            Value::Int(n) => head(1, !(*n) as u64, out),
            Value::Bytes(b) => {
                head(2, b.len() as u64, out);
                out.extend_from_slice(b);
            }
            Value::Text(s) => {
                head(3, s.len() as u64, out);
                out.extend_from_slice(s.as_bytes());
            }
            Value::Array(items) => {
                head(4, items.len() as u64, out);
                for item in items {
                    item.write(out);
                }
            }
            Value::Map(entries) => {
                let mut encoded: Vec<(Vec<u8>, &Value)> =
                    entries.iter().map(|(k, v)| (k.encode(), v)).collect();
                encoded.sort_by(|(a, _), (b, _)| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
                head(5, encoded.len() as u64, out);
                for (key, value) in encoded {
                    out.extend_from_slice(&key);
                    value.write(out);
                }
            }
            Value::Bool(false) => out.push(0xf4),
            Value::Bool(true) => out.push(0xf5),
        }
    }

    /// Read one item from the front of `bytes`: the item and how many bytes it took. Only what
    /// `encode` writes is understood (no floats, tags or indefinite lengths), and a map's keys
    /// must come in canonical order with none twice.
    pub fn decode(bytes: &[u8]) -> Option<(Value, usize)> {
        let (major, arg, mut at) = read_head(bytes)?;
        let value = match major {
            0 => Value::Int(i64::try_from(arg).ok()?),
            1 => Value::Int(-1 - i64::try_from(arg).ok()?),
            2 | 3 => {
                let len = usize::try_from(arg).ok()?;
                let data = bytes.get(at..at.checked_add(len)?)?;
                at += len;
                if major == 2 {
                    Value::Bytes(data.to_vec())
                } else {
                    Value::Text(String::from_utf8(data.to_vec()).ok()?)
                }
            }
            4 => {
                let mut items = Vec::new();
                for _ in 0..arg {
                    let (item, used) = Value::decode(bytes.get(at..)?)?;
                    at += used;
                    items.push(item);
                }
                Value::Array(items)
            }
            5 => {
                let mut entries: Vec<(Value, Value)> = Vec::new();
                let mut last: Option<Vec<u8>> = None;
                for _ in 0..arg {
                    let (key, used) = Value::decode(bytes.get(at..)?)?;
                    let raw = bytes.get(at..at + used)?.to_vec();
                    if let Some(prev) = &last
                        && (prev.len(), prev.as_slice()) >= (raw.len(), raw.as_slice())
                    {
                        return None;
                    }
                    last = Some(raw);
                    at += used;
                    let (value, used) = Value::decode(bytes.get(at..)?)?;
                    at += used;
                    entries.push((key, value));
                }
                Value::Map(entries)
            }
            7 if arg == 20 => Value::Bool(false),
            7 if arg == 21 => Value::Bool(true),
            _ => return None,
        };
        Some((value, at))
    }

    /// A map's value under `key`.
    pub fn get(&self, key: &Value) -> Option<&Value> {
        match self {
            Value::Map(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

/// A major type with its argument, in the fewest bytes that hold it.
fn head(major: u8, arg: u64, out: &mut Vec<u8>) {
    let major = major << 5;
    match arg {
        0..=23 => out.push(major | arg as u8),
        24..=0xff => out.extend_from_slice(&[major | 24, arg as u8]),
        0x100..=0xffff => {
            out.push(major | 25);
            out.extend_from_slice(&(arg as u16).to_be_bytes());
        }
        0x1_0000..=0xffff_ffff => {
            out.push(major | 26);
            out.extend_from_slice(&(arg as u32).to_be_bytes());
        }
        _ => {
            out.push(major | 27);
            out.extend_from_slice(&arg.to_be_bytes());
        }
    }
}

/// An item's major type, its argument and where its content starts. An argument written longer
/// than it needs is refused: canonical CBOR has one encoding.
fn read_head(bytes: &[u8]) -> Option<(u8, u64, usize)> {
    let first = *bytes.first()?;
    let (major, info) = (first >> 5, first & 0x1f);
    let wide = |n: usize| -> Option<u64> {
        let raw = bytes.get(1..1 + n)?;
        let mut buf = [0u8; 8];
        buf[8 - n..].copy_from_slice(raw);
        Some(u64::from_be_bytes(buf))
    };
    let (arg, at) = match info {
        0..=23 => (u64::from(info), 1),
        // (A simple value in a byte of its own is none of the ones written here.)
        24 => (wide(1).filter(|v| *v >= 24 && major != 7)?, 2),
        25 => (wide(2).filter(|v| *v > 0xff)?, 3),
        26 => (wide(4).filter(|v| *v > 0xffff)?, 5),
        27 => (wide(8).filter(|v| *v > 0xffff_ffff)?, 9),
        _ => return None,
    };
    Some((major, arg, at))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// RFC 8949, Appendix A ("Examples of Encoded CBOR Data Items").
    #[test]
    fn the_rfcs_examples_encode_to_its_bytes_and_read_back() {
        let map = |entries: Vec<(Value, Value)>| Value::Map(entries);
        let cases: Vec<(Value, &str)> = vec![
            (Value::Int(0), "00"),
            (Value::Int(1), "01"),
            (Value::Int(10), "0a"),
            (Value::Int(23), "17"),
            (Value::Int(24), "1818"),
            (Value::Int(25), "1819"),
            (Value::Int(100), "1864"),
            (Value::Int(1000), "1903e8"),
            (Value::Int(1_000_000), "1a000f4240"),
            (Value::Int(1_000_000_000_000), "1b000000e8d4a51000"),
            (Value::Int(-1), "20"),
            (Value::Int(-10), "29"),
            (Value::Int(-100), "3863"),
            (Value::Int(-1000), "3903e7"),
            (Value::Bool(false), "f4"),
            (Value::Bool(true), "f5"),
            (Value::Bytes(vec![]), "40"),
            (Value::Bytes(vec![1, 2, 3, 4]), "4401020304"),
            (Value::text(""), "60"),
            (Value::text("a"), "6161"),
            (Value::text("IETF"), "6449455446"),
            (Value::text("\"\\"), "62225c"),
            (Value::text("\u{00fc}"), "62c3bc"),
            (Value::text("\u{6c34}"), "63e6b0b4"),
            (Value::Array(vec![]), "80"),
            (
                Value::Array(vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
                "83010203",
            ),
            (map(vec![]), "a0"),
            (
                map(vec![
                    (Value::Int(1), Value::Int(2)),
                    (Value::Int(3), Value::Int(4)),
                ]),
                "a201020304",
            ),
            (
                map(vec![
                    (Value::text("a"), Value::Int(1)),
                    (
                        Value::text("b"),
                        Value::Array(vec![Value::Int(2), Value::Int(3)]),
                    ),
                ]),
                "a26161016162820203",
            ),
        ];
        for (value, want) in cases {
            assert_eq!(hex(&value.encode()), want, "{value:?}");
            let bytes = unhex(want);
            assert_eq!(
                Value::decode(&bytes),
                Some((value.clone(), bytes.len())),
                "{want}"
            );
        }
    }

    #[test]
    fn a_maps_keys_are_written_in_ctap2s_order_whatever_order_they_come_in() {
        // Shorter keys first, then by their bytes: 1, 3, -1, -2, -3 for a COSE key (RFC 9052's
        // labels; the order every authenticator writes them in), and "fmt" before "attStmt"
        // before "authData" for an attestation object.
        let cose = Value::Map(vec![
            (Value::Int(-3), Value::Int(0)),
            (Value::Int(3), Value::Int(-7)),
            (Value::Int(-1), Value::Int(1)),
            (Value::Int(1), Value::Int(2)),
            (Value::Int(-2), Value::Int(0)),
        ]);
        assert_eq!(hex(&cose.encode()), "a501020326200121002200");
        let attestation = Value::Map(vec![
            (Value::text("authData"), Value::Bytes(vec![0xaa])),
            (Value::text("attStmt"), Value::Map(vec![])),
            (Value::text("fmt"), Value::text("none")),
        ]);
        assert_eq!(
            hex(&attestation.encode()),
            "a363666d74646e6f6e656761747453746d74a068617574684461746141aa"
        );
    }

    #[test]
    fn what_isnt_canonical_isnt_read() {
        // 23 written in two bytes; a length that runs past the end; keys out of order; a key
        // twice; a float; an indefinite-length string.
        for bad in [
            "1817",
            "190017",
            "4401",
            "a203040102",
            "a201020103",
            "f93c00",
            "5f41aaff",
            // `false` and another simple value, each in a byte of its own.
            "f814",
            "f8ff",
            "",
        ] {
            assert_eq!(Value::decode(&unhex(bad)), None, "{bad}");
        }
        // What follows an item isn't part of it.
        assert_eq!(
            Value::decode(&unhex("0102")),
            Some((Value::Int(1), 1)),
            "one item"
        );
    }
}
