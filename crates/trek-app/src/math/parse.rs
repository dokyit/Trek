//! TeX math, as agents write it, into a tree `layout` can set. Forgiving by design: a brace
//! left open closes at the end, a missing argument is empty, and a command Trek doesn't know
//! shows its name — nothing panics and no text is dropped. `%` is a percent sign, not a comment.

use super::layout::{Class, Face, Style};
use super::tables;

/// How deep groups may nest before the rest is shown as written.
const MAX_DEPTH: usize = 48;

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    /// One glyph.
    Sym { ch: char, face: Face, class: Class },
    /// A run of text, set as one line (`\text{…}`, an operator's name).
    Text { text: String, face: Face },
    /// `{…}`: an ordinary atom.
    Group(Vec<Node>),
    /// A base with scripts. `limits`: set above and below (`Some(true)`), beside (`Some(false)`),
    /// or as the base decides.
    Scripts { base: Box<Node>, sup: Option<Vec<Node>>, sub: Option<Vec<Node>>, limits: Option<bool> },
    /// `\frac` and friends. `style`: forced (`\dfrac`, `\tfrac`); `delims`: `\binom`'s.
    Frac { num: Vec<Node>, den: Vec<Node>, rule: bool, style: Option<Style>, delims: Option<(char, char)> },
    Sqrt { index: Option<Vec<Node>>, body: Vec<Node> },
    /// A large operator (`\sum`, `\int`) or a named one (`\lim`, `\sin`, `\operatorname`).
    Op { body: OpBody, limits: bool },
    Accent { accent: Accent, body: Vec<Node> },
    /// `\left( … \middle| … \right)`: the delimiters, the parts between them.
    LeftRight { delims: Vec<char>, parts: Vec<Vec<Node>> },
    /// `\big(`, `\Bigg]`: a delimiter at a fixed size, 1 to 4.
    Big { delim: char, size: u8, class: Class },
    /// Space, in ems (negative: back).
    Space(f32),
    Boxed(Vec<Node>),
    /// The rest of a group in another style (`\displaystyle`).
    Styled { style: Style, body: Vec<Node> },
    /// An environment: rows of cells.
    Array(Array),
    Phantom { body: Vec<Node>, width: bool, height: bool },
    Cancel { body: Vec<Node>, forward: bool, back: bool },
    /// `\overset{over}{base}`, `\underset{under}{base}`, `\stackrel`.
    Stack { base: Vec<Node>, over: Option<Vec<Node>>, under: Option<Vec<Node>> },
    /// `\xrightarrow[under]{over}`.
    Arrow { right: bool, over: Vec<Node>, under: Vec<Node> },
    /// A slash through the next atom (`\not=`, `\neq`).
    Not(Box<Node>),
    /// The body as an atom of another class (`\mathrel{…}`).
    Class { class: Class, body: Vec<Node> },
    /// `\tag{…}`: shown after the formula, in parentheses.
    Tag(Vec<Node>),
    /// A command Trek doesn't know, shown as written.
    Unknown(String),
    /// `&` and `\\` outside an environment, for `layout` to set the formula as aligned rows.
    Tab,
    Newline,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OpBody {
    Glyph(char),
    Name(Vec<Node>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Accent {
    Glyph(char),
    /// `\vec`: a small arrow.
    Vec,
    Hat,
    Tilde,
    Overline,
    Underline,
    OverArrow { left: bool, right: bool },
    OverBrace,
    UnderBrace,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Array {
    pub rows: Vec<Vec<Vec<Node>>>,
    pub cols: Vec<Align>,
    /// Vertical rules: before which column (`cols.len()` for after the last).
    pub col_rules: Vec<usize>,
    /// Horizontal rules: above which row (`rows.len()` for below the last).
    pub row_rules: Vec<usize>,
    pub kind: ArrayKind,
    pub delims: (char, char),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ArrayKind {
    /// `matrix`, `array`: centred cells, text style.
    #[default]
    Matrix,
    /// `smallmatrix`, `substack`: script style, tight.
    Small,
    /// `aligned`, `align`: right-left pairs, display style.
    Aligned,
    /// `gathered`, `equation`: centred rows, display style.
    Gathered,
    /// `cases`: left-aligned, a quad apart.
    Cases { display: bool },
}

/// Delimiter for "none" (`\left.`).
pub const NO_DELIM: char = '.';

/// The font letters and digits are set in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MathFont {
    Default,
    Roman,
    Bold,
    Italic,
    Blackboard,
    Calligraphic,
    Fraktur,
    Script,
    Sans,
    Mono,
    BoldSymbol,
}

/// Parse `tex` (math mode).
pub fn parse(tex: &str) -> Vec<Node> {
    let mut p = Parser { src: tex, pos: 0, depth: 0, font: MathFont::Default };
    let mut out = p.list(Stop::End);
    // A stray `}` ends a list early: show it and carry on.
    while p.pos < p.src.len() {
        p.pos += 1;
        out.push(Node::Unknown("}".into()));
        out.extend(p.list(Stop::End));
    }
    out
}

#[derive(Clone, Copy, PartialEq)]
enum Stop {
    /// The end of the text, or a `}` that isn't ours.
    End,
    /// `}`.
    Brace,
    /// `]` at this nesting level (an optional argument).
    Bracket,
    /// `\right`.
    Right,
    /// `\end`.
    EndEnv,
}

struct Parser<'a> {
    src: &'a str,
    pos: usize,
    depth: usize,
    font: MathFont,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn skip_space(&mut self) {
        while let Some(c) = self.peek().filter(|c| c.is_whitespace()) {
            self.pos += c.len_utf8();
        }
    }

    /// A control sequence's name, after its backslash (letters, or one other character).
    fn command_name(&mut self) -> String {
        let rest = &self.src[self.pos..];
        let letters = rest.bytes().take_while(u8::is_ascii_alphabetic).count();
        if letters > 0 {
            let mut n = rest[..letters].to_string();
            self.pos += letters;
            // `\operatorname*`, `\hspace*`, `\tag*`.
            if self.peek() == Some('*') && matches!(n.as_str(), "operatorname" | "hspace" | "tag") {
                self.pos += 1;
                n.push('*');
            }
            self.skip_space();
            n
        } else {
            match self.bump() {
                Some(c) => c.to_string(),
                None => String::new(),
            }
        }
    }

    /// Whether the next thing is `\name`, without consuming it.
    fn at_command(&self, name: &str) -> bool {
        let rest = &self.src[self.pos..];
        rest.strip_prefix('\\').and_then(|r| r.strip_prefix(name)).is_some_and(|r| !r.starts_with(|c: char| c.is_ascii_alphabetic()))
    }

    fn list(&mut self, stop: Stop) -> Vec<Node> {
        self.depth += 1;
        let out = if self.depth > MAX_DEPTH { self.rest_as_text() } else { self.list_inner(stop) };
        self.depth -= 1;
        out
    }

    /// Too deep to be math: the rest as written.
    fn rest_as_text(&mut self) -> Vec<Node> {
        let rest = self.src[self.pos..].to_string();
        self.pos = self.src.len();
        vec![Node::Text { text: rest, face: Face::Main }]
    }

    fn list_inner(&mut self, stop: Stop) -> Vec<Node> {
        let mut out: Vec<Node> = Vec::new();
        let font = self.font;
        loop {
            self.skip_space();
            let Some(c) = self.peek() else { break };
            match c {
                '}' => {
                    if stop == Stop::Brace {
                        self.pos += 1;
                    }
                    break;
                }
                ']' if stop == Stop::Bracket => {
                    self.pos += 1;
                    break;
                }
                '^' | '_' => {
                    self.pos += 1;
                    let arg = self.arg();
                    attach(&mut out, c == '^', arg);
                }
                '\'' => {
                    let mut primes = Vec::new();
                    while self.peek() == Some('\'') {
                        self.pos += 1;
                        primes.push(Node::Sym { ch: '\u{2032}', face: Face::Main, class: Class::Ord });
                    }
                    attach(&mut out, true, primes);
                }
                '\\' => {
                    if stop == Stop::Right && self.at_command("right") {
                        break;
                    }
                    if stop == Stop::EndEnv && self.at_command("end") {
                        break;
                    }
                    self.pos += 1;
                    let name = self.command_name();
                    // Infix fractions take everything before and after them in the group.
                    if let Some((rule, delims)) = match name.as_str() {
                        "over" => Some((true, None)),
                        "atop" => Some((false, None)),
                        "choose" => Some((false, Some(('(', ')')))),
                        "brack" => Some((false, Some(('[', ']')))),
                        "brace" => Some((false, Some(('{', '}')))),
                        _ => None,
                    } {
                        let num = std::mem::take(&mut out);
                        let den = self.list(stop);
                        out.push(Node::Frac { num, den, rule, style: None, delims });
                        break;
                    }
                    if let Some(style) = style_switch(&name) {
                        let body = self.list(stop);
                        out.push(Node::Styled { style, body });
                        break;
                    }
                    if let Some(f) = font_switch(&name) {
                        self.font = f;
                        continue;
                    }
                    if name == "color" {
                        // `\color{red}`: the rest of the group, in the text's own colour.
                        self.arg_text();
                        continue;
                    }
                    if name == "limits" || name == "nolimits" || name == "displaylimits" {
                        set_limits(&mut out, name == "limits");
                        continue;
                    }
                    if name == "not" {
                        let next = self.atom();
                        out.push(Node::Not(Box::new(next.into_iter().next().unwrap_or(Node::Group(Vec::new())))));
                        continue;
                    }
                    out.extend(self.command(&name));
                }
                '{' => {
                    self.pos += 1;
                    let body = self.list(Stop::Brace);
                    out.push(Node::Group(body));
                }
                '&' => {
                    self.pos += 1;
                    out.push(Node::Tab);
                }
                '~' => {
                    self.pos += 1;
                    out.push(Node::Space(0.25));
                }
                _ => {
                    self.pos += c.len_utf8();
                    out.push(self.symbol(c));
                }
            }
        }
        self.font = font;
        out
    }

    /// One argument: a group, or a single token.
    fn arg(&mut self) -> Vec<Node> {
        self.skip_space();
        match self.peek() {
            None | Some('}') | Some('&') => Vec::new(),
            Some('{') => {
                self.pos += 1;
                self.list(Stop::Brace)
            }
            _ => self.atom(),
        }
    }

    /// One token as nodes: a symbol, a group, or a command with its arguments.
    fn atom(&mut self) -> Vec<Node> {
        self.skip_space();
        match self.peek() {
            None | Some('}') => Vec::new(),
            Some('{') => {
                self.pos += 1;
                vec![Node::Group(self.list(Stop::Brace))]
            }
            Some('\\') => {
                self.pos += 1;
                let name = self.command_name();
                self.command(&name)
            }
            Some(c) => {
                self.pos += c.len_utf8();
                vec![self.symbol(c)]
            }
        }
    }

    /// An argument read as plain text: a colour name, an environment name, a column spec.
    fn arg_text(&mut self) -> String {
        self.skip_space();
        match self.peek() {
            Some('{') => {
                self.pos += 1;
                let start = self.pos;
                let mut depth = 0;
                while let Some(c) = self.bump() {
                    match c {
                        '{' => depth += 1,
                        '}' if depth == 0 => return self.src[start..self.pos - 1].to_string(),
                        '}' => depth -= 1,
                        _ => {}
                    }
                }
                self.src[start..].to_string()
            }
            Some(c) if c != '}' => {
                self.pos += c.len_utf8();
                c.to_string()
            }
            _ => String::new(),
        }
    }

    /// `[…]`, when it comes next: an optional argument.
    fn optional(&mut self) -> Option<Vec<Node>> {
        self.skip_space();
        if self.peek() != Some('[') {
            return None;
        }
        self.pos += 1;
        Some(self.list(Stop::Bracket))
    }

    /// A delimiter after `\left`, `\big` and the like.
    fn delim(&mut self) -> char {
        self.skip_space();
        match self.peek() {
            None => NO_DELIM,
            Some('\\') => {
                self.pos += 1;
                let name = self.command_name();
                delim_named(&name).unwrap_or(NO_DELIM)
            }
            Some(c) => {
                self.pos += c.len_utf8();
                match c {
                    '<' => '\u{27e8}',
                    '>' => '\u{27e9}',
                    '|' => '|',
                    c => c,
                }
            }
        }
    }

    /// Text-mode content: `\text{…}`'s argument, with `$…$` inside it back in math.
    fn text_arg(&mut self, face: Face) -> Vec<Node> {
        self.skip_space();
        if self.peek() != Some('{') {
            return match self.bump() {
                Some(c) => vec![Node::Text { text: c.to_string(), face }],
                None => Vec::new(),
            };
        }
        self.pos += 1;
        self.depth += 1;
        let out = if self.depth > MAX_DEPTH { self.rest_as_text() } else { self.text_inner(face) };
        self.depth -= 1;
        out
    }

    fn text_inner(&mut self, face: Face) -> Vec<Node> {
        let mut out = Vec::new();
        let mut run = String::new();
        let flush = |run: &mut String, out: &mut Vec<Node>| {
            if !run.is_empty() {
                out.push(Node::Text { text: std::mem::take(run), face });
            }
        };
        while let Some(c) = self.bump() {
            match c {
                '}' => break,
                '{' => {
                    flush(&mut run, &mut out);
                    self.depth += 1;
                    let inner = if self.depth > MAX_DEPTH { self.rest_as_text() } else { self.text_inner(face) };
                    self.depth -= 1;
                    out.extend(inner);
                }
                '$' => {
                    flush(&mut run, &mut out);
                    let start = self.pos;
                    let end = self.src[start..].find('$').map_or(self.src.len(), |i| start + i);
                    let inner = &self.src[start..end];
                    let mut sub = Parser { src: inner, pos: 0, depth: self.depth, font: MathFont::Default };
                    out.push(Node::Group(sub.list(Stop::End)));
                    self.pos = (end + 1).min(self.src.len());
                }
                '\\' => {
                    let name = self.command_name();
                    match name.as_str() {
                        "$" | "%" | "&" | "#" | "_" | "{" | "}" => run.push_str(&name),
                        " " | "," | ";" | ":" => run.push(' '),
                        "textbackslash" => run.push('\\'),
                        "textbf" | "bf" | "textit" | "it" | "emph" | "texttt" | "textrm" | "textup" | "textnormal" | "textsf" => {
                            flush(&mut run, &mut out);
                            let inner = match name.as_str() {
                                "textbf" | "bf" if face == Face::MainItalic => Face::MainBoldItalic,
                                "textit" | "it" | "emph" if face == Face::MainBold => Face::MainBoldItalic,
                                "textbf" | "bf" => Face::MainBold,
                                "textit" | "it" | "emph" => Face::MainItalic,
                                "texttt" => Face::Typewriter,
                                "textsf" => Face::SansSerif,
                                _ => Face::Main,
                            };
                            out.extend(self.text_arg(inner));
                        }
                        "ldots" | "dots" | "textellipsis" => run.push('…'),
                        "quad" => run.push('\u{2003}'),
                        "qquad" => run.push_str("\u{2003}\u{2003}"),
                        "" => run.push('\\'),
                        _ => {
                            flush(&mut run, &mut out);
                            out.push(Node::Unknown(name));
                        }
                    }
                }
                '~' => run.push('\u{a0}'),
                c if c.is_whitespace() => {
                    if !run.ends_with(' ') {
                        run.push(' ');
                    }
                }
                '-' if self.src[self.pos..].starts_with("--") => {
                    self.pos += 2;
                    run.push('—');
                }
                '-' if self.src[self.pos..].starts_with('-') => {
                    self.pos += 1;
                    run.push('–');
                }
                '`' if self.src[self.pos..].starts_with('`') => {
                    self.pos += 1;
                    run.push('“');
                }
                '\'' if self.src[self.pos..].starts_with('\'') => {
                    self.pos += 1;
                    run.push('”');
                }
                '`' => run.push('‘'),
                '\'' => run.push('’'),
                c => run.push(c),
            }
        }
        flush(&mut run, &mut out);
        out
    }

    /// A character in math mode.
    fn symbol(&self, c: char) -> Node {
        if c.is_ascii_alphanumeric() {
            return letter(c, self.font);
        }
        let key = c.to_string();
        if let Some((ch, face, class, italic)) = lookup(&key) {
            return sym(ch, face, class, italic, self.font);
        }
        match c {
            '%' | '#' => Node::Sym { ch: c, face: Face::Main, class: Class::Ord },
            '|' => Node::Sym { ch: '\u{2223}', face: Face::Main, class: Class::Ord },
            // Anything else (accented letters, CJK) from the system's fonts.
            _ => Node::Text { text: c.to_string(), face: Face::Main },
        }
    }

    fn command(&mut self, name: &str) -> Vec<Node> {
        let one = |n: Node| vec![n];
        match name {
            "" => one(Node::Unknown("\\".into())),
            "\\" | "newline" | "cr" => {
                // `\\[2pt]`: the extra space doesn't matter here.
                let save = self.pos;
                if self.optional().is_none() {
                    self.pos = save;
                }
                one(Node::Newline)
            }
            "frac" | "dfrac" | "tfrac" | "cfrac" => {
                let num = self.arg();
                let den = self.arg();
                let style = match name {
                    "dfrac" | "cfrac" => Some(Style::Display),
                    "tfrac" => Some(Style::Text),
                    _ => None,
                };
                one(Node::Frac { num, den, rule: true, style, delims: None })
            }
            "binom" | "dbinom" | "tbinom" => {
                let num = self.arg();
                let den = self.arg();
                let style = match name {
                    "dbinom" => Some(Style::Display),
                    "tbinom" => Some(Style::Text),
                    _ => None,
                };
                one(Node::Frac { num, den, rule: false, style, delims: Some(('(', ')')) })
            }
            "genfrac" => {
                let left = self.arg_text();
                let right = self.arg_text();
                let thickness = self.arg_text();
                let style = self.arg_text();
                let num = self.arg();
                let den = self.arg();
                let d = |s: &str| delim_named(s.trim_start_matches('\\')).or_else(|| s.chars().next()).unwrap_or(NO_DELIM);
                let delims = (!left.is_empty() || !right.is_empty()).then(|| (d(&left), d(&right)));
                let style = match style.trim() {
                    "0" => Some(Style::Display),
                    "1" => Some(Style::Text),
                    "2" => Some(Style::Script),
                    "3" => Some(Style::ScriptScript),
                    _ => None,
                };
                one(Node::Frac { num, den, rule: !thickness.trim().starts_with('0'), style, delims })
            }
            "sqrt" => {
                let index = self.optional();
                let body = self.arg();
                one(Node::Sqrt { index, body })
            }
            "text" | "textrm" | "textnormal" | "textup" | "textmd" | "mbox" | "hbox" | "mathrm_text" => one(Node::Group(self.text_arg(Face::Main))),
            "textbf" => one(Node::Group(self.text_arg(Face::MainBold))),
            "textit" | "emph" | "textsl" => one(Node::Group(self.text_arg(Face::MainItalic))),
            "texttt" => one(Node::Group(self.text_arg(Face::Typewriter))),
            "textsf" => one(Node::Group(self.text_arg(Face::SansSerif))),
            "fbox" => one(Node::Boxed(self.text_arg(Face::Main))),
            "boxed" => one(Node::Boxed(vec![Node::Styled { style: Style::Display, body: self.arg() }])),
            "operatorname" | "operatorname*" | "mathop" => {
                let saved = self.font;
                if name != "mathop" {
                    self.font = MathFont::Roman;
                }
                let body = self.arg();
                self.font = saved;
                one(Node::Op { body: OpBody::Name(body), limits: name != "operatorname" })
            }
            "mathbin" | "mathrel" | "mathord" | "mathopen" | "mathclose" | "mathpunct" | "mathinner" => {
                let class = match name {
                    "mathbin" => Class::Bin,
                    "mathrel" => Class::Rel,
                    "mathopen" => Class::Open,
                    "mathclose" => Class::Close,
                    "mathpunct" => Class::Punct,
                    "mathinner" => Class::Inner,
                    _ => Class::Ord,
                };
                one(Node::Class { class, body: self.arg() })
            }
            "left" => {
                let mut delims = vec![self.delim()];
                let mut parts = vec![Vec::new()];
                // `\middle` arrives as a marker in the list.
                for node in self.list(Stop::Right) {
                    match node {
                        Node::Big { delim, size: 0, .. } => {
                            parts.push(Vec::new());
                            delims.push(delim);
                        }
                        other => parts.last_mut().expect("a part").push(other),
                    }
                }
                if self.at_command("right") {
                    self.pos += "\\right".len();
                    delims.push(self.delim());
                } else {
                    delims.push(NO_DELIM);
                }
                one(Node::LeftRight { delims, parts })
            }
            "middle" => one(Node::Big { delim: self.delim(), size: 0, class: Class::Rel }),
            "right" => {
                // A `\right` with no `\left`: its delimiter, as written.
                let d = self.delim();
                one(Node::Sym { ch: d, face: Face::Main, class: Class::Close })
            }
            "big" | "Big" | "bigg" | "Bigg" | "bigl" | "Bigl" | "biggl" | "Biggl" | "bigr" | "Bigr" | "biggr" | "Biggr" | "bigm" | "Bigm" | "biggm" | "Biggm" => {
                let size = match name.trim_end_matches(['l', 'r', 'm']) {
                    "big" => 1,
                    "Big" => 2,
                    "bigg" => 3,
                    _ => 4,
                };
                let class = match name.chars().last() {
                    Some('l') => Class::Open,
                    Some('r') => Class::Close,
                    Some('m') => Class::Rel,
                    _ => Class::Ord,
                };
                one(Node::Big { delim: self.delim(), size, class })
            }
            "," | "thinspace" => one(Node::Space(3. / 18.)),
            ":" | ">" | "medspace" => one(Node::Space(4. / 18.)),
            ";" | "thickspace" => one(Node::Space(5. / 18.)),
            "!" | "negthinspace" => one(Node::Space(-3. / 18.)),
            "negmedspace" => one(Node::Space(-4. / 18.)),
            "negthickspace" => one(Node::Space(-5. / 18.)),
            " " | "space" | "nobreakspace" => one(Node::Space(0.25)),
            "enspace" => one(Node::Space(0.5)),
            "quad" => one(Node::Space(1.)),
            "qquad" => one(Node::Space(2.)),
            "hspace" | "hspace*" => {
                let dim = self.arg_text();
                one(Node::Space(dimension(&dim)))
            }
            "kern" | "mkern" | "hskip" | "mskip" | "mspace" => {
                let dim = if self.peek() == Some('{') { self.arg_text() } else { self.dimension_text() };
                one(Node::Space(dimension(&dim)))
            }
            "displaystyle" | "textstyle" | "scriptstyle" | "scriptscriptstyle" => Vec::new(),
            "hat" | "widehat" | "check" | "widecheck" | "tilde" | "widetilde" | "bar" | "overline" | "underline" | "vec" | "dot" | "ddot" | "dddot" | "acute" | "grave" | "breve" | "mathring" | "overrightarrow" | "overleftarrow" | "overleftrightarrow" | "overbrace" | "underbrace" => {
                let accent = match name {
                    "hat" => Accent::Glyph('\u{2c6}'),
                    "check" | "widecheck" => Accent::Glyph('\u{2c7}'),
                    "tilde" => Accent::Glyph('\u{2dc}'),
                    "bar" => Accent::Glyph('\u{2c9}'),
                    "dot" => Accent::Glyph('\u{2d9}'),
                    "ddot" => Accent::Glyph('\u{a8}'),
                    "dddot" => Accent::Glyph('\u{20db}'),
                    "acute" => Accent::Glyph('\u{2ca}'),
                    "grave" => Accent::Glyph('\u{2cb}'),
                    "breve" => Accent::Glyph('\u{2d8}'),
                    "mathring" => Accent::Glyph('\u{2da}'),
                    "widehat" => Accent::Hat,
                    "widetilde" => Accent::Tilde,
                    "vec" => Accent::Vec,
                    "overline" => Accent::Overline,
                    "underline" => Accent::Underline,
                    "overrightarrow" => Accent::OverArrow { left: false, right: true },
                    "overleftarrow" => Accent::OverArrow { left: true, right: false },
                    "overleftrightarrow" => Accent::OverArrow { left: true, right: true },
                    "overbrace" => Accent::OverBrace,
                    _ => Accent::UnderBrace,
                };
                let body = self.arg();
                // A brace takes its label as a script set above or below it.
                let node = Node::Accent { accent, body };
                if matches!(accent, Accent::OverBrace | Accent::UnderBrace) {
                    one(Node::Scripts { base: Box::new(node), sup: None, sub: None, limits: Some(true) })
                } else {
                    one(node)
                }
            }
            "phantom" | "hphantom" | "vphantom" => {
                let body = self.arg();
                one(Node::Phantom { body, width: name != "vphantom", height: name != "hphantom" })
            }
            "smash" | "mathstrut" => {
                if name == "mathstrut" {
                    return one(Node::Phantom { body: vec![self.symbol('(')], width: false, height: true });
                }
                let _ = self.optional();
                one(Node::Group(self.arg()))
            }
            "cancel" | "bcancel" | "xcancel" | "sout" => {
                let body = self.arg();
                one(Node::Cancel { body, forward: name != "bcancel", back: name == "bcancel" || name == "xcancel" })
            }
            "overset" | "stackrel" => {
                let over = self.arg();
                let base = self.arg();
                let class = if name == "stackrel" { Class::Rel } else { Class::Ord };
                let stack = Node::Stack { base, over: Some(over), under: None };
                one(if class == Class::Rel { Node::Class { class, body: vec![stack] } } else { stack })
            }
            "underset" => {
                let under = self.arg();
                let base = self.arg();
                one(Node::Stack { base, over: None, under: Some(under) })
            }
            "xrightarrow" | "xleftarrow" | "xLeftarrow" | "xRightarrow" | "xleftrightarrow" | "xmapsto" => {
                let under = self.optional().unwrap_or_default();
                let over = self.arg();
                one(Node::Class { class: Class::Rel, body: vec![Node::Arrow { right: !name.contains("left"), over, under }] })
            }
            "textcolor" | "colorbox" => {
                self.arg_text();
                if name == "colorbox" { one(Node::Group(self.text_arg(Face::Main))) } else { one(Node::Group(self.arg())) }
            }
            "pmod" => {
                let arg = self.arg();
                let mut body = vec![Node::Space(1.), self.symbol('(')];
                body.push(Node::Text { text: "mod".into(), face: Face::Main });
                body.push(Node::Space(6. / 18.));
                body.extend(arg);
                body.push(self.symbol(')'));
                one(Node::Group(body))
            }
            "pod" => {
                let arg = self.arg();
                let mut body = vec![Node::Space(1.), self.symbol('(')];
                body.extend(arg);
                body.push(self.symbol(')'));
                one(Node::Group(body))
            }
            "bmod" => one(Node::Class { class: Class::Bin, body: vec![Node::Text { text: "mod".into(), face: Face::Main }] }),
            "mod" => one(Node::Group(vec![Node::Space(1.), Node::Text { text: "mod".into(), face: Face::Main }, Node::Space(6. / 18.)])),
            "tag" | "tag*" => {
                let body = self.text_arg(Face::Main);
                one(Node::Tag(body))
            }
            "label" | "ref" | "eqref" => {
                let text = self.arg_text();
                if name == "label" { Vec::new() } else { one(Node::Text { text: format!("({text})"), face: Face::Main }) }
            }
            "nonumber" | "notag" | "nolimits_" | "allowbreak" | "nobreak" | "relax" | "displaybreak" | "hline" | "hdashline" | "strut" => {
                if name == "hline" || name == "hdashline" {
                    return one(Node::Unknown("\\hline".into()));
                }
                Vec::new()
            }
            "substack" => {
                let body = self.arg();
                one(Node::Array(rows_of(body, ArrayKind::Small, vec![Align::Center], (NO_DELIM, NO_DELIM))))
            }
            "begin" => self.environment(),
            "end" => {
                // An `\end` with no `\begin`.
                let env = self.arg_text();
                one(Node::Unknown(format!("\\end{{{env}}}")))
            }
            "neq" | "ne" => one(Node::Not(Box::new(Node::Sym { ch: '=', face: Face::Main, class: Class::Rel }))),
            "notin" => one(Node::Not(Box::new(Node::Sym { ch: '\u{2208}', face: Face::Main, class: Class::Rel }))),
            "implies" => vec![Node::Space(5. / 18.), Node::Sym { ch: '\u{27f9}', face: Face::Main, class: Class::Rel }, Node::Space(5. / 18.)],
            "impliedby" => vec![Node::Space(5. / 18.), Node::Sym { ch: '\u{27f8}', face: Face::Main, class: Class::Rel }, Node::Space(5. / 18.)],
            "iff" => vec![Node::Space(5. / 18.), Node::Sym { ch: '\u{27fa}', face: Face::Main, class: Class::Rel }, Node::Space(5. / 18.)],
            "dots" | "dotsc" | "dotso" | "ldots" | "mathellipsis" => one(Node::Sym { ch: '\u{2026}', face: Face::Main, class: Class::Inner }),
            "cdots" | "dotsb" | "dotsm" | "dotsi" => one(Node::Sym { ch: '\u{22ef}', face: Face::Main, class: Class::Inner }),
            "vdots" => one(Node::Sym { ch: '\u{22ee}', face: Face::Main, class: Class::Ord }),
            "colon" => one(Node::Sym { ch: ':', face: Face::Main, class: Class::Punct }),
            "R" | "N" | "Z" | "Q" | "C" | "Reals" | "Complex" | "natnums" | "reals" | "Bbbk" => {
                let c = match name {
                    "Reals" | "reals" => 'R',
                    "Complex" => 'C',
                    "natnums" => 'N',
                    "Bbbk" => 'k',
                    n => n.chars().next().unwrap_or('R'),
                };
                one(Node::Sym { ch: c, face: Face::Ams, class: Class::Ord })
            }
            "lt" => one(Node::Sym { ch: '<', face: Face::Main, class: Class::Rel }),
            "gt" => one(Node::Sym { ch: '>', face: Face::Main, class: Class::Rel }),
            "argmax" | "argmin" => one(Node::Op { body: OpBody::Name(vec![Node::Text { text: format!("arg\u{2009}{}", &name[3..]), face: Face::Main }]), limits: true }),
            _ => {
                if let Some(text) = named_operator(name) {
                    let limits = matches!(name, "lim" | "liminf" | "limsup" | "max" | "min" | "sup" | "inf" | "det" | "gcd" | "Pr" | "injlim" | "projlim" | "varliminf" | "varlimsup");
                    return one(Node::Op { body: OpBody::Name(vec![Node::Text { text: text.into(), face: Face::Main }]), limits });
                }
                if let Some(f) = font_command(name) {
                    let saved = self.font;
                    self.font = f;
                    let body = self.arg();
                    self.font = saved;
                    return one(Node::Group(body));
                }
                let key = format!("\\{name}");
                if let Some((ch, face, class, italic)) = lookup(&key) {
                    if class == Class::Op {
                        let limits = !matches!(ch, '\u{222b}'..='\u{2233}');
                        return one(Node::Op { body: OpBody::Glyph(ch), limits });
                    }
                    return one(sym(ch, face, class, italic, self.font));
                }
                if let Some(d) = delim_named(name) {
                    return one(Node::Sym { ch: d, face: Face::Main, class: Class::Ord });
                }
                one(Node::Unknown(format!("\\{name}")))
            }
        }
    }

    /// A TeX dimension written without braces (`\kern 3mu`, `\mkern-2mu`).
    fn dimension_text(&mut self) -> String {
        self.skip_space();
        let start = self.pos;
        let rest = &self.src[start..];
        let number = rest.bytes().take_while(|b| b.is_ascii_digit() || matches!(b, b'.' | b'-' | b'+' | b' ')).count();
        let unit = rest[number..].bytes().take_while(u8::is_ascii_alphabetic).take(2).count();
        self.pos += number + unit;
        self.src[start..self.pos].to_string()
    }

    fn environment(&mut self) -> Vec<Node> {
        let env = self.arg_text();
        let base = env.trim_end_matches('*');
        let mut spec = None;
        if base == "array" || base == "subarray" || base == "alignat" || base == "alignedat" {
            spec = Some(self.arg_text());
        }
        let mut body = self.list(Stop::EndEnv);
        if self.at_command("end") {
            self.pos += "\\end".len();
            self.arg_text();
        }
        let (kind, delims, cols) = match base {
            "matrix" => (ArrayKind::Matrix, (NO_DELIM, NO_DELIM), vec![]),
            "pmatrix" => (ArrayKind::Matrix, ('(', ')'), vec![]),
            "bmatrix" => (ArrayKind::Matrix, ('[', ']'), vec![]),
            "Bmatrix" => (ArrayKind::Matrix, ('{', '}'), vec![]),
            "vmatrix" => (ArrayKind::Matrix, ('|', '|'), vec![]),
            "Vmatrix" => (ArrayKind::Matrix, ('\u{2016}', '\u{2016}'), vec![]),
            "smallmatrix" => (ArrayKind::Small, (NO_DELIM, NO_DELIM), vec![]),
            "cases" => (ArrayKind::Cases { display: false }, ('{', NO_DELIM), vec![Align::Left, Align::Left]),
            "dcases" => (ArrayKind::Cases { display: true }, ('{', NO_DELIM), vec![Align::Left, Align::Left]),
            "rcases" => (ArrayKind::Cases { display: false }, (NO_DELIM, '}'), vec![Align::Left, Align::Left]),
            "aligned" | "align" | "alignat" | "alignedat" | "split" | "eqnarray" | "flalign" => (ArrayKind::Aligned, (NO_DELIM, NO_DELIM), vec![]),
            "gathered" | "gather" | "equation" | "multline" | "displaymath" | "math" => (ArrayKind::Gathered, (NO_DELIM, NO_DELIM), vec![Align::Center]),
            "array" | "subarray" => {
                let (cols, _) = column_spec(spec.as_deref().unwrap_or("c"));
                (if base == "subarray" { ArrayKind::Small } else { ArrayKind::Matrix }, (NO_DELIM, NO_DELIM), cols)
            }
            _ => (ArrayKind::Matrix, (NO_DELIM, NO_DELIM), vec![]),
        };
        if base == "eqnarray" {
            body.retain(|n| !matches!(n, Node::Tag(_)));
        }
        let mut array = rows_of(body, kind, cols, delims);
        if base == "array" {
            array.col_rules = column_spec(spec.as_deref().unwrap_or("c")).1;
        }
        vec![Node::Array(array)]
    }
}

/// Split an environment's body into rows (at `\\`) and cells (at `&`), noting `\hline`s.
pub fn rows_of(body: Vec<Node>, kind: ArrayKind, cols: Vec<Align>, delims: (char, char)) -> Array {
    let mut rows: Vec<Vec<Vec<Node>>> = vec![vec![Vec::new()]];
    let mut row_rules = Vec::new();
    for node in body {
        match node {
            Node::Tab => rows.last_mut().expect("a row").push(Vec::new()),
            Node::Newline => rows.push(vec![Vec::new()]),
            Node::Unknown(ref name) if name == "\\hline" => {
                let at = rows.len() - 1;
                if rows.last().is_some_and(|r| r.len() == 1 && r[0].is_empty()) {
                    row_rules.push(at);
                } else {
                    row_rules.push(at + 1);
                }
            }
            other => rows.last_mut().expect("a row").last_mut().expect("a cell").push(other),
        }
    }
    // A trailing `\\` leaves an empty row.
    if rows.len() > 1 && rows.last().is_some_and(|r| r.len() == 1 && r[0].is_empty()) {
        rows.pop();
    }
    row_rules.iter_mut().for_each(|r| *r = (*r).min(rows.len()));
    Array { rows, cols, col_rules: Vec::new(), row_rules, kind, delims }
}

/// Columns of an `array` spec (`{c|cc}`), and where its vertical rules go.
fn column_spec(spec: &str) -> (Vec<Align>, Vec<usize>) {
    let mut cols = Vec::new();
    let mut rules = Vec::new();
    let mut chars = spec.chars();
    while let Some(c) = chars.next() {
        match c {
            'l' => cols.push(Align::Left),
            'c' => cols.push(Align::Center),
            'r' => cols.push(Align::Right),
            '|' => rules.push(cols.len()),
            'p' | 'm' | 'b' | '@' | '>' | '<' => {
                // A braced argument follows; `p{…}` is a left-aligned column.
                let mut depth = 0;
                for c in chars.by_ref() {
                    match c {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth <= 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                if matches!(c, 'p' | 'm' | 'b') {
                    cols.push(Align::Left);
                }
            }
            _ => {}
        }
    }
    (cols, rules)
}

/// A TeX length in ems (`3mu`, `1em`, `2pt`, `0.5ex`).
fn dimension(text: &str) -> f32 {
    let t = text.replace(' ', "");
    let split = t.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(t.len());
    let value: f32 = t[..split].parse().unwrap_or(0.);
    let unit = &t[split..];
    let em = match unit {
        "em" => 1.,
        "ex" => 0.431,
        "mu" => 1. / 18.,
        "pt" => 0.1,
        "pc" => 1.2,
        "px" => 0.1 * 0.75,
        "bp" => 0.1 * 72.27 / 72.,
        "mm" => 0.1 * 2.845,
        "cm" => 0.1 * 28.45,
        "in" => 0.1 * 72.27,
        _ => 0.1,
    };
    (value * em).clamp(-20., 20.)
}

/// `^` or `_` on the last atom of `out` (or on nothing).
fn attach(out: &mut Vec<Node>, sup: bool, arg: Vec<Node>) {
    let base = match out.pop() {
        Some(Node::Scripts { base, sup: s, sub, limits }) => {
            // `x^a_b`, `x_b^a`, `x'^2`: one set of scripts. A second of the same kind starts
            // a new atom, as TeX would complain and show it anyway.
            let taken = if sup { s.is_some() } else { sub.is_some() };
            if !taken || (sup && s.as_ref().is_some_and(|s| s.iter().all(|n| matches!(n, Node::Sym { ch: '\u{2032}', .. })))) {
                let (s, sub) = if sup {
                    let mut merged = s.unwrap_or_default();
                    merged.extend(arg);
                    (Some(merged), sub)
                } else {
                    (s, Some(arg))
                };
                out.push(Node::Scripts { base, sup: s, sub, limits });
                return;
            }
            out.push(Node::Scripts { base, sup: s, sub, limits });
            Node::Group(Vec::new())
        }
        Some(node @ (Node::Space(_) | Node::Tab | Node::Newline)) => {
            out.push(node);
            Node::Group(Vec::new())
        }
        Some(node) => node,
        None => Node::Group(Vec::new()),
    };
    let (s, sub) = if sup { (Some(arg), None) } else { (None, Some(arg)) };
    out.push(Node::Scripts { base: Box::new(base), sup: s, sub, limits: None });
}

/// `\limits` or `\nolimits` after an operator.
fn set_limits(out: &mut [Node], on: bool) {
    match out.last_mut() {
        Some(Node::Op { limits, .. }) => *limits = on,
        Some(Node::Scripts { limits, .. }) => *limits = Some(on),
        _ => {}
    }
}

fn style_switch(name: &str) -> Option<Style> {
    match name {
        "displaystyle" => Some(Style::Display),
        "textstyle" => Some(Style::Text),
        "scriptstyle" => Some(Style::Script),
        "scriptscriptstyle" => Some(Style::ScriptScript),
        _ => None,
    }
}

/// Old-style switches that change the font for the rest of the group.
fn font_switch(name: &str) -> Option<MathFont> {
    match name {
        "rm" => Some(MathFont::Roman),
        "bf" => Some(MathFont::Bold),
        "it" => Some(MathFont::Italic),
        "cal" => Some(MathFont::Calligraphic),
        "sf" => Some(MathFont::Sans),
        "tt" => Some(MathFont::Mono),
        _ => None,
    }
}

/// Commands that set their argument in another font.
fn font_command(name: &str) -> Option<MathFont> {
    match name {
        "mathrm" | "mathup" => Some(MathFont::Roman),
        "mathbf" | "bold" | "mathbfup" => Some(MathFont::Bold),
        "mathit" => Some(MathFont::Italic),
        "mathbb" | "Bbb" | "mathds" => Some(MathFont::Blackboard),
        "mathcal" => Some(MathFont::Calligraphic),
        "mathfrak" | "frak" => Some(MathFont::Fraktur),
        "mathscr" => Some(MathFont::Script),
        "mathsf" | "mathsfup" => Some(MathFont::Sans),
        "mathtt" => Some(MathFont::Mono),
        "boldsymbol" | "bm" | "pmb" | "mathbfit" => Some(MathFont::BoldSymbol),
        "mathnormal" => Some(MathFont::Default),
        _ => None,
    }
}

/// Upright operator names: their text.
fn named_operator(name: &str) -> Option<&'static str> {
    Some(match name {
        "sin" => "sin",
        "cos" => "cos",
        "tan" => "tan",
        "cot" => "cot",
        "sec" => "sec",
        "csc" => "csc",
        "arcsin" => "arcsin",
        "arccos" => "arccos",
        "arctan" => "arctan",
        "arctg" => "arctg",
        "arcctg" => "arcctg",
        "sinh" => "sinh",
        "cosh" => "cosh",
        "tanh" => "tanh",
        "coth" => "coth",
        "sh" => "sh",
        "ch" => "ch",
        "th" => "th",
        "tg" => "tg",
        "ctg" => "ctg",
        "cotg" => "cotg",
        "log" => "log",
        "ln" => "ln",
        "lg" => "lg",
        "exp" => "exp",
        "ker" => "ker",
        "dim" => "dim",
        "hom" => "hom",
        "deg" => "deg",
        "arg" => "arg",
        "lim" => "lim",
        "liminf" => "lim\u{2009}inf",
        "limsup" => "lim\u{2009}sup",
        "max" => "max",
        "min" => "min",
        "sup" => "sup",
        "inf" => "inf",
        "det" => "det",
        "gcd" => "gcd",
        "Pr" => "Pr",
        "injlim" => "inj\u{2009}lim",
        "projlim" => "proj\u{2009}lim",
        "tr" => "tr",
        "Tr" => "Tr",
        "rank" => "rank",
        "sgn" => "sgn",
        "sign" => "sign",
        "diag" => "diag",
        "span" => "span",
        "Var" => "Var",
        "Cov" => "Cov",
        "erf" => "erf",
        _ => return None,
    })
}

/// Delimiters by name, for `\left` and `\big`.
fn delim_named(name: &str) -> Option<char> {
    Some(match name {
        "{" | "lbrace" => '{',
        "}" | "rbrace" => '}',
        "|" | "Vert" | "lVert" | "rVert" => '\u{2016}',
        "vert" | "lvert" | "rvert" | "mid" => '|',
        "langle" | "lang" => '\u{27e8}',
        "rangle" | "rang" => '\u{27e9}',
        "lfloor" => '\u{230a}',
        "rfloor" => '\u{230b}',
        "lceil" => '\u{2308}',
        "rceil" => '\u{2309}',
        "lbrack" => '[',
        "rbrack" => ']',
        "backslash" => '\\',
        "uparrow" => '\u{2191}',
        "downarrow" => '\u{2193}',
        "updownarrow" => '\u{2195}',
        "Uparrow" => '\u{21d1}',
        "Downarrow" => '\u{21d3}',
        "lgroup" => '(',
        "rgroup" => ')',
        "." => NO_DELIM,
        _ => return None,
    })
}

/// A math symbol from KaTeX's table: its glyph, face, class, and whether it's an italic letter.
fn lookup(key: &str) -> Option<(char, Face, Class, bool)> {
    let i = tables::SYMBOLS.binary_search_by(|(name, ..)| name.as_bytes().cmp(key.as_bytes())).ok()?;
    let (_, ch, font, class, italic) = tables::SYMBOLS[i];
    let face = match font {
        super::layout::SymbolFont::Main => Face::Main,
        super::layout::SymbolFont::Ams => Face::Ams,
    };
    Some((ch, face, class, italic))
}

fn sym(ch: char, face: Face, class: Class, italic: bool, font: MathFont) -> Node {
    let face = if italic {
        match font {
            MathFont::Bold | MathFont::BoldSymbol => Face::MathBoldItalic,
            _ => Face::MathItalic,
        }
    } else {
        match font {
            MathFont::Bold | MathFont::BoldSymbol if face == Face::Main => Face::MainBold,
            _ => face,
        }
    };
    Node::Sym { ch, face, class }
}

/// A letter or digit in the current font.
fn letter(c: char, font: MathFont) -> Node {
    let digit = c.is_ascii_digit();
    let upper = c.is_ascii_uppercase();
    let face = match font {
        MathFont::Default => {
            if digit {
                Face::Main
            } else {
                Face::MathItalic
            }
        }
        MathFont::Roman => Face::Main,
        MathFont::Bold => Face::MainBold,
        MathFont::Italic => {
            if digit {
                Face::Main
            } else {
                Face::MainItalic
            }
        }
        MathFont::Blackboard if upper || c == 'k' => Face::Ams,
        MathFont::Calligraphic if upper => Face::Caligraphic,
        MathFont::Script if upper => Face::Script,
        MathFont::Fraktur => Face::Fraktur,
        MathFont::Sans => Face::SansSerif,
        MathFont::Mono => Face::Typewriter,
        MathFont::BoldSymbol => {
            if digit {
                Face::MainBold
            } else {
                Face::MathBoldItalic
            }
        }
        MathFont::Blackboard | MathFont::Calligraphic | MathFont::Script => Face::Main,
    };
    Node::Sym { ch: c, face, class: Class::Ord }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn syms(nodes: &[Node]) -> String {
        nodes
            .iter()
            .map(|n| match n {
                Node::Sym { ch, .. } => ch.to_string(),
                Node::Text { text, .. } => text.clone(),
                Node::Unknown(name) => name.clone(),
                Node::Group(g) => format!("{{{}}}", syms(g)),
                _ => "·".into(),
            })
            .collect()
    }

    #[test]
    fn parses_the_bayes_answer() {
        let nodes = parse(r" P(\theta=1\mid T=+) =\frac{0.99(0.001)}{0.99(0.001)+0.06(0.999)} \approx \boxed{0.0162482}. ");
        assert!(nodes.iter().any(|n| matches!(n, Node::Frac { .. })));
        assert!(nodes.iter().any(|n| matches!(n, Node::Boxed(_))));
        assert!(nodes.iter().any(|n| matches!(n, Node::Sym { ch: '\u{3b8}', face: Face::MathItalic, .. })));
        assert!(nodes.iter().any(|n| matches!(n, Node::Sym { ch: '\u{2223}', class: Class::Rel, .. })));
        assert!(nodes.iter().any(|n| matches!(n, Node::Sym { ch: '\u{2248}', class: Class::Rel, .. })));
        assert!(!nodes.iter().any(|n| matches!(n, Node::Unknown(_))));
    }

    #[test]
    fn scripts_attach_and_nest() {
        let nodes = parse("x_{i}^{2^{n}} + y'");
        let Node::Scripts { sup: Some(sup), sub: Some(sub), .. } = &nodes[0] else { panic!("{nodes:?}") };
        assert_eq!(syms(sub), "i");
        assert!(matches!(&sup[0], Node::Scripts { .. }));
        assert!(matches!(nodes.last(), Some(Node::Scripts { sup: Some(p), .. }) if syms(p) == "\u{2032}"));
        // Scripts on nothing still show.
        assert!(matches!(&parse("^2")[0], Node::Scripts { .. }));
    }

    #[test]
    fn fonts_and_text() {
        assert!(matches!(parse(r"\mathbb{R}")[0], Node::Group(ref g) if matches!(g[0], Node::Sym { ch: 'R', face: Face::Ams, .. })));
        assert!(matches!(parse(r"\mathbf{v}")[0], Node::Group(ref g) if matches!(g[0], Node::Sym { face: Face::MainBold, .. })));
        assert_eq!(syms(&parse(r"\text{if } x")), "{if }x");
        assert_eq!(syms(&parse(r"\text{costs \$5 and $x$}")), "{costs $5 and {x}}");
        assert!(matches!(parse(r"\operatorname{argmax}_x")[0], Node::Scripts { .. }));
        assert!(matches!(parse(r"\lim_{n\to\infty}")[0], Node::Scripts { ref base, .. } if matches!(**base, Node::Op { limits: true, .. })));
    }

    #[test]
    fn environments() {
        let Node::Array(a) = &parse(r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}")[0] else { panic!() };
        assert_eq!(a.rows.len(), 2);
        assert_eq!(a.rows[0].len(), 2);
        assert_eq!(a.delims, ('(', ')'));
        let Node::Array(c) = &parse(r"f(x)=\begin{cases} 1 & x>0 \\ 0 & \text{otherwise} \end{cases}")[5] else { panic!() };
        assert_eq!(c.kind, ArrayKind::Cases { display: false });
        let Node::Array(arr) = &parse(r"\begin{array}{c|c} 1 & 2 \\ \hline 3 & 4 \end{array}")[0] else { panic!() };
        assert_eq!(arr.col_rules, vec![1]);
        assert_eq!(arr.row_rules, vec![1]);
        let Node::Array(al) = &parse("\\begin{aligned} a &= b \\\\ &= c \\\\ \\end{aligned}")[0] else { panic!() };
        assert_eq!(al.rows.len(), 2);
    }

    #[test]
    fn left_right_and_middle() {
        let Node::LeftRight { delims, parts } = &parse(r"\left( \frac{a}{b} \middle| c \right]")[0] else { panic!() };
        assert_eq!(delims, &vec!['(', '|', ']']);
        assert_eq!(parts.len(), 2);
        // An unclosed `\left` closes at the end.
        assert!(matches!(&parse(r"\left\{ x")[0], Node::LeftRight { delims, .. } if delims == &vec!['{', NO_DELIM]));
    }

    #[test]
    fn unknown_commands_keep_their_names_and_arguments() {
        let nodes = parse(r"\foo{x} + \bar y");
        assert_eq!(syms(&nodes[..2]), "\\foo{x}");
        assert!(matches!(&parse(r"\%")[0], Node::Sym { ch: '%', .. }));
        assert!(matches!(&parse("50%")[2], Node::Sym { ch: '%', .. }));
    }

    #[test]
    fn broken_tex_never_panics_or_loses_text() {
        for s in [r"\frac{a}", r"\frac", r"x^", r"_", r"{{{", r"}}}", r"\left(", r"\right)", r"\begin{matrix} a &", r"\end{matrix}", r"\sqrt[", r"\sqrt[3", r"\text{abc", r"$", r"\\", r"\", r"\begin{array}{", r"a''''^2_3_4", r"\not", r"\hspace{", r"\kern", r"\genfrac{}{}{}{}"] {
            let _ = parse(s);
        }
        let nested = "{".repeat(500) + "x" + &"}".repeat(500);
        let _ = parse(&nested);
        let unbalanced = r"a}b";
        assert_eq!(syms(&parse(unbalanced)), "a}b");
    }

    #[test]
    fn multi_byte_whitespace_is_just_space() {
        for ws in ["\u{a0}", "\u{85}", "\u{2003}", "\u{202f}", "\u{3000}"] {
            assert_eq!(syms(&parse(&format!("a{ws}b"))), syms(&parse("a b")), "{ws:?}");
            let _ = parse(&format!("x{ws}={ws}1{ws}"));
            let _ = parse(&format!(r"\frac{ws}{{a}}{ws}{{b}}"));
            let _ = parse(&format!(r"\alpha{ws}\beta"));
        }
    }

    #[test]
    fn deep_text_braces_do_not_overflow_the_stack() {
        let _ = parse(&(r"\text{".to_string() + &"{".repeat(100_000) + "x"));
        let _ = parse(&(r"\text{".to_string() + &"{".repeat(100_000) + "x" + &"}".repeat(100_000)));
    }

    #[test]
    fn fuzz_never_panics() {
        let pieces = [r"\frac", r"\sqrt", "[", "]", "{", "}", "^", "_", "&", r"\\", r"\left", r"\right", r"\middle", "(", ")", "|", r"\begin{pmatrix}", r"\end{pmatrix}", r"\begin{cases}", r"\end{cases}", r"\begin{array}{c|l}", r"\end{array}", "x", "1", " ", r"\alpha", r"\text{", "$", r"\boxed", r"\hat", r"\overbrace", r"\over", r"\displaystyle", r"\not", r"\limits", "'", r"\hline", r"\big", r"\unknowncmd", "é", "日", r"\operatorname*", r"\xrightarrow[", r"\mathbb", r"\binom", r"\tag", r"\kern", "%", "~", r"\,", "\u{a0}", "\u{202f}", "\u{3000}", "\u{2003}", "\u{85}"];
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        for _ in 0..6000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let mut s = String::new();
            let mut x = seed;
            for _ in 0..(seed % 30) {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                s.push_str(pieces[(x % pieces.len() as u64) as usize]);
            }
            let _ = parse(&s);
        }
    }
}
