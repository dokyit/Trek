//! Files a phone sends with a message (feature `uploads`): a video from its photo library, or any
//! file from the Files app. Each goes up in pieces (`upload_part`, answered with
//! `upload_progress`), in order, then `send.files` / `new_thread.files` name it by the id the
//! phone chose, and the Mac mentions it in the message as its own composer mentions a file
//! (`@path`).

use serde::{Deserialize, Serialize};

/// The most one `upload_part` carries, in bytes before base64: 4 MiB, about 5.6 MB once in base64
/// (with JSON's `\/` escapes), well under a WebSocket message's 8 MiB.
pub const MAX_UPLOAD_PART: u64 = 4 << 20;

/// The biggest file a phone may send: 2 GiB. A phone's videos are what's big: at this size about
/// five minutes of 4K at 60 fps (HEVC), half an hour of 1080p; anything longer isn't something to
/// hand an agent, and four of these at once (`MAX_UPLOADS_IN_FLIGHT`) is all the room one phone
/// can make a Mac's disk give up.
pub const MAX_UPLOAD: u64 = 2 << 30;

/// Files one connection may have part-way up at once; one more is refused (`rate_limited`) until
/// one finishes. A composer sends its files one after another, so it never meets this.
pub const MAX_UPLOADS_IN_FLIGHT: usize = 4;

/// The longest name a file is kept under (in bytes, before the Mac's `iPhone <date> ` prefix).
pub const MAX_UPLOAD_NAME: usize = 150;

/// `upload_part`: the bytes of `name` from `offset`. The first piece is at 0 and each one after it
/// starts where the one before ended; the last one ends at `total`. Every piece names the same
/// `upload_id`, `name` and `total`. A refused piece ends the upload (send the file again, under a
/// new id); a piece at 0 for one part-way up starts it again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadPart {
    /// Chosen by the phone: a UUID (`8-4-4-4-12` hex digits, either case).
    pub upload_id: String,
    /// The file's name as the phone has it (`IMG_0042.MOV`, `Quarterly report.pdf`). The Mac keeps
    /// it as [`upload_file_name`] makes it.
    pub name: String,
    /// The whole file's length in bytes ([`MAX_UPLOAD`] at most).
    pub total: u64,
    pub offset: u64,
    /// Base64 of the piece: [`MAX_UPLOAD_PART`] bytes at most, empty only for an empty file.
    pub data: String,
}

/// An [`UploadPart`], checked and decoded: what the host writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadPiece {
    /// Lower case.
    pub upload_id: String,
    /// As [`upload_file_name`] makes it.
    pub name: String,
    pub total: u64,
    pub offset: u64,
    pub bytes: Vec<u8>,
}

/// The reply to `upload_part`: how much of the file the Mac has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadProgress {
    pub upload_id: String,
    /// Bytes in so far (where the next piece starts).
    pub received: u64,
    pub total: u64,
    /// The whole file is in: its id may go in `send.files` or `new_thread.files`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub done: bool,
}

impl UploadPart {
    /// What can be told without decoding the piece: the id is a UUID, the name is usable, the
    /// file isn't over [`MAX_UPLOAD`], the piece starts inside it and isn't over
    /// [`MAX_UPLOAD_PART`] by its base64's length.
    pub fn check(&self) -> Result<(), String> {
        if !valid_upload_id(&self.upload_id) {
            return Err(format!("{:?} isn't an upload id (a UUID)", self.upload_id));
        }
        if upload_file_name(&self.name).is_none() {
            return Err(format!("{:?} can't be a file's name", self.name));
        }
        if self.total > MAX_UPLOAD {
            return Err(format!(
                "That file is too big to send ({} at most)",
                size_words(MAX_UPLOAD)
            ));
        }
        if self.offset > self.total {
            return Err(format!(
                "That piece starts at byte {}, past the end of the file ({} bytes)",
                self.offset, self.total
            ));
        }
        // Base64 is 4 characters for every 3 bytes (a little slack for padding).
        if self.data.len() as u64 > MAX_UPLOAD_PART.div_ceil(3) * 4 + 4 {
            return Err(format!(
                "That piece is too big ({} at most)",
                size_words(MAX_UPLOAD_PART)
            ));
        }
        Ok(())
    }

    /// [`Self::check`], then the piece decoded: it fits in the file (no more bytes than `total`
    /// leaves after `offset`), and only an empty file sends an empty piece.
    pub fn decode(&self) -> Result<UploadPiece, String> {
        use base64::Engine as _;
        self.check()?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(self.data.trim())
            .map_err(|e| format!("That piece isn't base64: {e}"))?;
        if bytes.len() as u64 > MAX_UPLOAD_PART {
            return Err(format!(
                "That piece is too big ({} at most)",
                size_words(MAX_UPLOAD_PART)
            ));
        }
        if self.offset + bytes.len() as u64 > self.total {
            return Err(format!(
                "That piece runs past the end of the file ({} bytes from byte {}, of {})",
                bytes.len(),
                self.offset,
                self.total
            ));
        }
        if bytes.is_empty() && self.total > 0 {
            return Err("That piece is empty".into());
        }
        Ok(UploadPiece {
            upload_id: self.upload_id.to_ascii_lowercase(),
            name: upload_file_name(&self.name).unwrap_or_default(),
            total: self.total,
            offset: self.offset,
            bytes,
        })
    }
}

/// A UUID as text: 32 hex digits in groups of 8, 4, 4, 4 and 12, joined by `-` (either case).
pub fn valid_upload_id(id: &str) -> bool {
    let b = id.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// The name a phone's file is kept under: the last part of what it sent (after any `/` or `\`),
/// without control characters, bidirectional overrides (which can make `exe.pdf` read as
/// `fdp.exe`), leading dots or spaces, with `"` as `'` (a mention quotes a path in `"`) and the
/// characters Windows refuses as `-`; at most [`MAX_UPLOAD_NAME`] bytes, the extension kept.
/// `None` when nothing is left.
pub fn upload_file_name(name: &str) -> Option<String> {
    let last = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String = last
        .chars()
        .filter(|c| !c.is_control() && !is_bidi_control(*c))
        .map(|c| match c {
            '"' => '\'',
            '<' | '>' | ':' | '|' | '?' | '*' => '-',
            c => c,
        })
        .collect();
    // No hidden file, no `.`/`..`, and no trailing dot or space (Windows drops them).
    let cleaned = cleaned
        .trim_start_matches(|c: char| c == '.' || c.is_whitespace())
        .trim_end_matches(|c: char| c == '.' || c.is_whitespace());
    if cleaned.is_empty() {
        return None;
    }
    if cleaned.len() <= MAX_UPLOAD_NAME {
        return Some(cleaned.to_string());
    }
    // Too long: the stem is cut short, the extension (a short one) kept.
    let (stem, ext) = match cleaned.rfind('.') {
        Some(dot) if dot > 0 && cleaned.len() - dot <= 16 => cleaned.split_at(dot),
        _ => (cleaned, ""),
    };
    let mut end = MAX_UPLOAD_NAME - ext.len();
    while !stem.is_char_boundary(end) {
        end -= 1;
    }
    Some(format!("{}{ext}", stem[..end].trim_end()))
}

fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// `bytes` in words: "4 MB", "2 GB".
fn size_words(bytes: u64) -> String {
    if bytes >= 1 << 30 {
        format!("{} GB", bytes >> 30)
    } else {
        format!("{} MB", bytes >> 20)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_made_safe_and_keep_their_extension() {
        for (sent, kept) in [
            ("IMG_0042.MOV", Some("IMG_0042.MOV")),
            ("Quarterly report.pdf", Some("Quarterly report.pdf")),
            ("../../etc/passwd", Some("passwd")),
            ("C:\\Users\\me\\notes.txt", Some("notes.txt")),
            ("Standup 10:30.m4a", Some("Standup 10-30.m4a")),
            (".env", Some("env")),
            ("  ..hidden.txt. ", Some("hidden.txt")),
            ("bell\u{7}\u{0}.wav", Some("bell.wav")),
            ("invoice\u{202E}fdp.exe", Some("invoicefdp.exe")),
            ("say \"hi\".txt", Some("say 'hi'.txt")),
            ("what?.txt", Some("what-.txt")),
            ("..", None),
            ("../..", None),
            ("dir/", None),
            ("\u{7}", None),
            ("", None),
            (" . ", None),
        ] {
            assert_eq!(upload_file_name(sent).as_deref(), kept, "{sent:?}");
        }
        let long = format!("{}.mov", "é".repeat(200));
        let kept = upload_file_name(&long).unwrap();
        assert!(
            kept.len() <= MAX_UPLOAD_NAME && kept.ends_with("é.mov"),
            "{kept}"
        );
    }

    #[test]
    fn upload_ids_are_uuids() {
        assert!(valid_upload_id("3F2504E0-4F89-11D3-9A0C-0305E82C3301"));
        assert!(valid_upload_id("3f2504e0-4f89-11d3-9a0c-0305e82c3301"));
        for bad in [
            "",
            "3f2504e0",
            "3f2504e0-4f89-11d3-9a0c-0305e82c330",
            "3f2504e0-4f89-11d3-9a0c-0305e82c33012",
            "3f2504e0_4f89-11d3-9a0c-0305e82c3301",
            "../../../../../../../../etc/passwd/x",
            "zf2504e0-4f89-11d3-9a0c-0305e82c3301",
        ] {
            assert!(!valid_upload_id(bad), "{bad:?}");
        }
    }
}
