//! Maths in answers, drawn as maths. Agents answer maths questions in LaTeX, between `$…$`,
//! `$$…$$`, `\(…\)` or `\[…\]` (the markdown layer reads the last two as the first two: see
//! `parse_with_latex_delimiters` in the vendored `format/markdown.rs`). Printed as written it's
//! unreadable, so this module parses the LaTeX into a small tree (`parse`), sets the tree in
//! boxes the way TeX does, simplified (`Setter`), and paints the boxes on one canvas per formula:
//! glyph runs in the interface font (italic variables, upright digits and names), hairline rules
//! for fraction bars and overlines, and drawn strokes for radicals and tall delimiters.
//!
//! Inline maths sits on the text's baseline at its size. A formula alone in its paragraph, or a
//! `$$` block, is centred in the column, with a copy button for its source on hover. A command
//! the parser doesn't know draws as its name in mono on a faint tint, and the rest of the formula
//! still draws; a formula it can't parse at all draws as its source in mono.
//!
//! Prices stay prose: "$5 and $10" pairs up as a math span, and `claims` turns it down.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::text::{InlineElement, InlineRenderContext, MarkdownNode, MarkdownParseContext, MarkdownPlugin, markdown_ast};
use gpui_kit::component::{Icon, IconName, Sizable as _};
use gpui_kit::*;

const INLINE: &str = "trek-math";
const DISPLAY: &str = "trek-math-block";

/// The longest formula Trek sets; past this it shows the source.
const MAX_SOURCE: usize = 8_000;
/// How deep groups may nest before the parser gives up (and the source shows).
const MAX_DEPTH: usize = 48;

// ---------- the tree ----------

/// How an atom spaces against its neighbours, as TeX classes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Class {
    Ord,
    /// A large operator or a function name: `\sum`, `\sin`.
    Op,
    /// A binary operator: `+`, `\times`.
    Bin,
    /// A relation: `=`, `\le`, `\to`.
    Rel,
    Open,
    Close,
    Punct,
    /// A fraction or a `\left…\right` group.
    Inner,
}

/// The face a run is set in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Face {
    /// Variables.
    Italic,
    /// Digits, names, operators, `\text`.
    Upright,
    Bold,
    BoldItalic,
    /// Commands Trek doesn't know, and formulas it couldn't parse.
    Mono,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Accent {
    Hat,
    Tilde,
    Dot,
    Ddot,
    Check,
    Breve,
    Acute,
    Grave,
    Bar,
    Vec,
    Underline,
}

/// How a table's columns line up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Columns {
    /// `aligned`: right, left, right, left…, the pairs a gap apart.
    Aligned,
    /// `cases`: every column left.
    Left,
    /// `matrix`, `gathered`: every column centred.
    Centred,
}

/// A parsed formula.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Node {
    /// A symbol or a run of text in one face: `x`, `12.5`, `+`, `\text{if}`.
    Sym { text: String, face: Face, class: Class },
    /// A large operator (`glyph`: `\sum`, `\int`) or an operator name (`\lim`, `\sin`).
    /// `limits`: in display maths its scripts go above and below it.
    Op { text: String, glyph: bool, limits: bool },
    Group(Vec<Node>),
    /// `bar`: false for `\binom`'s stack. `style`: `\dfrac` and `\tfrac` force one.
    Frac { num: Box<Node>, den: Box<Node>, bar: bool, style: Option<Style> },
    Scripts { base: Box<Node>, sup: Option<Box<Node>>, sub: Option<Box<Node>> },
    /// `\overset`, `\underset`, `\stackrel`: always stacked.
    Stack { base: Box<Node>, over: Option<Box<Node>>, under: Option<Box<Node>> },
    Sqrt { index: Option<Box<Node>>, body: Box<Node> },
    /// `\left( … \right)`: delimiters sized to the body. `None` is `.`, no delimiter.
    Delimited { left: Option<char>, body: Box<Node>, right: Option<char> },
    /// `\big(` and its kin: a delimiter `size` ems tall.
    Big { delim: Option<char>, size: f32, class: Class },
    Accent { accent: Accent, body: Box<Node> },
    Boxed(Box<Node>),
    /// Room, in ems: `\,`, `\quad`, `\!`.
    Space(f32),
    /// Takes the room its body would, draws nothing: `\phantom`.
    Phantom(Box<Node>),
    Table { rows: Vec<Vec<Node>>, columns: Columns },
    /// A command Trek doesn't know, by name.
    Unknown(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Style {
    Display,
    Text,
    Script,
    ScriptScript,
}

impl Style {
    fn scale(self) -> f32 {
        match self {
            Style::Display | Style::Text => 1.,
            Style::Script => 0.7,
            Style::ScriptScript => 0.5,
        }
    }

    fn script(self) -> Style {
        match self {
            Style::Display | Style::Text => Style::Script,
            Style::Script | Style::ScriptScript => Style::ScriptScript,
        }
    }

    /// A fraction's numerator and denominator.
    fn fraction(self) -> Style {
        match self {
            Style::Display => Style::Text,
            other => other.script(),
        }
    }

    fn tight(self) -> bool {
        matches!(self, Style::Script | Style::ScriptScript)
    }
}

// ---------- parsing ----------

/// Why a formula couldn't be parsed; it shows as its source instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Unparsed;

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    /// `\name`, or `\` and one other character (`\,`, `\\`, `\{`).
    Cmd(String),
    Char(char),
    Open,
    Close,
    Sup,
    Sub,
    Amp,
    Space,
}

fn tokens(src: &str) -> Result<Vec<Tok>, Unparsed> {
    let mut out = Vec::new();
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                let Some(&next) = chars.peek() else { return Err(Unparsed) };
                if next.is_ascii_alphabetic() {
                    let mut name = String::new();
                    while let Some(&l) = chars.peek().filter(|l| l.is_ascii_alphabetic()) {
                        name.push(l);
                        chars.next();
                    }
                    out.push(Tok::Cmd(name));
                } else {
                    chars.next();
                    // `\` and a line break is a space, as `\ ` is.
                    out.push(Tok::Cmd(if next.is_whitespace() { " ".into() } else { next.to_string() }));
                }
            }
            '{' => out.push(Tok::Open),
            '}' => out.push(Tok::Close),
            '^' => out.push(Tok::Sup),
            '_' => out.push(Tok::Sub),
            '&' => out.push(Tok::Amp),
            c if c.is_whitespace() => {
                if out.last() != Some(&Tok::Space) {
                    out.push(Tok::Space);
                }
            }
            c => out.push(Tok::Char(c)),
        }
    }
    Ok(out)
}

/// What ends a list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    Eof,
    /// `}`
    Brace,
    /// `]`, ending `\sqrt`'s index.
    Bracket,
    /// `\right`
    Right,
    /// `&`
    Amp,
    /// `\\`
    Row,
    /// `\end`
    End,
}

/// A font command's hold on the letters and digits inside it: `\mathbf{x}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Letters {
    Face(Face),
    Blackboard,
    Calligraphic,
    Fraktur,
}

struct Parser {
    toks: Vec<Tok>,
    at: usize,
    depth: usize,
    letters: Option<Letters>,
}

/// Parse `src`, a formula's LaTeX without its delimiters.
pub(crate) fn parse(src: &str) -> Result<Node, Unparsed> {
    if src.len() > MAX_SOURCE {
        return Err(Unparsed);
    }
    let mut parser = Parser { toks: tokens(src)?, at: 0, depth: 0, letters: None };
    let (first, stop) = parser.list(&[Stop::Eof, Stop::Amp, Stop::Row])?;
    if stop == Stop::Eof {
        return Ok(Node::Group(first));
    }
    // `&` and `\\` outside an environment: lines of display maths, aligned when they say so.
    let rows = parser.rows(first, stop, Stop::Eof)?;
    let columns = if rows.iter().any(|row| row.len() > 1) { Columns::Aligned } else { Columns::Centred };
    Ok(Node::Table { rows, columns })
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.at)
    }

    fn next(&mut self) -> Option<Tok> {
        let tok = self.toks.get(self.at).cloned();
        self.at += 1;
        tok
    }

    fn skip_spaces(&mut self) {
        while self.peek() == Some(&Tok::Space) {
            self.at += 1;
        }
    }

    fn deeper(&mut self) -> Result<(), Unparsed> {
        self.depth += 1;
        if self.depth > MAX_DEPTH { Err(Unparsed) } else { Ok(()) }
    }

    /// Atoms up to one of `stops`, which is consumed and returned. Anything else that ends a
    /// list (a `}` with no `{`, a `\right` with no `\left`) is a formula Trek can't read.
    fn list(&mut self, stops: &[Stop]) -> Result<(Vec<Node>, Stop), Unparsed> {
        self.deeper()?;
        let mut items: Vec<Node> = Vec::new();
        let stop = loop {
            let Some(tok) = self.next() else {
                if stops.contains(&Stop::Eof) {
                    break Stop::Eof;
                }
                return Err(Unparsed);
            };
            let ended = match &tok {
                Tok::Close => Some(Stop::Brace),
                Tok::Char(']') if stops.contains(&Stop::Bracket) => Some(Stop::Bracket),
                Tok::Amp => Some(Stop::Amp),
                Tok::Cmd(name) if name == "\\" || name == "cr" => Some(Stop::Row),
                Tok::Cmd(name) if name == "right" => Some(Stop::Right),
                Tok::Cmd(name) if name == "end" => Some(Stop::End),
                _ => None,
            };
            if let Some(stop) = ended {
                if !stops.contains(&stop) {
                    return Err(Unparsed);
                }
                if stop == Stop::Row {
                    self.skip_row_spacing();
                }
                break stop;
            }
            match tok {
                Tok::Space => {}
                Tok::Sup | Tok::Sub => {
                    let script = self.arg()?;
                    attach(&mut items, tok == Tok::Sup, script);
                }
                Tok::Char('\'') => {
                    // A run of `'` is an ordinary atom: the prime glyph is already raised, so a
                    // superscript slot would double-raise it, and `f'^2` should still read `f′²`.
                    let mut primes = String::from("′");
                    while self.peek() == Some(&Tok::Char('\'')) {
                        self.at += 1;
                        primes.push('′');
                    }
                    items.push(Node::Sym { text: primes, face: Face::Upright, class: Class::Ord });
                }
                Tok::Cmd(name) if name == "limits" || name == "nolimits" => {
                    if let Some(Node::Op { limits, .. }) = items.last_mut() {
                        *limits = name == "limits";
                    }
                }
                tok => {
                    if let Some(node) = self.atom(tok)? {
                        items.push(node);
                    }
                }
            }
        };
        self.depth -= 1;
        Ok((items, stop))
    }

    /// `\\[4pt]`: the extra room is ignored.
    fn skip_row_spacing(&mut self) {
        let start = self.at;
        self.skip_spaces();
        if self.peek() == Some(&Tok::Char('[')) {
            while let Some(tok) = self.next() {
                if tok == Tok::Char(']') {
                    return;
                }
            }
        }
        self.at = start;
    }

    /// The rows of a table whose first cell is `first`, ended by `stop`, up to `end`.
    fn rows(&mut self, first: Vec<Node>, mut stop: Stop, end: Stop) -> Result<Vec<Vec<Node>>, Unparsed> {
        let mut rows = vec![vec![Node::Group(first)]];
        while stop != end {
            if stop == Stop::Row {
                rows.push(Vec::new());
            }
            let (cell, next) = self.list(&[Stop::Amp, Stop::Row, end])?;
            rows.last_mut().expect("a row").push(Node::Group(cell));
            stop = next;
        }
        // A trailing `\\` leaves an empty last row.
        if rows.len() > 1 && rows.last().is_some_and(|row| row.iter().all(|cell| matches!(cell, Node::Group(items) if items.is_empty()))) {
            rows.pop();
        }
        Ok(rows)
    }

    /// One argument: a `{group}`, or a single token (`\frac12`).
    fn arg(&mut self) -> Result<Node, Unparsed> {
        // `\sqrt\sqrt\sqrt…` nests without a brace: count it too.
        self.deeper()?;
        self.skip_spaces();
        let node = match self.next() {
            Some(Tok::Open) => Node::Group(self.list(&[Stop::Brace])?.0),
            // An argument without braces is one token: `\frac12` is ½, not 12 over nothing.
            Some(Tok::Char(c)) if c.is_ascii_digit() => self.lettered(c.to_string(), Face::Upright, Class::Ord),
            Some(tok @ (Tok::Char(_) | Tok::Cmd(_))) =>self.atom(tok)?.unwrap_or(Node::Group(Vec::new())),
            _ => return Err(Unparsed),
        };
        self.depth -= 1;
        Ok(unwrap(node))
    }

    /// `[…]` if there is one.
    fn optional(&mut self) -> Result<Option<Node>, Unparsed> {
        self.skip_spaces();
        if self.peek() != Some(&Tok::Char('[')) {
            return Ok(None);
        }
        self.at += 1;
        Ok(Some(unwrap(Node::Group(self.list(&[Stop::Bracket])?.0))))
    }

    /// An argument read as plain text: `\text{if } x`.
    fn text(&mut self) -> Result<String, Unparsed> {
        self.skip_spaces();
        match self.next() {
            Some(Tok::Open) => {}
            Some(Tok::Char(c)) => return Ok(c.to_string()),
            _ => return Err(Unparsed),
        }
        let mut out = String::new();
        let mut depth = 0usize;
        loop {
            match self.next().ok_or(Unparsed)? {
                Tok::Open => depth += 1,
                Tok::Close if depth == 0 => break,
                Tok::Close => depth -= 1,
                Tok::Char('$') => {}
                Tok::Char('~') | Tok::Space => out.push(' '),
                Tok::Char(c) => out.push(c),
                Tok::Sup => out.push('^'),
                Tok::Sub => out.push('_'),
                Tok::Amp => out.push('&'),
                Tok::Cmd(name) => match name.as_str() {
                    " " | "\\" | "," | ";" | "quad" | "qquad" => out.push(' '),
                    "%" | "$" | "_" | "&" | "#" | "{" | "}" => out.push_str(&name),
                    // A command in text (`\textbf{…}`, `\ldots`): its symbol if it has one, else
                    // just its argument's text.
                    other => {
                        if let Some((sym, _, _)) = symbol(other) {
                            out.push_str(sym);
                        }
                    }
                },
            }
        }
        Ok(out)
    }

    /// A delimiter after `\left`, `\right` or `\big`.
    fn delimiter(&mut self) -> Result<Option<char>, Unparsed> {
        self.skip_spaces();
        let delim = match self.next().ok_or(Unparsed)? {
            Tok::Char('.') => return Ok(None),
            Tok::Char(c @ ('(' | ')' | '[' | ']' | '|' | '/')) => c,
            Tok::Char('<') => '⟨',
            Tok::Char('>') => '⟩',
            Tok::Cmd(name) => match name.as_str() {
                "{" | "lbrace" => '{',
                "}" | "rbrace" => '}',
                "|" | "Vert" | "lVert" | "rVert" => '‖',
                "vert" | "lvert" | "rvert" | "mid" => '|',
                "langle" => '⟨',
                "rangle" => '⟩',
                "lfloor" => '⌊',
                "rfloor" => '⌋',
                "lceil" => '⌈',
                "rceil" => '⌉',
                "lbrack" => '[',
                "rbrack" => ']',
                "backslash" => '\\',
                _ => return Err(Unparsed),
            },
            _ => return Err(Unparsed),
        };
        Ok(Some(delim))
    }

    /// `arg` with `letters` set in `face`.
    fn arg_in(&mut self, letters: Letters) -> Result<Node, Unparsed> {
        let outer = self.letters.replace(letters);
        let node = self.arg();
        self.letters = outer;
        node
    }

    /// One atom from `tok`; `None` for commands that draw nothing (`\displaystyle`).
    fn atom(&mut self, tok: Tok) -> Result<Option<Node>, Unparsed> {
        Ok(Some(match tok {
            Tok::Open => Node::Group(self.list(&[Stop::Brace])?.0),
            Tok::Char(c) => self.character(c),
            Tok::Cmd(name) => return self.command(&name),
            _ => return Err(Unparsed),
        }))
    }

    fn character(&mut self, c: char) -> Node {
        if c.is_ascii_digit() || c == '.' && matches!(self.peek(), Some(Tok::Char(d)) if d.is_ascii_digit()) {
            // A number is one run: `0.0162482`.
            let mut text = c.to_string();
            loop {
                match self.peek() {
                    Some(Tok::Char(d)) if d.is_ascii_digit() => text.push(*d),
                    Some(Tok::Char('.')) if matches!(self.toks.get(self.at + 1), Some(Tok::Char(d)) if d.is_ascii_digit()) => text.push('.'),
                    _ => break,
                }
                self.at += 1;
            }
            return self.lettered(text, Face::Upright, Class::Ord);
        }
        if c.is_alphabetic() {
            return self.lettered(c.to_string(), Face::Italic, Class::Ord);
        }
        let class = match c {
            '+' | '-' | '−' | '*' => Class::Bin,
            '=' | '<' | '>' | ':' => Class::Rel,
            '(' | '[' => Class::Open,
            ')' | ']' | '!' | '?' => Class::Close,
            ',' | ';' => Class::Punct,
            '~' => return Node::Space(0.25),
            other => unicode_class(other),
        };
        // A hyphen typed for minus is the minus sign; `*` the centred asterisk.
        let text = match c {
            '-' => '−',
            '*' => '∗',
            other => other,
        };
        Node::Sym { text: text.to_string(), face: Face::Upright, class }
    }

    /// A run of letters or digits, in the face a font command holds them in.
    fn lettered(&self, text: String, face: Face, class: Class) -> Node {
        let (text, face) = match self.letters {
            None => (text, face),
            Some(Letters::Face(Face::BoldItalic)) => (text, if face == Face::Italic { Face::BoldItalic } else { Face::Bold }),
            Some(Letters::Face(held)) => (text, held),
            Some(Letters::Blackboard) => (text.chars().map(blackboard).collect(), Face::Upright),
            Some(Letters::Calligraphic) => (text.chars().map(calligraphic).collect(), Face::Upright),
            Some(Letters::Fraktur) => (text.chars().map(fraktur).collect(), Face::Upright),
        };
        Node::Sym { text, face, class }
    }

    fn command(&mut self, name: &str) -> Result<Option<Node>, Unparsed> {
        if let Some((text, class, italic)) = symbol(name) {
            let face = if italic { Face::Italic } else { Face::Upright };
            // `\mathbf{\alpha}`: Greek letters follow the font command too.
            let node = if italic { self.lettered(text.to_string(), face, class) } else { Node::Sym { text: text.to_string(), face, class } };
            return Ok(Some(node));
        }
        if let Some((text, limits)) = operator_name(name) {
            return Ok(Some(Node::Op { text: text.to_string(), glyph: false, limits }));
        }
        if let Some((text, limits)) = large_operator(name) {
            return Ok(Some(Node::Op { text: text.to_string(), glyph: true, limits }));
        }
        if let Some(em) = space(name) {
            return Ok(Some(Node::Space(em)));
        }
        if let Some(accent) = accent(name) {
            return Ok(Some(Node::Accent { accent, body: Box::new(self.arg()?) }));
        }
        let node = match name {
            "frac" | "dfrac" | "tfrac" | "cfrac" => {
                let style = match name {
                    "dfrac" | "cfrac" => Some(Style::Display),
                    "tfrac" => Some(Style::Text),
                    _ => None,
                };
                let num = self.arg()?;
                let den = self.arg()?;
                Node::Frac { num: Box::new(num), den: Box::new(den), bar: true, style }
            }
            "binom" | "dbinom" | "tbinom" => {
                let style = match name {
                    "dbinom" => Some(Style::Display),
                    "tbinom" => Some(Style::Text),
                    _ => None,
                };
                let num = self.arg()?;
                let den = self.arg()?;
                let stack = Node::Frac { num: Box::new(num), den: Box::new(den), bar: false, style };
                Node::Delimited { left: Some('('), body: Box::new(stack), right: Some(')') }
            }
            "sqrt" => {
                let index = self.optional()?;
                Node::Sqrt { index: index.map(Box::new), body: Box::new(self.arg()?) }
            }
            "left" => {
                let left = self.delimiter()?;
                let (body, _) = self.list(&[Stop::Right])?;
                let right = self.delimiter()?;
                Node::Delimited { left, body: Box::new(Node::Group(body)), right }
            }
            "middle" => {
                let delim = self.delimiter()?;
                Node::Sym { text: delim.map(String::from).unwrap_or_default(), face: Face::Upright, class: Class::Rel }
            }
            "big" | "Big" | "bigg" | "Bigg" | "bigl" | "Bigl" | "biggl" | "Biggl" | "bigr" | "Bigr" | "biggr" | "Biggr" | "bigm" | "Bigm" | "biggm" | "Biggm" => {
                let size = match name.trim_end_matches(['l', 'r', 'm']) {
                    "big" => 1.2,
                    "Big" => 1.8,
                    "bigg" => 2.4,
                    _ => 3.0,
                };
                let delim = self.delimiter()?;
                let class = match name.as_bytes().last() {
                    Some(b'l') => Class::Open,
                    Some(b'r') => Class::Close,
                    Some(b'm') => Class::Rel,
                    _ => match delim {
                        Some('(' | '[' | '{' | '⟨' | '⌊' | '⌈') => Class::Open,
                        Some(')' | ']' | '}' | '⟩' | '⌋' | '⌉') => Class::Close,
                        _ => Class::Ord,
                    },
                };
                Node::Big { delim, size, class }
            }
            "text" | "textrm" | "textnormal" | "textup" | "textsf" | "mbox" | "hbox" => Node::Sym { text: self.text()?, face: Face::Upright, class: Class::Ord },
            "textbf" => Node::Sym { text: self.text()?, face: Face::Bold, class: Class::Ord },
            "textit" | "emph" => Node::Sym { text: self.text()?, face: Face::Italic, class: Class::Ord },
            "texttt" => Node::Sym { text: self.text()?, face: Face::Mono, class: Class::Ord },
            "mathrm" | "mathup" | "mathsf" | "rm" | "mathnormal" => self.arg_in(Letters::Face(Face::Upright))?,
            "mathbf" | "bf" => self.arg_in(Letters::Face(Face::Bold))?,
            "boldsymbol" | "bm" | "pmb" => self.arg_in(Letters::Face(Face::BoldItalic))?,
            "mathit" | "it" => self.arg_in(Letters::Face(Face::Italic))?,
            "mathtt" => self.arg_in(Letters::Face(Face::Mono))?,
            "mathbb" | "Bbb" => self.arg_in(Letters::Blackboard)?,
            "mathcal" | "mathscr" => self.arg_in(Letters::Calligraphic)?,
            "mathfrak" => self.arg_in(Letters::Fraktur)?,
            "operatorname" => {
                let limits = self.peek() == Some(&Tok::Char('*'));
                if limits {
                    self.at += 1;
                }
                Node::Op { text: self.text()?, glyph: false, limits }
            }
            "boxed" | "fbox" | "framebox" => {
                if name == "fbox" || name == "framebox" {
                    Node::Boxed(Box::new(Node::Sym { text: self.text()?, face: Face::Upright, class: Class::Ord }))
                } else {
                    Node::Boxed(Box::new(self.arg()?))
                }
            }
            "overset" | "stackrel" => {
                let over = self.arg()?;
                let base = self.arg()?;
                let base = Node::Group(vec![base]);
                Node::Stack { base: Box::new(base), over: Some(Box::new(over)), under: None }
            }
            "underset" => {
                let under = self.arg()?;
                let base = self.arg()?;
                Node::Stack { base: Box::new(Node::Group(vec![base])), over: None, under: Some(Box::new(under)) }
            }
            "xrightarrow" | "xleftarrow" => {
                let under = self.optional()?;
                let over = self.arg()?;
                let arrow = if name == "xrightarrow" { "⟶" } else { "⟵" };
                let base = Node::Sym { text: arrow.into(), face: Face::Upright, class: Class::Rel };
                return Ok(Some(Node::Stack { base: Box::new(base), over: Some(Box::new(over)), under: under.map(Box::new) }));
            }
            "phantom" | "vphantom" | "hphantom" => Node::Phantom(Box::new(self.arg()?)),
            "substack" => {
                self.skip_spaces();
                if self.next() != Some(Tok::Open) {
                    return Err(Unparsed);
                }
                let (first, stop) = self.list(&[Stop::Row, Stop::Brace])?;
                let rows = self.rows(first, stop, Stop::Brace)?;
                Node::Table { rows, columns: Columns::Centred }
            }
            "begin" => return self.environment().map(Some),
            "not" => {
                // `\not=`, `\not\in`: the symbol struck through.
                self.skip_spaces();
                let Some(tok) = self.next() else { return Err(Unparsed) };
                match self.atom(tok)? {
                    Some(Node::Sym { text, face, class }) => Node::Sym { text: negated(&text), face, class },
                    other => return Ok(other),
                }
            }
            "pmod" => {
                let body = self.arg()?;
                let word = Node::Sym { text: "mod".into(), face: Face::Upright, class: Class::Ord };
                Node::Group(vec![Node::Space(0.5), Node::Sym { text: "(".into(), face: Face::Upright, class: Class::Open }, word, Node::Space(1. / 3.), body, Node::Sym { text: ")".into(), face: Face::Upright, class: Class::Close }])
            }
            "color" => {
                self.text()?;
                return Ok(None);
            }
            "textcolor" | "colorbox" => {
                self.text()?;
                self.arg()?
            }
            "cancel" | "bcancel" | "xcancel" | "mathop" | "mathord" | "mathbin" | "mathrel" => self.arg()?,
            "label" | "tag" => {
                if self.peek() == Some(&Tok::Char('*')) {
                    self.at += 1;
                }
                self.text()?;
                return Ok(None);
            }
            "displaystyle" | "textstyle" | "scriptstyle" | "scriptscriptstyle" | "nonumber" | "notag" | "hline" | "mathstrut" | "strut" | "relax" | "allowbreak" | "nobreak" => return Ok(None),
            _ => Node::Unknown(name.to_string()),
        };
        Ok(Some(node))
    }

    /// `\begin{name} … \end{name}`.
    fn environment(&mut self) -> Result<Node, Unparsed> {
        let name = self.text()?;
        let name = name.trim();
        if matches!(name, "array" | "darray" | "subarray" | "alignat" | "alignat*") {
            // The column spec (`{cc|c}`) or column count.
            self.text()?;
        }
        let (first, stop) = self.list(&[Stop::Amp, Stop::Row, Stop::End])?;
        let rows = self.rows(first, stop, Stop::End)?;
        if self.text()?.trim() != name {
            return Err(Unparsed);
        }
        let (columns, left, right) = match name {
            "aligned" | "align" | "align*" | "alignat" | "alignat*" | "alignedat" | "eqnarray" | "eqnarray*" | "split" | "flalign" | "flalign*" => (Columns::Aligned, None, None),
            "cases" | "dcases" => (Columns::Left, Some('{'), None),
            "rcases" => (Columns::Left, None, Some('}')),
            "pmatrix" => (Columns::Centred, Some('('), Some(')')),
            "bmatrix" => (Columns::Centred, Some('['), Some(']')),
            "Bmatrix" => (Columns::Centred, Some('{'), Some('}')),
            "vmatrix" => (Columns::Centred, Some('|'), Some('|')),
            "Vmatrix" => (Columns::Centred, Some('‖'), Some('‖')),
            _ => (Columns::Centred, None, None),
        };
        let table = Node::Table { rows, columns };
        Ok(if left.is_some() || right.is_some() { Node::Delimited { left, body: Box::new(table), right } } else { table })
    }
}

/// A group of one is that one: `x^{2}` is `x^2`.
fn unwrap(node: Node) -> Node {
    match node {
        Node::Group(mut items) if items.len() == 1 => items.pop().expect("one item"),
        other => other,
    }
}

/// Hang `script` on the last atom, or on nothing when it comes first.
fn attach(items: &mut Vec<Node>, sup: bool, script: Node) {
    let base = items.pop().unwrap_or(Node::Group(Vec::new()));
    let (base, mut old_sup, mut old_sub) = match base {
        Node::Scripts { base, sup, sub } => (*base, sup, sub),
        other => (other, None, None),
    };
    let slot = if sup { &mut old_sup } else { &mut old_sub };
    match slot.as_deref_mut() {
        // `x^a^b` and more: scripts after the first join the slot's group, side by side.
        Some(Node::Group(items)) => items.push(script),
        Some(existing) => {
            let first = std::mem::replace(existing, Node::Group(Vec::new()));
            *existing = Node::Group(vec![first, script]);
        }
        None => *slot = Some(Box::new(script)),
    }
    items.push(Node::Scripts { base: Box::new(base), sup: old_sup, sub: old_sub });
}

fn negated(text: &str) -> String {
    match text {
        "=" => "≠".into(),
        "∈" => "∉".into(),
        "⊂" => "⊄".into(),
        "⊆" => "⊈".into(),
        "≡" => "≢".into(),
        "<" => "≮".into(),
        ">" => "≯".into(),
        "∼" => "≁".into(),
        "≈" => "≉".into(),
        other => format!("{other}\u{338}"),
    }
}

/// Symbols by command name: the text, its class, and whether it's a letter set in italic.
fn symbol(name: &str) -> Option<(&'static str, Class, bool)> {
    use Class::*;
    let greek = |s| Some((s, Ord, true));
    let upright = |s, class| Some((s, class, false));
    match name {
        "alpha" => greek("α"),
        "beta" => greek("β"),
        "gamma" => greek("γ"),
        "delta" => greek("δ"),
        "epsilon" => greek("ϵ"),
        "varepsilon" => greek("ε"),
        "zeta" => greek("ζ"),
        "eta" => greek("η"),
        "theta" => greek("θ"),
        "vartheta" => greek("ϑ"),
        "iota" => greek("ι"),
        "kappa" => greek("κ"),
        "lambda" => greek("λ"),
        "mu" => greek("μ"),
        "nu" => greek("ν"),
        "xi" => greek("ξ"),
        "omicron" => greek("ο"),
        "pi" => greek("π"),
        "varpi" => greek("ϖ"),
        "rho" => greek("ρ"),
        "varrho" => greek("ϱ"),
        "sigma" => greek("σ"),
        "varsigma" => greek("ς"),
        "tau" => greek("τ"),
        "upsilon" => greek("υ"),
        "phi" => greek("ϕ"),
        "varphi" => greek("φ"),
        "chi" => greek("χ"),
        "psi" => greek("ψ"),
        "omega" => greek("ω"),
        "Gamma" => upright("Γ", Ord),
        "Delta" => upright("Δ", Ord),
        "Theta" => upright("Θ", Ord),
        "Lambda" => upright("Λ", Ord),
        "Xi" => upright("Ξ", Ord),
        "Pi" => upright("Π", Ord),
        "Sigma" => upright("Σ", Ord),
        "Upsilon" => upright("Υ", Ord),
        "Phi" => upright("Φ", Ord),
        "Psi" => upright("Ψ", Ord),
        "Omega" => upright("Ω", Ord),
        // Binary operators.
        "cdot" => upright("·", Bin),
        "times" => upright("×", Bin),
        "div" => upright("÷", Bin),
        "pm" => upright("±", Bin),
        "mp" => upright("∓", Bin),
        "ast" => upright("∗", Bin),
        "star" => upright("⋆", Bin),
        "circ" => upright("∘", Bin),
        "bullet" => upright("•", Bin),
        "cup" => upright("∪", Bin),
        "cap" => upright("∩", Bin),
        "setminus" => upright("∖", Bin),
        "oplus" => upright("⊕", Bin),
        "ominus" => upright("⊖", Bin),
        "otimes" => upright("⊗", Bin),
        "odot" => upright("⊙", Bin),
        "land" | "wedge" => upright("∧", Bin),
        "lor" | "vee" => upright("∨", Bin),
        "mod" | "bmod" => upright("mod", Bin),
        // Relations.
        "le" | "leq" => upright("≤", Rel),
        "ge" | "geq" => upright("≥", Rel),
        "leqslant" => upright("⩽", Rel),
        "geqslant" => upright("⩾", Rel),
        "ne" | "neq" => upright("≠", Rel),
        "approx" => upright("≈", Rel),
        "sim" => upright("∼", Rel),
        "simeq" => upright("≃", Rel),
        "cong" => upright("≅", Rel),
        "equiv" => upright("≡", Rel),
        "propto" => upright("∝", Rel),
        "ll" => upright("≪", Rel),
        "gg" => upright("≫", Rel),
        "prec" => upright("≺", Rel),
        "succ" => upright("≻", Rel),
        "to" | "rightarrow" => upright("→", Rel),
        "leftarrow" | "gets" => upright("←", Rel),
        "leftrightarrow" => upright("↔", Rel),
        "Rightarrow" => upright("⇒", Rel),
        "Leftarrow" => upright("⇐", Rel),
        "Leftrightarrow" => upright("⇔", Rel),
        "implies" | "Longrightarrow" => upright("⟹", Rel),
        "impliedby" | "Longleftarrow" => upright("⟸", Rel),
        "iff" | "Longleftrightarrow" => upright("⟺", Rel),
        "longrightarrow" => upright("⟶", Rel),
        "longleftarrow" => upright("⟵", Rel),
        "mapsto" => upright("↦", Rel),
        "uparrow" => upright("↑", Rel),
        "downarrow" => upright("↓", Rel),
        // Inter has no U+2223; the ASCII bar is the same mark and doesn't fall back.
        "mid" => upright("|", Rel),
        "nmid" => upright("∤", Rel),
        "parallel" => upright("∥", Rel),
        "perp" => upright("⊥", Rel),
        "in" => upright("∈", Rel),
        "notin" => upright("∉", Rel),
        "ni" => upright("∋", Rel),
        "subset" => upright("⊂", Rel),
        "subseteq" => upright("⊆", Rel),
        "supset" => upright("⊃", Rel),
        "supseteq" => upright("⊇", Rel),
        "vdash" => upright("⊢", Rel),
        "models" => upright("⊨", Rel),
        "coloneqq" => upright("≔", Rel),
        "triangleq" => upright("≜", Rel),
        "colon" => upright(":", Punct),
        // Ordinary symbols.
        "infty" => upright("∞", Ord),
        "partial" => upright("∂", Ord),
        "nabla" => upright("∇", Ord),
        "forall" => upright("∀", Ord),
        "exists" => upright("∃", Ord),
        "nexists" => upright("∄", Ord),
        "neg" | "lnot" => upright("¬", Ord),
        "emptyset" | "varnothing" => upright("∅", Ord),
        "ldots" | "dots" | "dotsc" | "dotsb" | "dotso" => upright("…", Ord),
        "cdots" => upright("⋯", Ord),
        "vdots" => upright("⋮", Ord),
        "ddots" => upright("⋱", Ord),
        "prime" => upright("′", Ord),
        "degree" => upright("°", Ord),
        "angle" => upright("∠", Ord),
        "triangle" => upright("△", Ord),
        "top" => upright("⊤", Ord),
        "bot" => upright("⊥", Ord),
        "hbar" => upright("ℏ", Ord),
        "ell" => greek("ℓ"),
        "Re" => upright("ℜ", Ord),
        "Im" => upright("ℑ", Ord),
        "aleph" => upright("ℵ", Ord),
        "dagger" => upright("†", Ord),
        "checkmark" => upright("✓", Ord),
        "%" => upright("%", Ord),
        "#" => upright("#", Ord),
        "&" => upright("&", Ord),
        "$" => upright("$", Ord),
        "_" => upright("_", Ord),
        "|" | "Vert" => upright("‖", Ord),
        "vert" => upright("|", Ord),
        "backslash" => upright("\\", Ord),
        "{" | "lbrace" => upright("{", Open),
        "}" | "rbrace" => upright("}", Close),
        "langle" => upright("⟨", Open),
        "rangle" => upright("⟩", Close),
        "lfloor" => upright("⌊", Open),
        "rfloor" => upright("⌋", Close),
        "lceil" => upright("⌈", Open),
        "rceil" => upright("⌉", Close),
        "lvert" => upright("|", Open),
        "rvert" => upright("|", Close),
        "lVert" => upright("‖", Open),
        "rVert" => upright("‖", Close),
        _ => None,
    }
}

/// Operator names set upright: the text and whether its scripts go above and below in display.
fn operator_name(name: &str) -> Option<(&'static str, bool)> {
    let text = match name {
        "sin" => "sin",
        "cos" => "cos",
        "tan" => "tan",
        "sec" => "sec",
        "csc" => "csc",
        "cot" => "cot",
        "arcsin" => "arcsin",
        "arccos" => "arccos",
        "arctan" => "arctan",
        "sinh" => "sinh",
        "cosh" => "cosh",
        "tanh" => "tanh",
        "log" => "log",
        "ln" => "ln",
        "lg" => "lg",
        "exp" => "exp",
        "deg" => "deg",
        "dim" => "dim",
        "ker" => "ker",
        "arg" => "arg",
        "hom" => "hom",
        "Pr" => return Some(("Pr", true)),
        "min" => return Some(("min", true)),
        "max" => return Some(("max", true)),
        "sup" => return Some(("sup", true)),
        "inf" => return Some(("inf", true)),
        "det" => return Some(("det", true)),
        "gcd" => return Some(("gcd", true)),
        "lim" => return Some(("lim", true)),
        "liminf" => return Some(("lim inf", true)),
        "limsup" => return Some(("lim sup", true)),
        "argmax" => return Some(("arg max", true)),
        "argmin" => return Some(("arg min", true)),
        _ => return None,
    };
    Some((text, false))
}

/// Large operators: the glyph and whether its scripts go above and below in display.
fn large_operator(name: &str) -> Option<(&'static str, bool)> {
    Some(match name {
        "sum" => ("∑", true),
        "prod" => ("∏", true),
        "coprod" => ("∐", true),
        "int" => ("∫", true),
        "iint" => ("∬", true),
        "iiint" => ("∭", true),
        "oint" => ("∮", true),
        "bigcup" => ("⋃", true),
        "bigcap" => ("⋂", true),
        "bigoplus" => ("⨁", true),
        "bigotimes" => ("⨂", true),
        "bigvee" => ("⋁", true),
        "bigwedge" => ("⋀", true),
        _ => return None,
    })
}

fn space(name: &str) -> Option<f32> {
    Some(match name {
        "," | "thinspace" => 3. / 18.,
        ":" | ">" | "medspace" => 4. / 18.,
        ";" | "thickspace" => 5. / 18.,
        "!" | "negthinspace" => -3. / 18.,
        " " => 0.25,
        "enspace" => 0.5,
        "quad" => 1.,
        "qquad" => 2.,
        _ => return None,
    })
}

fn accent(name: &str) -> Option<Accent> {
    Some(match name {
        "hat" | "widehat" => Accent::Hat,
        "tilde" | "widetilde" => Accent::Tilde,
        "dot" => Accent::Dot,
        "ddot" => Accent::Ddot,
        "check" => Accent::Check,
        "breve" => Accent::Breve,
        "acute" => Accent::Acute,
        "grave" => Accent::Grave,
        "bar" | "overline" | "overbrace" => Accent::Bar,
        "vec" | "overrightarrow" => Accent::Vec,
        "underline" | "underbrace" => Accent::Underline,
        _ => return None,
    })
}

/// A character typed straight into the formula (`≤`, `→`), by what it is.
fn unicode_class(c: char) -> Class {
    match c {
        '≤' | '≥' | '≠' | '≈' | '∼' | '≃' | '≅' | '≡' | '∝' | '→' | '←' | '↔' | '⇒' | '⇐' | '⇔' | '↦' | '∈' | '∉' | '⊂' | '⊆' | '⊃' | '⊇' | '∣' | '⟹' | '⟺' => Class::Rel,
        '±' | '∓' | '×' | '÷' | '·' | '∘' | '∪' | '∩' | '∧' | '∨' | '⊕' | '⊗' => Class::Bin,
        '{' | '⟨' | '⌊' | '⌈' => Class::Open,
        '}' | '⟩' | '⌋' | '⌉' => Class::Close,
        _ => Class::Ord,
    }
}

fn blackboard(c: char) -> char {
    match c {
        'C' => 'ℂ',
        'H' => 'ℍ',
        'N' => 'ℕ',
        'P' => 'ℙ',
        'Q' => 'ℚ',
        'R' => 'ℝ',
        'Z' => 'ℤ',
        'A'..='Z' => char::from_u32(0x1D538 + (c as u32 - 'A' as u32)).unwrap_or(c),
        'a'..='z' => char::from_u32(0x1D552 + (c as u32 - 'a' as u32)).unwrap_or(c),
        '0'..='9' => char::from_u32(0x1D7D8 + (c as u32 - '0' as u32)).unwrap_or(c),
        _ => c,
    }
}

fn calligraphic(c: char) -> char {
    match c {
        'B' => 'ℬ',
        'E' => 'ℰ',
        'F' => 'ℱ',
        'H' => 'ℋ',
        'I' => 'ℐ',
        'L' => 'ℒ',
        'M' => 'ℳ',
        'R' => 'ℛ',
        'A'..='Z' => char::from_u32(0x1D49C + (c as u32 - 'A' as u32)).unwrap_or(c),
        _ => c,
    }
}

fn fraktur(c: char) -> char {
    match c {
        'C' => 'ℭ',
        'H' => 'ℌ',
        'I' => 'ℑ',
        'R' => 'ℜ',
        'Z' => 'ℨ',
        'A'..='Z' => char::from_u32(0x1D504 + (c as u32 - 'A' as u32)).unwrap_or(c),
        'a'..='z' => char::from_u32(0x1D51E + (c as u32 - 'a' as u32)).unwrap_or(c),
        _ => c,
    }
}

// ---------- claiming ----------

/// Whether a `$…$` span is maths, from its content and the character after its closing dollar.
/// Dollar amounts pair up too ("$5 and $10" is a span holding `5 and `), so a span reads as
/// maths only when it looks like it: no space just inside either dollar and no digit just after
/// the closing one (as Pandoc decides), and either no spaces at all or something only maths has.
pub(crate) fn claims(content: &str, after: Option<char>) -> bool {
    let trimmed = content.trim();
    if trimmed.is_empty() || content.starts_with(char::is_whitespace) || content.ends_with(char::is_whitespace) || after.is_some_and(|c| c.is_ascii_digit()) {
        return false;
    }
    !trimmed.contains(char::is_whitespace) || trimmed.contains(['\\', '^', '_', '=', '{', '+', '<', '>', '/'])
}

/// A formula as parsed, with its LaTeX for the copy button and the fallback.
#[derive(Debug)]
struct Formula {
    tex: String,
    tree: Result<Node, Unparsed>,
}

/// `source` is written with explicit delimiters, `$$…$$`, `\(…\)` or `\[…\]`, which nothing but
/// maths uses, so it's maths whatever it holds.
fn explicit(source: &str) -> bool {
    source.starts_with("$$") || source.starts_with("\\(") || source.starts_with("\\[")
}

fn displayed(source: &str) -> bool {
    source.starts_with("$$") || source.starts_with("\\[")
}

fn formula(node: &markdown_ast::Node, tex: &str, source: &str, display: bool) -> MarkdownNode {
    let name = if display { DISPLAY } else { INLINE };
    let tex = tex.trim().to_string();
    let tree = parse(&tex);
    MarkdownNode::new(name, Formula { tex, tree }).text(source.to_string()).markdown(source.to_string()).accessibility_label(match node {
        markdown_ast::Node::Math(math) => math.value.trim().to_string(),
        markdown_ast::Node::InlineMath(math) => math.value.trim().to_string(),
        _ => source.to_string(),
    })
}

// ---------- the plugins ----------

/// Inline maths: `$x^2$`, `\(x^2\)`, on the text's baseline at its size and in its colour.
#[derive(Clone)]
pub struct InlineMath {
    pub ink: MathInk,
}

impl MarkdownPlugin for InlineMath {
    fn name(&self) -> &str {
        INLINE
    }

    fn parse(&self, node: &markdown_ast::Node, cx: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
        let markdown_ast::Node::InlineMath(math) = node else { return None };
        let source = cx.node_source(node)?;
        if !explicit(source) {
            let after = node.position().and_then(|p| cx.source().get(p.end.offset..)).and_then(|rest| rest.chars().next());
            if !claims(&math.value, after) {
                return None;
            }
        }
        Some(formula(node, &math.value, source, false))
    }

    fn render_inline(&self, node: &MarkdownNode, context: &InlineRenderContext, window: &mut Window, cx: &mut App) -> Option<InlineElement> {
        let formula = node.data::<Formula>()?;
        let size = context.font_size().as_f32();
        let fonts = Fonts { ui: context.text_style().font_family.clone(), mono: cx.theme().mono_font_family.clone() };
        let ink = MathInk { ink: context.text_style().color, ..self.ink };
        let laid = set(formula, Style::Text, size, &fonts, window);
        let key = node.source_range().map_or(0, |r| r.start);
        let baseline = px(laid.ascent);
        Some(InlineElement::new(canvas_of(laid, SharedString::from(format!("math-{key}")), ink, fonts)).with_baseline(baseline))
    }
}

/// Display maths: a `$$` block, or a formula alone in its paragraph, centred in the column at the
/// transcript's text size `size`, with a copy button for its LaTeX on hover.
#[derive(Clone)]
pub struct DisplayMath {
    pub size: Pixels,
    pub ink: MathInk,
}

impl MarkdownPlugin for DisplayMath {
    fn is_block(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        DISPLAY
    }

    fn parse(&self, node: &markdown_ast::Node, cx: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
        match node {
            markdown_ast::Node::Math(math) => Some(formula(node, &math.value, cx.node_source(node)?, true)),
            // `\[ … \]` or `$$ … $$` on one line is inline maths to the parser; alone in its
            // paragraph it's a displayed formula.
            markdown_ast::Node::Paragraph(paragraph) => {
                let mut inner = paragraph.children.iter().filter(|child| !matches!(child, markdown_ast::Node::Text(t) if t.value.trim().is_empty()));
                let (Some(only @ markdown_ast::Node::InlineMath(math)), None) = (inner.next(), inner.next()) else { return None };
                let source = cx.node_source(only)?;
                displayed(source).then(|| formula(node, &math.value, cx.node_source(node).unwrap_or(source), true))
            }
            _ => None,
        }
    }

    fn render(&self, node: &MarkdownNode, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Some(formula) = node.data::<Formula>() else { return div().into_any_element() };
        let theme = cx.theme();
        let fonts = Fonts { ui: theme.font_family.clone(), mono: theme.mono_font_family.clone() };
        let muted = theme.muted_foreground;
        let key = node.source_range().map_or(0, |r| r.start);
        let laid = set(formula, Style::Display, self.size.as_f32(), &fonts, window);
        let tex = formula.tex.clone();
        let group = SharedString::from(format!("math-block-{key}"));
        div()
            .id(group.clone())
            .test_support()
            .group(group.clone())
            .relative()
            .w_full()
            .py(self.size * 0.25)
            .child(
                // Centred while it fits; wider than the column, it starts at the left edge and
                // scrolls (auto margins give way to the overflow instead of cutting it off).
                div()
                    .id(SharedString::from(format!("math-scroll-{key}")))
                    .w_full()
                    .overflow_x_scroll()
                    .flex()
                    .child(div().flex_none().mx_auto().child(canvas_of(laid, SharedString::from(format!("math-{key}")), self.ink, fonts))),
            )
            .child(
                div().absolute().top_0().right_0().invisible().group_hover(group, |s| s.visible()).child(
                    gpui_kit::component::button::Button::new(SharedString::from(format!("math-copy-{key}")))
                        .ghost()
                        .xsmall()
                        .icon(Icon::new(IconName::Copy).text_color(muted))
                        .tooltip("Copy LaTeX")
                        .on_click(move |_, window, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(tex.clone()));
                            crate::toast::push(window, "Copied", cx);
                        }),
                ),
            )
            .into_any_element()
    }
}

/// The colours maths draws with: the text's ink (glyphs, fraction bars, radicals), and for a
/// command Trek doesn't know, its name in `faint` on a `tint`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MathInk {
    pub ink: Hsla,
    pub faint: Hsla,
    pub tint: Hsla,
}

impl MathInk {
    pub fn new(ink: Hsla, foreground: Hsla, muted: Hsla) -> Self {
        Self { ink, faint: muted, tint: foreground.opacity(0.06) }
    }
}

// ---------- setting ----------

/// How wide a run of text is. GPUI's text system in the app (`Shaper`); a fixed advance in tests.
pub(crate) trait Measure {
    fn width(&mut self, text: &str, face: Face, size: f32) -> f32;
}

/// Something to paint, relative to the box's left edge and baseline (y grows down).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Paint {
    /// A run whose baseline is at `y`.
    Text { x: f32, y: f32, text: String, face: Face, size: f32, faint: bool },
    /// A filled rectangle whose top is at `y`: fraction bars, overlines.
    Rule { x: f32, y: f32, w: f32, h: f32 },
    /// A stroked line through `points`, `width` thick: radicals and tall delimiters.
    Stroke { segments: Vec<Segment>, width: f32 },
    /// The faint ground under an unknown command's name.
    Tint { x: f32, y: f32, w: f32, h: f32 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Segment {
    Move(f32, f32),
    Line(f32, f32),
    /// To the point, through two control points.
    Curve(f32, f32, (f32, f32), (f32, f32)),
}

/// A set box: its width, how far it reaches above and below the baseline, and what to paint.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Laid {
    pub width: f32,
    pub ascent: f32,
    pub descent: f32,
    pub paint: Vec<Paint>,
}

impl Laid {
    /// Paint `other` with its baseline at (`dx`, `dy`) in this box, growing the extents to fit.
    fn place(&mut self, other: Laid, dx: f32, dy: f32) {
        self.ascent = self.ascent.max(other.ascent - dy);
        self.descent = self.descent.max(other.descent + dy);
        self.paint.extend(other.paint.into_iter().map(|p| p.moved(dx, dy)));
    }

    fn height(&self) -> f32 {
        self.ascent + self.descent
    }
}

impl Paint {
    fn moved(self, dx: f32, dy: f32) -> Paint {
        match self {
            Paint::Text { x, y, text, face, size, faint } => Paint::Text { x: x + dx, y: y + dy, text, face, size, faint },
            Paint::Rule { x, y, w, h } => Paint::Rule { x: x + dx, y: y + dy, w, h },
            Paint::Tint { x, y, w, h } => Paint::Tint { x: x + dx, y: y + dy, w, h },
            Paint::Stroke { segments, width } => Paint::Stroke {
                segments: segments
                    .into_iter()
                    .map(|s| match s {
                        Segment::Move(x, y) => Segment::Move(x + dx, y + dy),
                        Segment::Line(x, y) => Segment::Line(x + dx, y + dy),
                        Segment::Curve(x, y, (ax, ay), (bx, by)) => Segment::Curve(x + dx, y + dy, (ax + dx, ay + dy), (bx + dx, by + dy)),
                    })
                    .collect(),
                width,
            },
        }
    }
}

/// The height of the maths axis above the baseline, in ems: where fraction bars, `+` and `=`
/// sit, and the centre that tall delimiters and large operators grow from. Inter's.
const AXIS: f32 = 0.3;
/// Inter's x-height, in ems.
const X_HEIGHT: f32 = 0.55;

/// How far a character reaches above and below the baseline, in ems. Close enough for spacing
/// boxes around it; widths come from the font.
fn extent(c: char) -> (f32, f32) {
    match c {
        'a' | 'c' | 'e' | 'm' | 'n' | 'o' | 'r' | 's' | 'u' | 'v' | 'w' | 'x' | 'z' | 'ı' | 'α' | 'ε' | 'ϵ' | 'ι' | 'κ' | 'ν' | 'ο' | 'π' | 'σ' | 'τ' | 'υ' | 'ω' | 'ϖ' | '∞' | '∝' => (X_HEIGHT, 0.),
        'g' | 'p' | 'q' | 'y' | 'γ' | 'η' | 'μ' | 'ρ' | 'ϱ' | 'χ' | 'ς' => (X_HEIGHT, 0.21),
        'j' | 'β' | 'ζ' | 'ξ' | 'φ' | 'ϕ' | 'ψ' | 'Q' | 'f' => (0.75, 0.21),
        '(' | ')' | '[' | ']' | '{' | '}' | '|' | '‖' | '/' | '\\' | '⟨' | '⟩' | '⌊' | '⌋' | '⌈' | '⌉' => (0.77, 0.23),
        '∫' | '∬' | '∭' | '∮' => (0.8, 0.22),
        '+' | '−' | '=' | '±' | '∓' | '×' | '÷' | '·' | '∗' | '∘' | '<' | '>' | '≈' | '∼' | '≃' | '≅' | '≡' | '≠' | '→' | '←' | '↔' | '⇒' | '⇐' | '⇔' | '⟹' | '⟸' | '⟺' | '⟶' | '⟵' | '↦' | '∪' | '∩' | '∧' | '∨' | '¬' => (0.6, 0.),
        '≤' | '≥' | '⩽' | '⩾' | '∈' | '∉' | '⊂' | '⊆' | '⊃' | '⊇' => (0.64, 0.08),
        ',' | ';' => (0.12, 0.15),
        '.' | '…' => (0.12, 0.),
        '⋯' => (0.35, 0.),
        ' ' => (0., 0.),
        _ => (0.73, 0.),
    }
}

/// Lays formulas out in boxes, measuring text with `M`.
pub(crate) struct Setter<'m, M: Measure> {
    pub measure: &'m mut M,
    /// The surrounding text's size, in pixels.
    pub size: f32,
    depth: usize,
}

impl<'m, M: Measure> Setter<'m, M> {
    pub fn new(measure: &'m mut M, size: f32) -> Self {
        Self { measure, size, depth: 0 }
    }

    /// The thickness of fraction bars, radicals and overlines: a hairline, growing with the text.
    fn hair(&self) -> f32 {
        (self.size * 0.068).max(1.)
    }

    pub fn formula(&mut self, tree: &Node, style: Style) -> Laid {
        self.node(tree, style)
    }

    /// A formula Trek couldn't parse: its source in mono, a line per line.
    pub fn source(&mut self, tex: &str) -> Laid {
        let size = self.size * 0.88;
        let mut laid = Laid::default();
        let pitch = size * 1.35;
        for (i, line) in tex.lines().enumerate() {
            let w = self.measure.width(line, Face::Mono, size);
            laid.width = laid.width.max(w);
            laid.paint.push(Paint::Text { x: 0., y: i as f32 * pitch, text: line.to_string(), face: Face::Mono, size, faint: false });
            laid.descent = i as f32 * pitch + size * 0.22;
        }
        laid.ascent = size * 0.78;
        laid
    }

    fn sym(&mut self, text: &str, face: Face, size: f32, faint: bool) -> Laid {
        let width = self.measure.width(text, face, size);
        let (a, d) = text.chars().map(extent).fold((0f32, 0f32), |(a, d), (ca, cd)| (a.max(ca), d.max(cd)));
        Laid { width, ascent: a * size, descent: d * size, paint: vec![Paint::Text { x: 0., y: 0., text: text.to_string(), face, size, faint }] }
    }

    fn node(&mut self, node: &Node, style: Style) -> Laid {
        // The parser bounds nesting; this guards hand-made trees.
        if self.depth > MAX_DEPTH * 2 {
            return Laid::default();
        }
        self.depth += 1;
        let laid = self.node_inner(node, style);
        self.depth -= 1;
        laid
    }

    fn node_inner(&mut self, node: &Node, style: Style) -> Laid {
        let size = self.size * style.scale();
        match node {
            Node::Sym { text, face, .. } => self.sym(text, *face, size, false),
            Node::Op { text, glyph: false, .. } => self.sym(text, Face::Upright, size, false),
            Node::Op { text, glyph: true, .. } => {
                // Larger in display, and centred on the axis.
                let k = match style {
                    Style::Display => 1.5,
                    Style::Text => 1.1,
                    _ => 1.,
                };
                let laid = self.sym(text, Face::Upright, size * k, false);
                let centre = (laid.ascent - laid.descent) / 2.;
                let lift = AXIS * size - centre;
                let mut out = Laid { width: laid.width, ..Laid::default() };
                out.place(laid, 0., -lift);
                out
            }
            Node::Group(items) => self.list(items, style, None),
            Node::Frac { num, den, bar, style: forced } => self.fraction(num, den, *bar, forced.unwrap_or(style), size),
            Node::Scripts { base, sup, sub } => {
                let stacked = style == Style::Display && matches!(base.as_ref(), Node::Op { limits: true, .. });
                if stacked {
                    self.stacked(base, sup.as_deref(), sub.as_deref(), style)
                } else {
                    self.scripts(base, sup.as_deref(), sub.as_deref(), style)
                }
            }
            Node::Stack { base, over, under } => self.stacked(base, over.as_deref(), under.as_deref(), style),
            Node::Sqrt { index, body } => self.radical(index.as_deref(), body, style),
            Node::Delimited { left, body, right } => {
                let body = self.node(body, style);
                let half = (body.ascent - AXIS * size).max(body.descent + AXIS * size);
                let tall = 2. * half + 0.1 * size;
                let mut out = Laid::default();
                for (laid, gap) in [(self.delimiter(*left, tall, size), 0.), (body, 0.04 * size), (self.delimiter(*right, tall, size), 0.04 * size)] {
                    let x = out.width + gap;
                    out.width = x + laid.width;
                    out.place(laid, x, 0.);
                }
                out
            }
            Node::Big { delim, size: em, .. } => self.drawn_delimiter(*delim, em * size, size),
            Node::Accent { accent, body } => self.accent(*accent, body, style),
            Node::Boxed(body) => {
                let body = self.node(body, style);
                let pad = 0.3 * size;
                let t = self.hair();
                let (w, top, bottom) = (body.width + 2. * pad, -(body.ascent + pad), body.descent + pad);
                let mut out = Laid { width: w, ..Laid::default() };
                out.place(body, pad, 0.);
                out.ascent = -top;
                out.descent = bottom;
                out.paint.extend([
                    Paint::Rule { x: 0., y: top, w, h: t },
                    Paint::Rule { x: 0., y: bottom - t, w, h: t },
                    Paint::Rule { x: 0., y: top, w: t, h: bottom - top },
                    Paint::Rule { x: w - t, y: top, w: t, h: bottom - top },
                ]);
                out
            }
            Node::Space(em) => Laid { width: em * size, ..Laid::default() },
            Node::Phantom(body) => {
                let laid = self.node(body, style);
                Laid { paint: Vec::new(), ..laid }
            }
            Node::Table { rows, columns } => self.table(rows, *columns, style),
            Node::Unknown(name) => {
                let mono = size * 0.86;
                let text = format!("\\{name}");
                let laid = self.sym(&text, Face::Mono, mono, true);
                let pad = 0.15 * size;
                let width = laid.width + 2. * pad;
                let mut out = Laid { width, ascent: 0.8 * size, descent: 0.22 * size, paint: vec![Paint::Tint { x: 0., y: -0.8 * size, w: width, h: 1.02 * size }] };
                out.place(laid, pad, 0.);
                out
            }
        }
    }

    /// Atoms side by side, spaced by class. `lead`: the class of an atom before the first (in an
    /// aligned table's right-hand cells, `=` spaces as if something stood before it).
    fn list(&mut self, items: &[Node], style: Style, lead: Option<Class>) -> Laid {
        let size = self.size * style.scale();
        let classes = classes(items, lead);
        let mut out = Laid::default();
        let mut prev = lead;
        for (item, class) in items.iter().zip(classes) {
            if let Node::Space(em) = item {
                out.width += em * size;
                continue;
            }
            let laid = self.node(item, style);
            if let (Some(p), Some(c)) = (prev, class) {
                out.width += gap(p, c, style) * size;
            }
            let x = out.width;
            out.width += laid.width;
            out.place(laid, x, 0.);
            prev = class.or(prev);
        }
        out
    }

    fn fraction(&mut self, num: &Node, den: &Node, bar: bool, style: Style, size: f32) -> Laid {
        let inner = style.fraction();
        let num = self.node(num, inner);
        let den = self.node(den, inner);
        let t = if bar { self.hair() } else { 0. };
        let display = style == Style::Display;
        let gap = (if display { 0.16 } else { 0.1 } * size).max(t * 1.5);
        let axis = AXIS * size;
        // At least TeX's default shifts, so a short numerator doesn't hug the bar.
        let (least_up, least_down) = if display { (0.68 * size, 0.69 * size) } else { (0.4 * size, 0.35 * size) };
        let up = (axis + t / 2. + gap + num.descent).max(least_up);
        let down = (den.ascent + gap + t / 2. - axis).max(least_down);
        let pad = 0.12 * size;
        let width = num.width.max(den.width) + 2. * pad;
        let mut out = Laid { width, ..Laid::default() };
        let (nw, dw) = (num.width, den.width);
        out.place(num, (width - nw) / 2., -up);
        out.place(den, (width - dw) / 2., down);
        if bar {
            out.paint.push(Paint::Rule { x: pad * 0.5, y: -(axis + t / 2.), w: width - pad, h: t });
        }
        out
    }

    /// Scripts above and below a large operator in display, and `\overset`'s.
    fn stacked(&mut self, base: &Node, over: Option<&Node>, under: Option<&Node>, style: Style) -> Laid {
        let size = self.size * style.scale();
        let base = self.node(base, style);
        let over = over.map(|n| self.node(n, style.script()));
        let under = under.map(|n| self.node(n, style.script()));
        let gap = 0.12 * size;
        let width = base.width.max(over.as_ref().map_or(0., |l| l.width)).max(under.as_ref().map_or(0., |l| l.width));
        let mut out = Laid { width, ..Laid::default() };
        let (ascent, descent, bw) = (base.ascent, base.descent, base.width);
        out.place(base, (width - bw) / 2., 0.);
        if let Some(over) = over {
            let y = -(ascent + gap + over.descent);
            let w = over.width;
            out.place(over, (width - w) / 2., y);
        }
        if let Some(under) = under {
            let y = descent + gap + under.ascent;
            let w = under.width;
            out.place(under, (width - w) / 2., y);
        }
        out
    }

    /// Superscripts and subscripts beside their base, at 0.7×.
    fn scripts(&mut self, base_node: &Node, sup: Option<&Node>, sub: Option<&Node>, style: Style) -> Laid {
        let size = self.size * style.scale();
        let script = style.script();
        let script_size = self.size * script.scale();
        let base = self.node(base_node, style);
        let simple = matches!(base_node, Node::Sym { .. } | Node::Op { glyph: false, .. });
        let italic = matches!(base_node, Node::Sym { face: Face::Italic | Face::BoldItalic, .. });
        let sup = sup.map(|n| self.node(n, script));
        let sub = sub.map(|n| self.node(n, script));
        let mut up = if simple { 0.42 * size } else { (base.ascent - 0.39 * script_size).max(0.42 * size) };
        let mut down = if simple { 0.2 * size } else { (base.descent + 0.05 * script_size).max(0.2 * size) };
        if let Some(sup) = &sup {
            up = up.max(sup.descent + 0.25 * X_HEIGHT * size);
        }
        if let Some(sub) = &sub {
            down = down.max(sub.ascent - 0.8 * X_HEIGHT * size);
        }
        if let (Some(sup), Some(sub)) = (&sup, &sub) {
            down = down.max(0.25 * size);
            let clearance = (up - sup.descent) - (sub.ascent - down);
            let wanted = 4. * self.hair();
            if clearance < wanted {
                down += wanted - clearance;
            }
        }
        let x = base.width;
        let mut out = Laid { width: base.width, ..Laid::default() };
        out.place(base, 0., 0.);
        let mut right = x;
        if let Some(sup) = sup {
            let at = x + if italic { 0.06 * size } else { 0. };
            right = right.max(at + sup.width);
            out.place(sup, at, -up);
        }
        if let Some(sub) = sub {
            right = right.max(x + sub.width);
            out.place(sub, x, down);
        }
        out.width = right + 0.05 * size;
        out
    }

    fn radical(&mut self, index: Option<&Node>, body: &Node, style: Style) -> Laid {
        let size = self.size * style.scale();
        let body = self.node(body, style);
        let t = self.hair();
        let gap = if style == Style::Display { 0.2 * size } else { 0.12 * size };
        let top = -(body.ascent.max(X_HEIGHT * size) + gap + t / 2.);
        let bottom = body.descent.max(0.) + 0.08 * size;
        let height = bottom - top;
        let sign = 0.5 * size + 0.08 * height.min(4. * size);
        // The index sits in the crook of the sign, and pushes it right when it's wide.
        let index = index.map(|n| self.node(n, Style::ScriptScript));
        let shift = index.as_ref().map_or(0., |i| (i.width - 0.55 * sign).max(0.));
        let x0 = shift;
        let tick = (x0, top + height * 0.58);
        let points = [Segment::Move(tick.0, tick.1), Segment::Line(x0 + 0.2 * sign, top + height * 0.5), Segment::Line(x0 + 0.5 * sign, bottom), Segment::Line(x0 + sign, top), Segment::Line(x0 + sign + body.width + 0.1 * size, top)];
        let width = x0 + sign + body.width + 0.1 * size;
        let mut out = Laid { width, ..Laid::default() };
        let index_at = index.as_ref().map(|i| (x0 + 0.55 * sign - i.width, top + height * 0.5 - i.descent - 0.1 * size));
        out.place(body, x0 + sign, 0.);
        out.ascent = out.ascent.max(-top + t);
        out.descent = out.descent.max(bottom);
        out.paint.push(Paint::Stroke { segments: points.to_vec(), width: t });
        if let (Some(index), Some((x, y))) = (index, index_at) {
            out.place(index, x.max(0.), y);
        }
        out
    }

    /// A delimiter for a body `tall` high about the axis: the font's glyph when that's tall
    /// enough, else drawn.
    fn delimiter(&mut self, delim: Option<char>, tall: f32, size: f32) -> Laid {
        match delim {
            None => Laid { width: 0.12 * size, ..Laid::default() },
            Some(c) if tall <= 1.2 * size => self.sym(&c.to_string(), Face::Upright, size, false),
            Some(_) => self.drawn_delimiter(delim, tall, size),
        }
    }

    fn drawn_delimiter(&mut self, delim: Option<char>, tall: f32, size: f32) -> Laid {
        let Some(c) = delim else { return Laid { width: 0.12 * size, ..Laid::default() } };
        let t = self.hair() * 1.15;
        let axis = AXIS * size;
        let (top, bottom) = (-(axis + tall / 2.), -axis + tall / 2.);
        let middle = -axis;
        let w = match c {
            '(' | ')' => (0.3 * size + 0.04 * tall).min(0.7 * size),
            '{' | '}' => 0.5 * size,
            '|' | '‖' => if c == '|' { 0.3 * size } else { 0.45 * size },
            '⟨' | '⟩' => (0.3 * size + 0.05 * tall).min(0.7 * size),
            _ => 0.35 * size,
        };
        let (pad_in, pad_out) = (0.08 * size, 0.08 * size);
        // `outer` is the side away from the body: left for an opener.
        let opener = matches!(c, '(' | '[' | '{' | '⟨' | '⌊' | '⌈');
        let (outer, inner) = if opener { (pad_out, w - pad_in) } else { (w - pad_out, pad_in) };
        let mid = (outer + inner) / 2.;
        let segments = match c {
            '(' | ')' => {
                let bulge = outer + (outer - inner) * 0.15;
                vec![Segment::Move(inner, top), Segment::Curve(inner, bottom, (bulge, top + tall * 0.22), (bulge, bottom - tall * 0.22))]
            }
            '[' | ']' => vec![Segment::Move(inner, top), Segment::Line(outer, top), Segment::Line(outer, bottom), Segment::Line(inner, bottom)],
            '⌊' | '⌋' => vec![Segment::Move(outer, top), Segment::Line(outer, bottom), Segment::Line(inner, bottom)],
            '⌈' | '⌉' => vec![Segment::Move(inner, top), Segment::Line(outer, top), Segment::Line(outer, bottom)],
            '{' | '}' => {
                let r = (tall * 0.08).min(0.25 * size);
                vec![
                    Segment::Move(inner, top),
                    Segment::Curve(mid, top + r, (mid, top), (mid, top)),
                    Segment::Line(mid, middle - r),
                    Segment::Curve(outer, middle, (mid, middle), (mid, middle)),
                    Segment::Curve(mid, middle + r, (mid, middle), (mid, middle)),
                    Segment::Line(mid, bottom - r),
                    Segment::Curve(inner, bottom, (mid, bottom), (mid, bottom)),
                ]
            }
            '⟨' | '⟩' => vec![Segment::Move(inner, top), Segment::Line(outer, middle), Segment::Line(inner, bottom)],
            '‖' => {
                let (a, b) = (w * 0.35, w * 0.65);
                vec![Segment::Move(a, top), Segment::Line(a, bottom), Segment::Move(b, top), Segment::Line(b, bottom)]
            }
            '/' => vec![Segment::Move(w - pad_out, top), Segment::Line(pad_out, bottom)],
            '\\' => vec![Segment::Move(pad_out, top), Segment::Line(w - pad_out, bottom)],
            _ => vec![Segment::Move(w / 2., top), Segment::Line(w / 2., bottom)],
        };
        Laid { width: w, ascent: -top, descent: bottom, paint: vec![Paint::Stroke { segments, width: t }] }
    }

    fn accent(&mut self, accent: Accent, body: &Node, style: Style) -> Laid {
        let size = self.size * style.scale();
        let t = self.hair();
        let body = self.node(body, style);
        let (ascent, descent, width) = (body.ascent, body.descent, body.width);
        let mut out = Laid { width, ..Laid::default() };
        out.place(body, 0., 0.);
        let above = ascent.max(X_HEIGHT * size);
        match accent {
            Accent::Bar => {
                let y = -(above + 0.12 * size);
                out.paint.push(Paint::Rule { x: 0.04 * size, y: y - t, w: (width - 0.08 * size).max(t), h: t });
                out.ascent = out.ascent.max(-y + t);
            }
            Accent::Underline => {
                let y = descent + 0.1 * size;
                out.paint.push(Paint::Rule { x: 0., y, w: width, h: t });
                out.descent = out.descent.max(y + t);
            }
            Accent::Vec => {
                let y = -(above + 0.18 * size);
                let (x0, x1) = (0.04 * size, width.max(0.5 * size) - 0.04 * size);
                let head = 0.14 * size;
                out.paint.push(Paint::Stroke {
                    segments: vec![Segment::Move(x0, y), Segment::Line(x1, y), Segment::Move(x1 - head, y - head * 0.7), Segment::Line(x1, y), Segment::Line(x1 - head, y + head * 0.7)],
                    width: t,
                });
                out.width = out.width.max(x1 + 0.04 * size);
                out.ascent = out.ascent.max(-y + head);
            }
            glyph => {
                let mark = match glyph {
                    Accent::Hat => "ˆ",
                    Accent::Tilde => "˜",
                    Accent::Dot => "˙",
                    Accent::Ddot => "¨",
                    Accent::Check => "ˇ",
                    Accent::Breve => "˘",
                    Accent::Acute => "´",
                    _ => "`",
                };
                // The mark is drawn for a lowercase letter; lift it over anything taller.
                let lift = (ascent - X_HEIGHT * size).max(0.);
                let mw = self.measure.width(mark, Face::Upright, size);
                out.paint.push(Paint::Text { x: (width - mw) / 2. + 0.04 * size, y: -lift, text: mark.into(), face: Face::Upright, size, faint: false });
                out.ascent = out.ascent.max(lift + 0.8 * size);
            }
        }
        out
    }

    fn table(&mut self, rows: &[Vec<Node>], columns: Columns, style: Style) -> Laid {
        let size = self.size * style.scale();
        let cell_style = if style.tight() { Style::ScriptScript } else { Style::Text };
        let cell_style = if style == Style::Display || style == Style::Text { cell_style } else { Style::ScriptScript };
        let laid: Vec<Vec<Laid>> = rows
            .iter()
            .map(|row| {
                row.iter()
                    .enumerate()
                    .map(|(i, cell)| {
                        let lead = (columns == Columns::Aligned && i % 2 == 1).then_some(Class::Ord);
                        match cell {
                            Node::Group(items) => self.list(items, cell_style, lead),
                            other => self.node(other, cell_style),
                        }
                    })
                    .collect()
            })
            .collect();
        let count = laid.iter().map(Vec::len).max().unwrap_or(0);
        let mut widths = vec![0f32; count];
        for row in &laid {
            for (i, cell) in row.iter().enumerate() {
                widths[i] = widths[i].max(cell.width);
            }
        }
        let column_gap = |i: usize| match columns {
            Columns::Aligned if i % 2 == 1 => 1.5 * size,
            Columns::Aligned => 0.,
            Columns::Left => 1. * size,
            Columns::Centred => 0.8 * size,
        };
        let mut lefts = Vec::with_capacity(count);
        let mut x = 0.;
        for (i, w) in widths.iter().enumerate() {
            if i > 0 {
                x += column_gap(i - 1);
            }
            lefts.push(x);
            x += w;
        }
        let width = x;
        let row_gap = if columns == Columns::Aligned { 0.3 * size } else { 0.25 * size };
        let mut out = Laid { width, ..Laid::default() };
        let mut y = 0.;
        let mut placed = Vec::new();
        for (r, row) in laid.into_iter().enumerate() {
            let ascent = row.iter().map(|c| c.ascent).fold(0.75 * size, f32::max);
            let descent = row.iter().map(|c| c.descent).fold(0.28 * size, f32::max);
            if r > 0 {
                y += row_gap;
            }
            y += ascent;
            for (i, cell) in row.into_iter().enumerate() {
                let slack = widths[i] - cell.width;
                let dx = match columns {
                    Columns::Aligned if i % 2 == 0 => slack,
                    Columns::Aligned | Columns::Left => 0.,
                    Columns::Centred => slack / 2.,
                };
                placed.push((cell, lefts[i] + dx, y));
            }
            y += descent;
        }
        // Centred on the axis.
        let shift = y / 2. + AXIS * size;
        for (cell, x, baseline) in placed {
            out.place(cell, x, baseline - shift);
        }
        out.ascent = out.ascent.max(shift);
        out.descent = out.descent.max(y - shift);
        out
    }
}

/// Each item's class for spacing (`None` for room), with binary operators that have nothing to
/// operate on read as ordinary: the minus in `-x`, or in `(-1)`.
fn classes(items: &[Node], lead: Option<Class>) -> Vec<Option<Class>> {
    let mut out: Vec<Option<Class>> = items.iter().map(class).collect();
    let mut prev = lead;
    for i in 0..out.len() {
        let Some(c) = out[i] else { continue };
        if c == Class::Bin && (prev.is_none() || matches!(prev, Some(Class::Bin | Class::Op | Class::Rel | Class::Open | Class::Punct))) {
            out[i] = Some(Class::Ord);
        }
        prev = out[i];
    }
    // And before a relation, a closer, punctuation or the end.
    let mut next: Option<Class> = None;
    for i in (0..out.len()).rev() {
        let Some(c) = out[i] else { continue };
        if c == Class::Bin && (next.is_none() || matches!(next, Some(Class::Rel | Class::Close | Class::Punct))) {
            out[i] = Some(Class::Ord);
        }
        next = out[i];
    }
    out
}

fn class(node: &Node) -> Option<Class> {
    Some(match node {
        Node::Sym { class, .. } => *class,
        Node::Op { .. } => Class::Op,
        Node::Scripts { base, .. } | Node::Stack { base, .. } => class(base).unwrap_or(Class::Ord),
        Node::Frac { .. } | Node::Delimited { .. } => Class::Inner,
        Node::Big { class, .. } => *class,
        Node::Space(_) => return None,
        Node::Group(items) if items.is_empty() => return None,
        _ => Class::Ord,
    })
}

/// The room between two atoms, in ems, as TeX's table has it (in scripts, only the thin spaces
/// around operators remain).
fn gap(left: Class, right: Class, style: Style) -> f32 {
    use Class::*;
    let (mu, always) = match (left, right) {
        (Ord | Close | Inner, Op) | (Op, Ord | Op) => (3., true),
        (Ord | Close | Inner, Bin) | (Bin, Ord | Op | Open | Inner) => (4., false),
        (Ord | Op | Close | Inner, Rel) | (Rel, Ord | Op | Open | Inner) => (5., false),
        (Punct, Ord | Op | Rel | Open | Close | Punct | Inner) => (3., false),
        (Ord | Close | Inner, Inner) | (Inner, Ord | Open | Punct) | (Op, Inner) => (3., false),
        _ => (0., true),
    };
    if !always && style.tight() { 0. } else { mu / 18. }
}

// ---------- drawing ----------

/// The interface and mono families.
#[derive(Clone)]
struct Fonts {
    ui: SharedString,
    mono: SharedString,
}

impl Fonts {
    fn font(&self, face: Face) -> Font {
        match face {
            Face::Italic => font(self.ui.clone()).italic(),
            Face::Upright => font(self.ui.clone()),
            Face::Bold => font(self.ui.clone()).bold(),
            Face::BoldItalic => font(self.ui.clone()).bold().italic(),
            Face::Mono => font(self.mono.clone()),
        }
    }
}

fn text_run(text: &str, font: Font, color: Hsla) -> TextRun {
    TextRun { len: text.len(), font, color, background_color: None, underline: None, strikethrough: None }
}

/// Measures with the window's text system.
struct Shaper<'w> {
    window: &'w Window,
    fonts: &'w Fonts,
}

impl Measure for Shaper<'_> {
    fn width(&mut self, text: &str, face: Face, size: f32) -> f32 {
        if text.is_empty() {
            return 0.;
        }
        let line = self.window.text_system().shape_line(SharedString::from(text.to_string()), px(size), &[text_run(text, self.fonts.font(face), black())], None);
        line.width.as_f32()
    }
}

/// `formula` set in `style` at `size`.
fn set(formula: &Formula, style: Style, size: f32, fonts: &Fonts, window: &Window) -> Laid {
    let mut shaper = Shaper { window, fonts };
    let mut setter = Setter::new(&mut shaper, size);
    match &formula.tree {
        Ok(tree) => setter.formula(tree, style),
        Err(Unparsed) => setter.source(&formula.tex),
    }
}

/// The element that paints `laid`, sized to it, under the test-support `id`.
fn canvas_of(laid: Laid, id: SharedString, ink: MathInk, fonts: Fonts) -> impl IntoElement {
    let (width, height, ascent) = (laid.width.max(1.), laid.height().max(1.), laid.ascent);
    div().id(id).test_support().flex_none().w(px(width)).h(px(height)).child(
        canvas(
            |_, _, _| {},
            move |bounds, _, window, cx| {
                let origin = point(bounds.origin.x, bounds.origin.y + px(ascent));
                paint(&laid, origin, &ink, &fonts, window, cx);
            },
        )
        .size_full(),
    )
}

fn paint(laid: &Laid, origin: Point<Pixels>, ink: &MathInk, fonts: &Fonts, window: &mut Window, cx: &mut App) {
    let scale = window.scale_factor();
    // Rules land on whole device pixels, so a hairline stays one crisp line.
    let snap = |v: Pixels| px((v.as_f32() * scale).round() / scale);
    let at = |x: f32, y: f32| point(origin.x + px(x), origin.y + px(y));
    for item in &laid.paint {
        match item {
            Paint::Tint { x, y, w, h } => {
                window.paint_quad(fill(Bounds::new(at(*x, *y), size(px(*w), px(*h))), ink.tint).corner_radii(px(3.)));
            }
            Paint::Rule { x, y, w, h } => {
                let top = snap(origin.y + px(*y));
                let h = px(((*h * scale).round().max(1.)) / scale);
                window.paint_quad(fill(Bounds::new(point(origin.x + px(*x), top), size(px(*w), h)), ink.ink));
            }
            Paint::Text { x, y, text, face, size, faint } => {
                let color = if *faint { ink.faint } else { ink.ink };
                let line = window.text_system().shape_line(SharedString::from(text.clone()), px(*size), &[text_run(text, fonts.font(*face), color)], None);
                let top = at(*x, *y - line.ascent.as_f32());
                let _ = line.paint(top, line.ascent + line.descent, TextAlign::Left, None, window, cx);
            }
            Paint::Stroke { segments, width } => {
                let mut path = PathBuilder::stroke(px(*width));
                for segment in segments {
                    match *segment {
                        Segment::Move(x, y) => path.move_to(at(x, y)),
                        Segment::Line(x, y) => path.line_to(at(x, y)),
                        Segment::Curve(x, y, (ax, ay), (bx, by)) => path.cubic_bezier_to(at(x, y), at(ax, ay), at(bx, by)),
                    }
                }
                if let Ok(path) = path.build() {
                    window.paint_path(path, ink.ink);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
