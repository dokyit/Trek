//! What a program prints, capped: the first half of the budget and the last half are kept, and
//! the middle is counted and dropped. The log keeps every line, so nothing is lost for a search.

pub(crate) struct Output {
    cap: usize,
    head: String,
    tail: String,
    dropped: u64,
}

impl Output {
    pub(crate) fn new(cap: usize) -> Self {
        Self {
            cap,
            head: String::new(),
            tail: String::new(),
            dropped: 0,
        }
    }

    /// One `print` call: its text and a newline.
    pub(crate) fn push_line(&mut self, line: &str) {
        self.push(line);
        self.push("\n");
    }

    fn push(&mut self, mut s: &str) {
        let half = self.cap / 2;
        if self.head.len() < half {
            let take = floor_char(s, half - self.head.len());
            self.head.push_str(&s[..take]);
            s = &s[take..];
        }
        if s.is_empty() {
            return;
        }
        self.tail.push_str(s);
        // Trim only once the tail is well over its share, so a stream of small prints doesn't
        // shift the buffer on every line.
        let keep = self.cap - self.head.len();
        if self.tail.len() > keep.max(1024).saturating_mul(2) {
            self.trim_tail(keep);
        }
    }

    fn trim_tail(&mut self, keep: usize) {
        if self.tail.len() <= keep {
            return;
        }
        let cut = ceil_char(&self.tail, self.tail.len() - keep);
        self.dropped += cut as u64;
        self.tail.drain(..cut);
    }

    /// The text for the model and how many bytes were dropped from its middle. `log` is named
    /// in the marker so the model knows where the rest went.
    pub(crate) fn finish(mut self, log: Option<&std::path::Path>) -> (String, u64) {
        let keep = self.cap - self.head.len();
        self.trim_tail(keep);
        if self.dropped == 0 {
            self.head.push_str(&self.tail);
            return (self.head, 0);
        }
        // Start the tail at a line where one is near, so it doesn't open mid-word.
        if let Some(nl) = self.tail[..floor_char(&self.tail, 200)].find('\n') {
            self.dropped += nl as u64 + 1;
            self.tail.drain(..=nl);
        }
        let where_ = match log {
            Some(p) => format!("; the full output is in {}", p.display()),
            None => String::new(),
        };
        let mut text = self.head;
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&format!(
            "[... {} bytes of output dropped{where_} ...]\n",
            self.dropped
        ));
        text.push_str(&self.tail);
        (text, self.dropped)
    }
}

/// `s` in at most about `max` bytes: its head and tail, with a marker for the middle.
pub(crate) fn cap_text(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let head = floor_char(s, max / 2);
    let tail = ceil_char(s, s.len() - max / 2);
    format!(
        "{}\n[... {} bytes dropped ...]\n{}",
        &s[..head],
        tail - head,
        &s[tail..]
    )
}

/// The largest index at most `i` that falls on a char boundary of `s`.
pub(crate) fn floor_char(s: &str, i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    (0..=i).rev().find(|&j| s.is_char_boundary(j)).unwrap_or(0)
}

/// The smallest index at least `i` that falls on a char boundary of `s`.
pub(crate) fn ceil_char(s: &str, i: usize) -> usize {
    (i..=s.len())
        .find(|&j| s.is_char_boundary(j))
        .unwrap_or(s.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_output_is_kept_whole() {
        let mut o = Output::new(100);
        o.push_line("hello");
        o.push_line("world");
        assert_eq!(o.finish(None), ("hello\nworld\n".to_string(), 0));
    }

    #[test]
    fn long_output_keeps_head_and_tail() {
        let mut o = Output::new(1000);
        for i in 0..10_000 {
            o.push_line(&format!("line {i}"));
        }
        let (text, dropped) = o.finish(None);
        assert!(text.starts_with("line 0\nline 1\n"));
        assert!(text.ends_with("line 9999\n"));
        assert!(text.contains("bytes of output dropped"));
        assert!(text.len() < 1100, "{}", text.len());
        let total: u64 = (0..10_000)
            .map(|i| format!("line {i}\n").len() as u64)
            .sum();
        assert!(
            dropped > total - 1000 && dropped < total - 900,
            "{dropped} of {total}"
        );
    }

    #[test]
    fn a_tail_of_wide_chars_finishes() {
        // `finish` looks for a line start in the tail's first 200 bytes; with 3-byte chars,
        // byte 200 is mid-char.
        let mut o = Output::new(1000);
        for _ in 0..50 {
            o.push_line(&"€".repeat(100));
        }
        let (text, dropped) = o.finish(None);
        assert!(dropped > 0);
        assert!(text.ends_with(&format!("{}\n", "€".repeat(100))));
    }

    #[test]
    fn cap_text_keeps_both_ends() {
        assert_eq!(cap_text("short", 10), "short");
        let t = cap_text(&format!("A{}Z", "é".repeat(5000)), 100);
        assert!(
            t.starts_with('A') && t.ends_with('Z') && t.len() < 150,
            "{t}"
        );
    }

    #[test]
    fn cuts_fall_on_char_boundaries() {
        let mut o = Output::new(11);
        o.push_line(&"é".repeat(400));
        let (text, dropped) = o.finish(None);
        assert!(dropped > 0);
        assert!(text.contains('é'));
    }
}
