//! `edit_file`'s matching: the exact text first, then forgiving what models really get wrong about
//! a file (line endings, trailing spaces, indentation, typographic characters, line numbers copied
//! from `read_file`), and a failure that shows the closest text so the next try can be right. A
//! candidate that isn't unique at any level is refused: guessing wrong is worse than asking.

/// A longer line is cut when shown back: one minified line can fill a context by itself.
const MAX_SHOWN_LINE: usize = 2_000;

/// Where the old text matched, in byte offsets of the file. The ranges are in order and don't
/// overlap; each has the text to put there (indentation may have been shifted to fit).
#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    pub ranges: Vec<(usize, usize)>,
    pub replacements: Vec<String>,
    /// Which step of the ladder found it, when it wasn't the exact text.
    pub how: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Found(Match),
    /// Several places match. Each is a 1-based line number.
    Ambiguous {
        count: usize,
        lines: Vec<usize>,
    },
    /// Nowhere, and this is what to tell the model.
    NotFound(String),
}

/// Find `old` in `file`, at each rung of the ladder in turn.
pub fn locate(path: &str, file: &str, old: &str, new: &str, replace_all: bool) -> Outcome {
    let crlf_file = file.matches("\r\n").count() * 2 > file.matches('\n').count();
    // New lines in the file's own endings: a file shouldn't end up with two kinds.
    let in_file_endings = |text: &str| -> String {
        if crlf_file {
            text.replace("\r\n", "\n").replace('\n', "\r\n")
        } else {
            text.to_string()
        }
    };
    // The text as given, before anything is second-guessed: code can look like a copied line number.
    let exact = occurrences(file, old);
    if !exact.is_empty() {
        return settle(file, exact, in_file_endings(new), None, replace_all);
    }
    let stripped = strip_line_numbers(old);
    let numbered = matches!(stripped, std::borrow::Cow::Owned(_));
    let new_stripped = if numbered {
        strip_line_numbers(new)
    } else {
        new.into()
    };
    let (old, new) = (stripped.as_ref(), new_stripped.as_ref());
    if numbered {
        let exact = occurrences(file, old);
        if !exact.is_empty() {
            return settle(file, exact, in_file_endings(new), None, replace_all);
        }
    }
    let lines = line_spans(file);
    let want: Vec<&str> = old.split('\n').map(|l| l.trim_end_matches('\r')).collect();
    let want: &[&str] = match want.split_last() {
        Some((last, rest)) if last.is_empty() && !rest.is_empty() => rest,
        _ => &want,
    };
    // Only spaces and line ends match every blank line in the file, which is never what's meant.
    if want.iter().all(|l| l.trim().is_empty()) {
        return Outcome::NotFound(not_found_message(path, file, want, new));
    }
    // The file's own line endings, whichever the model wrote: it was shown LF.
    if file.contains("\r\n") {
        let old_crlf = old.replace("\r\n", "\n").replace('\n', "\r\n");
        let found = occurrences(file, &old_crlf);
        if !found.is_empty() {
            return settle(
                file,
                found,
                in_file_endings(new),
                Some("line endings"),
                replace_all,
            );
        }
    }

    // Whole lines that compare the same, by `same`. They don't overlap: a window taken once is
    // gone, as `str::matches` does for exact text.
    let rung = |same: &dyn Fn(&str, &str) -> bool| -> Vec<(usize, usize)> {
        let mut found = Vec::new();
        let mut at = 0;
        while at + want.len() <= lines.len() {
            if want
                .iter()
                .enumerate()
                .all(|(i, w)| same(line_text(file, lines[at + i]), w))
            {
                found.push((at, at + want.len()));
                at += want.len();
            } else {
                at += 1;
            }
        }
        found
    };
    let whole = |found: &[(usize, usize)]| -> Vec<(usize, usize)> {
        found
            .iter()
            .map(|(first, end)| (lines[*first].0, lines[*end - 1].1))
            .collect()
    };
    let as_file = |text: &str| -> String { in_file_endings(text.trim_end_matches('\n')) };
    // Each match gets the new text at its own indentation.
    let reindented = |found: &[(usize, usize)]| -> Vec<String> {
        let said = indent_of(want.first().copied().unwrap_or(""));
        found
            .iter()
            .map(|(first, _)| {
                let have = indent_of(line_text(file, lines[*first]));
                as_file(&reindent(new, said, have))
            })
            .collect()
    };

    let found = rung(&|a, b| a.trim_end() == b.trim_end());
    if !found.is_empty() {
        return settle(
            file,
            whole(&found),
            as_file(new),
            Some("trailing whitespace"),
            replace_all,
        );
    }

    // Indentation: the same lines at another depth, or in tabs for spaces.
    let found = rung(&|a, b| a.trim() == b.trim());
    if !found.is_empty() {
        return settle_each(
            file,
            whole(&found),
            reindented(&found),
            Some("indentation"),
            replace_all,
        );
    }

    // Curly quotes, long dashes, odd spaces.
    let found = rung(&|a, b| unify(a.trim()) == unify(b.trim()));
    if !found.is_empty() {
        return settle_each(
            file,
            whole(&found),
            reindented(&found),
            Some("typographic characters"),
            replace_all,
        );
    }

    // Any run of whitespace for any other, whole lines only.
    let found = rung(&|a, b| collapse(a) == collapse(b));
    if !found.is_empty() {
        return settle(
            file,
            whole(&found),
            as_file(new),
            Some("whitespace"),
            replace_all,
        );
    }

    Outcome::NotFound(not_found_message(path, file, want, new))
}

fn settle(
    file: &str,
    ranges: Vec<(usize, usize)>,
    replacement: String,
    how: Option<&'static str>,
    replace_all: bool,
) -> Outcome {
    let replacements = vec![replacement; ranges.len()];
    settle_each(file, ranges, replacements, how, replace_all)
}

fn settle_each(
    file: &str,
    ranges: Vec<(usize, usize)>,
    replacements: Vec<String>,
    how: Option<&'static str>,
    replace_all: bool,
) -> Outcome {
    if ranges.len() > 1 && !replace_all {
        let lines = ranges
            .iter()
            .take(8)
            .map(|(start, _)| file[..*start].matches('\n').count() + 1)
            .collect();
        return Outcome::Ambiguous {
            count: ranges.len(),
            lines,
        };
    }
    Outcome::Found(Match {
        ranges,
        replacements,
        how,
    })
}

/// `file` with `m` applied.
pub fn apply(file: &str, m: &Match) -> String {
    let mut out = String::with_capacity(file.len());
    let mut at = 0;
    for ((start, end), replacement) in m.ranges.iter().zip(&m.replacements) {
        out.push_str(&file[at..*start]);
        out.push_str(replacement);
        at = *end;
    }
    out.push_str(&file[at..]);
    out
}

fn clip_line(line: &str) -> std::borrow::Cow<'_, str> {
    if line.len() <= MAX_SHOWN_LINE {
        return line.into();
    }
    let mut end = MAX_SHOWN_LINE;
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    format!("{} …", &line[..end]).into()
}

/// What the model sees after an edit: where it went, and the lines around it to check it by.
pub fn snippet(updated: &str, start_byte: usize, new_len: usize) -> String {
    let start_line = updated[..start_byte.min(updated.len())]
        .matches('\n')
        .count();
    let new_lines = updated
        .get(start_byte..start_byte + new_len)
        .map_or(1, |s| s.matches('\n').count() + 1);
    let lines: Vec<&str> = updated.lines().collect();
    let from = start_line.saturating_sub(2);
    let to = (start_line + new_lines + 2).min(lines.len());
    let mut out = String::new();
    // A huge replacement isn't worth repeating back: the model just wrote it.
    if to - from > 24 {
        for (i, line) in lines.iter().enumerate().take(from + 10).skip(from) {
            out.push_str(&format!("{:4} | {}\n", i + 1, clip_line(line)));
        }
        out.push_str(&format!("     | … {} more lines …\n", to - from - 16));
        for (i, line) in lines.iter().enumerate().take(to).skip(to - 6) {
            out.push_str(&format!("{:4} | {}\n", i + 1, clip_line(line)));
        }
        return out;
    }
    for (i, line) in lines.iter().enumerate().take(to).skip(from) {
        out.push_str(&format!("{:4} | {}\n", i + 1, clip_line(line)));
    }
    out
}

fn occurrences(file: &str, needle: &str) -> Vec<(usize, usize)> {
    if needle.is_empty() {
        return Vec::new();
    }
    file.match_indices(needle)
        .map(|(i, m)| (i, i + m.len()))
        .collect()
}

/// `read_file`'s line numbers, copied along with the text. Only in `read_file`'s own shape (padded
/// to four, then ` | `) and only when every line has one, so a match arm like `1 | 2 => x` is safe.
fn strip_line_numbers(text: &str) -> std::borrow::Cow<'_, str> {
    let numbered = |line: &str| -> Option<usize> {
        let padding = line.len() - line.trim_start_matches(' ').len();
        let rest = &line[padding..];
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        if digits == 0 || padding + digits < 4 {
            return None;
        }
        let after = rest[digits..].strip_prefix(" |")?;
        Some(line.len() - after.strip_prefix(' ').unwrap_or(after).len())
    };
    let lines: Vec<&str> = text.split('\n').collect();
    let body: &[&str] = match lines.split_last() {
        Some((last, rest)) if last.is_empty() => rest,
        _ => &lines,
    };
    if body.is_empty() || !body.iter().all(|l| numbered(l).is_some()) {
        return text.into();
    }
    let mut out: Vec<&str> = body.iter().map(|l| &l[numbered(l).unwrap()..]).collect();
    if body.len() != lines.len() {
        out.push("");
    }
    out.join("\n").into()
}

/// Each line's byte span, without its line ending.
fn line_spans(file: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = 0;
    for (i, _) in file.match_indices('\n') {
        let end = if i > start && file.as_bytes()[i - 1] == b'\r' {
            i - 1
        } else {
            i
        };
        spans.push((start, end));
        start = i + 1;
    }
    if start < file.len() {
        spans.push((start, file.len()));
    }
    spans
}

fn line_text(file: &str, span: (usize, usize)) -> &str {
    &file[span.0..span.1]
}

fn indent_of(line: &str) -> &str {
    &line[..line.len() - line.trim_start().len()]
}

/// `text` with the model's indent `said` swapped for the file's `have` on each line. A line that
/// doesn't start with `said` is less indented than the first was, and takes `have` alone.
fn reindent(text: &str, said: &str, have: &str) -> String {
    text.split('\n')
        .map(|line| {
            if line.trim().is_empty() {
                line.to_string()
            } else if let Some(rest) = line.strip_prefix(said) {
                format!("{have}{rest}")
            } else {
                format!("{have}{}", line.trim_start())
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn unify(line: &str) -> String {
    line.chars()
        .map(|c| match c {
            '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
            '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}' => '\'',
            '\u{201c}' | '\u{201d}' | '\u{201e}' | '\u{201f}' => '"',
            '\u{00a0}' | '\u{2002}'..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}' => ' ',
            c => c,
        })
        .collect()
}

fn collapse(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The model's old text isn't in the file. It gets the likeliest place to compare with (the most
/// lines in common), so the next try can be right without another read.
fn not_found_message(path: &str, file: &str, want: &[&str], new: &str) -> String {
    let mut message = format!("Error: old_str was not found in {path}.");
    if !new.trim().is_empty() && file.contains(new.trim()) {
        message.push_str(" new_str is already in the file, so this edit may already be applied.");
    }
    let lines: Vec<&str> = file.lines().collect();
    let wanted: Vec<&str> = want
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();
    let mut best: Option<(usize, usize)> = None;
    if !wanted.is_empty() {
        for at in 0..lines.len() {
            let mut score = 0;
            for i in 0..want.len().min(lines.len() - at) {
                let line = lines[at + i].trim();
                if !line.is_empty() && wanted.contains(&line) {
                    score += 1;
                }
            }
            if score > 0 && best.is_none_or(|(_, s)| score > s) {
                best = Some((at, score));
            }
        }
    }
    match best {
        Some((at, score)) => {
            let from = at.saturating_sub(2);
            let to = (at + want.len() + 2).min(lines.len());
            message.push_str(&format!(
                " The closest text ({score} of {} lines the same) is here:\n",
                wanted.len()
            ));
            for (i, line) in lines.iter().enumerate().take(to).skip(from) {
                message.push_str(&format!("{:4} | {}\n", i + 1, clip_line(line)));
            }
            message.push_str("Copy old_str from these lines exactly, without the line numbers.");
        }
        None => message.push_str(
            " No line of it is in the file: read the file again, since it may differ from what you remember.",
        ),
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edited(file: &str, old: &str, new: &str) -> (String, Option<&'static str>) {
        match locate("f", file, old, new, false) {
            Outcome::Found(m) => (apply(file, &m), m.how),
            other => panic!("not found: {other:?}"),
        }
    }

    #[test]
    fn exact_text_is_replaced_as_it_is() {
        assert_eq!(
            edited("a\nb\nc\n", "b", "x"),
            ("a\nx\nc\n".to_string(), None)
        );
    }

    #[test]
    fn several_exact_matches_are_refused_with_their_lines() {
        assert_eq!(
            locate("f", "x\ny\nx\n", "x", "z", false),
            Outcome::Ambiguous {
                count: 2,
                lines: vec![1, 3]
            }
        );
    }

    #[test]
    fn replace_all_takes_every_match() {
        let Outcome::Found(m) = locate("f", "x y x\n", "x", "z", true) else {
            panic!()
        };
        assert_eq!(apply("x y x\n", &m), "z y z\n");
    }

    #[test]
    fn crlf_files_match_text_written_with_lf() {
        assert_eq!(
            edited("a\r\nb\r\nc\r\n", "a\nb\n", "x\ny\n"),
            ("x\r\ny\r\nc\r\n".to_string(), Some("line endings"))
        );
    }

    #[test]
    fn trailing_spaces_are_forgiven() {
        assert_eq!(
            edited("a  \nb\t\nc\n", "a\nb", "x\ny"),
            ("x\ny\nc\n".to_string(), Some("trailing whitespace"))
        );
    }

    #[test]
    fn indentation_moves_with_the_replacement() {
        let file = "fn f() {\n        let a = 1;\n        let b = 2;\n}\n";
        let (out, how) = edited(
            file,
            "let a = 1;\nlet b = 2;",
            "let a = 3;\nif a {\n    b();\n}",
        );
        assert_eq!(how, Some("indentation"));
        assert_eq!(
            out,
            "fn f() {\n        let a = 3;\n        if a {\n            b();\n        }\n}\n"
        );
    }

    #[test]
    fn too_much_indentation_comes_off_too() {
        let file = "a:\n  b: 1\n";
        let (out, _) = edited(file, "      b: 1", "      b: 2\n      c: 3");
        assert_eq!(out, "a:\n  b: 2\n  c: 3\n");
    }

    #[test]
    fn typographic_characters_are_forgiven() {
        let file = "say \u{201c}hi\u{201d} \u{2014} now\n";
        let (out, how) = edited(file, "say \"hi\" - now", "say hello");
        assert_eq!(how, Some("typographic characters"));
        assert_eq!(out, "say hello\n");
    }

    #[test]
    fn inner_whitespace_is_forgiven_on_whole_lines() {
        let file = "let   x  = 1;\n";
        let (out, how) = edited(file, "let x = 1;", "let x = 2;");
        assert_eq!(how, Some("whitespace"));
        assert_eq!(out, "let x = 2;\n");
    }

    #[test]
    fn line_numbers_copied_from_read_file_come_off() {
        assert_eq!(
            edited("a\nb\n", "   1 | a\n   2 | b\n", "   1 | x\n   2 | y\n"),
            ("x\ny\n".to_string(), None)
        );
        // Only when every line has one.
        assert_eq!(edited("12 | a\nb\n", "12 | a\nb", "x").0, "x\n");
    }

    #[test]
    fn a_loose_match_in_several_places_is_refused() {
        let file = "  a\n    a\n";
        assert!(matches!(
            locate("f", file, "a ", "b", false),
            Outcome::Ambiguous { count: 2, .. }
        ));
    }

    #[test]
    fn not_found_shows_the_closest_lines() {
        let file = "one\ntwo\nthree\nfour\nfive\n";
        let Outcome::NotFound(message) = locate("f", file, "two\nthree\nFOUR", "x", false) else {
            panic!()
        };
        assert!(message.contains("2 of 3 lines"), "{message}");
        assert!(message.contains("   3 | three"), "{message}");
        assert!(message.contains("Copy old_str"), "{message}");
    }

    #[test]
    fn not_found_says_when_the_edit_looks_applied() {
        let Outcome::NotFound(message) = locate("f", "new line\n", "old line", "new line", false)
        else {
            panic!()
        };
        assert!(message.contains("already be applied"), "{message}");
    }

    #[test]
    fn nothing_in_common_says_to_read_again() {
        let Outcome::NotFound(message) = locate("f", "a\n", "zzz", "y", false) else {
            panic!()
        };
        assert!(message.contains("read the file again"), "{message}");
    }

    #[test]
    fn code_that_looks_like_a_line_number_is_left_alone() {
        // A Rust match arm, not `read_file`'s output.
        assert_eq!(
            edited(
                "0 | 1 | 2 => 4,\n",
                "0 | 1 | 2 => 4,",
                "0 | 1 | 2 | 3 => 4,"
            )
            .0,
            "0 | 1 | 2 | 3 => 4,\n"
        );
        assert_eq!(edited("1 | x\n", "1 | x", "2 | x").0, "2 | x\n");
    }

    #[test]
    fn overlapping_loose_matches_cant_overlap() {
        let file = "  x\n  x\n  x\n";
        let Outcome::Found(m) = locate("f", file, "x\nx", "y", true) else {
            panic!()
        };
        assert_eq!(m.ranges.len(), 1);
        assert_eq!(apply(file, &m), "  y\n  x\n");
    }

    #[test]
    fn tabs_and_each_matchs_own_indent_are_kept() {
        assert_eq!(
            edited("\tfoo();\n", "    foo();", "    bar();").0,
            "\tbar();\n"
        );
        let file = "a:\n  x: 1\nb:\n      x: 1\n";
        let Outcome::Found(m) = locate("f", file, "x: 1 ", "x: 2", true) else {
            panic!()
        };
        assert_eq!(apply(file, &m), "a:\n  x: 2\nb:\n      x: 2\n");
    }

    #[test]
    fn exact_matches_in_a_crlf_file_keep_its_endings() {
        assert_eq!(edited("a\r\nb\r\n", "a", "a\nc").0, "a\r\nc\r\nb\r\n");
        // A file mostly in LF stays that way.
        assert_eq!(edited("a\nb\nc\r\n", "a", "a\nc").0, "a\nc\nb\nc\r\n");
    }

    #[test]
    fn only_whitespace_never_matches_blank_lines() {
        assert!(matches!(
            locate("f", "a\n\nb\n", "   ", "x", true),
            Outcome::NotFound(_)
        ));
    }

    #[test]
    fn a_very_long_line_is_cut_when_shown_back() {
        let long = "x".repeat(50_000);
        let updated = format!("{long}\nnew\n");
        let out = snippet(&updated, long.len() + 1, 3);
        assert!(out.len() < MAX_SHOWN_LINE + 100, "{}", out.len());
        let Outcome::NotFound(message) = locate(
            "f",
            &format!("{long}\n"),
            &format!("{long}\nmore"),
            "y",
            false,
        ) else {
            panic!()
        };
        assert!(message.len() < MAX_SHOWN_LINE + 300, "{}", message.len());
    }

    #[test]
    fn the_snippet_shows_the_new_lines_in_place() {
        let updated = "1\n2\n3\nNEW\n5\n6\n7\n8\n";
        let start = updated.find("NEW").unwrap();
        let out = snippet(updated, start, 3);
        assert!(out.contains("   4 | NEW"), "{out}");
        assert!(out.contains("   2 | 2"), "{out}");
        assert!(out.contains("   6 | 6"), "{out}");
        assert!(!out.contains("   8 | 8"), "{out}");
    }

    #[test]
    fn a_long_replacement_is_not_repeated_in_full() {
        let new: String = (0..100).map(|i| format!("line{i}\n")).collect();
        let out = snippet(&new, 0, new.len());
        assert!(out.lines().count() < 20, "{out}");
        assert!(out.contains("more lines"), "{out}");
    }
}
