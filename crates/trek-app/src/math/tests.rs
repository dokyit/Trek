// Not `super::*`: that brings GPUI's `test` attribute in over the standard one.
use super::{AXIS, Accent, Class, Columns, Face, Laid, MAX_SOURCE, Measure, Node, Paint, Segment, Setter, Style, Unparsed, X_HEIGHT, claims, classes, displayed, explicit, parse};

/// Every character a fixed advance (half an em, mono a little wider), so layout is exact.
struct Fixed;

impl Measure for Fixed {
    fn width(&mut self, text: &str, face: Face, size: f32) -> f32 {
        let per = if face == Face::Mono { 0.6 } else { 0.5 };
        text.chars().count() as f32 * per * size
    }
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 1.
}

fn italic(text: &str) -> Node {
    Node::Sym { text: text.into(), face: Face::Italic, class: Class::Ord }
}

fn upright(text: &str, class: Class) -> Node {
    Node::Sym { text: text.into(), face: Face::Upright, class }
}

fn number(text: &str) -> Node {
    upright(text, Class::Ord)
}

/// The top-level atoms of `src`.
fn atoms(src: &str) -> Vec<Node> {
    match parse(src) {
        Ok(Node::Group(items)) => items,
        Ok(other) => vec![other],
        Err(e) => panic!("{src}: {e:?}"),
    }
}

fn one(src: &str) -> Node {
    let mut items = atoms(src);
    assert_eq!(items.len(), 1, "{src}: {items:?}");
    items.pop().unwrap()
}

fn lay(src: &str, style: Style, size: f32) -> Laid {
    let tree = parse(src).unwrap_or_else(|e| panic!("{src}: {e:?}"));
    Setter::new(&mut Fixed, size).formula(&tree, style)
}

fn texts(laid: &Laid) -> Vec<(String, f32, f32, f32)> {
    laid.paint
        .iter()
        .filter_map(|p| match p {
            Paint::Text { x, y, text, size, .. } => Some((text.clone(), *x, *y, *size)),
            _ => None,
        })
        .collect()
}

fn text_at(laid: &Laid, wanted: &str) -> (f32, f32, f32) {
    texts(laid).into_iter().find(|(t, ..)| t == wanted).map(|(_, x, y, s)| (x, y, s)).unwrap_or_else(|| panic!("no {wanted} in {:?}", texts(laid)))
}

fn contains_unknown(node: &Node) -> bool {
    match node {
        Node::Unknown(_) => true,
        Node::Sym { .. } | Node::Op { .. } | Node::Space(_) | Node::Big { .. } => false,
        Node::Group(items) => items.iter().any(contains_unknown),
        Node::Frac { num, den, .. } => contains_unknown(num) || contains_unknown(den),
        Node::Scripts { base, sup, sub } => contains_unknown(base) || sup.as_deref().is_some_and(contains_unknown) || sub.as_deref().is_some_and(contains_unknown),
        Node::Stack { base, over, under } => contains_unknown(base) || over.as_deref().is_some_and(contains_unknown) || under.as_deref().is_some_and(contains_unknown),
        Node::Sqrt { index, body } => contains_unknown(body) || index.as_deref().is_some_and(contains_unknown),
        Node::Delimited { body, .. } | Node::Accent { body, .. } | Node::Boxed(body) | Node::Phantom(body) => contains_unknown(body),
        Node::Table { rows, .. } => rows.iter().flatten().any(contains_unknown),
    }
}

// ---------- claiming ----------

#[test]
fn maths_is_claimed_and_prices_are_not() {
    for (content, after) in [("x", None), ("x^2", Some(' ')), ("E = mc^2", Some('.')), ("\\frac{a}{b}", None), ("p", Some(',')), ("a+b", None), ("n/2", None), ("P(A \\mid B)", None)] {
        assert!(claims(content, after), "{content:?} should draw as maths");
    }
    // "$5 and $10" pairs into a span holding `5 and `, followed by a digit; "$20/mo vs $30/mo"
    // into `20/mo vs `. Words between dollars are prose too.
    for (content, after) in [("5 and ", Some('1')), ("5 and ", None), ("20/mo vs ", Some('3')), ("1,000 to ", Some('2')), ("5", Some('0')), (" x", None), ("two words", None), ("", None), ("  ", None)] {
        assert!(!claims(content, after), "{content:?} should stay prose");
    }
}

#[test]
fn explicit_delimiters_are_maths_whatever_they_hold() {
    assert!(explicit("$$5 and 10$$") && explicit("\\(5 and 10\\)") && explicit("\\[x\\]"));
    assert!(!explicit("$5 and $"));
    assert!(displayed("$$x$$") && displayed("\\[x\\]") && !displayed("\\(x\\)") && !displayed("$x$"));
}

// ---------- parsing ----------

#[test]
fn letters_are_italic_and_numbers_one_upright_run() {
    assert_eq!(atoms("x"), vec![italic("x")]);
    assert_eq!(atoms("ab"), vec![italic("a"), italic("b")]);
    assert_eq!(atoms("0.0162482"), vec![number("0.0162482")]);
    assert_eq!(atoms("2x"), vec![number("2"), italic("x")]);
    // A full stop after a number ends the sentence; it isn't part of the number.
    assert_eq!(atoms("1."), vec![number("1"), upright(".", Class::Ord)]);
    assert_eq!(atoms("a-b=c"), vec![italic("a"), upright("−", Class::Bin), italic("b"), upright("=", Class::Rel), italic("c")]);
    assert_eq!(atoms("(x)"), vec![upright("(", Class::Open), italic("x"), upright(")", Class::Close)]);
}

#[test]
fn groups_hold_their_atoms() {
    assert_eq!(atoms("{a}{bc}"), vec![Node::Group(vec![italic("a")]), Node::Group(vec![italic("b"), italic("c")])]);
    assert_eq!(atoms("{}"), vec![Node::Group(vec![])]);
}

#[test]
fn fractions_and_binomials() {
    let frac = |num: Node, den: Node, style| Node::Frac { num: Box::new(num), den: Box::new(den), bar: true, style };
    assert_eq!(one("\\frac{a}{b}"), frac(italic("a"), italic("b"), None));
    assert_eq!(one("\\frac12"), frac(number("1"), number("2"), None));
    assert_eq!(one("\\dfrac{1}{2}"), frac(number("1"), number("2"), Some(Style::Display)));
    assert_eq!(one("\\tfrac{1}{2}"), frac(number("1"), number("2"), Some(Style::Text)));
    assert_eq!(
        one("\\frac{a+b}{c}"),
        frac(Node::Group(vec![italic("a"), upright("+", Class::Bin), italic("b")]), italic("c"), None)
    );
    assert_eq!(
        one("\\binom{n}{k}"),
        Node::Delimited { left: Some('('), body: Box::new(Node::Frac { num: Box::new(italic("n")), den: Box::new(italic("k")), bar: false, style: None }), right: Some(')') }
    );
}

#[test]
fn scripts_hang_on_the_atom_before_them() {
    let scripts = |base: Node, sup: Option<Node>, sub: Option<Node>| Node::Scripts { base: Box::new(base), sup: sup.map(Box::new), sub: sub.map(Box::new) };
    assert_eq!(one("x^2"), scripts(italic("x"), Some(number("2")), None));
    assert_eq!(one("x_i^2"), scripts(italic("x"), Some(number("2")), Some(italic("i"))));
    assert_eq!(one("x^{2}_{i}"), scripts(italic("x"), Some(number("2")), Some(italic("i"))));
    assert_eq!(one("e^{-x}"), scripts(italic("e"), Some(Node::Group(vec![upright("−", Class::Bin), italic("x")])), None));
    assert_eq!(one("f'"), scripts(italic("f"), Some(upright("′", Class::Ord)), None));
    assert_eq!(one("f''"), scripts(italic("f"), Some(upright("′′", Class::Ord)), None));
    assert_eq!(one("^2"), scripts(Node::Group(vec![]), Some(number("2")), None));
    let sum = Node::Op { text: "∑".into(), glyph: true, limits: true };
    assert_eq!(
        one("\\sum_{i=1}^{n}"),
        scripts(sum, Some(italic("n")), Some(Node::Group(vec![italic("i"), upright("=", Class::Rel), number("1")])))
    );
    let int = Node::Op { text: "∫".into(), glyph: true, limits: true };
    assert_eq!(one("\\int_0^1"), scripts(int, Some(number("1")), Some(number("0"))));
    let lim = Node::Op { text: "lim".into(), glyph: false, limits: true };
    assert!(matches!(one("\\lim_{x \\to 0}"), Node::Scripts { base, .. } if *base == lim));
    // `\nolimits` keeps the scripts beside even in display.
    assert!(matches!(one("\\sum\\nolimits_i"), Node::Scripts { base, .. } if matches!(*base, Node::Op { limits: false, .. })));
}

#[test]
fn radicals() {
    assert_eq!(one("\\sqrt{x}"), Node::Sqrt { index: None, body: Box::new(italic("x")) });
    assert_eq!(one("\\sqrt[3]{x}"), Node::Sqrt { index: Some(Box::new(number("3"))), body: Box::new(italic("x")) });
    assert_eq!(one("\\sqrt2"), Node::Sqrt { index: None, body: Box::new(number("2")) });
}

#[test]
fn delimiters_scale_to_their_content() {
    assert_eq!(
        one("\\left( x \\right)"),
        Node::Delimited { left: Some('('), body: Box::new(Node::Group(vec![italic("x")])), right: Some(')') }
    );
    assert_eq!(one("\\left. x \\right|"), Node::Delimited { left: None, body: Box::new(Node::Group(vec![italic("x")])), right: Some('|') });
    assert_eq!(one("\\left\\{ x \\right\\}"), Node::Delimited { left: Some('{'), body: Box::new(Node::Group(vec![italic("x")])), right: Some('}') });
    assert_eq!(one("\\left\\langle x \\right\\rangle"), Node::Delimited { left: Some('⟨'), body: Box::new(Node::Group(vec![italic("x")])), right: Some('⟩') });
    assert_eq!(one("\\big("), Node::Big { delim: Some('('), size: 1.2, class: Class::Open });
    assert_eq!(one("\\Bigr]"), Node::Big { delim: Some(']'), size: 1.8, class: Class::Close });
    assert_eq!(one("\\bigg|"), Node::Big { delim: Some('|'), size: 2.4, class: Class::Ord });
    assert_eq!(one("\\Biggl\\lfloor"), Node::Big { delim: Some('⌊'), size: 3.0, class: Class::Open });
}

#[test]
fn text_and_font_commands() {
    assert_eq!(atoms("\\text{if } x"), vec![upright("if ", Class::Ord), italic("x")]);
    assert_eq!(atoms("\\mathrm{d}x"), vec![number("d"), italic("x")]);
    assert_eq!(one("\\mathbf{v}"), Node::Sym { text: "v".into(), face: Face::Bold, class: Class::Ord });
    assert_eq!(one("\\mathit{ab}"), Node::Group(vec![italic("a"), italic("b")]));
    assert_eq!(one("\\mathbb{R}"), number("ℝ"));
    assert_eq!(one("\\mathbb R"), number("ℝ"));
    assert_eq!(one("\\mathcal{L}"), number("ℒ"));
    assert_eq!(one("\\operatorname{argmax}"), Node::Op { text: "argmax".into(), glyph: false, limits: false });
    assert_eq!(one("\\operatorname*{argmax}"), Node::Op { text: "argmax".into(), glyph: false, limits: true });
    assert_eq!(one("\\boldsymbol{\\alpha}"), Node::Sym { text: "α".into(), face: Face::BoldItalic, class: Class::Ord });
}

#[test]
fn boxes_bars_and_accents() {
    assert_eq!(one("\\boxed{x}"), Node::Boxed(Box::new(italic("x"))));
    for (cmd, accent) in [("overline", Accent::Bar), ("bar", Accent::Bar), ("underline", Accent::Underline), ("hat", Accent::Hat), ("vec", Accent::Vec), ("dot", Accent::Dot), ("ddot", Accent::Ddot), ("tilde", Accent::Tilde)] {
        assert_eq!(one(&format!("\\{cmd}{{x}}")), Node::Accent { accent, body: Box::new(italic("x")) }, "{cmd}");
    }
}

#[test]
fn greek_letters_and_operator_names() {
    assert_eq!(atoms("\\alpha\\theta\\Omega"), vec![italic("α"), italic("θ"), upright("Ω", Class::Ord)]);
    for name in ["sin", "cos", "tan", "log", "ln", "exp", "min", "max", "det", "lim"] {
        let node = one(&format!("\\{name}"));
        assert!(matches!(&node, Node::Op { text, glyph: false, .. } if text == name), "{name}: {node:?}");
    }
}

#[test]
fn every_symbol_the_answers_use_is_known() {
    let names = "cdot times pm mp le ge ne neq approx sim simeq equiv propto infty partial nabla to rightarrow leftarrow Rightarrow Leftarrow Leftrightarrow mapsto mid in notin subset subseteq supset cup cap forall exists neg land lor ldots cdots vdots ddots prime circ degree % { } , ; ! quad qquad langle rangle lfloor rfloor lceil rceil |";
    for name in names.split(' ') {
        let src = format!("\\{name}");
        let tree = parse(&src).unwrap_or_else(|e| panic!("{src}: {e:?}"));
        assert!(!contains_unknown(&tree), "{src}: {tree:?}");
    }
    assert_eq!(one("\\mid"), upright("∣", Class::Rel));
    assert_eq!(one("\\approx"), upright("≈", Class::Rel));
    assert_eq!(one("\\{"), upright("{", Class::Open));
    assert_eq!(one("\\,"), Node::Space(3. / 18.));
    assert_eq!(one("\\quad"), Node::Space(1.));
    assert_eq!(one("\\!"), Node::Space(-3. / 18.));
}

#[test]
fn environments_make_tables() {
    let Node::Table { rows, columns } = one("\\begin{aligned} a &= b \\\\ c &= d \\end{aligned}") else { panic!() };
    assert_eq!(columns, Columns::Aligned);
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row.len() == 2));
    assert_eq!(rows[1][0], Node::Group(vec![italic("c")]));
    assert!(matches!(one("\\begin{align*} a &= b \\end{align*}"), Node::Table { columns: Columns::Aligned, .. }));
    let Node::Delimited { left: Some('{'), right: None, body } = one("f(x) = \\begin{cases} 1 & x > 0 \\\\ 0 & \\text{otherwise} \\end{cases}".split_once("= ").unwrap().1) else { panic!() };
    assert!(matches!(body.as_ref(), Node::Table { columns: Columns::Left, rows } if rows.len() == 2));
    for (env, left, right) in [("matrix", None, None), ("pmatrix", Some('('), Some(')')), ("bmatrix", Some('['), Some(']')), ("vmatrix", Some('|'), Some('|'))] {
        let src = format!("\\begin{{{env}}} 1 & 2 \\\\ 3 & 4 \\end{{{env}}}");
        let node = one(&src);
        let table = match (&node, left) {
            (Node::Delimited { left: l, right: r, body }, Some(_)) => {
                assert_eq!((*l, *r), (left, right), "{env}");
                body.as_ref().clone()
            }
            (table, None) => table.clone(),
            _ => panic!("{env}: {node:?}"),
        };
        let Node::Table { rows, columns: Columns::Centred } = table else { panic!("{env}") };
        assert_eq!(rows, vec![vec![Node::Group(vec![number("1")]), Node::Group(vec![number("2")])], vec![Node::Group(vec![number("3")]), Node::Group(vec![number("4")])]], "{env}");
    }
    // A trailing `\\` adds no empty row.
    let Node::Table { rows, .. } = one("\\begin{matrix} a \\\\ b \\\\ \\end{matrix}") else { panic!() };
    assert_eq!(rows.len(), 2);
}

#[test]
fn lines_outside_an_environment_are_a_table() {
    let Node::Table { rows, columns } = parse("a = b \\\\ c = d").unwrap() else { panic!() };
    assert_eq!((rows.len(), columns), (2, Columns::Centred));
    let Node::Table { columns, .. } = parse("a &= b \\\\ c &= d").unwrap() else { panic!() };
    assert_eq!(columns, Columns::Aligned);
}

#[test]
fn an_unknown_command_keeps_its_name_and_the_rest_draws() {
    assert_eq!(atoms("\\foo + x"), vec![Node::Unknown("foo".into()), upright("+", Class::Bin), italic("x")]);
}

#[test]
fn what_the_parser_cant_read_is_an_error_not_a_panic() {
    for src in ["\\frac{a", "{x", "x}", "\\left( x", "x \\right)", "\\begin{matrix} a", "\\begin{matrix} a \\end{pmatrix}", "\\sqrt[3", "x^", "\\", "\\left\\foo x \\right)"] {
        assert_eq!(parse(src), Err(Unparsed), "{src}");
    }
    // Deep nesting stops cleanly instead of running out of stack.
    assert_eq!(parse(&format!("{}x{}", "{".repeat(4000), "}".repeat(4000))), Err(Unparsed));
    assert_eq!(parse(&format!("{}x", "\\sqrt".repeat(1500))), Err(Unparsed));
    assert_eq!(parse(&"x".repeat(MAX_SOURCE + 1)), Err(Unparsed));
}

#[test]
fn garbage_never_panics() {
    const PIECES: &[&str] = &[
        "x", "1", "0.5", " ", "+", "-", "=", "(", ")", "[", "]", "{", "}", "^", "_", "'", "&", "\\\\", "\\frac", "\\dfrac", "\\binom", "\\sqrt", "\\left(", "\\right)", "\\left.", "\\right\\}", "\\big(", "\\Bigr]", "\\text{", "\\mathbb", "\\alpha", "\\sum", "\\int", "\\lim", "\\limits", "\\begin{matrix}", "\\end{matrix}", "\\begin{cases}", "\\end{cases}", "\\begin{aligned}", "\\end{aligned}", "\\boxed", "\\hat", "\\vec", "\\overline", "\\unknown", "\\", "$", "%", "é", "∑", "\\operatorname", "*", "\\not", "=", "\\,", "\\quad", "\\phantom", "\\substack", "\\overset", "\\color", "\\textcolor", "\\middle|",
    ];
    let mut seed: u64 = 0x5eed_1a7e;
    let mut next = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 33) as usize
    };
    for _ in 0..600 {
        let len = 1 + next() % 40;
        let src: String = (0..len).map(|_| PIECES[next() % PIECES.len()]).collect();
        for style in [Style::Display, Style::Text] {
            let laid = match parse(&src) {
                Ok(tree) => Setter::new(&mut Fixed, 14.5).formula(&tree, style),
                Err(Unparsed) => Setter::new(&mut Fixed, 14.5).source(&src),
            };
            assert!(laid.width.is_finite() && laid.ascent.is_finite() && laid.descent.is_finite(), "{src}: {laid:?}");
            assert!(laid.width >= 0. && laid.height() >= 0., "{src}: {laid:?}");
        }
    }
}

// ---------- layout ----------

#[test]
fn inline_maths_sits_on_the_baseline_at_the_text_size() {
    let laid = lay("x", Style::Text, 20.);
    assert_eq!(texts(&laid), vec![("x".to_string(), 0., 0., 20.)]);
    assert!(near(laid.width, 10.) && near(laid.ascent, X_HEIGHT * 20.) && near(laid.descent, 0.));
    // A relation gets TeX's thick spaces around it; a binary operator the medium ones.
    let laid = lay("a=b", Style::Text, 18.);
    let (eq, _, _) = text_at(&laid, "=");
    assert!(near(eq, 9. + 18. * 5. / 18.), "{eq}");
    let laid = lay("-a", Style::Text, 18.);
    assert!(near(text_at(&laid, "a").0, 9.), "a leading minus is a sign, unspaced");
}

#[test]
fn everything_scales_with_the_text_size() {
    for src in ["\\frac{a}{b}", "x_i^2", "\\sqrt[3]{x+1}", "\\left( \\frac{1}{2} \\right)", "\\begin{pmatrix} 1 & 2 \\\\ 3 & 4 \\end{pmatrix}", "\\sum_{i=1}^{n} i"] {
        let small = lay(src, Style::Display, 14.5);
        let big = lay(src, Style::Display, 29.);
        assert!(near(small.width * 2., big.width) && near(small.height() * 2., big.height()), "{src}: {small:?} vs {big:?}");
    }
}

#[test]
fn a_fraction_bar_sits_on_the_axis_between_its_parts() {
    let size = 20.;
    let laid = lay("\\frac{a}{b}", Style::Display, size);
    let bar = laid.paint.iter().find_map(|p| match p {
        Paint::Rule { y, h, w, .. } => Some((*y, *h, *w)),
        _ => None,
    });
    let (y, h, w) = bar.expect("a bar");
    assert!(near(y + h / 2., -AXIS * size), "{y} {h}");
    assert!(h <= 2., "a hairline: {h}");
    let (_, num_y, num_size) = text_at(&laid, "a");
    let (_, den_y, _) = text_at(&laid, "b");
    assert!(num_y < y && den_y > y + h, "a over the bar, b under it");
    assert!(near(num_size, size), "display fractions set their parts at text size");
    assert!(w >= 10., "the bar spans its parts");
    // Inline, the parts are script size.
    let (_, _, inline_size) = text_at(&lay("\\frac{a}{b}", Style::Text, size), "a");
    assert!(near(inline_size, 0.7 * size));
}

#[test]
fn scripts_are_smaller_and_stack_only_on_large_operators_in_display() {
    let size = 20.;
    let laid = lay("x^2", Style::Text, size);
    let (x, y, s) = text_at(&laid, "2");
    assert!(near(s, 14.) && y < -5. && near(x, 10. + 0.06 * size), "{x} {y} {s}");
    let laid = lay("x_i^2", Style::Text, size);
    let ((sx, sy, _), (bx, by, _)) = (text_at(&laid, "2"), text_at(&laid, "i"));
    assert!(near(sx, bx + 0.06 * size) && sy < 0. && by > 0., "beside, one above and one below");

    let laid = lay("\\sum_{i=1}^{n}", Style::Display, size);
    let (ox, _, osize) = text_at(&laid, "∑");
    let op_centre = ox + 0.5 * osize / 2.;
    let (nx, ny, _) = text_at(&laid, "n");
    assert!(near(nx + 0.5 * 14. / 2., op_centre), "n centred over ∑");
    assert!(ny < -osize * 0.5, "above it");
    assert!(near(osize, 1.5 * size), "a display sum is larger");
    let laid = lay("\\sum_{i=1}^{n}", Style::Text, size);
    let (nx, _, _) = text_at(&laid, "n");
    assert!(nx >= text_at(&laid, "∑").0 + 0.5 * 1.1 * size - 0.5, "inline, beside it");
    // Integrals and limits stack in display too.
    let laid = lay("\\int_0^1", Style::Display, size);
    assert!(near(text_at(&laid, "1").0 + 0.5 * 14. / 2., text_at(&laid, "∫").0 + 0.5 * 1.5 * size / 2.));
    let laid = lay("\\lim_{x \\to 0}", Style::Display, size);
    assert!(text_at(&laid, "x").1 > 0., "under lim");
}

#[test]
fn a_radical_is_drawn_over_its_body() {
    let size = 20.;
    let laid = lay("\\sqrt{x}", Style::Text, size);
    let Some(Paint::Stroke { segments, width }) = laid.paint.iter().find(|p| matches!(p, Paint::Stroke { .. })) else { panic!("{laid:?}") };
    assert!(*width <= 2.);
    let (bx, _, _) = text_at(&laid, "x");
    let Some(Segment::Line(end_x, top)) = segments.last() else { panic!() };
    assert!(*end_x >= bx + 10., "the overline covers the body");
    assert!(*top < -X_HEIGHT * size, "above it");
    let indexed = lay("\\sqrt[3]{x}", Style::Text, size);
    let (ix, iy, isize) = text_at(&indexed, "3");
    assert!(ix >= 0. && iy < 0. && near(isize, 0.5 * size), "the index in the crook, small");
}

#[test]
fn tall_delimiters_are_drawn_and_short_ones_are_glyphs() {
    let size = 20.;
    let short = lay("\\left( x \\right)", Style::Text, size);
    assert!(texts(&short).iter().any(|(t, ..)| t == "("));
    assert!(!short.paint.iter().any(|p| matches!(p, Paint::Stroke { .. })));
    let tall = lay("\\left( \\frac{a}{b} \\right)", Style::Display, size);
    let strokes: Vec<_> = tall.paint.iter().filter(|p| matches!(p, Paint::Stroke { .. })).collect();
    assert_eq!(strokes.len(), 2, "both parens drawn");
    let frac = lay("\\frac{a}{b}", Style::Display, size);
    assert!(tall.height() >= frac.height() - 1., "they reach the fraction's height");
    let big = lay("\\Big(", Style::Text, size);
    assert!(near(big.height(), 1.8 * size));
}

#[test]
fn a_matrix_sets_its_cells_in_rows_and_columns() {
    let laid = lay("\\begin{pmatrix} 1 & 2 \\\\ 3 & 4 \\end{pmatrix}", Style::Display, 20.);
    let (x1, y1, _) = text_at(&laid, "1");
    let (x2, y2, _) = text_at(&laid, "2");
    let (x3, y3, _) = text_at(&laid, "3");
    assert!(near(y1, y2) && y3 > y1 && near(x1, x3) && x2 > x1);
    // Centred on the axis.
    let bare = lay("\\begin{matrix} 1 & 2 \\\\ 3 & 4 \\end{matrix}", Style::Display, 20.);
    assert!(near((bare.descent - bare.ascent) / 2., -AXIS * 20.), "{bare:?}");
    let aligned = lay("\\begin{aligned} a &= b \\\\ cc &= d \\end{aligned}", Style::Display, 20.);
    let ((ax, ..), (cx, ..)) = (text_at(&aligned, "a"), text_at(&aligned, "c"));
    assert!(ax > cx, "the left column is right-aligned");
    let eq: Vec<_> = texts(&aligned).into_iter().filter(|(t, ..)| t == "=").map(|(_, x, ..)| x).collect();
    assert!(near(eq[0], eq[1]), "the equals signs line up");
}

#[test]
fn an_unknown_command_is_mono_on_a_tint() {
    let laid = lay("\\foo x", Style::Text, 20.);
    assert!(laid.paint.iter().any(|p| matches!(p, Paint::Tint { .. })));
    assert!(laid.paint.iter().any(|p| matches!(p, Paint::Text { text, face: Face::Mono, faint: true, .. } if text == "\\foo")));
    assert!(texts(&laid).iter().any(|(t, ..)| t == "x"), "the rest still draws");
}

#[test]
fn an_unreadable_formula_shows_its_source() {
    let laid = Setter::new(&mut Fixed, 20.).source("\\frac{a\nb");
    let lines: Vec<_> = laid.paint.iter().filter_map(|p| match p {
        Paint::Text { text, face: Face::Mono, .. } => Some(text.as_str()),
        _ => None,
    }).collect();
    assert_eq!(lines, vec!["\\frac{a", "b"]);
}

#[test]
fn the_bayes_answer_sets() {
    let src = "P(\\theta=1\\mid T=+) =\\frac{0.99(0.001)}{0.99(0.001)+0.06(0.999)} \\approx \\boxed{0.0162482}";
    let tree = parse(src).unwrap();
    assert!(!contains_unknown(&tree));
    let laid = Setter::new(&mut Fixed, 14.5).formula(&tree, Style::Display);
    assert!(laid.paint.iter().any(|p| matches!(p, Paint::Rule { .. })), "a fraction bar and a box");
    assert!(texts(&laid).iter().any(|(t, ..)| t == "θ"));
    // `+)`: a plus with nothing after it is a sign, not an operator.
    let items = atoms("T=+)");
    assert_eq!(classes(&items, None)[2], Some(Class::Ord));
}
