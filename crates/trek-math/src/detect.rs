//! Where an answer writes math: `$…$` and `\(…\)` in a line of text, `$$…$$` and `\[…\]` set on
//! their own. Found in the Markdown source before Markdown reads it, since Markdown takes `\[` for
//! an escaped bracket and `\\` for an escaped backslash, and hidden from it (`mask`) so it reaches
//! the math plugin whole.
//!
//! Dollars are also money and shell variables, so a single `$` only opens math the way Pandoc
//! has it: not followed by a space, closed by a `$` that isn't preceded by a space or followed by
//! a digit ("costs $5 and $10" stays prose). Code — fenced blocks and code spans — is never math,
//! and an escaped `\$` is a dollar.

use std::ops::Range;

/// How a formula is delimited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delim {
    /// `$…$`
    Dollar,
    /// `$$…$$`
    Dollars,
    /// `\(…\)`
    Paren,
    /// `\[…\]`
    Bracket,
}

impl Delim {
    /// Display math: set on its own line, centred.
    pub fn display(self) -> bool {
        matches!(self, Delim::Dollars | Delim::Bracket)
    }

    fn open(self) -> &'static str {
        match self {
            Delim::Dollar => "$",
            Delim::Dollars => "$$",
            Delim::Paren => "\\(",
            Delim::Bracket => "\\[",
        }
    }

    fn close(self) -> &'static str {
        match self {
            Delim::Dollar => "$",
            Delim::Dollars => "$$",
            Delim::Paren => "\\)",
            Delim::Bracket => "\\]",
        }
    }
}

/// A formula in the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    /// Delimiters included.
    pub range: Range<usize>,
    pub delim: Delim,
    /// Its closing delimiter has arrived. An open one runs to the end of the text: an answer
    /// still being written, whose formula shows as plain text until it closes.
    pub closed: bool,
}

impl Region {
    /// The TeX between the delimiters, in `source` (the text the region was found in). Lines
    /// of a formula inside a block quote lose their `>`.
    pub fn tex(&self, source: &str) -> String {
        let Some(text) = source.get(self.range.clone()) else {
            return String::new();
        };
        let inner = text.strip_prefix(self.delim.open()).unwrap_or(text);
        let inner = if self.closed {
            inner.strip_suffix(self.delim.close()).unwrap_or(inner)
        } else {
            inner
        };
        let line_start = source[..self.range.start].rfind('\n').map_or(0, |i| i + 1);
        if !source[line_start..self.range.start]
            .trim_start()
            .starts_with('>')
            || !inner.contains('\n')
        {
            return inner.to_string();
        }
        inner
            .split('\n')
            .enumerate()
            .map(|(i, line)| if i == 0 { line } else { unquote(line) })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn unquote(mut line: &str) -> &str {
    loop {
        let trimmed = line.trim_start();
        match trimmed.strip_prefix('>') {
            Some(rest) => line = rest.strip_prefix(' ').unwrap_or(rest),
            None => return line,
        }
    }
}

/// The byte the mask fills a formula with: Markdown keeps it as it is, nothing writes it, and
/// the plugin knows a formula it masked by it.
pub const FILL: u8 = 0x01;

/// `source` with every formula in it turned into `$` + `FILL`… + `$`, the same length: inline
/// math to Markdown, which it hands to the plugin whole whatever is inside. `None` when there's
/// no math.
pub fn mask(source: &str) -> Option<String> {
    let regions = regions(source);
    let mut bytes = source.as_bytes().to_vec();
    let mut any = false;
    for region in regions {
        let Range { start, end } = region.range;
        if end - start < 3 {
            continue;
        }
        bytes[start] = b'$';
        bytes[start + 1..end - 1].fill(FILL);
        bytes[end - 1] = b'$';
        any = true;
    }
    // Only ASCII went in, over whole characters (a region starts and ends at an ASCII
    // delimiter, or at the end of the text), so this is still UTF-8.
    any.then(|| String::from_utf8(bytes).ok()).flatten()
}

/// `text` is exactly one formula, delimiters included: the source of a node the mask made.
pub fn whole(text: &str) -> Option<Region> {
    let found = regions(text);
    match found.as_slice() {
        [region]
            if region.range == (0..text.len()) || (!region.closed && region.range.start == 0) =>
        {
            Some(region.clone())
        }
        _ => None,
    }
}

/// Inline math longer than this isn't inline math.
const INLINE_MAX: usize = 2000;

/// Every formula in `source`, in order.
pub fn regions(source: &str) -> Vec<Region> {
    let b = source.as_bytes();
    let fences = fences(source);
    let mut fence = fences.iter().peekable();
    let mut out = Vec::new();
    let mut i = 0;
    // `\[` openers before this offset can't close: an earlier one scanned to the end of its
    // paragraph without finding a closer, and a later one's scan is a part of that. (`$$` can't
    // repeat the failure: a second `$$` in the paragraph would have closed the first.)
    let mut dead_bracket = 0;
    while i < b.len() {
        while fence.peek().is_some_and(|f| f.end <= i) {
            fence.next();
        }
        if let Some(f) = fence.peek()
            && f.start <= i
        {
            i = f.end;
            continue;
        }
        let next_fence = fence.peek().map_or(b.len(), |f| f.start);
        match b[i] {
            b'\\' => {
                let delim = match b.get(i + 1) {
                    Some(b'(') => Some(Delim::Paren),
                    Some(b'[') => Some(Delim::Bracket),
                    _ => None,
                };
                match delim {
                    Some(Delim::Bracket) if i < dead_bracket => i += 2,
                    Some(delim) => {
                        let mut dead = 0;
                        match closing(source, i, delim, next_fence, &mut dead) {
                            Some(region) => {
                                i = region.range.end;
                                out.push(region);
                            }
                            None => {
                                if delim == Delim::Bracket {
                                    dead_bracket = dead_bracket.max(dead);
                                }
                                i += 2;
                            }
                        }
                    }
                    // An escape: whatever it escapes is literal.
                    None => i += 1 + b.get(i + 1).map_or(0, |_| char_len(source, i + 1)),
                }
            }
            b'`' => {
                let run = run_of(b, i, b'`');
                i = code_span_end(b, i, run, next_fence).unwrap_or(i + run);
            }
            b'$' => {
                let delim = if b.get(i + 1) == Some(&b'$') {
                    Delim::Dollars
                } else {
                    Delim::Dollar
                };
                match closing(source, i, delim, next_fence, &mut 0) {
                    Some(region) => {
                        i = region.range.end;
                        out.push(region);
                    }
                    None => i += delim.open().len(),
                }
            }
            _ => i += char_len(source, i),
        }
    }
    out
}

fn char_len(source: &str, i: usize) -> usize {
    source[i..].chars().next().map_or(1, char::len_utf8)
}

fn run_of(b: &[u8], i: usize, c: u8) -> usize {
    b[i..].iter().take_while(|&&x| x == c).count()
}

/// Where the code span opened by `run` backticks at `i` ends: past a run of exactly as many,
/// before a blank line or `limit` (a fence).
fn code_span_end(b: &[u8], i: usize, run: usize, limit: usize) -> Option<usize> {
    let mut j = i + run;
    while j < limit {
        if b[j] == b'`' {
            let n = run_of(b, j, b'`');
            if n == run {
                return Some(j + n);
            }
            j += n;
        } else if b[j] == b'\n' && blank_line_after(b, j) {
            return None;
        } else {
            j += 1;
        }
    }
    None
}

/// The line after the line ending at `nl` is blank (or a quote's empty line), or there is none.
fn blank_line_after(b: &[u8], nl: usize) -> bool {
    let rest = &b[nl + 1..];
    let line = rest.split(|&c| c == b'\n').next().unwrap_or(&[]);
    line.iter()
        .all(|c| matches!(c, b' ' | b'\t' | b'\r' | b'>'))
}

/// A line that starts a list item or heading: inline math can't run into one.
fn starts_block(line: &[u8]) -> bool {
    let line = line.trim_ascii_start();
    let line = line
        .strip_prefix(b">")
        .map_or(line, |l| l.trim_ascii_start());
    match line {
        [b'-' | b'*' | b'+', b' ', ..] | [b'#', ..] => true,
        _ => {
            let digits = line.iter().take_while(|c| c.is_ascii_digit()).count();
            digits > 0
                && matches!(line.get(digits), Some(b'.' | b')'))
                && matches!(line.get(digits + 1), Some(b' '))
        }
    }
}

/// The formula `delim` opens at `i`, if it closes — or, when the text ends first (an answer
/// still being written), the open one running to its end. `$` never stays open: it's as often
/// money as math. When it finds no closer before the end of the paragraph (or `limit`), `dead` is
/// set to where it gave up: nothing opened before that offset can close either.
fn closing(source: &str, i: usize, delim: Delim, limit: usize, dead: &mut usize) -> Option<Region> {
    let b = source.as_bytes();
    let open = delim.open().len();
    let start = i + open;
    if delim == Delim::Dollar {
        match b.get(start) {
            None => return None,
            Some(c) if c.is_ascii_whitespace() => return None,
            _ => {}
        }
    }
    let inline = !delim.display();
    let mut j = start;
    while j < limit {
        if inline && j - start > INLINE_MAX {
            return None;
        }
        match b[j] {
            b'\\' => {
                if let Some(rest) = source.get(j..)
                    && delim.close().starts_with('\\')
                    && rest.starts_with(delim.close())
                {
                    if source[start..j].trim().is_empty() {
                        return None;
                    }
                    return Some(Region {
                        range: i..j + 2,
                        delim,
                        closed: true,
                    });
                }
                j += 1 + b.get(j + 1).map_or(0, |_| char_len(source, j + 1));
            }
            b'$' if matches!(delim, Delim::Dollar | Delim::Dollars) => {
                if delim == Delim::Dollars {
                    if b.get(j + 1) == Some(&b'$') {
                        if source[start..j].trim().is_empty() {
                            return None;
                        }
                        return Some(Region {
                            range: i..j + 2,
                            delim,
                            closed: true,
                        });
                    }
                    j += 1;
                    continue;
                }
                // A single dollar closes, or it was never math.
                return dollar_closes(b, start, j).then(|| Region {
                    range: i..j + 1,
                    delim,
                    closed: true,
                });
            }
            b'\n' => {
                if blank_line_after(b, j) {
                    *dead = j;
                    // A paragraph can't hold a formula past its end; if this is the end of the
                    // text, the formula is still being written.
                    let rest_blank = b[j..].iter().all(u8::is_ascii_whitespace);
                    return (rest_blank && delim != Delim::Dollar && limit == b.len())
                        .then(|| open_region(source, i, delim));
                }
                if inline {
                    let line = b[j + 1..].split(|&c| c == b'\n').next().unwrap_or(&[]);
                    if starts_block(line) {
                        return None;
                    }
                }
                j += 1;
            }
            _ => j += char_len(source, j),
        }
    }
    *dead = limit;
    (delim != Delim::Dollar && limit == b.len()).then(|| open_region(source, i, delim))
}

/// The formula `delim` opened at `i` and hasn't closed, to the end of the text.
fn open_region(source: &str, i: usize, delim: Delim) -> Region {
    let end = source.trim_end().len().max(i + delim.open().len());
    Region {
        range: i..end,
        delim,
        closed: false,
    }
}

/// The `$` at `j` closes the one before `start`: something between, no space before it, and no
/// digit after it ("$5 and $10"). A letter right after it is only allowed after a letter, digit
/// or bracket — `$n$th`, not `$PATH:$HOME`.
fn dollar_closes(b: &[u8], start: usize, j: usize) -> bool {
    if j == start {
        return false;
    }
    let before = b[j - 1];
    if before.is_ascii_whitespace() {
        return false;
    }
    match b.get(j + 1) {
        Some(c) if c.is_ascii_digit() => false,
        Some(c) if c.is_ascii_alphabetic() || *c == b'_' => {
            before.is_ascii_alphanumeric() || matches!(before, b'}' | b')' | b']' | b'\'')
        }
        _ => true,
    }
}

/// The fenced code blocks in `source`, as byte ranges; one never closed runs to the end.
fn fences(source: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut open: Option<(usize, u8, usize)> = None;
    let mut offset = 0;
    for line in source.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let body = fence_body(line);
        match open {
            None => {
                if let Some((c, n)) = fence_marker(body) {
                    // A backtick fence's info string has no backticks.
                    if c == b'`' && body[n..].contains('`') {
                        continue;
                    }
                    open = Some((start, c, n));
                }
            }
            Some((from, c, n)) => {
                if let Some((c2, n2)) = fence_marker(body)
                    && c2 == c
                    && n2 >= n
                    && body[n2..].trim().is_empty()
                {
                    out.push(from..offset);
                    open = None;
                }
            }
        }
    }
    if let Some((from, ..)) = open {
        out.push(from..source.len());
    }
    out
}

/// A line without its quote markers, list marker and indent.
fn fence_body(line: &str) -> &str {
    let mut s = line.trim_start();
    while let Some(rest) = s.strip_prefix('>') {
        s = rest.trim_start();
    }
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = s.strip_prefix(marker) {
            return rest.trim_start();
        }
    }
    let digits = s.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 && (s[digits..].starts_with(". ") || s[digits..].starts_with(") ")) {
        return s[digits + 2..].trim_start();
    }
    s
}

fn fence_marker(body: &str) -> Option<(u8, usize)> {
    let c = *body.as_bytes().first()?;
    if c != b'`' && c != b'~' {
        return None;
    }
    let n = run_of(body.as_bytes(), 0, c);
    (n >= 3).then_some((c, n))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The formulas in `s`, as written.
    fn found(s: &str) -> Vec<&str> {
        regions(s).into_iter().map(|r| &s[r.range]).collect()
    }

    #[test]
    fn finds_the_four_delimiters() {
        assert_eq!(found(r"Euler: $e^{i\pi}+1=0$."), [r"$e^{i\pi}+1=0$"]);
        assert_eq!(found(r"so \(a^2+b^2=c^2\) holds"), [r"\(a^2+b^2=c^2\)"]);
        assert_eq!(
            found("$$\n\\int_0^1 x\\,dx\n$$"),
            ["$$\n\\int_0^1 x\\,dx\n$$"]
        );
        assert_eq!(
            found("rule, \\[ P(\\theta=1\\mid T=+) \\approx \\boxed{0.0162}. \\]"),
            ["\\[ P(\\theta=1\\mid T=+) \\approx \\boxed{0.0162}. \\]"]
        );
        let r = regions(r"x \[a\] y $b$")[0].clone();
        assert!(r.delim.display() && r.closed);
        assert!(!regions(r"x $b$")[0].delim.display());
    }

    #[test]
    fn display_math_spans_lines_but_not_paragraphs() {
        assert_eq!(
            found("\\[\na = b \\\\\nc = d\n\\]"),
            ["\\[\na = b \\\\\nc = d\n\\]"]
        );
        assert!(found("\\[\na = b\n\nc = d\n\\]").is_empty());
        assert!(found("$$\na\n\nb\n$$ c\n\nmore").is_empty());
    }

    #[test]
    fn money_is_not_math() {
        for s in [
            "It costs $5 and $10.",
            "between $1,000 and $2,000",
            "$5-$10 range",
            "Pay $5, then $6 more",
            "I have $3",
            "$ 5 and 6 $",
            "US$5 vs US$6",
        ] {
            assert!(found(s).is_empty(), "{s}: {:?}", found(s));
        }
        // An escaped dollar is a dollar.
        assert!(found(r"\$5 and \$10 and \$x\$").is_empty());
        assert_eq!(found(r"\$5 but $x$"), ["$x$"]);
    }

    #[test]
    fn shell_is_not_math() {
        for s in [
            "echo $HOME and $PATH",
            "export PATH=$HOME/bin:$PATH",
            "run $(pwd) and $(date)",
            "use ${HOME} or ${PATH}",
            "cost: $FOO, $BAR",
        ] {
            assert!(found(s).is_empty(), "{s}: {:?}", found(s));
        }
        // But `$n$th` is math.
        assert_eq!(found("the $n$th term"), ["$n$"]);
    }

    #[test]
    fn code_is_never_math() {
        assert!(found("use `$x$` and `\\(y\\)`").is_empty());
        assert!(found("``a $x$ ` b``").is_empty());
        assert!(found("```sh\necho $A $B$\n\\[x\\]\n```").is_empty());
        assert!(found("~~~\n$x$\n~~~").is_empty());
        assert!(found("- item\n  ```\n  $x$\n  ```").is_empty());
        // An unclosed fence runs to the end.
        assert!(found("```\n$x$").is_empty());
        // Math after the code still counts; an unmatched backtick is just a backtick.
        assert_eq!(found("`$a$` then $b$"), ["$b$"]);
        assert_eq!(found("```\n$a$\n```\n$b$"), ["$b$"]);
        assert_eq!(found("a ` b $c$"), ["$c$"]);
    }

    #[test]
    fn escaped_backslashes_are_not_delimiters() {
        // `\\[` is an escaped backslash, then a bracket.
        assert!(found(r"a \\[b\\] c").is_empty());
        assert!(found(r"\\(x\\)").is_empty());
    }

    #[test]
    fn unclosed_math_at_the_end_is_still_being_written() {
        for (s, start) in [
            ("By Bayes, \\[ P(\\theta", 10),
            ("$$\n\\frac{a}", 0),
            ("so \\(x^", 3),
            ("a\n\n$$", 3),
        ] {
            let r = regions(s);
            assert_eq!(r.len(), 1, "{s}");
            assert!(
                !r[0].closed && r[0].range.start == start && r[0].range.end == s.len(),
                "{s}: {r:?}"
            );
        }
        // Single dollars never are; neither is an opener a paragraph break left behind.
        assert!(regions("costs $5").is_empty());
        assert!(regions("\\[ a\n\nmore text").is_empty());
        assert!(regions("$$ a\n\nmore").is_empty());
        // Once it closes, it's whole.
        assert!(regions("\\[ P(\\theta) \\]")[0].closed);
    }

    #[test]
    fn neighbouring_formulas_are_each_found() {
        let s = r"- Inline: \(e^{i\pi} + 1 = 0\), $\sqrt{2} \approx 1.4142$, $x_{n+1} = x_n - \frac{f(x_n)}{f'(x_n)}$, and $\hat\theta$, $\bar{x}$, $\vec{v}$, $\alpha \le \beta \ne \gamma$.";
        assert_eq!(
            found(s),
            [
                r"\(e^{i\pi} + 1 = 0\)",
                r"$\sqrt{2} \approx 1.4142$",
                r"$x_{n+1} = x_n - \frac{f(x_n)}{f'(x_n)}$",
                r"$\hat\theta$",
                r"$\bar{x}$",
                r"$\vec{v}$",
                r"$\alpha \le \beta \ne \gamma$"
            ]
        );
    }

    #[test]
    fn inline_math_stops_at_a_new_list_item() {
        assert!(found("- a $x\n- b y$").is_empty());
        assert_eq!(found("$a +\nb$"), ["$a +\nb$"]);
    }

    #[test]
    fn tex_is_between_the_delimiters() {
        let s = r"a \[ x^2 \] b $y$ c \(z\) $$w$$";
        let tex: Vec<String> = regions(s).iter().map(|r| r.tex(s)).collect();
        assert_eq!(tex, [" x^2 ", "y", "z", "w"]);
        let quoted = "> \\[\n> a \\\\\n> b\n> \\]";
        assert_eq!(regions(quoted)[0].tex(quoted), "\na \\\\\nb\n");
        let open = "x \\[ a + b";
        assert_eq!(regions(open)[0].tex(open), " a + b");
    }

    #[test]
    fn the_mask_keeps_length_and_hides_only_math() {
        let s =
            "Price $5. By Bayes, \\[ P(\\theta=1\\mid T=+) \\] and `\\(c\\)` and \\(x_1\\) — ü $α$";
        let m = mask(s).unwrap();
        assert_eq!(m.len(), s.len());
        assert!(m.starts_with("Price $5. By Bayes, $\u{1}"));
        assert!(m.contains("`\\(c\\)`"));
        assert!(!m.contains("\\["));
        assert!(m.ends_with("\u{1}$"));
        assert!(mask("no math here, $5 and $6").is_none());
    }

    #[test]
    fn whole_accepts_one_formula_only() {
        assert!(whole("$x$").is_some());
        assert!(whole(r"\[ a \]").is_some_and(|r| r.delim == Delim::Bracket));
        assert!(whole(r"\[ a + ").is_some_and(|r| !r.closed));
        assert!(whole("$x$ and $y$").is_none());
        assert!(whole("$5 and $").is_none());
    }

    #[test]
    fn never_panics_on_odd_text() {
        let pieces = [
            "$", "$$", "\\", "(", ")", "[", "]", "\\(", "\\[", "\\)", "\\]", "`", "```", "\n",
            "\n\n", " ", "x", "é", "日本", "5", "> ", "- ", "{", "}", "~~~", "\\$", "\r\n",
        ];
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        for _ in 0..4000 {
            let mut s = String::new();
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let len = (seed % 24) as usize;
            let mut x = seed;
            for _ in 0..len {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                s.push_str(pieces[(x % pieces.len() as u64) as usize]);
            }
            let found = regions(&s);
            for r in &found {
                assert!(
                    r.range.end <= s.len()
                        && s.is_char_boundary(r.range.start)
                        && s.is_char_boundary(r.range.end),
                    "{s:?}"
                );
                let _ = r.tex(&s);
            }
            if let Some(m) = mask(&s) {
                assert_eq!(m.len(), s.len(), "{s:?}");
            }
        }
    }

    #[test]
    fn unterminated_display_openers_are_not_rescanned() {
        let text = format!("{}\n\nafter \\[ b \\] done", "\\[ ".repeat(60_000));
        let start = std::time::Instant::now();
        let got = found(&text);
        assert!(start.elapsed().as_secs() < 3, "{:?}", start.elapsed());
        assert_eq!(got, ["\\[ b \\]"]);
        // An opener whose body is empty doesn't stop a later one in the same paragraph.
        assert_eq!(found("\\[\\] \\[ x \\]"), ["\\[ x \\]"]);
    }
}
