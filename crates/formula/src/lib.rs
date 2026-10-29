//! Formulas in model answers (F-62, Q-65 revised in B53). The model writes
//! LaTeX between `\( … \)` (inline) or `\[ … \]` (display); dollars are not
//! formulas (Excel's `$B$3`). A strict parser accepts a known subset of LaTeX
//! math and gives two outputs from one tree: Typst math for rendering and
//! Unicode for plain text. Anything outside the subset is an error, and the
//! caller shows text instead: a formula is never guessed, and raw `\frac`
//! never reaches the reader.

mod table;

use serde::Serialize;

pub use table::{FUNCTIONS, symbol};

/// A piece of an answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Segment {
    Text(String),
    Math { tex: String, display: bool },
}

/// Splits text at `\( … \)` and `\[ … \]`. An opening without its closing is
/// not a formula: the rest is text with its LaTeX made readable.
pub fn segments(text: &str) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut rest = text;
    loop {
        let next = [("\\(", "\\)", false), ("\\[", "\\]", true)]
            .into_iter()
            .filter_map(|(open, close, display)| rest.find(open).map(|i| (i, open, close, display)))
            .min_by_key(|(i, ..)| *i);
        let Some((start, open, close, display)) = next else {
            if !rest.is_empty() {
                out.push(Segment::Text(rest.to_string()));
            }
            return out;
        };
        if start > 0 {
            out.push(Segment::Text(rest[..start].to_string()));
        }
        let body = &rest[start + open.len()..];
        match body.find(close) {
            Some(end) => {
                let tex = body[..end].trim();
                if !tex.is_empty() {
                    out.push(Segment::Math {
                        tex: tex.to_string(),
                        display,
                    });
                }
                rest = &body[end + close.len()..];
            }
            None => {
                out.push(Segment::Text(loose(body)));
                return out;
            }
        }
    }
}

/// The whole text with every formula as Unicode: for the journal, the CLI and
/// anything that cannot draw.
pub fn plain(text: &str) -> String {
    segments(text)
        .into_iter()
        .map(|segment| match segment {
            Segment::Text(text) => text,
            Segment::Math { tex, .. } => unicode_of(&tex),
        })
        .collect()
}

/// Whether the text has any formula at all.
pub fn has_math(text: &str) -> bool {
    segments(text)
        .iter()
        .any(|s| matches!(s, Segment::Math { .. }))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormulaError {
    Unknown(String),
    Unbalanced,
    Missing(&'static str),
}

impl std::fmt::Display for FormulaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FormulaError::Unknown(name) => write!(f, "unknown command \\{name}"),
            FormulaError::Unbalanced => write!(f, "unbalanced braces or delimiters"),
            FormulaError::Missing(what) => write!(f, "missing {what}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    /// One letter, digit or punctuation sign.
    Char(char),
    /// A run of digits with a decimal point or comma.
    Number(String),
    /// A named symbol: (Typst, Unicode).
    Symbol(&'static str, &'static str),
    /// `\sin`, `\lim`… as upright text.
    Function(&'static str),
    /// `\operatorname{rank}`.
    Operator(String),
    /// `\text{…}`.
    Text(String),
    Group(Vec<Node>),
    Frac(Vec<Node>, Vec<Node>),
    Sqrt(Option<Vec<Node>>, Vec<Node>),
    Sup(Box<Node>, Vec<Node>),
    Sub(Box<Node>, Vec<Node>),
    Font(Font, Vec<Node>),
    Accent(Accent, Vec<Node>),
    /// `\left( … \right)`: delimiters as (Typst, Unicode); empty for `.`.
    Delimited(
        (&'static str, &'static str),
        Vec<Node>,
        (&'static str, &'static str),
    ),
    Matrix(MatrixKind, Vec<Vec<Vec<Node>>>),
    Space(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Font {
    Blackboard,
    Calligraphic,
    Bold,
    Upright,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accent {
    Bar,
    Hat,
    Vec,
    Tilde,
    Dot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatrixKind {
    Plain,
    Paren,
    Bracket,
    Bar,
    Cases,
}

/// Marks a decimal comma inside a number until output.
const DECIMAL_COMMA: char = '\u{E000}';

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Command(String),
    Open,
    Close,
    Sup,
    Sub,
    Amp,
    Char(char),
}

fn tokenize(tex: &str) -> Vec<Token> {
    let chars: Vec<char> = tex.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' => {
                let start = i + 1;
                let mut end = start;
                while end < chars.len() && chars[end].is_ascii_alphabetic() {
                    end += 1;
                }
                if end == start && start < chars.len() {
                    end = start + 1;
                }
                tokens.push(Token::Command(chars[start..end].iter().collect()));
                i = end;
                continue;
            }
            '{' => tokens.push(Token::Open),
            '}' => tokens.push(Token::Close),
            '^' => tokens.push(Token::Sup),
            '_' => tokens.push(Token::Sub),
            '&' => tokens.push(Token::Amp),
            c if c.is_whitespace() => {}
            // «1,5»: a comma between digits is a decimal comma, not a list.
            ',' if i > 0
                && chars[i - 1].is_ascii_digit()
                && chars.get(i + 1).is_some_and(char::is_ascii_digit) =>
            {
                tokens.push(Token::Char(DECIMAL_COMMA))
            }
            c => tokens.push(Token::Char(c)),
        }
        i += 1;
    }
    tokens
}

struct Parser {
    tokens: Vec<Token>,
    at: usize,
}

/// Where a sequence ends.
#[derive(PartialEq)]
enum Until {
    End,
    Close,
    Right,
    EndEnv,
}

pub fn parse(tex: &str) -> Result<Vec<Node>, FormulaError> {
    let mut parser = Parser {
        tokens: tokenize(tex),
        at: 0,
    };
    let nodes = parser.sequence(Until::End)?;
    if parser.at < parser.tokens.len() {
        return Err(FormulaError::Unbalanced);
    }
    Ok(nodes)
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.at).cloned();
        self.at += 1;
        token
    }

    fn sequence(&mut self, until: Until) -> Result<Vec<Node>, FormulaError> {
        let mut nodes: Vec<Node> = Vec::new();
        loop {
            match self.peek() {
                None => {
                    return if until == Until::End {
                        Ok(nodes)
                    } else {
                        Err(FormulaError::Unbalanced)
                    };
                }
                Some(Token::Close) => {
                    return if until == Until::Close {
                        self.at += 1;
                        Ok(nodes)
                    } else {
                        Err(FormulaError::Unbalanced)
                    };
                }
                Some(Token::Command(name)) if name == "right" => {
                    return if until == Until::Right {
                        Ok(nodes)
                    } else {
                        Err(FormulaError::Unbalanced)
                    };
                }
                Some(Token::Command(name)) if name == "end" || name == "\\" => {
                    return if until == Until::EndEnv {
                        Ok(nodes)
                    } else {
                        Err(FormulaError::Unbalanced)
                    };
                }
                Some(Token::Amp) => {
                    return if until == Until::EndEnv {
                        Ok(nodes)
                    } else {
                        Err(FormulaError::Unknown("&".into()))
                    };
                }
                Some(Token::Sup) | Some(Token::Sub) => {
                    let sup = self.next() == Some(Token::Sup);
                    let base = nodes.pop().unwrap_or(Node::Group(Vec::new()));
                    let script = self.argument()?;
                    nodes.push(if sup {
                        Node::Sup(Box::new(base), script)
                    } else {
                        Node::Sub(Box::new(base), script)
                    });
                }
                Some(_) => {
                    let mut atom = self.atom()?;
                    // `1{,}5`: the LaTeX way to write a decimal comma.
                    if atom == Node::Group(vec![Node::Char(',')])
                        && matches!(nodes.last(), Some(Node::Number(_)))
                    {
                        atom = Node::Char(DECIMAL_COMMA);
                    }
                    // Digits with a decimal separator make one number.
                    if let (Node::Char(c), Some(Node::Number(n))) = (&atom, nodes.last_mut())
                        && (c.is_ascii_digit()
                            || ((*c == '.' || *c == DECIMAL_COMMA)
                                && !n.ends_with(['.', DECIMAL_COMMA])))
                    {
                        n.push(*c);
                        continue;
                    }
                    if let Node::Char(c) = atom
                        && c.is_ascii_digit()
                    {
                        nodes.push(Node::Number(c.to_string()));
                        continue;
                    }
                    nodes.push(atom);
                }
            }
        }
    }

    /// The argument of `^`, `_` or a command: a group or one atom.
    fn argument(&mut self) -> Result<Vec<Node>, FormulaError> {
        match self.peek() {
            None => Err(FormulaError::Missing("argument")),
            Some(Token::Open) => {
                self.at += 1;
                self.sequence(Until::Close)
            }
            Some(Token::Close) | Some(Token::Sup) | Some(Token::Sub) | Some(Token::Amp) => {
                Err(FormulaError::Missing("argument"))
            }
            Some(_) => Ok(vec![self.atom()?]),
        }
    }

    /// Raw text inside braces: `\text{…}`, `\operatorname{…}`, `\begin{…}`.
    fn raw_group(&mut self) -> Result<String, FormulaError> {
        if self.next() != Some(Token::Open) {
            return Err(FormulaError::Missing("{"));
        }
        let mut text = String::new();
        let mut depth = 0;
        loop {
            match self.next() {
                None => return Err(FormulaError::Unbalanced),
                Some(Token::Close) if depth == 0 => return Ok(text),
                Some(Token::Close) => {
                    depth -= 1;
                    text.push('}');
                }
                Some(Token::Open) => {
                    depth += 1;
                    text.push('{');
                }
                Some(Token::Char(c)) => text.push(c),
                Some(Token::Command(name)) if name == " " || name == "," => text.push(' '),
                Some(Token::Command(name)) if name.len() == 1 => text.push_str(&name),
                Some(Token::Command(name)) => return Err(FormulaError::Unknown(name)),
                Some(Token::Sup) => text.push('^'),
                Some(Token::Sub) => text.push('_'),
                Some(Token::Amp) => text.push('&'),
            }
        }
    }

    fn atom(&mut self) -> Result<Node, FormulaError> {
        match self.next() {
            None => Err(FormulaError::Missing("atom")),
            Some(Token::Open) => Ok(Node::Group(self.sequence(Until::Close)?)),
            Some(Token::Char(c)) => Ok(Node::Char(c)),
            Some(Token::Command(name)) => self.command(&name),
            Some(_) => Err(FormulaError::Unbalanced),
        }
    }

    fn command(&mut self, name: &str) -> Result<Node, FormulaError> {
        if let Some((typst, unicode)) = symbol(name) {
            return Ok(Node::Symbol(typst, unicode));
        }
        if let Some(function) = FUNCTIONS.iter().find(|f| **f == name) {
            return Ok(Node::Function(function));
        }
        Ok(match name {
            "frac" | "dfrac" | "tfrac" => Node::Frac(self.argument()?, self.argument()?),
            "sqrt" => {
                let index = if self.peek() == Some(&Token::Char('[')) {
                    self.at += 1;
                    let mut index = Vec::new();
                    loop {
                        match self.peek() {
                            None => return Err(FormulaError::Unbalanced),
                            Some(Token::Char(']')) => {
                                self.at += 1;
                                break;
                            }
                            Some(_) => index.push(self.atom()?),
                        }
                    }
                    Some(index)
                } else {
                    None
                };
                Node::Sqrt(index, self.argument()?)
            }
            "mathbb" => Node::Font(Font::Blackboard, self.argument()?),
            "mathcal" => Node::Font(Font::Calligraphic, self.argument()?),
            "mathbf" | "boldsymbol" | "bm" => Node::Font(Font::Bold, self.argument()?),
            "mathrm" => Node::Font(Font::Upright, self.argument()?),
            "text" | "textrm" | "mathrm{}" | "mbox" | "textit" => Node::Text(self.raw_group()?),
            "operatorname" => Node::Operator(self.raw_group()?),
            "bar" | "overline" => Node::Accent(Accent::Bar, self.argument()?),
            "hat" | "widehat" => Node::Accent(Accent::Hat, self.argument()?),
            "vec" | "overrightarrow" => Node::Accent(Accent::Vec, self.argument()?),
            "tilde" | "widetilde" => Node::Accent(Accent::Tilde, self.argument()?),
            "dot" => Node::Accent(Accent::Dot, self.argument()?),
            "left" => {
                let open = self.delimiter()?;
                let body = self.sequence(Until::Right)?;
                self.at += 1; // \right
                let close = self.delimiter()?;
                Node::Delimited(open, body, close)
            }
            "begin" => {
                let env = self.raw_group()?;
                let kind = match env.as_str() {
                    "matrix" => MatrixKind::Plain,
                    "pmatrix" => MatrixKind::Paren,
                    "bmatrix" => MatrixKind::Bracket,
                    "vmatrix" => MatrixKind::Bar,
                    "cases" => MatrixKind::Cases,
                    other => return Err(FormulaError::Unknown(format!("begin{{{other}}}"))),
                };
                let mut rows = Vec::new();
                let mut row = Vec::new();
                loop {
                    row.push(self.sequence(Until::EndEnv)?);
                    match self.next() {
                        Some(Token::Amp) => {}
                        Some(Token::Command(c)) if c == "\\" => rows.push(std::mem::take(&mut row)),
                        Some(Token::Command(c)) if c == "end" => {
                            if self.raw_group()? != env {
                                return Err(FormulaError::Unbalanced);
                            }
                            rows.push(row);
                            break;
                        }
                        _ => return Err(FormulaError::Unbalanced),
                    }
                }
                Node::Matrix(kind, rows)
            }
            "," | ":" | ";" | "quad" | "qquad" | " " => Node::Space(match name {
                "quad" => "quad",
                "qquad" => "wide",
                ";" => "thick",
                ":" => "med",
                _ => "thin",
            }),
            "!" => Node::Group(Vec::new()),
            "{" => Node::Char('{'),
            "}" => Node::Char('}'),
            "%" | "#" | "$" | "_" | "&" => Node::Char(name.chars().next().unwrap_or(' ')),
            "|" => Node::Symbol("bar.v.double", "‖"),
            other => return Err(FormulaError::Unknown(other.to_string())),
        })
    }

    fn delimiter(&mut self) -> Result<(&'static str, &'static str), FormulaError> {
        Ok(match self.next() {
            Some(Token::Char('(')) => ("(", "("),
            Some(Token::Char(')')) => (")", ")"),
            Some(Token::Char('[')) => ("[", "["),
            Some(Token::Char(']')) => ("]", "]"),
            Some(Token::Char('|')) => ("|", "|"),
            Some(Token::Char('.')) => ("", ""),
            Some(Token::Command(c)) => match c.as_str() {
                "{" | "lbrace" => ("{", "{"),
                "}" | "rbrace" => ("}", "}"),
                "|" | "Vert" => ("||", "‖"),
                "langle" => ("angle.l", "⟨"),
                "rangle" => ("angle.r", "⟩"),
                "lvert" | "rvert" | "vert" => ("|", "|"),
                other => return Err(FormulaError::Unknown(other.to_string())),
            },
            _ => return Err(FormulaError::Missing("delimiter")),
        })
    }
}

// ---------- Typst ----------

/// Typst math source for the nodes (without the surrounding `$`).
pub fn to_typst(nodes: &[Node]) -> String {
    nodes.iter().map(typst_node).collect::<Vec<_>>().join(" ")
}

fn typst_char(c: char) -> String {
    match c {
        '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';' | '/' | '"' | '#' | '$' | '_' | '^' | '&'
        | '\\' | '@' | '`' | '~' | '*' | '<' | '>' | '=' | '-' | '+' | '!' | ':' | '|' | '\'' => {
            match c {
                // Plain operators and relations keep their math spacing.
                '<' | '>' | '=' | '-' | '+' | '!' | ':' | '|' | '\'' => c.to_string(),
                _ => format!("\\{c}"),
            }
        }
        // An ordinary sign: Typst puts no space after it, unlike punctuation.
        DECIMAL_COMMA => "class(\"normal\", \\,)".into(),
        c => c.to_string(),
    }
}

fn typst_group(nodes: &[Node]) -> String {
    format!("({})", to_typst(nodes))
}

fn typst_node(node: &Node) -> String {
    match node {
        Node::Char(c) => typst_char(*c),
        Node::Number(n) => n.replace(DECIMAL_COMMA, " class(\"normal\", \\,) "),
        Node::Symbol(typst, _) => typst.to_string(),
        Node::Function(name) => name.to_string(),
        Node::Operator(name) => format!("op(\"{}\")", name.replace('"', "\\\"")),
        Node::Text(text) => format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\"")),
        Node::Group(nodes) => to_typst(nodes),
        Node::Frac(a, b) => format!("frac({}, {})", to_typst(a), to_typst(b)),
        Node::Sqrt(None, body) => format!("sqrt({})", to_typst(body)),
        Node::Sqrt(Some(index), body) => format!("root({}, {})", to_typst(index), to_typst(body)),
        Node::Sup(base, script) => format!("{}^{}", typst_base(base), typst_group(script)),
        Node::Sub(base, script) => format!("{}_{}", typst_base(base), typst_group(script)),
        Node::Font(font, body) => {
            let f = match font {
                Font::Blackboard => "bb",
                Font::Calligraphic => "cal",
                Font::Bold => "bold",
                Font::Upright => "upright",
            };
            format!("{f}({})", to_typst(body))
        }
        Node::Accent(accent, body) => {
            let a = match accent {
                Accent::Bar => "overline",
                Accent::Hat => "hat",
                Accent::Vec => "arrow",
                Accent::Tilde => "tilde",
                Accent::Dot => "dot",
            };
            format!("{a}({})", to_typst(body))
        }
        Node::Delimited((open, _), body, (close, _)) => {
            let side = |d: &str| match d {
                "" => String::new(),
                "{" | "}" | "[" | "]" | "(" | ")" => d.to_string(),
                other => other.to_string(),
            };
            format!("lr({} {} {})", side(open), to_typst(body), side(close))
        }
        Node::Matrix(kind, rows) => {
            let body = rows
                .iter()
                .map(|row| {
                    row.iter()
                        .map(|cell| to_typst(cell))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .collect::<Vec<_>>();
            match kind {
                MatrixKind::Cases => format!(
                    "cases({})",
                    rows.iter()
                        .map(|row| row
                            .iter()
                            .map(|cell| to_typst(cell))
                            .collect::<Vec<_>>()
                            .join(" & "))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                _ => {
                    let delim = match kind {
                        MatrixKind::Paren => "\"(\"",
                        MatrixKind::Bracket => "\"[\"",
                        MatrixKind::Bar => "\"|\"",
                        _ => "#none",
                    };
                    format!("mat(delim: {delim}, {})", body.join("; "))
                }
            }
        }
        Node::Space(kind) => kind.to_string(),
    }
}

fn typst_base(base: &Node) -> String {
    match base {
        Node::Group(nodes) if nodes.is_empty() => "zws".into(),
        // `{a+b}^2`: Typst has no invisible grouping in math; the visible
        // parentheses keep the meaning instead of attaching to `b` alone.
        Node::Group(nodes) if nodes.len() > 1 => typst_group(nodes),
        other => typst_node(other),
    }
}

// ---------- Unicode ----------

/// Signs that get a space on each side in plain text: `a = b`, `x → 0`.
const SPACED: &str = "=<>+−±∓×÷·≤≥≠≈≡∼≃≅∝≪≫∈∉∋⊂⊆⊃⊇∪∩∖∣∤→←↔⇒⇐⇔⟺⟹⟶↦∧∨";

/// Unicode text for the nodes: what the reader sees without rendering.
pub fn to_unicode(nodes: &[Node]) -> String {
    let mut out = String::new();
    for node in nodes {
        let piece = unicode_node(node);
        let binary = piece.chars().count() == 1 && piece.chars().all(|c| SPACED.contains(c));
        // A minus or plus that starts the formula or follows «(» or another
        // sign is a sign of the number, not an operation.
        let unary = matches!(piece.as_str(), "−" | "+")
            && (out.trim_end().is_empty()
                || out.trim_end().ends_with(['(', '['])
                || out.ends_with(' '));
        if binary && !unary {
            out = out.trim_end().to_string();
            out.push(' ');
            out.push_str(&piece);
            out.push(' ');
            continue;
        }
        if matches!(node, Node::Function(_) | Node::Operator(_)) {
            if out.chars().last().is_some_and(char::is_alphanumeric) {
                out.push(' ');
            }
            out.push_str(&piece);
            out.push(' ');
            continue;
        }
        out.push_str(&piece);
        // «lim_(x→0) f»: an operator with limits still stands apart.
        let operator_with_limits = match node {
            Node::Sub(base, _) | Node::Sup(base, _) => matches!(**base, Node::Function(_)),
            _ => false,
        };
        if operator_with_limits || is_big(node) {
            out.push(' ');
        }
    }
    out.split(' ')
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// ∑, ∏, ∫ with limits: the summand follows after a space.
fn is_big(node: &Node) -> bool {
    let base = match node {
        Node::Sub(base, _) | Node::Sup(base, _) => base,
        _ => return false,
    };
    match &**base {
        Node::Symbol(_, u) => ["∑", "∏", "∫", "∬", "∭", "∮", "⋃", "⋂"].contains(u),
        Node::Sub(..) | Node::Sup(..) => is_big(base),
        _ => false,
    }
}

fn simple(nodes: &[Node]) -> bool {
    match nodes {
        [Node::Char(_)] | [Node::Number(_)] | [Node::Symbol(..)] => true,
        [Node::Group(inner)] => simple(inner),
        _ => false,
    }
}

fn wrapped(nodes: &[Node]) -> String {
    let text = to_unicode(nodes);
    if simple(nodes) {
        text
    } else {
        format!("({text})")
    }
}

const SUPERSCRIPT: [(char, char); 22] = [
    ('0', '⁰'),
    ('1', '¹'),
    ('2', '²'),
    ('3', '³'),
    ('4', '⁴'),
    ('5', '⁵'),
    ('6', '⁶'),
    ('7', '⁷'),
    ('8', '⁸'),
    ('9', '⁹'),
    ('+', '⁺'),
    ('-', '⁻'),
    ('−', '⁻'),
    ('=', '⁼'),
    ('(', '⁽'),
    (')', '⁾'),
    ('n', 'ⁿ'),
    ('i', 'ⁱ'),
    ('k', 'ᵏ'),
    ('x', 'ˣ'),
    ('T', 'ᵀ'),
    ('′', '′'),
];

const SUBSCRIPT: [(char, char); 22] = [
    ('0', '₀'),
    ('1', '₁'),
    ('2', '₂'),
    ('3', '₃'),
    ('4', '₄'),
    ('5', '₅'),
    ('6', '₆'),
    ('7', '₇'),
    ('8', '₈'),
    ('9', '₉'),
    ('+', '₊'),
    ('-', '₋'),
    ('−', '₋'),
    ('=', '₌'),
    ('(', '₍'),
    (')', '₎'),
    ('n', 'ₙ'),
    ('i', 'ᵢ'),
    ('k', 'ₖ'),
    ('x', 'ₓ'),
    ('j', 'ⱼ'),
    ('m', 'ₘ'),
];

fn script(text: &str, table: &[(char, char)], fallback: char) -> String {
    let text = &text.replace(' ', "");
    let mapped: Option<String> = text
        .chars()
        .map(|c| table.iter().find(|(from, _)| *from == c).map(|(_, to)| *to))
        .collect();
    match mapped {
        Some(s) if !s.is_empty() => s,
        _ if text.chars().count() == 1 => format!("{fallback}{text}"),
        _ => format!("{fallback}({text})"),
    }
}

fn unicode_node(node: &Node) -> String {
    match node {
        Node::Char('-') => "−".into(),
        Node::Char('*') => "·".into(),
        Node::Char('\'') => "′".into(),
        Node::Char(DECIMAL_COMMA) => ",".into(),
        Node::Char(c) => c.to_string(),
        Node::Number(n) => n.replace(DECIMAL_COMMA, ","),
        Node::Symbol(_, unicode) => unicode.to_string(),
        Node::Function(name) => name.to_string(),
        Node::Operator(name) => name.clone(),
        Node::Text(text) => text.clone(),
        Node::Group(nodes) => to_unicode(nodes),
        Node::Frac(a, b) => format!("{}/{}", wrapped(a), wrapped(b)),
        Node::Sqrt(None, body) => format!("√{}", wrapped(body)),
        Node::Sqrt(Some(index), body) => {
            format!(
                "{}√{}",
                script(&to_unicode(index), &SUPERSCRIPT, '^'),
                wrapped(body)
            )
        }
        Node::Sup(base, sup) => format!(
            "{}{}",
            unicode_base(base),
            script(&to_unicode(sup), &SUPERSCRIPT, '^')
        ),
        Node::Sub(base, sub) => format!(
            "{}{}",
            unicode_base(base),
            script(&to_unicode(sub), &SUBSCRIPT, '_')
        ),
        Node::Font(Font::Blackboard, body) => {
            let text = to_unicode(body);
            match text.as_str() {
                "R" => "ℝ".into(),
                "N" => "ℕ".into(),
                "Z" => "ℤ".into(),
                "Q" => "ℚ".into(),
                "C" => "ℂ".into(),
                _ => text,
            }
        }
        Node::Font(_, body) => to_unicode(body),
        Node::Accent(accent, body) => {
            let mark = match accent {
                Accent::Bar => '\u{0304}',
                Accent::Hat => '\u{0302}',
                Accent::Vec => '\u{20D7}',
                Accent::Tilde => '\u{0303}',
                Accent::Dot => '\u{0307}',
            };
            let text = to_unicode(body);
            if text.chars().count() == 1 {
                format!("{text}{mark}")
            } else {
                format!("({text}){mark}")
            }
        }
        Node::Delimited((_, open), body, (_, close)) => {
            format!("{open}{}{close}", to_unicode(body))
        }
        Node::Matrix(kind, rows) => {
            let body = rows
                .iter()
                .map(|row| {
                    row.iter()
                        .map(|cell| to_unicode(cell))
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .collect::<Vec<_>>()
                .join("; ");
            match kind {
                MatrixKind::Paren | MatrixKind::Plain => format!("({body})"),
                MatrixKind::Bracket => format!("[{body}]"),
                MatrixKind::Bar => format!("|{body}|"),
                MatrixKind::Cases => rows
                    .iter()
                    .map(|row| match row.as_slice() {
                        [value, condition] => {
                            format!("{} при {}", to_unicode(value), to_unicode(condition))
                        }
                        cells => cells
                            .iter()
                            .map(|c| to_unicode(c))
                            .collect::<Vec<_>>()
                            .join(" "),
                    })
                    .collect::<Vec<_>>()
                    .join("; "),
            }
        }
        Node::Space(_) => " ".into(),
    }
}

fn unicode_base(base: &Node) -> String {
    match base {
        Node::Group(nodes) if nodes.len() > 1 => format!("({})", to_unicode(nodes)),
        other => unicode_node(other),
    }
}

/// Unicode for a formula: through the tree when it parses, else a loose pass
/// that still never leaves a backslash or a brace.
pub fn unicode_of(tex: &str) -> String {
    match parse(tex) {
        Ok(nodes) => to_unicode(&nodes),
        Err(_) => loose(tex),
    }
}

/// Best effort for LaTeX that did not parse: known commands become their
/// signs, the rest of the markup disappears.
pub fn loose(tex: &str) -> String {
    let tex = &fractions_as_slashes(tex);
    let mut out = String::new();
    for token in tokenize_keep_spaces(tex) {
        match token {
            Loose::Command(name) => {
                if let Some((_, unicode)) = symbol(&name) {
                    out.push_str(unicode);
                } else if FUNCTIONS.contains(&name.as_str()) {
                    out.push_str(&name);
                    out.push(' ');
                } else if name == "sqrt" {
                    out.push('√');
                } else if name.len() == 1
                    && !name.chars().all(char::is_alphabetic)
                    && name != "\\"
                    && name != "!"
                {
                    out.push_str(if name == "," || name == " " {
                        " "
                    } else {
                        &name
                    });
                }
            }
            Loose::Char(c) => out.push(c),
        }
    }
    out.replace(['{', '}'], "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// `\frac{a}{b}` → `(a)/(b)` wherever both groups are balanced, so a
/// fraction never reads as a product.
fn fractions_as_slashes(tex: &str) -> String {
    let mut text = tex.to_string();
    for name in ["\\dfrac", "\\tfrac", "\\frac"] {
        while let Some(start) = text.find(name) {
            let after = start + name.len();
            let Some((a, rest)) = brace_group(&text[after..]) else {
                break;
            };
            let Some((b, rest)) = brace_group(rest) else {
                break;
            };
            let replaced = format!("{}({a})/({b}){rest}", &text[..start]);
            text = replaced;
        }
    }
    text
}

/// The content of a leading `{…}` group and what follows it.
fn brace_group(text: &str) -> Option<(&str, &str)> {
    let text = text.trim_start();
    if !text.starts_with('{') {
        return None;
    }
    let mut depth = 0;
    for (i, c) in text.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((&text[1..i], &text[i + 1..]));
                }
            }
            _ => {}
        }
    }
    None
}

enum Loose {
    Command(String),
    Char(char),
}

fn tokenize_keep_spaces(tex: &str) -> Vec<Loose> {
    let chars: Vec<char> = tex.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' {
            let start = i + 1;
            let mut end = start;
            while end < chars.len() && chars[end].is_ascii_alphabetic() {
                end += 1;
            }
            if end == start && start < chars.len() {
                end = start + 1;
            }
            out.push(Loose::Command(chars[start..end].iter().collect()));
            i = end;
        } else {
            out.push(Loose::Char(chars[i]));
            i += 1;
        }
    }
    out
}

/// Formulas as models write them in our subjects: every one parses here and
/// renders through Typst in the desktop test.
pub const CORPUS: &[&str] = &[
    r"x_{n+1} = x_n - \frac{f(x_n)}{f'(x_n)}",
    r"\sqrt{2} \approx 1.414",
    r"P(A \mid B) = \frac{P(A \cap B)}{P(B)}",
    r"\sum_{i=1}^{n} i^2 = \frac{n(n+1)(2n+1)}{6}",
    r"\operatorname{rank} A < \operatorname{rank}(A \mid b)",
    r"\lim_{x \to 0} \frac{\sin x}{x} = 1",
    r"\begin{pmatrix} 1 & 2 \\ 3 & 4 \end{pmatrix}",
    r"x \in \mathbb{R}^n",
    r"\int_0^1 x^2 \, dx = \frac{1}{3}",
    r"\left( \frac{a}{b} \right)^2 \ne \frac{a^2}{b}",
    r"f(x) = \begin{cases} x^2 & x \ge 0 \\ -x & x < 0 \end{cases}",
    r"\vec{v} \cdot \vec{w} = |v| |w| \cos \varphi",
    r"e^{i\pi} + 1 = 0",
    r"\sqrt[3]{27} = 3",
    r"0{,}5 \cdot 1,5 = 0,75",
    r"\text{если } a > 0",
    r"\{x \mid x > 0\}",
    r"\bar{x} = \frac{1}{n} \sum x_i",
    r"\Delta = b^2 - 4ac",
];

#[cfg(test)]
mod tests;
