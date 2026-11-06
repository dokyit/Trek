//! A command line as a shell would see it, for permission rules: which programs it would run,
//! whatever the quoting. `rm -rf x; echo y`, `FOO=1 rm x`, `\rm x`, `$(rm x)` and
//! `sh -c 'rm x'` all run `rm`.
//!
//! This isn't a shell. It reads the shapes that decide what runs (quotes, escapes, separators,
//! substitutions, redirects, here-documents, the programs that run other programs) and says
//! `opaque` about anything it can't follow, so a rule can fail closed: an opaque line counts as
//! matching a deny and as not matching an allow. Windows's `cmd` and PowerShell lines go through
//! the same reading; what they do differently (PowerShell's backtick escape, say) reads as
//! substitution or as unclosed, which fails closed too.

/// How far nested commands are followed (`sh -c "sh -c '…'"`) before the line counts as opaque.
const MAX_DEPTH: usize = 8;

/// How many brackets may stand open at once.
const MAX_BRACKETS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordKind {
    /// Exactly this text.
    Fixed,
    /// Has an unquoted glob or brace pattern: the shell turns it into other words.
    Glob,
    /// Holds something only known when it runs (`$X`, `$(…)`), which may split into several words.
    Dynamic,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    pub text: String,
    pub kind: WordKind,
    /// As written, where that held a backslash: on Windows `C:\\bin\\rm.exe` is a path, not escapes.
    pub raw: Option<String>,
}

impl Word {
    pub(super) fn fixed(text: impl Into<String>) -> Word {
        Word {
            text: text.into(),
            kind: WordKind::Fixed,
            raw: None,
        }
    }

    fn dynamic(text: impl Into<String>) -> Word {
        Word {
            text: text.into(),
            kind: WordKind::Dynamic,
            raw: None,
        }
    }
}

/// One simple command of a line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Part {
    /// `FOO=1` before the command.
    pub assignments: Vec<Word>,
    pub words: Vec<Word>,
    /// Its output goes to a file (`> x`), not just the terminal or `/dev/null`.
    pub writes: bool,
    /// A shell that only runs what's in a string (`sh -c '…'`, `eval '…'`), whose contents are
    /// parts of their own: an allow needs to cover those, not this.
    pub transparent: bool,
    /// Words after this index may be a command it runs (`sudo`, `xargs`, `timeout 5`). Where the
    /// wrapped command starts isn't known without knowing each tool's flags, so a deny tries each.
    pub wraps_from: Option<usize>,
    /// Code it runs that Trek can't read as a command line (`python -c '…'`): a deny looks for
    /// its words anywhere in it.
    pub code: Option<String>,
    /// Text its standard input gets, from a here-document or here-string.
    stdin: Option<Word>,
    /// The here-document that will fill `stdin`, while the parser hasn't reached it.
    heredoc: Option<usize>,
}

impl Part {
    /// The command as words, for showing what a rule caught.
    pub fn display(&self) -> String {
        self.assignments
            .iter()
            .chain(self.words.iter())
            .map(|w| w.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The program's own name: `/bin/RM.exe` is `rm` (macOS and Windows find programs whatever
    /// their case).
    pub fn program(&self) -> Option<String> {
        self.words.first().map(|w| program_name(&w.text))
    }
}

/// A line's commands, including the ones inside it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Line {
    pub parts: Vec<Part>,
    /// Why some of it can't be read (an unclosed quote, a shell fed from a pipe). `None`: all of
    /// it could.
    pub opaque: Option<&'static str>,
}

/// `/usr/bin/GIT.EXE` → `git`.
pub fn program_name(word: &str) -> String {
    let base = word.rsplit(['/', '\\']).next().unwrap_or(word);
    let lower = base.to_lowercase();
    for ext in [".exe", ".cmd", ".bat", ".com", ".ps1"] {
        if let Some(stem) = lower.strip_suffix(ext) {
            return stem.to_string();
        }
    }
    lower
}

pub fn parse(line: &str) -> Line {
    parse_at(line, 0)
}

fn parse_at(line: &str, depth: usize) -> Line {
    let mut out = Line::default();
    if depth > MAX_DEPTH {
        out.opaque = Some("It nests commands too deeply to follow.");
        return out;
    }
    let (parts, opaque) = read_all(line, depth);
    out.opaque = opaque;
    for part in parts {
        follow(part, &mut out, depth);
    }
    out
}

/// A line's simple commands, as written (what they run inside them is `follow`'s).
fn read_all(line: &str, depth: usize) -> (Vec<Part>, Option<&'static str>) {
    let chars: Vec<char> = line.chars().collect();
    let mut p = Parser {
        chars: &chars,
        pos: 0,
        depth,
        nesting: 0,
        parts: vec![],
        opaque: None,
        pending: vec![],
        bodies: vec![],
    };
    p.list(None);
    if !p.pending.is_empty() {
        p.fail("A here-document never ends.");
    }
    let bodies = std::mem::take(&mut p.bodies);
    let mut parts = std::mem::take(&mut p.parts);
    for part in &mut parts {
        if let Some(body) = part.heredoc.take().and_then(|i| bodies.get(i).cloned()) {
            part.stdin = body;
        }
    }
    (parts, p.opaque)
}

struct Parser<'a> {
    chars: &'a [char],
    pos: usize,
    depth: usize,
    /// Brackets open right now: a line built to run the stack out reads as opaque instead.
    nesting: usize,
    parts: Vec<Part>,
    opaque: Option<&'static str>,
    /// Here-documents waiting for the end of their line: (delimiter, strip tabs, quoted, slot).
    pending: Vec<(String, bool, bool, usize)>,
    bodies: Vec<Option<Word>>,
}

/// Words that open or join a compound command rather than name a program.
const RESERVED: &[&str] = &[
    "if", "then", "elif", "else", "fi", "do", "done", "while", "until", "esac", "!", "time",
    "coproc",
];

impl Parser<'_> {
    fn peek(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.pos + ahead).copied()
    }

    fn fail(&mut self, why: &'static str) {
        self.opaque.get_or_insert(why);
    }

    /// Commands, up to `close` (`$(…)`, a subshell) or the end.
    fn list(&mut self, close: Option<char>) {
        if self.nesting >= MAX_BRACKETS {
            self.fail("It nests commands too deeply to follow.");
            self.pos = self.chars.len();
            return;
        }
        self.nesting += 1;
        self.list_inside(close);
        self.nesting -= 1;
    }

    fn list_inside(&mut self, close: Option<char>) {
        let mut cur = Part::default();
        loop {
            self.skip_blanks();
            let Some(c) = self.peek(0) else {
                if close.is_some() {
                    self.fail("A bracket never closes.");
                }
                self.finish(&mut cur);
                return;
            };
            match c {
                '#' => {
                    while self.peek(0).is_some_and(|c| c != '\n') {
                        self.pos += 1;
                    }
                }
                '\n' | '\r' => {
                    self.finish(&mut cur);
                    self.pos += 1;
                    self.read_heredocs();
                }
                ';' => {
                    self.finish(&mut cur);
                    while self.peek(0).is_some_and(|c| c == ';' || c == '&') {
                        self.pos += 1;
                    }
                }
                '&' if self.peek(1) == Some('>') => self.redirect(&mut cur),
                '&' | '|' => {
                    self.finish(&mut cur);
                    while self.peek(0).is_some_and(|c| c == '&' || c == '|') {
                        self.pos += 1;
                    }
                }
                '(' => {
                    self.finish(&mut cur);
                    self.pos += 1;
                    self.list(Some(')'));
                }
                ')' => {
                    self.finish(&mut cur);
                    self.pos += 1;
                    if close == Some(')') {
                        return;
                    }
                }
                '<' | '>' if self.peek(1) == Some('(') => {
                    self.pos += 2;
                    self.list(Some(')'));
                    cur.words.push(Word::dynamic(format!("{c}(…)")));
                }
                '<' | '>' => self.redirect(&mut cur),
                _ if self.starts_with_fd() => self.redirect(&mut cur),
                _ => {
                    let word = self.word();
                    self.take_word(word, &mut cur);
                }
            }
        }
    }

    fn take_word(&mut self, word: Word, cur: &mut Part) {
        let bare = word.kind == WordKind::Fixed;
        // `{ a; }` and `function f { a; }`: the brace starts a new command wherever it stands.
        if bare && (word.text == "{" || word.text == "}") {
            self.finish(cur);
            return;
        }
        if cur.words.is_empty() {
            if is_assignment(&word.text) {
                cur.assignments.push(word);
                return;
            }
            if bare && RESERVED.contains(&word.text.as_str()) {
                return;
            }
            // `for x in a b`, up to its `do`: names and values, no command.
            if bare && (word.text == "for" || word.text == "select") {
                while let Some(c) = self.peek(0) {
                    if c == ';' || c == '\n' {
                        break;
                    }
                    if c == '$' || c == '`' {
                        let _ = self.word();
                        continue;
                    }
                    self.pos += 1;
                }
                return;
            }
        }
        cur.words.push(word);
    }

    fn finish(&mut self, cur: &mut Part) {
        let done = std::mem::take(cur);
        if !done.words.is_empty() || !done.assignments.is_empty() {
            self.parts.push(done);
        }
    }

    fn skip_blanks(&mut self) {
        while let Some(c) = self.peek(0) {
            if c == ' ' || c == '\t' {
                self.pos += 1;
            } else if c == '\\' && self.peek(1) == Some('\n') {
                self.pos += 2;
            } else {
                break;
            }
        }
    }

    /// `2>` and friends: digits right before a redirect, at a word's start.
    fn starts_with_fd(&self) -> bool {
        let mut i = 0;
        while self.peek(i).is_some_and(|c| c.is_ascii_digit()) {
            i += 1;
        }
        i > 0 && matches!(self.peek(i), Some('<' | '>'))
    }

    fn redirect(&mut self, cur: &mut Part) {
        while self.peek(0).is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        let mut op = String::new();
        while let Some(c) = self.peek(0).filter(|c| matches!(c, '<' | '>' | '&' | '|')) {
            op.push(c);
            self.pos += 1;
        }
        if op == "<<" {
            let strip = self.peek(0) == Some('-');
            self.pos += usize::from(strip);
            return self.heredoc(strip, cur);
        }
        // `2>&-` closes a descriptor.
        if op.ends_with('&') && self.peek(0) == Some('-') {
            self.pos += 1;
            return;
        }
        self.skip_blanks();
        // `>&2`, `2>&1`: another descriptor, no file.
        let duplicate = op.ends_with('&') && op != "&>";
        let target = self.word();
        let writes_file = op.contains('>')
            && !(duplicate && target.text.chars().all(|c| c.is_ascii_digit()))
            && target.text != "/dev/null";
        if target.text.is_empty() {
            self.fail("A redirect has nowhere to go.");
        }
        cur.writes |= writes_file;
        if op == "<<<" {
            cur.stdin = Some(target);
        }
    }

    fn heredoc(&mut self, strip: bool, cur: &mut Part) {
        self.skip_blanks();
        let start = self.pos;
        let word = self.word();
        let quoted = self.chars[start..self.pos]
            .iter()
            .any(|c| matches!(c, '\'' | '"' | '\\'));
        let slot = self.bodies.len();
        self.bodies.push(None);
        cur.heredoc = Some(slot);
        self.pending.push((word.text, strip, quoted, slot));
    }

    /// The bodies of the here-documents this line opened.
    fn read_heredocs(&mut self) {
        for (delim, strip, quoted, slot) in std::mem::take(&mut self.pending) {
            let mut body = String::new();
            let mut closed = false;
            loop {
                if self.peek(0).is_none() {
                    break;
                }
                let mut line = String::new();
                while let Some(c) = self.peek(0) {
                    self.pos += 1;
                    if c == '\n' {
                        break;
                    }
                    line.push(c);
                }
                let test = if strip {
                    line.trim_start_matches('\t')
                } else {
                    line.as_str()
                };
                if test == delim {
                    closed = true;
                    break;
                }
                body.push_str(&line);
                body.push('\n');
            }
            if !closed {
                self.fail("A here-document never ends.");
            }
            // Unquoted, the shell expands it first, and what it runs is only known then.
            let dynamic = !quoted && (body.contains('$') || body.contains('`'));
            self.bodies[slot] = Some(if dynamic {
                Word::dynamic(body)
            } else {
                Word::fixed(body)
            });
        }
    }

    /// One word, quotes and escapes gone.
    fn word(&mut self) -> Word {
        let start = self.pos;
        let mut text = String::new();
        let mut kind = WordKind::Fixed;
        let raise = |kind: &mut WordKind, to: WordKind| {
            if *kind != WordKind::Dynamic {
                *kind = to;
            }
        };
        while let Some(c) = self.peek(0) {
            match c {
                ' ' | '\t' | '\n' | '\r' | ';' | '&' | '|' | '(' | ')' | '<' | '>' => break,
                '\\' => match self.peek(1) {
                    Some('\n') => self.pos += 2,
                    Some(next) => {
                        text.push(next);
                        self.pos += 2;
                    }
                    None => {
                        self.fail("A backslash has nothing after it.");
                        self.pos += 1;
                    }
                },
                '\'' => {
                    self.pos += 1;
                    let mut closed = false;
                    while let Some(c) = self.peek(0) {
                        self.pos += 1;
                        if c == '\'' {
                            closed = true;
                            break;
                        }
                        text.push(c);
                    }
                    if !closed {
                        self.fail("A quote never closes.");
                    }
                }
                '"' => {
                    self.pos += 1;
                    let mut closed = false;
                    while let Some(c) = self.peek(0) {
                        match c {
                            '"' => {
                                self.pos += 1;
                                closed = true;
                                break;
                            }
                            '\\' => {
                                match self.peek(1) {
                                    Some('\n') => {}
                                    Some(next @ ('"' | '\\' | '$' | '`')) => text.push(next),
                                    Some(next) => {
                                        text.push('\\');
                                        text.push(next);
                                    }
                                    None => self.fail("A quote never closes."),
                                }
                                self.pos += 2;
                            }
                            '$' => {
                                if self.dollar(&mut text) {
                                    kind = WordKind::Dynamic;
                                }
                            }
                            '`' => {
                                self.backtick(&mut text);
                                kind = WordKind::Dynamic;
                            }
                            other => {
                                text.push(other);
                                self.pos += 1;
                            }
                        }
                    }
                    if !closed {
                        self.fail("A quote never closes.");
                    }
                }
                '$' => {
                    if self.dollar(&mut text) {
                        kind = WordKind::Dynamic;
                    }
                }
                '`' => {
                    self.backtick(&mut text);
                    kind = WordKind::Dynamic;
                }
                '*' | '?' | '[' => {
                    raise(&mut kind, WordKind::Glob);
                    text.push(c);
                    self.pos += 1;
                }
                '{' if self.is_brace_expansion() => {
                    raise(&mut kind, WordKind::Glob);
                    text.push(c);
                    self.pos += 1;
                }
                other => {
                    text.push(other);
                    self.pos += 1;
                }
            }
        }
        let written = &self.chars[start..self.pos];
        let raw = written
            .contains(&'\\')
            .then(|| written.iter().collect::<String>());
        Word { text, kind, raw }
    }

    /// `{a,b}` or `{1..3}`: the shell makes words of it. (`{` alone is just a brace.)
    fn is_brace_expansion(&self) -> bool {
        let mut i = 1;
        let mut found = false;
        while let Some(c) = self.peek(i) {
            match c {
                '}' => return found,
                ',' => found = true,
                '.' if self.peek(i + 1) == Some('.') => found = true,
                ' ' | '\t' | '\n' | ';' | '|' | '&' => return false,
                _ => {}
            }
            i += 1;
        }
        false
    }

    /// `$…` at the cursor. Commands inside become parts; `true`: its value isn't known.
    fn dollar(&mut self, text: &mut String) -> bool {
        match self.peek(1) {
            Some('(') if self.peek(2) == Some('(') => {
                // Arithmetic, which can hold substitutions of its own.
                self.pos += 3;
                text.push_str("$((…))");
                let mut depth = 2;
                while let Some(c) = self.peek(0) {
                    match c {
                        '$' if self.peek(1) == Some('(') => {
                            let mut inner = String::new();
                            self.dollar(&mut inner);
                            continue;
                        }
                        '`' => {
                            let mut inner = String::new();
                            self.backtick(&mut inner);
                            continue;
                        }
                        '(' => depth += 1,
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                self.pos += 1;
                                return true;
                            }
                        }
                        _ => {}
                    }
                    self.pos += 1;
                }
                self.fail("A bracket never closes.");
                true
            }
            Some('(') => {
                self.pos += 2;
                text.push_str("$(…)");
                self.list(Some(')'));
                true
            }
            Some('{') => {
                self.pos += 2;
                text.push_str("${…}");
                let mut depth = 1;
                while let Some(c) = self.peek(0) {
                    match c {
                        '$' if self.peek(1) == Some('(') => {
                            let mut inner = String::new();
                            self.dollar(&mut inner);
                            continue;
                        }
                        '`' => {
                            let mut inner = String::new();
                            self.backtick(&mut inner);
                            continue;
                        }
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                self.pos += 1;
                                return true;
                            }
                        }
                        _ => {}
                    }
                    self.pos += 1;
                }
                self.fail("A bracket never closes.");
                true
            }
            // `$'\x72m'` spells any text at all, so it reads as unknown.
            Some('\'') => {
                self.pos += 2;
                text.push_str("$'…'");
                let mut closed = false;
                while let Some(c) = self.peek(0) {
                    self.pos += 1;
                    if c == '\\' {
                        self.pos += 1;
                    } else if c == '\'' {
                        closed = true;
                        break;
                    }
                }
                if !closed {
                    self.fail("A quote never closes.");
                }
                true
            }
            Some('"') => {
                self.pos += 1;
                true
            }
            Some(c) if c.is_alphanumeric() || c == '_' => {
                text.push('$');
                self.pos += 1;
                while let Some(c) = self.peek(0).filter(|c| c.is_alphanumeric() || *c == '_') {
                    text.push(c);
                    self.pos += 1;
                }
                true
            }
            Some('?' | '$' | '!' | '#' | '*' | '@' | '-') => {
                text.push('$');
                text.push(self.peek(1).unwrap_or('$'));
                self.pos += 2;
                true
            }
            // A lone dollar is a dollar.
            _ => {
                text.push('$');
                self.pos += 1;
                false
            }
        }
    }

    /// `` `…` `` at the cursor: its commands become parts.
    fn backtick(&mut self, text: &mut String) {
        self.pos += 1;
        text.push_str("`…`");
        let mut inner = String::new();
        let mut closed = false;
        while let Some(c) = self.peek(0) {
            self.pos += 1;
            match c {
                '`' => {
                    closed = true;
                    break;
                }
                '\\' => {
                    if let Some(next) = self.peek(0) {
                        if !matches!(next, '`' | '\\' | '$') {
                            inner.push('\\');
                        }
                        inner.push(next);
                        self.pos += 1;
                    }
                }
                other => inner.push(other),
            }
        }
        if !closed {
            self.fail("A backtick never closes.");
        }
        if self.depth >= MAX_DEPTH {
            self.fail("It nests commands too deeply to follow.");
            return;
        }
        let (parts, opaque) = read_all(&inner, self.depth + 1);
        if let Some(why) = opaque {
            self.fail(why);
        }
        self.parts.extend(parts);
    }
}

fn is_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    let name = name.strip_suffix('+').unwrap_or(name);
    !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Programs that run the command they're given.
const WRAPPERS: &[&str] = &[
    "sudo",
    "doas",
    "env",
    "command",
    "builtin",
    "exec",
    "nohup",
    "nice",
    "ionice",
    "timeout",
    "gtimeout",
    "stdbuf",
    "setsid",
    "caffeinate",
    "watch",
    "xargs",
    "parallel",
    "flock",
    "unbuffer",
    "chronic",
    "busybox",
    "toybox",
    "ssh",
    "rsh",
    "su",
    "arch",
    "time",
];

const SHELLS: &[&str] = &[
    "sh", "bash", "zsh", "dash", "ksh", "ash", "fish", "csh", "tcsh",
];

/// Programs that take code in a flag, and the flags.
const INTERPRETERS: &[(&str, &[&str])] = &[
    ("python", &["-c"]),
    ("python2", &["-c"]),
    ("python3", &["-c"]),
    ("node", &["-e", "-p", "--eval", "--print"]),
    ("deno", &["eval"]),
    ("bun", &["-e", "--eval"]),
    ("perl", &["-e", "-E"]),
    ("ruby", &["-e"]),
    ("php", &["-r"]),
    ("lua", &["-e"]),
    ("osascript", &["-e"]),
    ("tclsh", &["-c"]),
];

/// Add `part` to `out`, with the commands it runs inside it.
fn follow(mut part: Part, out: &mut Line, depth: usize) {
    let Some(name) = part.program() else {
        out.parts.push(part);
        return;
    };
    if part.words[0].kind != WordKind::Fixed {
        out.parts.push(part);
        return;
    }
    let nested = |text: &str, out: &mut Line| {
        let inner = parse_at(text, depth + 1);
        if let Some(why) = inner.opaque {
            out.opaque.get_or_insert(why);
        }
        out.parts.extend(inner.parts);
    };
    let as_opaque = |why: &'static str, out: &mut Line| {
        out.opaque.get_or_insert(why);
    };
    if SHELLS.contains(&name.as_str()) {
        follow_shell(part, out, depth);
        return;
    }
    match name.as_str() {
        "eval" => {
            if part.words[1..].iter().any(|w| w.kind == WordKind::Dynamic) {
                as_opaque("It runs text only known when it runs.", out);
            } else {
                let text = join(&part.words[1..]);
                nested(&text, out);
            }
            part.transparent = true;
        }
        "cmd" => {
            let at = part.words[1..].iter().position(|w| {
                let flag = w.text.to_ascii_lowercase();
                flag == "/c" || flag == "/k" || flag == "/r"
            });
            match at {
                Some(at)
                    if part.words[at + 2..]
                        .iter()
                        .all(|w| w.kind == WordKind::Fixed) =>
                {
                    let text = join(&part.words[at + 2..]);
                    nested(&text, out);
                    part.transparent = true;
                }
                _ => as_opaque("It runs text only known when it runs.", out),
            }
        }
        "powershell" | "pwsh" => {
            let flag = |w: &Word| w.text.to_ascii_lowercase();
            let code = part.words[1..].iter().position(|w| {
                let f = flag(w);
                f.len() > 1 && f.starts_with('-') && "-command".starts_with(&f)
            });
            let encoded = part.words[1..].iter().any(|w| {
                let f = flag(w);
                f.len() > 2 && f.starts_with("-e") && "-encodedcommand".starts_with(&f)
            });
            let file = part.words[1..].iter().any(|w| {
                let f = flag(w);
                f.len() > 1 && f.starts_with('-') && "-file".starts_with(&f)
            });
            if encoded {
                as_opaque("Its command is encoded.", out);
            } else if let Some(at) = code {
                if part.words[at + 2..]
                    .iter()
                    .all(|w| w.kind == WordKind::Fixed)
                {
                    let text = join(&part.words[at + 2..]);
                    nested(&text, out);
                    part.transparent = true;
                } else {
                    as_opaque("It runs text only known when it runs.", out);
                }
            } else if !file && part.words.len() == 1 {
                as_opaque("It reads its commands from somewhere Trek can't see.", out);
            }
        }
        "find" => {
            let mut i = 1;
            while i < part.words.len() {
                if matches!(
                    part.words[i].text.as_str(),
                    "-exec" | "-execdir" | "-ok" | "-okdir"
                ) {
                    let end = part.words[i + 1..]
                        .iter()
                        .position(|w| w.text == ";" || w.text == "+")
                        .map_or(part.words.len(), |e| e + i + 1);
                    let inner = Part {
                        words: part.words[i + 1..end].to_vec(),
                        ..Part::default()
                    };
                    follow(inner, out, depth + 1);
                    i = end;
                }
                i += 1;
            }
        }
        "fd" | "fdfind" => {
            let at = part.words[1..]
                .iter()
                .position(|w| matches!(w.text.as_str(), "-x" | "-X" | "--exec" | "--exec-batch"));
            if let Some(at) = at {
                let inner = Part {
                    words: part.words[at + 2..].to_vec(),
                    ..Part::default()
                };
                follow(inner, out, depth + 1);
            }
        }
        "git" => {
            // `git -c alias.x='!rm y' x` and `-c core.sshCommand=…`: a value can be a command.
            let values: Vec<&str> = part
                .words
                .iter()
                .zip(part.words.iter().skip(1))
                .filter(|(flag, _)| flag.text == "-c")
                .map(|(_, value)| value.text.as_str())
                .collect();
            if !values.is_empty() {
                part.code = Some(values.join("\n"));
            }
        }
        "alias" => {
            let values: Vec<String> = part.words[1..]
                .iter()
                .filter_map(|w| w.text.split_once('=').map(|(_, v)| v.to_string()))
                .collect();
            for value in values {
                nested(&value, out);
            }
        }
        "trap" => {
            if let Some(code) = part.words.get(1) {
                if code.kind == WordKind::Fixed {
                    let text = code.text.clone();
                    nested(&text, out);
                } else {
                    as_opaque("It runs text only known when it runs.", out);
                }
            }
        }
        "awk" | "gawk" | "nawk" | "mawk" => {
            // Its first plain word is its program, and `system()` runs commands from it.
            part.code = part.words[1..]
                .iter()
                .find(|w| !w.text.starts_with('-'))
                .map(|w| w.text.clone());
        }
        other => {
            let interpreter = INTERPRETERS
                .iter()
                .find(|(n, _)| *n == other || (*n == "python" && other.starts_with("python")));
            // `command -v rm` only asks where `rm` is.
            let asks_only = other == "command"
                && part
                    .words
                    .get(1)
                    .is_some_and(|w| w.text == "-v" || w.text == "-V");
            if let Some((_, flags)) = interpreter {
                follow_interpreter(&mut part, flags, out);
            } else if WRAPPERS.contains(&other) && !asks_only {
                part.wraps_from = Some(1);
                // A wrapped shell's string, or `watch 'rm x'`: what it runs is in one word.
                let tail = part.words[1..].to_vec();
                for start in 0..tail.len() {
                    let word = &tail[start];
                    let name = program_name(&word.text);
                    if SHELLS.contains(&name.as_str()) || name == "eval" {
                        let inner = Part {
                            words: tail[start..].to_vec(),
                            stdin: part.stdin.clone(),
                            ..Part::default()
                        };
                        follow(inner, out, depth + 1);
                        break;
                    }
                    if word.kind == WordKind::Fixed && word.text.contains([' ', ';', '|', '&']) {
                        nested(&word.text.clone(), out);
                    }
                }
            }
        }
    }
    out.parts.push(part);
}

/// `sh -c '…'` is what's in the string; `sh` fed from a pipe could be anything; `sh x.sh` is a
/// program like any other.
fn follow_shell(mut part: Part, out: &mut Line, depth: usize) {
    let mut script = None;
    let mut simple = true;
    let mut i = 1;
    let mut from_stdin = false;
    let mut code_at = None;
    while i < part.words.len() {
        let w = &part.words[i].text;
        if w == "--" {
            script = part.words.get(i + 1).map(|_| i + 1);
            break;
        }
        if w.starts_with("--") || w == "-" {
            simple = false;
            if w == "-" {
                from_stdin = true;
            }
            i += 1;
            continue;
        }
        if let Some(cluster) = w.strip_prefix('-').or_else(|| w.strip_prefix('+')) {
            if w.starts_with('-') && cluster.contains('c') {
                code_at = Some(i + 1);
                simple &= cluster.chars().all(|c| "cleiux".contains(c));
                break;
            }
            if cluster.contains('s') {
                from_stdin = true;
            }
            // These take a value of their own.
            if cluster.ends_with(['o', 'O']) {
                i += 1;
            }
            simple = false;
            i += 1;
            continue;
        }
        script = Some(i);
        break;
    }
    let run = |text: &str, out: &mut Line| {
        let inner = parse_at(text, depth + 1);
        if let Some(why) = inner.opaque {
            out.opaque.get_or_insert(why);
        }
        out.parts.extend(inner.parts);
    };
    if let Some(at) = code_at {
        match part.words.get(at) {
            Some(code) if code.kind == WordKind::Fixed => {
                let text = code.text.clone();
                run(&text, out);
                part.transparent = simple;
            }
            Some(_) => {
                out.opaque
                    .get_or_insert("It runs text only known when it runs.");
            }
            None => {
                out.opaque
                    .get_or_insert("It reads its commands from somewhere Trek can't see.");
            }
        }
    } else if script.is_none() || from_stdin {
        match &part.stdin {
            Some(body) if body.kind == WordKind::Fixed => {
                let text = body.text.clone();
                run(&text, out);
            }
            _ => {
                out.opaque
                    .get_or_insert("It reads its commands from somewhere Trek can't see.");
            }
        }
    }
    out.parts.push(part);
}

/// `python -c '…'`: code a deny can look in, and an opaque line if it's only known when it runs.
fn follow_interpreter(part: &mut Part, flags: &[&str], out: &mut Line) {
    let code = part
        .words
        .iter()
        .position(|w| flags.contains(&w.text.as_str()))
        .and_then(|at| part.words.get(at + 1))
        .or(part.stdin.as_ref());
    if let Some(code) = code {
        if code.kind == WordKind::Fixed {
            part.code = Some(code.text.clone());
        } else {
            out.opaque
                .get_or_insert("It runs text only known when it runs.");
        }
    }
}

fn join(words: &[Word]) -> String {
    words
        .iter()
        .map(|w| w.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}
