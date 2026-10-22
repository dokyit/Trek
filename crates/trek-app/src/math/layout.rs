//! Typesetting: the parsed formula as boxes — glyphs, rules and drawn shapes placed in ems
//! around a baseline — following TeX's rules as KaTeX implements them (the font dimensions
//! below are TeX's own, `cmsy10`/`cmex10`, by way of KaTeX's `fontMetrics.ts`), with glyph
//! metrics from the KaTeX fonts Trek embeds. Coordinates: x to the right, y up from the
//! baseline; one em is the formula's font size.

use super::parse::{Accent, Align, Array, ArrayKind, NO_DELIM, Node, OpBody};
use super::tables;

/// TeX's atom classes, which decide the space between neighbours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Ord,
    Op,
    Bin,
    Rel,
    Open,
    Close,
    Punct,
    Inner,
}

/// The two fonts KaTeX's symbol table draws from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolFont {
    Main,
    Ams,
}

/// A KaTeX font face.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Face {
    Main,
    MainBold,
    MainItalic,
    MainBoldItalic,
    MathItalic,
    MathBoldItalic,
    Ams,
    Caligraphic,
    Fraktur,
    Script,
    SansSerif,
    Typewriter,
    Size1,
    Size2,
    Size3,
    Size4,
}

impl Face {
    /// The family name in the font file.
    pub fn family(self) -> &'static str {
        match self {
            Face::Main | Face::MainBold | Face::MainItalic | Face::MainBoldItalic => "KaTeX_Main",
            Face::MathItalic | Face::MathBoldItalic => "KaTeX_Math",
            Face::Ams => "TrekMath_AMS",
            Face::Caligraphic => "TrekMath_Caligraphic",
            Face::Fraktur => "KaTeX_Fraktur",
            Face::Script => "TrekMath_Script",
            Face::SansSerif => "KaTeX_SansSerif",
            Face::Typewriter => "KaTeX_Typewriter",
            Face::Size1 => "TrekMath_Size1",
            Face::Size2 => "TrekMath_Size2",
            Face::Size3 => "TrekMath_Size3",
            Face::Size4 => "TrekMath_Size4",
        }
    }

    pub fn bold(self) -> bool {
        matches!(
            self,
            Face::MainBold | Face::MainBoldItalic | Face::MathBoldItalic
        )
    }

    pub fn italic(self) -> bool {
        matches!(
            self,
            Face::MainItalic | Face::MainBoldItalic | Face::MathItalic | Face::MathBoldItalic
        )
    }

    fn table(self) -> &'static [(u32, tables::Metrics)] {
        match self {
            Face::Main => tables::MAIN_REGULAR,
            Face::MainBold => tables::MAIN_BOLD,
            Face::MainItalic => tables::MAIN_ITALIC,
            Face::MainBoldItalic => tables::MAIN_BOLD_ITALIC,
            Face::MathItalic => tables::MATH_ITALIC,
            Face::MathBoldItalic => tables::MATH_BOLD_ITALIC,
            Face::Ams => tables::AMS_REGULAR,
            Face::Caligraphic => tables::CALIGRAPHIC_REGULAR,
            Face::Fraktur => tables::FRAKTUR_REGULAR,
            Face::Script => tables::SCRIPT_REGULAR,
            Face::SansSerif => tables::SANS_SERIF_REGULAR,
            Face::Typewriter => tables::TYPEWRITER_REGULAR,
            Face::Size1 => tables::SIZE1_REGULAR,
            Face::Size2 => tables::SIZE2_REGULAR,
            Face::Size3 => tables::SIZE3_REGULAR,
            Face::Size4 => tables::SIZE4_REGULAR,
        }
    }

    /// Depth, height, italic correction, skew and width of `ch`, when the face has it.
    pub fn metrics(self, ch: char) -> Option<tables::Metrics> {
        let table = self.table();
        table
            .binary_search_by_key(&(ch as u32), |(c, _)| *c)
            .ok()
            .map(|i| table[i].1)
    }
}

/// TeX's math styles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Display,
    Text,
    Script,
    ScriptScript,
}

/// Widths of text the font tables can't give (`\text{…}` runs, characters outside the KaTeX
/// fonts): the renderer shapes them; tests use the tables.
pub trait Measure {
    /// The width of `text` in `face`, in ems.
    fn width(&self, text: &str, face: Face) -> Option<f32>;
}

#[cfg(test)]
/// Measures by the font tables alone.
pub struct ByTables;

#[cfg(test)]
impl Measure for ByTables {
    fn width(&self, _: &str, _: Face) -> Option<f32> {
        None
    }
}

/// A drawn shape, sized by the box it sits in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    /// A stroke corner to corner: bottom-left to top-right (`forward`) or top-left to
    /// bottom-right.
    Strike { forward: bool },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// Text on a baseline at (x, y), `size` ems tall.
    Glyph {
        x: f32,
        y: f32,
        text: String,
        face: Face,
        size: f32,
    },
    /// A filled rectangle; `y` its bottom.
    Rule { x: f32, y: f32, w: f32, h: f32 },
    /// A shape filling the rectangle; `y` its bottom; `t` its stroke.
    Shape {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        t: f32,
        shape: Shape,
    },
    /// A rectangle's outline, `t` thick; `y` its bottom.
    Frame {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        t: f32,
    },
}

impl Item {
    fn shift(&mut self, dx: f32, dy: f32) {
        match self {
            Item::Glyph { x, y, .. }
            | Item::Rule { x, y, .. }
            | Item::Shape { x, y, .. }
            | Item::Frame { x, y, .. } => {
                *x += dx;
                *y += dy;
            }
        }
    }
}

/// A box: its width, height above the baseline and depth below, and what's drawn in it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MBox {
    pub w: f32,
    pub h: f32,
    pub d: f32,
    pub items: Vec<Item>,
    /// The italic correction of a lone glyph, which a subscript tucks under.
    italic: f32,
    /// A lone glyph's skew, which centres an accent over its slant.
    skew: f32,
    /// A single character: scripts sit at fixed heights rather than from its edges.
    char_box: bool,
}

impl MBox {
    fn kern(w: f32) -> MBox {
        MBox {
            w,
            ..MBox::default()
        }
    }

    /// Place `other` with its origin at (x, y) in this box, without changing this box's size.
    fn put(&mut self, other: MBox, x: f32, y: f32) {
        self.items.extend(other.items.into_iter().map(|mut item| {
            item.shift(x, y);
            item
        }));
    }

    fn append(&mut self, other: MBox) {
        let x = self.w;
        self.h = self.h.max(other.h);
        self.d = self.d.max(other.d);
        self.w += other.w;
        self.put(other, x, 0.);
    }

    fn shifted(mut self, dy: f32) -> MBox {
        self.h += dy;
        self.d -= dy;
        for item in &mut self.items {
            item.shift(0., dy);
        }
        self
    }
}

/// TeX's font dimensions, for text, script and scriptscript sizes (KaTeX's `sigmasAndXis`).
mod sigma {
    pub const X_HEIGHT: [f32; 3] = [0.431, 0.431, 0.431];
    pub const NUM1: [f32; 3] = [0.677, 0.732, 0.925];
    pub const NUM2: [f32; 3] = [0.394, 0.384, 0.387];
    pub const NUM3: [f32; 3] = [0.444, 0.471, 0.504];
    pub const DENOM1: [f32; 3] = [0.686, 0.752, 1.025];
    pub const DENOM2: [f32; 3] = [0.345, 0.344, 0.532];
    pub const SUP1: [f32; 3] = [0.413, 0.503, 0.504];
    pub const SUP2: [f32; 3] = [0.363, 0.431, 0.404];
    pub const SUP3: [f32; 3] = [0.289, 0.286, 0.294];
    pub const SUB1: [f32; 3] = [0.150, 0.143, 0.200];
    pub const SUB2: [f32; 3] = [0.247, 0.286, 0.400];
    pub const SUP_DROP: [f32; 3] = [0.386, 0.353, 0.494];
    pub const SUB_DROP: [f32; 3] = [0.050, 0.071, 0.100];
    pub const DELIM1: [f32; 3] = [2.390, 1.700, 1.980];
    pub const DELIM2: [f32; 3] = [1.010, 1.157, 1.420];
    pub const AXIS: [f32; 3] = [0.25, 0.25, 0.25];
    pub const RULE: [f32; 3] = [0.04, 0.049, 0.049];
    pub const BIG_OP1: [f32; 3] = [0.111, 0.111, 0.111];
    pub const BIG_OP2: [f32; 3] = [0.166, 0.166, 0.166];
    pub const BIG_OP3: [f32; 3] = [0.2, 0.2, 0.2];
    pub const BIG_OP4: [f32; 3] = [0.6, 0.611, 0.611];
    pub const BIG_OP5: [f32; 3] = [0.1, 0.143, 0.143];
}

/// The style a box is set in.
#[derive(Debug, Clone, Copy)]
struct Env {
    style: Style,
    cramped: bool,
}

impl Env {
    fn size(self) -> f32 {
        match self.style {
            Style::Display | Style::Text => 1.,
            Style::Script => 0.7,
            Style::ScriptScript => 0.5,
        }
    }

    /// A font dimension at this style, in ems of the formula.
    fn get(self, table: [f32; 3]) -> f32 {
        let i = match self.style {
            Style::Display | Style::Text => 0,
            Style::Script => 1,
            Style::ScriptScript => 2,
        };
        table[i] * self.size()
    }

    fn with(self, style: Style) -> Env {
        Env { style, ..self }
    }

    fn cramp(self) -> Env {
        Env {
            cramped: true,
            ..self
        }
    }

    fn sup(self) -> Env {
        let style = match self.style {
            Style::Display | Style::Text => Style::Script,
            _ => Style::ScriptScript,
        };
        self.with(style)
    }

    fn sub(self) -> Env {
        self.sup().cramp()
    }

    fn num(self) -> Env {
        let style = match self.style {
            Style::Display => Style::Text,
            Style::Text => Style::Script,
            _ => Style::ScriptScript,
        };
        self.with(style)
    }

    fn den(self) -> Env {
        self.num().cramp()
    }

    fn tight(self) -> bool {
        matches!(self.style, Style::Script | Style::ScriptScript)
    }

    fn mu(self) -> f32 {
        self.size() / 18.
    }
}

/// Typeset `nodes`: display math, or inline (text style).
pub fn typeset(nodes: &[Node], display: bool, measure: &dyn Measure) -> MBox {
    let env = Env {
        style: if display { Style::Display } else { Style::Text },
        cramped: false,
    };
    let t = Typesetter { measure };
    let (tags, body): (Vec<&Node>, Vec<&Node>) =
        nodes.iter().partition(|n| matches!(n, Node::Tag(_)));
    let body: Vec<Node> = body.into_iter().cloned().collect();
    // `\\` and `&` outside an environment: rows, aligned at the `&`s when there are any.
    let mut b = if body.iter().any(|n| matches!(n, Node::Newline | Node::Tab)) {
        let aligned = body.iter().any(|n| matches!(n, Node::Tab));
        let kind = if aligned {
            ArrayKind::Aligned
        } else {
            ArrayKind::Gathered
        };
        let array = super::parse::rows_of(
            body,
            kind,
            if aligned { vec![] } else { vec![Align::Center] },
            (NO_DELIM, NO_DELIM),
        );
        t.array(&array, env)
    } else {
        t.list(&body, env)
    };
    for tag in tags {
        if let Node::Tag(inner) = tag {
            b.append(MBox::kern(2.));
            let mut wrapped = vec![Node::Sym {
                ch: '(',
                face: Face::Main,
                class: Class::Open,
            }];
            wrapped.extend(inner.iter().cloned());
            wrapped.push(Node::Sym {
                ch: ')',
                face: Face::Main,
                class: Class::Close,
            });
            b.append(t.list(&wrapped, env.with(Style::Text)));
        }
    }
    sanitize(b)
}

/// Whatever went wrong upstream, a box the renderer can trust: finite and not negative.
fn sanitize(mut b: MBox) -> MBox {
    let ok = |v: f32| if v.is_finite() { v } else { 0. };
    b.w = ok(b.w).max(0.);
    b.h = ok(b.h);
    b.d = ok(b.d);
    if b.h + b.d < 0. {
        b.d = -b.h;
    }
    b.items.retain(|item| match item {
        Item::Glyph { x, y, size, .. } => {
            x.is_finite() && y.is_finite() && size.is_finite() && *size > 0.
        }
        Item::Rule { x, y, w, h }
        | Item::Shape { x, y, w, h, .. }
        | Item::Frame { x, y, w, h, .. } => {
            [x, y, w, h].iter().all(|v| v.is_finite()) && *w >= 0. && *h >= 0.
        }
    });
    b
}

struct Typesetter<'a> {
    measure: &'a dyn Measure,
}

/// The space TeX puts between atoms of classes `l` and `r`, in mu.
fn spacing(l: Class, r: Class, tight: bool) -> f32 {
    use Class::*;
    let (mu, always) = match (l, r) {
        (Ord, Op) | (Op, Ord) | (Op, Op) | (Close, Op) | (Inner, Op) => (3., true),
        (Ord, Bin)
        | (Bin, Ord)
        | (Bin, Op)
        | (Bin, Open)
        | (Bin, Inner)
        | (Close, Bin)
        | (Inner, Bin)
        | (Op, Bin) => (4., false),
        (Ord, Rel)
        | (Op, Rel)
        | (Rel, Ord)
        | (Rel, Op)
        | (Rel, Open)
        | (Rel, Inner)
        | (Close, Rel)
        | (Inner, Rel) => (5., false),
        (Ord, Inner)
        | (Inner, Ord)
        | (Inner, Open)
        | (Inner, Punct)
        | (Inner, Inner)
        | (Close, Inner) => (3., false),
        (Punct, Ord | Op | Rel | Open | Close | Punct | Inner) => (3., false),
        _ => (0., true),
    };
    if always || !tight { mu } else { 0. }
}

impl Typesetter<'_> {
    fn list(&self, nodes: &[Node], env: Env) -> MBox {
        // Atoms with their classes; `None` is space, which doesn't change the neighbours.
        let mut atoms: Vec<(Option<Class>, MBox)> = Vec::with_capacity(nodes.len());
        for node in nodes {
            match node {
                Node::Space(em) => atoms.push((None, MBox::kern(em * env.size()))),
                Node::Tab | Node::Newline => atoms.push((None, MBox::kern(0.5 * env.size()))),
                Node::Styled { style, body } => {
                    let inner = Env {
                        style: *style,
                        cramped: env.cramped,
                    };
                    atoms.push((Some(Class::Ord), self.list(body, inner)));
                }
                _ => atoms.push(self.atom(node, env)),
            }
        }
        // A binary operator with nothing to operate on is an ordinary symbol (TeX's rules 5, 6).
        let classed: Vec<usize> = atoms
            .iter()
            .enumerate()
            .filter(|(_, (c, _))| c.is_some())
            .map(|(i, _)| i)
            .collect();
        for (k, &i) in classed.iter().enumerate() {
            if atoms[i].0 != Some(Class::Bin) {
                continue;
            }
            let prev = k.checked_sub(1).and_then(|p| atoms[classed[p]].0);
            let next = classed.get(k + 1).and_then(|&n| atoms[n].0);
            let lonely = matches!(
                prev,
                None | Some(Class::Bin | Class::Op | Class::Rel | Class::Open | Class::Punct)
            ) || matches!(next, None | Some(Class::Rel | Class::Close | Class::Punct));
            if lonely {
                atoms[i].0 = Some(Class::Ord);
            }
        }
        let mut out = MBox::default();
        let mut prev: Option<Class> = None;
        let count = atoms.len();
        for (class, b) in atoms {
            if let Some(class) = class {
                if let Some(p) = prev {
                    out.append(MBox::kern(spacing(p, class, env.tight()) * env.mu()));
                }
                prev = Some(class);
            }
            let (italic, skew, char_box) = (b.italic, b.skew, b.char_box);
            out.append(b);
            // A lone glyph stays one, for the scripts and accents set on it.
            if count == 1 {
                out.italic = italic;
                out.skew = skew;
                out.char_box = char_box;
            }
        }
        out
    }

    fn atom(&self, node: &Node, env: Env) -> (Option<Class>, MBox) {
        let (class, b) = match node {
            Node::Sym { ch, face, class } => (*class, self.glyph(*ch, *face, env)),
            Node::Text { text, face } => (Class::Ord, self.text(text, *face, env)),
            Node::Group(body) => (Class::Ord, self.list(body, env)),
            Node::Scripts {
                base,
                sup,
                sub,
                limits,
            } => self.scripts(base, sup.as_deref(), sub.as_deref(), *limits, env),
            Node::Frac {
                num,
                den,
                rule,
                style,
                delims,
            } => (Class::Ord, self.frac(num, den, *rule, *style, *delims, env)),
            Node::Sqrt { index, body } => (Class::Ord, self.sqrt(index.as_deref(), body, env)),
            Node::Op { body, .. } => (Class::Op, self.op(body, env)),
            Node::Accent { accent, body } => (Class::Ord, self.accent(*accent, body, env)),
            Node::LeftRight { delims, parts } => {
                (Class::Inner, self.left_right(delims, parts, env))
            }
            Node::Big { delim, size, class } => {
                let heights = [1.0, 1.2, 1.8, 2.4, 3.0];
                let height = heights[(*size as usize).min(4)] * env.size();
                (*class, self.delimiter(*delim, height, env))
            }
            Node::Boxed(body) => (Class::Ord, self.boxed(body, env)),
            Node::Array(array) => (Class::Ord, self.array(array, env)),
            Node::Phantom {
                body,
                width,
                height,
            } => {
                let b = self.list(body, env);
                (
                    Class::Ord,
                    MBox {
                        w: if *width { b.w } else { 0. },
                        h: if *height { b.h } else { 0. },
                        d: if *height { b.d } else { 0. },
                        ..MBox::default()
                    },
                )
            }
            Node::Cancel {
                body,
                forward,
                back,
            } => (Class::Ord, self.cancel(body, *forward, *back, env)),
            Node::Stack { base, over, under } => {
                let class = match base.as_slice() {
                    [
                        Node::Sym {
                            class: Class::Rel, ..
                        },
                    ] => Class::Rel,
                    _ => Class::Ord,
                };
                let b = self.list(base, env);
                (
                    class,
                    self.limits(b, 0., over.as_deref(), under.as_deref(), env, 0.1),
                )
            }
            Node::Arrow { right, over, under } => {
                (Class::Rel, self.x_arrow(*right, over, under, env))
            }
            Node::Not(inner) => {
                let (class, b) = self.atom(inner, env);
                (class.unwrap_or(Class::Rel), self.not(b, env))
            }
            Node::Class { class, body } => (*class, self.list(body, env)),
            Node::Tag(body) => (Class::Ord, self.list(body, env)),
            Node::Unknown(name) => (Class::Ord, self.text(name, Face::Main, env)),
            Node::Space(_) | Node::Tab | Node::Newline | Node::Styled { .. } => {
                (Class::Ord, MBox::default())
            }
        };
        (Some(class), b)
    }

    fn glyph(&self, ch: char, face: Face, env: Env) -> MBox {
        let s = env.size();
        let found = face
            .metrics(ch)
            .map(|m| (face, m))
            .or_else(|| Face::Main.metrics(ch).map(|m| (Face::Main, m)));
        match found {
            Some((face, [d, h, italic, skew, w])) => MBox {
                w: (w + italic) * s,
                h: h * s,
                d: d * s,
                items: vec![Item::Glyph {
                    x: 0.,
                    y: 0.,
                    text: ch.to_string(),
                    face,
                    size: s,
                }],
                italic: italic * s,
                skew: skew * s,
                char_box: true,
            },
            None => self.text(&ch.to_string(), face, env),
        }
    }

    /// A run of text in one face, set as a line.
    fn text(&self, text: &str, face: Face, env: Env) -> MBox {
        let s = env.size();
        let mut h: f32 = 0.;
        let mut d: f32 = 0.;
        let mut sum = 0.;
        let mut known = true;
        for c in text.chars() {
            match face.metrics(c).or_else(|| Face::Main.metrics(c)) {
                Some([cd, ch, italic, _, w]) => {
                    h = h.max(ch);
                    d = d.max(cd);
                    sum += w + if face.italic() { italic } else { 0. };
                }
                None => {
                    known = false;
                    h = h.max(0.72);
                    d = d.max(0.2);
                    sum += if c.len_utf8() > 2 { 1.0 } else { 0.55 };
                }
            }
        }
        if text.trim().is_empty() {
            h = 0.;
            d = 0.;
        }
        let w = self
            .measure
            .width(text, face)
            .filter(|w| w.is_finite() && *w >= 0.)
            .unwrap_or(sum);
        let _ = known;
        MBox {
            w: w * s,
            h: h * s,
            d: d * s,
            items: vec![Item::Glyph {
                x: 0.,
                y: 0.,
                text: text.to_string(),
                face,
                size: s,
            }],
            ..MBox::default()
        }
    }

    fn scripts(
        &self,
        base: &Node,
        sup: Option<&[Node]>,
        sub: Option<&[Node]>,
        limits: Option<bool>,
        env: Env,
    ) -> (Class, MBox) {
        let (class, b) = match base {
            Node::Group(g) if g.len() == 1 => self.atom(&g[0], env),
            _ => self.atom(base, env),
        };
        let class = class.unwrap_or(Class::Ord);
        // Operators set their limits above and below in display style, or when asked.
        let op_limits = match base {
            Node::Op { limits: l, .. } => limits.unwrap_or(*l && env.style == Style::Display),
            _ => limits == Some(true),
        };
        if op_limits {
            let slant = if matches!(
                base,
                Node::Op {
                    body: OpBody::Glyph(_),
                    ..
                }
            ) {
                b.italic
            } else {
                0.
            };
            let gap = if matches!(base, Node::Accent { .. }) {
                0.1 * env.size()
            } else {
                0.
            };
            return (class, self.limits(b, slant, sup, sub, env, gap));
        }
        let is_op_glyph = matches!(
            base,
            Node::Op {
                body: OpBody::Glyph(_),
                ..
            }
        );
        let char_box = b.char_box && !is_op_glyph;
        let italic = if char_box || is_op_glyph {
            b.italic
        } else {
            0.
        };
        let sup_env = env.sup();
        let sub_env = env.sub();
        let sup_box = sup.map(|s| self.list(s, sup_env));
        let sub_box = sub.map(|s| self.list(s, sub_env));
        let mut sup_shift = if char_box {
            0.
        } else {
            b.h - sup_env.get(sigma::SUP_DROP)
        };
        let mut sub_shift = if char_box {
            0.
        } else {
            b.d + sub_env.get(sigma::SUB_DROP)
        };
        let min_sup = if env.style == Style::Display {
            env.get(sigma::SUP1)
        } else if env.cramped {
            env.get(sigma::SUP3)
        } else {
            env.get(sigma::SUP2)
        };
        let xh = env.get(sigma::X_HEIGHT);
        let rule = env.get(sigma::RULE);
        match (&sup_box, &sub_box) {
            (Some(p), Some(q)) => {
                sup_shift = sup_shift.max(min_sup).max(p.d + 0.25 * xh);
                sub_shift = sub_shift.max(env.get(sigma::SUB2));
                let gap = (sup_shift - p.d) - (q.h - sub_shift);
                if gap < 4. * rule {
                    sub_shift = 4. * rule - (sup_shift - p.d) + q.h;
                    let psi = 0.8 * xh - (sup_shift - p.d);
                    if psi > 0. {
                        sup_shift += psi;
                        sub_shift -= psi;
                    }
                }
            }
            (None, Some(q)) => sub_shift = sub_shift.max(env.get(sigma::SUB1)).max(q.h - 0.8 * xh),
            (Some(p), None) => sup_shift = sup_shift.max(min_sup).max(p.d + 0.25 * xh),
            (None, None) => {}
        }
        let script_space = 0.05;
        let x = b.w;
        let mut out = MBox {
            w: b.w,
            h: b.h,
            d: b.d,
            ..MBox::default()
        };
        out.put(b, 0., 0.);
        let mut right = x;
        if let Some(p) = sup_box {
            out.h = out.h.max(sup_shift + p.h);
            out.d = out.d.max(p.d - sup_shift);
            right = right.max(x + p.w);
            out.put(p, x, sup_shift);
        }
        if let Some(q) = sub_box {
            out.d = out.d.max(sub_shift + q.d);
            out.h = out.h.max(q.h - sub_shift);
            right = right.max(x - italic + q.w);
            out.put(q, x - italic, -sub_shift);
        }
        out.w = right + script_space;
        (class, out)
    }

    /// `base` with `sup` centred above it and `sub` below, as for `\sum` in display style.
    fn limits(
        &self,
        base: MBox,
        slant: f32,
        sup: Option<&[Node]>,
        sub: Option<&[Node]>,
        env: Env,
        gap: f32,
    ) -> MBox {
        let sup_box = sup.map(|s| self.list(s, env.sup()));
        let sub_box = sub.map(|s| self.list(s, env.sub()));
        let w = base
            .w
            .max(sup_box.as_ref().map_or(0., |b| b.w))
            .max(sub_box.as_ref().map_or(0., |b| b.w));
        let mut out = MBox {
            w,
            h: base.h,
            d: base.d,
            ..MBox::default()
        };
        let base_w = base.w;
        let (base_h, base_d) = (base.h, base.d);
        out.put(base, (w - base_w) / 2., 0.);
        if let Some(p) = sup_box {
            let kern = env
                .get(sigma::BIG_OP1)
                .max(env.get(sigma::BIG_OP3) - p.d)
                .max(gap);
            let y = base_h + kern + p.d;
            out.h = y + p.h + env.get(sigma::BIG_OP5);
            let pw = p.w;
            out.put(p, (w - pw) / 2. + slant / 2., y);
        }
        if let Some(q) = sub_box {
            let kern = env
                .get(sigma::BIG_OP2)
                .max(env.get(sigma::BIG_OP4) - q.h)
                .max(gap);
            let y = base_d + kern + q.h;
            out.d = y + q.d + env.get(sigma::BIG_OP5);
            let qw = q.w;
            out.put(q, (w - qw) / 2. - slant / 2., -y);
        }
        out
    }

    fn frac(
        &self,
        num: &[Node],
        den: &[Node],
        rule: bool,
        style: Option<Style>,
        delims: Option<(char, char)>,
        env: Env,
    ) -> MBox {
        let env = match style {
            Some(style) => Env {
                style,
                cramped: env.cramped,
            },
            None => env,
        };
        let n = self.list(num, env.num());
        let d = self.list(den, env.den());
        let rule_spacing = env.get(sigma::RULE);
        let t = if rule { rule_spacing } else { 0. };
        let axis = env.get(sigma::AXIS);
        let display = env.style == Style::Display;
        let (mut num_shift, clearance, mut den_shift) = if display {
            (
                env.get(sigma::NUM1),
                if rule { 3. * t } else { 7. * rule_spacing },
                env.get(sigma::DENOM1),
            )
        } else {
            (
                if rule {
                    env.get(sigma::NUM2)
                } else {
                    env.get(sigma::NUM3)
                },
                if rule { t } else { 3. * rule_spacing },
                env.get(sigma::DENOM2),
            )
        };
        if rule {
            let above = (num_shift - n.d) - (axis + t / 2.);
            if above < clearance {
                num_shift += clearance - above;
            }
            let below = (axis - t / 2.) - (d.h - den_shift);
            if below < clearance {
                den_shift += clearance - below;
            }
        } else {
            let candidate = (num_shift - n.d) - (d.h - den_shift);
            if candidate < clearance {
                num_shift += 0.5 * (clearance - candidate);
                den_shift += 0.5 * (clearance - candidate);
            }
        }
        let width = n.w.max(d.w);
        let pad = 0.12 * env.size();
        let mut out = MBox {
            w: width + 2. * pad,
            h: num_shift + n.h,
            d: den_shift + d.d,
            ..MBox::default()
        };
        let (nw, dw) = (n.w, d.w);
        out.put(n, pad + (width - nw) / 2., num_shift);
        out.put(d, pad + (width - dw) / 2., -den_shift);
        if rule {
            out.items.push(Item::Rule {
                x: pad,
                y: axis - t / 2.,
                w: width,
                h: t,
            });
        }
        match delims {
            Some((l, r)) => {
                let size = if display {
                    env.get(sigma::DELIM1)
                } else {
                    env.get(sigma::DELIM2)
                };
                let mut wrapped = self.delimiter(l, size, env);
                // The delimiters stand in for the null space on either side.
                out.w -= 2. * pad;
                for item in &mut out.items {
                    item.shift(-pad, 0.);
                }
                wrapped.append(out);
                wrapped.append(self.delimiter(r, size, env));
                wrapped
            }
            None => out,
        }
    }

    fn sqrt(&self, index: Option<&[Node]>, body: &[Node], env: Env) -> MBox {
        let mut inner = self.list(body, env.cramp());
        if inner.h <= 0. {
            inner.h = env.get(sigma::X_HEIGHT);
        }
        let theta = env.get(sigma::RULE);
        let phi = if env.style == Style::Display {
            env.get(sigma::X_HEIGHT)
        } else {
            theta
        };
        let mut clearance = theta + phi / 4.;
        let min_height = inner.h + inner.d + clearance + theta;
        let s = env.size();
        // The radical sign: the font's, at the first size tall enough, else built from its
        // pieces (a hook at the bottom, a stem as long as it must be).
        let surd = [
            Face::Main,
            Face::Size1,
            Face::Size2,
            Face::Size3,
            Face::Size4,
        ]
        .into_iter()
        .filter_map(|face| face.metrics('\u{221a}').map(|m| (face, m)))
        .find(|(_, [d, h, ..])| (h + d) * s >= min_height);
        let height = surd.map_or(min_height, |(_, [d, h, ..])| (h + d) * s);
        let advance = surd.map_or(1.056, |(_, m)| m[4]) * s;
        let rule = 0.04 * s;
        let delim_depth = height - rule;
        if delim_depth > inner.h + inner.d + clearance {
            clearance = (clearance + delim_depth - inner.h - inner.d) / 2.;
        }
        let top = inner.h + clearance + rule;
        let bottom = top - height;
        let inner_w = inner.w;
        let mut out = MBox {
            w: advance + inner_w,
            h: top + theta.min(rule),
            d: (-bottom).max(inner.d),
            ..MBox::default()
        };
        match surd {
            Some((face, [_, h, ..])) => out.items.push(Item::Glyph {
                x: 0.,
                y: top - h * s,
                text: "\u{221a}".into(),
                face,
                size: s,
            }),
            None => out.items.extend(
                self.stacked(
                    ['\u{e001}', '\u{e000}', '\u{e000}', '\u{23b7}'],
                    top,
                    bottom,
                    env,
                )
                .items,
            ),
        }
        out.items.push(Item::Rule {
            x: advance - 0.0001,
            y: top - rule,
            w: inner_w + 0.0001,
            h: rule,
        });
        out.put(inner, advance, 0.);
        let Some(index) = index else { return out };
        let ix = self.list(
            index,
            Env {
                style: Style::ScriptScript,
                cramped: false,
            },
        );
        let raise = 0.6 * (out.h - out.d);
        let before = 5. * env.mu();
        let back = -10. * env.mu();
        let mut root = MBox {
            w: before,
            ..MBox::default()
        };
        let ix_w = ix.w;
        root.h = root.h.max(raise + ix.h);
        root.d = root.d.max(ix.d - raise);
        root.put(ix, before, raise);
        root.w = (before + ix_w + back).max(0.);
        root.append(out);
        root
    }

    /// A delimiter built from the largest font's pieces, as TeX builds tall ones: `top` and
    /// `bottom` ends (and a `middle`, for braces, when it isn't the `repeat`), with the
    /// `repeat` piece overlapping itself to fill between them. It spans `top` to `bottom`.
    fn stacked(
        &self,
        [top_c, repeat, middle, bottom_c]: [char; 4],
        top: f32,
        bottom: f32,
        env: Env,
    ) -> MBox {
        let s = env.size();
        let face = Face::Size4;
        let m = |c: char| face.metrics(c).unwrap_or([0., 0.5, 0., 0., 0.5]);
        let glyph = |c: char, y: f32| Item::Glyph {
            x: 0.,
            y,
            text: c.to_string(),
            face,
            size: s,
        };
        let [td, th, _, _, w] = m(top_c);
        let [bd, bh, ..] = m(bottom_c);
        let mut items = vec![glyph(top_c, top - th * s), glyph(bottom_c, bottom + bd * s)];
        let top_end = top - (th + td) * s;
        let bottom_start = bottom + (bh + bd) * s;
        let mut gaps = Vec::new();
        if middle != repeat {
            let [md, mh, ..] = m(middle);
            let base = (top + bottom) / 2. - (mh - md) / 2. * s;
            items.push(glyph(middle, base));
            gaps.push((top_end, base + mh * s));
            gaps.push((base - md * s, bottom_start));
        } else {
            gaps.push((top_end, bottom_start));
        }
        let [rd, rh, ..] = m(repeat);
        let r = (rh + rd) * s;
        for (hi, lo) in gaps {
            if hi <= lo || r <= 0. {
                continue;
            }
            // Copies that overlap a little, so no seam shows; the last one ends at `lo`.
            let n = (((hi - lo - r) / (r * 0.9)).ceil().max(0.) as usize + 1).min(200);
            let step = if n > 1 {
                (hi - lo - r) / (n - 1) as f32
            } else {
                0.
            };
            for i in 0..n {
                let copy_top = if n > 1 {
                    hi - i as f32 * step
                } else {
                    (hi + lo + r) / 2.
                };
                items.push(glyph(repeat, copy_top - rh * s));
            }
        }
        // The ends go on top of the stem.
        items.rotate_left(if middle != repeat { 3 } else { 2 });
        MBox {
            w: w * s,
            h: top,
            d: -bottom,
            items,
            ..MBox::default()
        }
    }

    fn op(&self, body: &OpBody, env: Env) -> MBox {
        match body {
            OpBody::Glyph(ch) => {
                let face = if env.style == Style::Display {
                    Face::Size2
                } else {
                    Face::Size1
                };
                let face = if face.metrics(*ch).is_some() {
                    face
                } else {
                    Face::Main
                };
                let mut b = self.glyph(*ch, face, env);
                let shift = (b.h - b.d) / 2. - env.get(sigma::AXIS);
                b = b.shifted(-shift);
                b.char_box = false;
                b
            }
            OpBody::Name(nodes) => {
                let mut b = self.list(nodes, env);
                b.char_box = false;
                b
            }
        }
    }

    /// `ch` in `face` at `size` ems, as a box.
    fn sized_glyph(&self, ch: char, face: Face, size: f32) -> MBox {
        let [d, h, _, _, w] = face.metrics(ch).unwrap_or([0., 0.7, 0., 0., 0.5]);
        MBox {
            w: w * size,
            h: h * size,
            d: d * size,
            items: vec![Item::Glyph {
                x: 0.,
                y: 0.,
                text: ch.to_string(),
                face,
                size,
            }],
            ..MBox::default()
        }
    }

    fn accent(&self, accent: Accent, body: &[Node], env: Env) -> MBox {
        let b = self.list(body, env.cramp());
        let s = env.size();
        let single = b.char_box;
        let ink_w = b.w - if single { b.italic } else { 0. };
        let theta = env.get(sigma::RULE);
        let xh = env.get(sigma::X_HEIGHT);
        let (bh, bd) = (b.h, b.d);
        let mut out = MBox {
            w: b.w,
            h: b.h,
            d: b.d,
            italic: b.italic,
            ..MBox::default()
        };
        let skew = if single { b.skew } else { 0. };
        out.put(b, 0., 0.);
        match accent {
            Accent::Glyph(_) | Accent::Hat | Accent::Tilde => {
                // The font's accent; a wide hat or tilde from the first size wide enough.
                let mark = match accent {
                    Accent::Glyph(c) => self.glyph(c, Face::Main, env),
                    _ => {
                        let c = if accent == Accent::Hat {
                            '\u{2c6}'
                        } else {
                            '\u{2dc}'
                        };
                        let face = [
                            Face::Main,
                            Face::Size1,
                            Face::Size2,
                            Face::Size3,
                            Face::Size4,
                        ]
                        .into_iter()
                        .find(|f| f.metrics(c).is_some_and(|m| m[4] * s >= ink_w * 0.9))
                        .unwrap_or(Face::Size4);
                        self.glyph(c, face, env)
                    }
                };
                let clearance = bh.min(xh);
                let y = bh - clearance + mark.d;
                let x = (ink_w - mark.w) / 2. + skew;
                out.h = out.h.max(y + mark.h);
                out.put(mark, x, y);
            }
            Accent::Vec => {
                // A small arrow just above the letter.
                let k = 0.62 * s;
                let mark = self.sized_glyph('\u{2192}', Face::Main, k);
                let y = bh.max(xh) + 0.06 * s + 0.133 * k;
                let x = (ink_w - mark.w) / 2. + skew;
                out.h = out.h.max(y + mark.h);
                out.put(mark, x, y);
            }
            Accent::Overline => {
                let y = bh + 3. * theta;
                out.h = y + 2. * theta;
                out.items.push(Item::Rule {
                    x: 0.,
                    y,
                    w: ink_w,
                    h: theta,
                });
            }
            Accent::Underline => {
                let y = -(bd + 3. * theta) - theta;
                out.d = bd + 5. * theta;
                out.items.push(Item::Rule {
                    x: 0.,
                    y,
                    w: ink_w,
                    h: theta,
                });
            }
            Accent::OverArrow { left, right } => {
                // The font's arrowheads with a rule between them, as long as the body.
                let k = 0.8 * s;
                let base = bh + 0.08 * s + 0.133 * k;
                let t = 0.04 * k;
                let head = 1.0 * k;
                let w = ink_w.max(head);
                let (mut from, mut to) = (0., w);
                if right {
                    out.put(self.sized_glyph('\u{2192}', Face::Main, k), w - head, base);
                    to = w - head * 0.5;
                }
                if left {
                    out.put(self.sized_glyph('\u{2190}', Face::Main, k), 0., base);
                    from = head * 0.5;
                }
                if to > from {
                    out.items.push(Item::Rule {
                        x: from,
                        y: base + 0.25 * k - t / 2.,
                        w: to - from,
                        h: t,
                    });
                }
                out.w = out.w.max(w);
                out.h = base + 0.367 * k;
            }
            Accent::OverBrace | Accent::UnderBrace => {
                // A brace of rules: a bar, ends turned towards the body, a point away from it.
                let over = accent == Accent::OverBrace;
                let t = 0.05 * s;
                let h = 0.22 * s;
                let gap = 0.1 * s;
                let (bar, end, tip) = if over {
                    (bh + gap + h / 2., bh + gap, bh + gap + h / 2.)
                } else {
                    (-(bd + gap + h / 2.), -(bd + gap + h / 2.), -(bd + gap + h))
                };
                let half = h / 2.;
                out.items.push(Item::Rule {
                    x: 0.,
                    y: bar - t / 2.,
                    w: ink_w,
                    h: t,
                });
                for x in [0., ink_w - t] {
                    out.items.push(Item::Rule {
                        x,
                        y: end,
                        w: t,
                        h: half,
                    });
                }
                out.items.push(Item::Rule {
                    x: (ink_w - t) / 2.,
                    y: tip,
                    w: t,
                    h: half,
                });
                if over {
                    out.h = bh + gap + h;
                } else {
                    out.d = bd + gap + h;
                }
            }
        }
        out
    }

    fn left_right(&self, delims: &[char], parts: &[Vec<Node>], env: Env) -> MBox {
        let boxes: Vec<MBox> = parts.iter().map(|p| self.list(p, env)).collect();
        let h = boxes.iter().map(|b| b.h).fold(0., f32::max);
        let d = boxes.iter().map(|b| b.d).fold(0., f32::max);
        let height = self.delim_height(h, d, env);
        let mut out = MBox::default();
        for (i, b) in boxes.into_iter().enumerate() {
            let delim = delims.get(i).copied().unwrap_or(NO_DELIM);
            if i > 0 {
                // `\middle`: a relation's space either side.
                out.append(MBox::kern(5. / 18. * env.size() * 0.6));
            }
            out.append(self.delimiter(delim, height, env));
            if i > 0 {
                out.append(MBox::kern(5. / 18. * env.size() * 0.6));
            }
            out.append(b);
        }
        out.append(self.delimiter(delims.last().copied().unwrap_or(NO_DELIM), height, env));
        out
    }

    /// How tall `\left`/`\right` delimiters must be around content `h` high and `d` deep
    /// (TeX's \delimiterfactor and \delimitershortfall).
    fn delim_height(&self, h: f32, d: f32, env: Env) -> f32 {
        let axis = env.get(sigma::AXIS);
        let dist = (h - axis).max(d + axis);
        (dist / 500. * 901.).max(2. * dist - 0.5 * env.size())
    }

    /// The delimiter `ch` at least `height` tall, centred on the math axis: the font's own
    /// glyph at the first size that's tall enough, or drawn.
    fn delimiter(&self, ch: char, height: f32, env: Env) -> MBox {
        let s = env.size();
        if ch == NO_DELIM {
            return MBox::kern(0.12 * s);
        }
        let axis = env.get(sigma::AXIS);
        let glyph = match ch {
            '|' => '\u{2223}',
            '\u{2016}' => '\u{2225}',
            c => c,
        };
        let faces: &[Face] = if matches!(glyph, '\u{2223}' | '\u{2225}') {
            &[Face::Main]
        } else {
            &[
                Face::Main,
                Face::Size1,
                Face::Size2,
                Face::Size3,
                Face::Size4,
            ]
        };
        let mut last = None;
        for face in faces {
            if let Some([d, h, ..]) = face.metrics(glyph) {
                let total = (h + d) * s;
                last = Some(*face);
                if total >= height - 0.01 {
                    let b = self.glyph(glyph, *face, env);
                    let shift = axis - (b.h - b.d) / 2.;
                    let mut b = b.shifted(shift);
                    b.char_box = false;
                    b.italic = 0.;
                    return b;
                }
            }
        }
        let (top, bottom) = (axis + height / 2., axis - height / 2.);
        // Taller than any glyph: built from the pieces of the largest size, as TeX does.
        let pieces = match ch {
            '(' => Some(['\u{239b}', '\u{239c}', '\u{239c}', '\u{239d}']),
            ')' => Some(['\u{239e}', '\u{239f}', '\u{239f}', '\u{23a0}']),
            '[' => Some(['\u{23a1}', '\u{23a2}', '\u{23a2}', '\u{23a3}']),
            ']' => Some(['\u{23a4}', '\u{23a5}', '\u{23a5}', '\u{23a6}']),
            '\u{230a}' => Some(['\u{23a2}', '\u{23a2}', '\u{23a2}', '\u{23a3}']),
            '\u{230b}' => Some(['\u{23a5}', '\u{23a5}', '\u{23a5}', '\u{23a6}']),
            '\u{2308}' => Some(['\u{23a1}', '\u{23a2}', '\u{23a2}', '\u{23a2}']),
            '\u{2309}' => Some(['\u{23a4}', '\u{23a5}', '\u{23a5}', '\u{23a5}']),
            '{' => Some(['\u{23a7}', '\u{23aa}', '\u{23a8}', '\u{23a9}']),
            '}' => Some(['\u{23ab}', '\u{23aa}', '\u{23ac}', '\u{23ad}']),
            _ => None,
        };
        if let Some(pieces) = pieces {
            return self.stacked(pieces, top, bottom, env);
        }
        // Bars are straight: rules as long as they must be.
        let bars: &[f32] = match ch {
            '|' => &[0.139],
            '\u{2016}' => &[0.139, 0.361],
            _ => &[],
        };
        if !bars.is_empty() {
            let t = 0.056 * s;
            let w = if bars.len() == 1 { 0.278 } else { 0.5 } * s;
            let items = bars
                .iter()
                .map(|x| Item::Rule {
                    x: x * s - t / 2.,
                    y: bottom,
                    w: t,
                    h: height,
                })
                .collect();
            return MBox {
                w,
                h: top,
                d: -bottom,
                items,
                ..MBox::default()
            };
        }
        // Nothing to build it from: the largest glyph there is.
        match last {
            Some(face) => {
                let b = self.glyph(glyph, face, env);
                let shift = axis - (b.h - b.d) / 2.;
                b.shifted(shift)
            }
            None => self.glyph(ch, Face::Main, env),
        }
    }

    fn boxed(&self, body: &[Node], env: Env) -> MBox {
        let b = self.list(body, env);
        let s = env.size();
        let pad = 0.3 * s;
        let t = 0.04 * s;
        let (w, h, d) = (b.w + 2. * (pad + t), b.h + pad + t, b.d + pad + t);
        let mut out = MBox {
            w,
            h,
            d,
            ..MBox::default()
        };
        out.put(b, pad + t, 0.);
        out.items.push(Item::Frame {
            x: 0.,
            y: -d,
            w,
            h: h + d,
            t,
        });
        out
    }

    fn cancel(&self, body: &[Node], forward: bool, back: bool, env: Env) -> MBox {
        let mut b = self.list(body, env);
        let s = env.size();
        let over = 0.1 * s;
        let (x, y, w, h) = (0., -b.d - over * 0.5, b.w, b.h + b.d + over);
        if forward {
            b.items.push(Item::Shape {
                x,
                y,
                w,
                h,
                t: 0.045 * s,
                shape: Shape::Strike { forward: true },
            });
        }
        if back {
            b.items.push(Item::Shape {
                x,
                y,
                w,
                h,
                t: 0.045 * s,
                shape: Shape::Strike { forward: false },
            });
        }
        b.char_box = false;
        b
    }

    /// A slash through `b` (`\not`).
    fn not(&self, mut b: MBox, env: Env) -> MBox {
        // The font's own `\not` slash: no width, drawn over a relation 0.778 em wide.
        let s = env.size();
        let slash = self.sized_glyph('\u{e020}', Face::Main, s);
        b.h = b.h.max(slash.h);
        b.d = b.d.max(slash.d);
        let x = (b.w - 0.778 * s) / 2.;
        b.put(slash, x, 0.);
        b.char_box = false;
        b
    }

    fn x_arrow(&self, right: bool, over: &[Node], under: &[Node], env: Env) -> MBox {
        let s = env.size();
        let o = self.list(over, env.sup());
        let u = self.list(under, env.sub());
        let w = o.w.max(u.w) + 0.9 * s;
        let w = w.max(1.4 * s);
        let axis = env.get(sigma::AXIS);
        let ah = 0.5 * s;
        let mut out = MBox {
            w,
            h: axis + ah / 2.,
            d: (ah / 2. - axis).max(0.),
            ..MBox::default()
        };
        // The font's arrowhead (its shaft sits on the axis) and a rule for the rest.
        let t = 0.04 * s;
        let head = self.sized_glyph(if right { '\u{2192}' } else { '\u{2190}' }, Face::Main, s);
        let (from, to) = if right {
            (0., w - 0.5 * s)
        } else {
            (0.5 * s, w)
        };
        out.items.push(Item::Rule {
            x: from,
            y: axis - t / 2.,
            w: to - from,
            h: t,
        });
        out.put(head, if right { w - s } else { 0. }, 0.);
        if o.w > 0. {
            let y = axis + ah / 2. + 0.111 * s + o.d;
            out.h = out.h.max(y + o.h);
            let ow = o.w;
            out.put(o, (w - ow) / 2., y);
        }
        if u.w > 0. {
            let y = axis - ah / 2. - 0.111 * s - u.h;
            out.d = out.d.max(-y + u.d);
            let uw = u.w;
            out.put(u, (w - uw) / 2., y);
        }
        out
    }

    fn array(&self, array: &Array, env: Env) -> MBox {
        let s = env.size();
        let cell_env = match array.kind {
            ArrayKind::Matrix => env.with(if env.style == Style::Display {
                Style::Text
            } else {
                env.style
            }),
            ArrayKind::Small => env.with(Style::Script),
            ArrayKind::Aligned | ArrayKind::Gathered => env.with(Style::Display),
            ArrayKind::Cases { display: true } => env.with(Style::Display),
            ArrayKind::Cases { display: false } => env.with(if env.style == Style::Display {
                Style::Text
            } else {
                env.style
            }),
        };
        let cell_s = cell_env.size();
        let rows: Vec<Vec<MBox>> = array
            .rows
            .iter()
            .map(|row| {
                row.iter()
                    .enumerate()
                    .map(|(i, cell)| {
                        // `aligned`'s left-hand columns start with the relation they align on;
                        // set them as if something came before it, so it keeps its space.
                        if array.kind == ArrayKind::Aligned && i % 2 == 1 {
                            let mut padded = vec![Node::Group(Vec::new())];
                            padded.extend(cell.iter().cloned());
                            self.list(&padded, cell_env)
                        } else {
                            self.list(cell, cell_env)
                        }
                    })
                    .collect()
            })
            .collect();
        let ncols = rows.iter().map(Vec::len).max().unwrap_or(0).max(1);
        let mut widths = vec![0f32; ncols];
        for row in &rows {
            for (i, cell) in row.iter().enumerate() {
                widths[i] = widths[i].max(cell.w);
            }
        }
        let align = |i: usize| -> Align {
            match array.kind {
                ArrayKind::Aligned => {
                    if i.is_multiple_of(2) {
                        Align::Right
                    } else {
                        Align::Left
                    }
                }
                ArrayKind::Cases { .. } => Align::Left,
                _ => array.cols.get(i).copied().unwrap_or(Align::Center),
            }
        };
        let gap = |i: usize| -> f32 {
            // Space before column `i`.
            if i == 0 {
                return 0.;
            }
            match array.kind {
                ArrayKind::Matrix => 1.0 * s,
                ArrayKind::Small => 0.5556 * cell_s,
                ArrayKind::Aligned => {
                    if i.is_multiple_of(2) {
                        1.0 * s
                    } else {
                        0.
                    }
                }
                ArrayKind::Gathered => 1.0 * s,
                ArrayKind::Cases { .. } => 1.0 * s,
            }
        };
        let (stretch, jot) = match array.kind {
            ArrayKind::Small => (0.5, 0.),
            ArrayKind::Aligned | ArrayKind::Gathered => (1.0, 0.3 * s),
            _ => (1.0, 0.),
        };
        let strut_h = 0.7 * 1.2 * stretch * s;
        let strut_d = 0.3 * 1.2 * stretch * s;
        let row_dims: Vec<(f32, f32)> = rows
            .iter()
            .map(|row| {
                let h = row.iter().map(|c| c.h).fold(strut_h, f32::max);
                let d = row.iter().map(|c| c.d).fold(strut_d, f32::max);
                (h, d)
            })
            .collect();
        let total: f32 = row_dims.iter().map(|(h, d)| h + d).sum::<f32>()
            + jot * row_dims.len().saturating_sub(1) as f32;
        let axis = env.get(sigma::AXIS);
        let top = total / 2. + axis;
        let outer = if array.kind == ArrayKind::Matrix && !array.col_rules.is_empty() {
            0.5 * s
        } else {
            0.
        };
        let mut xs = Vec::with_capacity(ncols);
        let mut x = outer;
        for (i, w) in widths.iter().enumerate() {
            x += gap(i);
            xs.push(x);
            x += w;
        }
        let width = x + outer;
        let mut out = MBox {
            w: width,
            h: top,
            d: total - top,
            ..MBox::default()
        };
        let mut y = top;
        let mut row_tops = Vec::with_capacity(rows.len() + 1);
        for (r, row) in rows.into_iter().enumerate() {
            let (h, d) = row_dims[r];
            if r > 0 {
                y -= jot;
            }
            row_tops.push(y);
            let baseline = y - h;
            for (i, cell) in row.into_iter().enumerate() {
                let cx = match align(i) {
                    Align::Left => xs[i],
                    Align::Center => xs[i] + (widths[i] - cell.w) / 2.,
                    Align::Right => xs[i] + widths[i] - cell.w,
                };
                out.put(cell, cx, baseline);
            }
            y = baseline - d;
        }
        row_tops.push(y);
        let t = 0.04 * s;
        for &r in &array.row_rules {
            if let Some(&ry) = row_tops.get(r) {
                out.items.push(Item::Rule {
                    x: 0.,
                    y: ry - t / 2.,
                    w: width,
                    h: t,
                });
            }
        }
        for &c in &array.col_rules {
            let cx = if c == 0 {
                0.
            } else if c >= ncols {
                width - t
            } else {
                xs[c] - gap(c) / 2. - t / 2.
            };
            out.items.push(Item::Rule {
                x: cx,
                y: -(total - top),
                w: t,
                h: total,
            });
        }
        let (l, r) = array.delims;
        if l == NO_DELIM && r == NO_DELIM {
            return out;
        }
        let height = self.delim_height(out.h, out.d, env);
        let mut wrapped = if l == NO_DELIM {
            MBox::default()
        } else {
            self.delimiter(l, height, env)
        };
        if matches!(array.kind, ArrayKind::Cases { .. }) {
            wrapped.append(MBox::kern(0.17 * s));
        }
        wrapped.append(out);
        if r != NO_DELIM {
            wrapped.append(self.delimiter(r, height, env));
        }
        wrapped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::parse::parse;

    fn set(tex: &str, display: bool) -> MBox {
        typeset(&parse(tex), display, &ByTables)
    }

    #[test]
    fn every_symbol_in_the_table_has_metrics() {
        // What the table names, the fonts can draw (a few KaTeX builds from pieces).
        let missing: Vec<_> = tables::SYMBOLS
            .iter()
            .filter(|(_, ch, font, _, italic)| {
                let face = if *italic {
                    Face::MathItalic
                } else if *font == SymbolFont::Ams {
                    Face::Ams
                } else {
                    Face::Main
                };
                face.metrics(*ch).is_none() && Face::Main.metrics(*ch).is_none()
            })
            .map(|(name, ..)| *name)
            .collect();
        assert!(missing.len() < 40, "{missing:?}");
    }

    #[test]
    fn a_fraction_stacks_around_the_axis() {
        let b = set(r"\frac{a}{b}", true);
        let rule = b
            .items
            .iter()
            .find_map(|i| {
                if let Item::Rule { y, h, w, .. } = i {
                    Some((*y, *h, *w))
                } else {
                    None
                }
            })
            .expect("a rule");
        assert!(
            (rule.0 + rule.1 / 2. - 0.25).abs() < 1e-4,
            "the bar sits on the axis: {rule:?}"
        );
        assert!(b.h > 0.9 && b.d > 0.6, "{b:?}");
        let glyphs: Vec<(f32, &str)> = b
            .items
            .iter()
            .filter_map(|i| {
                if let Item::Glyph { y, text, .. } = i {
                    Some((*y, text.as_str()))
                } else {
                    None
                }
            })
            .collect();
        assert!(glyphs.iter().any(|(y, t)| *t == "a" && *y > 0.5));
        assert!(glyphs.iter().any(|(y, t)| *t == "b" && *y < -0.5));
        // Inline, it's smaller and tighter.
        let inline = set(r"\frac{a}{b}", false);
        assert!(inline.h < b.h && inline.d < b.d);
        assert!(
            inline
                .items
                .iter()
                .any(|i| matches!(i, Item::Glyph { size, .. } if (*size - 0.7).abs() < 1e-6))
        );
    }

    #[test]
    fn scripts_are_smaller_and_offset() {
        let b = set("x^2", false);
        let two = b
            .items
            .iter()
            .find_map(|i| {
                if let Item::Glyph {
                    text, y, size, x, ..
                } = i
                {
                    (text == "2").then_some((*x, *y, *size))
                } else {
                    None
                }
            })
            .unwrap();
        assert!(
            two.1 > 0.3 && (two.2 - 0.7).abs() < 1e-6 && two.0 > 0.5,
            "{two:?}"
        );
        let nested = set("e^{x^{2}}", false);
        assert!(nested.items.iter().any(|i| matches!(i, Item::Glyph { text, size, .. } if text == "2" && (*size - 0.5).abs() < 1e-6)));
        let sub = set("x_i", false);
        assert!(
            sub.items
                .iter()
                .any(|i| matches!(i, Item::Glyph { text, y, .. } if text == "i" && *y < 0.))
        );
    }

    #[test]
    fn relations_and_binaries_get_their_space() {
        let tight = set("ab", false).w;
        let rel = set("a=b", false).w;
        let eq = Face::Main.metrics('=').unwrap()[4];
        assert!(
            (rel - tight - eq - 2. * 5. / 18.).abs() < 1e-4,
            "{rel} {tight}"
        );
        // A leading minus is a sign, not an operator.
        let neg = set("-a", false).w;
        let minus = Face::Main.metrics('\u{2212}').unwrap()[4];
        let a = set("a", false).w;
        assert!((neg - minus - a).abs() < 1e-4);
    }

    #[test]
    fn display_operators_are_large_with_limits() {
        let d = set(r"\sum_{i=1}^n i", true);
        let t = set(r"\sum_{i=1}^n i", false);
        assert!(d.h > t.h && d.d > t.d);
        assert!(d.items.iter().any(|i| matches!(
            i,
            Item::Glyph {
                face: Face::Size2,
                ..
            }
        )));
        assert!(t.items.iter().any(|i| matches!(
            i,
            Item::Glyph {
                face: Face::Size1,
                ..
            }
        )));
    }

    #[test]
    fn delimiters_grow_with_their_content() {
        let small = set(r"\left( x \right)", true);
        let big = set(r"\left( \frac{a}{b} \right)", true);
        let tall = set(
            r"\left( \begin{matrix} a \\ b \\ c \\ d \\ e \end{matrix} \right)",
            true,
        );
        assert!(small.h < big.h && big.h < tall.h);
        assert!(
            tall.items.iter().any(
                |i| matches!(i, Item::Glyph { text, face: Face::Size4, .. } if text == "\u{239b}")
            ),
            "built from pieces"
        );
    }

    #[test]
    fn roots_boxes_and_rows() {
        let r = set(r"\sqrt{x}", false);
        assert!(
            r.items
                .iter()
                .any(|i| matches!(i, Item::Glyph { text, .. } if text == "\u{221a}"))
        );
        let nth = set(r"\sqrt[3]{x}", false);
        assert!(
            nth.w >= r.w
                && nth
                    .items
                    .iter()
                    .any(|i| matches!(i, Item::Glyph { text, .. } if text == "3"))
        );
        let boxed = set(r"\boxed{0.0162}", true);
        assert!(boxed.items.iter().any(|i| matches!(i, Item::Frame { .. })));
        let rows = set(r"a = b \\ c = d", true);
        assert!(rows.h + rows.d > 2.);
    }

    #[test]
    fn inline_boxes_can_sit_on_a_baseline() {
        // The text view takes an inline object only with its baseline inside it.
        for tex in [
            r"\vec{v}",
            r"\hat\theta",
            r"\bar{x}",
            "x",
            "-",
            "=",
            r"\cdot",
            ".",
            r"\overrightarrow{AB}",
            r"\not=",
            r"\cancel{x}",
        ] {
            let b = set(tex, false);
            assert!(b.h > 0. && b.d >= 0., "{tex}: {b:?}");
        }
    }

    #[test]
    fn unknown_commands_show_their_names() {
        let b = set(r"\frobnicate{x}", false);
        assert!(
            b.items
                .iter()
                .any(|i| matches!(i, Item::Glyph { text, .. } if text == r"\frobnicate"))
        );
    }

    #[test]
    fn fuzz_layout_is_finite() {
        let pieces = [
            r"\frac",
            r"\sqrt",
            "[",
            "]",
            "{",
            "}",
            "^",
            "_",
            "&",
            r"\\",
            r"\left(",
            r"\right)",
            r"\middle|",
            r"\begin{bmatrix}",
            r"\end{bmatrix}",
            r"\begin{cases}",
            r"\end{cases}",
            r"\begin{aligned}",
            r"\end{aligned}",
            "x",
            "1",
            " ",
            r"\alpha",
            r"\text{a b}",
            r"\boxed",
            r"\hat",
            r"\widehat",
            r"\overbrace",
            r"\underbrace",
            r"\over",
            r"\displaystyle",
            r"\not",
            r"\limits",
            "'",
            r"\hline",
            r"\Big(",
            r"\zzz",
            "é",
            r"\sum",
            r"\int",
            r"\lim",
            r"\xrightarrow",
            r"\cancel",
            r"\overset",
            r"\binom",
            r"\tag{1}",
            r"\kern-3mu",
            r"\hspace{2em}",
            r"\vec",
            r"\overline",
        ];
        let mut seed = 0xdead_beef_cafe_f00du64;
        for _ in 0..3000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let mut s = String::new();
            let mut x = seed;
            for _ in 0..(seed % 24) {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                s.push_str(pieces[(x % pieces.len() as u64) as usize]);
            }
            for display in [false, true] {
                let b = set(&s, display);
                assert!(
                    b.w.is_finite()
                        && b.h.is_finite()
                        && b.d.is_finite()
                        && b.w >= 0.
                        && b.h + b.d >= 0.,
                    "{s}: {b:?}"
                );
            }
        }
    }
}
