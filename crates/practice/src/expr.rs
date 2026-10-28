//! Exact arithmetic for probe templates and the arithmetic filter (F-96).
//!
//! The language is the one the v1 contract promised the model:
//! `+ - * / ** % == != < <= > >= and or not gcd divides abs`, integers and
//! names; `//` (floor division) is accepted too, since models write it. Numbers are exact fractions; nothing is rounded, so «⅓·6 = 2» is
//! checked as 2, not as 1.9999. Human notation is accepted too: `−`, `·`, `×`,
//! `÷`, `^`, `²`, `³`, and decimals with a comma or a point.

use std::collections::HashMap;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ratio {
    num: i128,
    den: i128,
}

impl Ratio {
    pub fn int(value: i128) -> Self {
        Self { num: value, den: 1 }
    }

    pub fn new(num: i128, den: i128) -> Result<Self, String> {
        if den == 0 {
            return Err("деление на ноль".into());
        }
        let g = gcd(num, den).max(1);
        let sign = if den < 0 { -1 } else { 1 };
        Ok(Self {
            num: sign * num / g,
            den: sign * den / g,
        })
    }

    pub fn is_integer(&self) -> bool {
        self.den == 1
    }

    pub fn as_integer(&self) -> Option<i128> {
        self.is_integer().then_some(self.num)
    }

    fn checked(num: Option<i128>, den: Option<i128>) -> Result<Self, String> {
        match (num, den) {
            (Some(n), Some(d)) => Self::new(n, d),
            _ => Err("слишком большое число".into()),
        }
    }

    fn add(self, o: Self) -> Result<Self, String> {
        let num = self
            .num
            .checked_mul(o.den)
            .zip(o.num.checked_mul(self.den))
            .and_then(|(a, b)| a.checked_add(b));
        Self::checked(num, self.den.checked_mul(o.den))
    }

    fn neg(self) -> Self {
        Self {
            num: -self.num,
            den: self.den,
        }
    }

    fn mul(self, o: Self) -> Result<Self, String> {
        Self::checked(self.num.checked_mul(o.num), self.den.checked_mul(o.den))
    }

    fn div(self, o: Self) -> Result<Self, String> {
        if o.num == 0 {
            return Err("деление на ноль".into());
        }
        Self::checked(self.num.checked_mul(o.den), self.den.checked_mul(o.num))
    }

    fn pow(self, exponent: Self) -> Result<Self, String> {
        let e = exponent
            .as_integer()
            .ok_or("дробная степень не поддерживается")?;
        if e.abs() > 64 {
            return Err("слишком большая степень".into());
        }
        let mut result = Self::int(1);
        for _ in 0..e.abs() {
            result = result.mul(self)?;
        }
        if e < 0 {
            Self::int(1).div(result)
        } else {
            Ok(result)
        }
    }

    fn cmp_value(self, o: Self) -> Result<std::cmp::Ordering, String> {
        let left = self.num.checked_mul(o.den);
        let right = o.num.checked_mul(self.den);
        match (left, right) {
            (Some(l), Some(r)) => Ok(l.cmp(&r)),
            _ => Err("слишком большое число".into()),
        }
    }
}

impl fmt::Display for Ratio {
    /// Integers as integers, the rest as a/b; minus as the typographic «−».
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.num < 0 { "−" } else { "" };
        if self.den == 1 {
            write!(f, "{sign}{}", self.num.abs())
        } else {
            write!(f, "{sign}{}/{}", self.num.abs(), self.den)
        }
    }
}

fn gcd(a: i128, b: i128) -> i128 {
    let (mut a, mut b) = (a.abs(), b.abs());
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value {
    Num(Ratio),
    Bool(bool),
}

impl Value {
    pub fn num(self) -> Result<Ratio, String> {
        match self {
            Value::Num(r) => Ok(r),
            Value::Bool(_) => Err("ожидалось число".into()),
        }
    }

    pub fn truth(self) -> Result<bool, String> {
        match self {
            Value::Bool(b) => Ok(b),
            Value::Num(_) => Err("ожидалось условие".into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Num(Ratio),
    Name(String),
    Op(&'static str),
    Open,
    Close,
    Comma,
}

/// Replaces human notation with the formal one.
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '−' | '–' => out.push('-'),
            '·' | '×' | '⋅' | '∙' => out.push('*'),
            '÷' | ':' => out.push('/'),
            '^' => out.push_str("**"),
            '²' => out.push_str("**2"),
            '³' => out.push_str("**3"),
            _ => out.push(c),
        }
    }
    out
}

fn tokenize(text: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = normalize(text).chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            let whole: String = chars[start..i].iter().collect();
            let mut value = Ratio::int(whole.parse().map_err(|_| "слишком большое число")?);
            let decimal = i + 1 < chars.len()
                && (chars[i] == '.' || chars[i] == ',')
                && chars[i + 1].is_ascii_digit()
                && !(chars[i] == ',' && tokens.contains(&Token::Open));
            if decimal {
                i += 1;
                let start = i;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
                let digits: String = chars[start..i].iter().collect();
                let scale = 10i128
                    .checked_pow(digits.len() as u32)
                    .ok_or("слишком длинная дробь")?;
                let frac: i128 = digits.parse().map_err(|_| "слишком длинная дробь")?;
                value = value.add(Ratio::new(frac, scale)?)?;
            }
            tokens.push(Token::Num(value));
        } else if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            tokens.push(Token::Name(chars[start..i].iter().collect()));
        } else {
            let two: String = chars[i..(i + 2).min(chars.len())].iter().collect();
            let op = match two.as_str() {
                "**" => Some("**"),
                "//" => Some("//"),
                "==" => Some("=="),
                "!=" => Some("!="),
                "<=" => Some("<="),
                ">=" => Some(">="),
                _ => None,
            };
            if let Some(op) = op {
                tokens.push(Token::Op(op));
                i += 2;
                continue;
            }
            tokens.push(match c {
                '+' => Token::Op("+"),
                '-' => Token::Op("-"),
                '*' => Token::Op("*"),
                '/' => Token::Op("/"),
                '%' => Token::Op("%"),
                '<' => Token::Op("<"),
                '>' => Token::Op(">"),
                '=' => Token::Op("=="),
                '(' => Token::Open,
                ')' => Token::Close,
                ',' => Token::Comma,
                other => return Err(format!("непонятный символ «{other}»")),
            });
            i += 1;
        }
    }
    Ok(tokens)
}

pub fn eval(text: &str, names: &HashMap<String, Ratio>) -> Result<Value, String> {
    let tokens = tokenize(text)?;
    let mut parser = Parser {
        tokens: &tokens,
        at: 0,
        names,
    };
    let value = parser.or()?;
    if parser.at != tokens.len() {
        return Err(format!("лишнее в выражении «{text}»"));
    }
    Ok(value)
}

struct Parser<'a> {
    tokens: &'a [Token],
    at: usize,
    names: &'a HashMap<String, Ratio>,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn eat_op(&mut self, ops: &[&'static str]) -> Option<&'static str> {
        match self.peek() {
            Some(Token::Op(op)) if ops.contains(op) => {
                let op = *op;
                self.at += 1;
                Some(op)
            }
            _ => None,
        }
    }

    fn eat_word(&mut self, word: &str) -> bool {
        if matches!(self.peek(), Some(Token::Name(n)) if n == word) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn or(&mut self) -> Result<Value, String> {
        let mut left = self.and()?;
        while self.eat_word("or") {
            let right = self.and()?;
            left = Value::Bool(left.truth()? || right.truth()?);
        }
        Ok(left)
    }

    fn and(&mut self) -> Result<Value, String> {
        let mut left = self.not()?;
        while self.eat_word("and") {
            let right = self.not()?;
            left = Value::Bool(left.truth()? && right.truth()?);
        }
        Ok(left)
    }

    fn not(&mut self) -> Result<Value, String> {
        if self.eat_word("not") {
            return Ok(Value::Bool(!self.not()?.truth()?));
        }
        self.comparison()
    }

    fn comparison(&mut self) -> Result<Value, String> {
        let left = self.sum()?;
        let Some(op) = self.eat_op(&["==", "!=", "<", "<=", ">", ">="]) else {
            return Ok(left);
        };
        let right = self.sum()?;
        use std::cmp::Ordering::*;
        let ordering = left.num()?.cmp_value(right.num()?)?;
        Ok(Value::Bool(match op {
            "==" => ordering == Equal,
            "!=" => ordering != Equal,
            "<" => ordering == Less,
            "<=" => ordering != Greater,
            ">" => ordering == Greater,
            _ => ordering != Less,
        }))
    }

    fn sum(&mut self) -> Result<Value, String> {
        let mut left = self.product()?;
        while let Some(op) = self.eat_op(&["+", "-"]) {
            let right = self.product()?.num()?;
            let right = if op == "-" { right.neg() } else { right };
            left = Value::Num(left.num()?.add(right)?);
        }
        Ok(left)
    }

    fn product(&mut self) -> Result<Value, String> {
        let mut left = self.unary()?;
        loop {
            let op = match self.eat_op(&["*", "//", "/", "%"]) {
                Some(op) => op,
                // Implicit multiplication: «2x», «2(x+1)», «3 a».
                None if matches!(self.peek(), Some(Token::Name(n)) if !is_word(n))
                    || matches!(self.peek(), Some(Token::Open)) =>
                {
                    "*"
                }
                None => break,
            };
            let right = self.unary()?.num()?;
            let l = left.num()?;
            left = Value::Num(match op {
                "*" => l.mul(right)?,
                "/" => l.div(right)?,
                "//" => {
                    let q = l.div(right)?;
                    Ratio::int(q.num.div_euclid(q.den))
                }
                _ => {
                    let (a, b) = (
                        l.as_integer().ok_or("% только для целых")?,
                        right.as_integer().ok_or("% только для целых")?,
                    );
                    if b == 0 {
                        return Err("деление на ноль".into());
                    }
                    Ratio::int(a.rem_euclid(b))
                }
            });
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Value, String> {
        if self.eat_op(&["-"]).is_some() {
            return Ok(Value::Num(self.unary()?.num()?.neg()));
        }
        if self.eat_op(&["+"]).is_some() {
            return self.unary();
        }
        self.power()
    }

    fn power(&mut self) -> Result<Value, String> {
        let base = self.atom()?;
        if self.eat_op(&["**"]).is_some() {
            let exponent = self.unary()?.num()?;
            return Ok(Value::Num(base.num()?.pow(exponent)?));
        }
        Ok(base)
    }

    fn atom(&mut self) -> Result<Value, String> {
        match self.tokens.get(self.at).cloned() {
            Some(Token::Num(r)) => {
                self.at += 1;
                Ok(Value::Num(r))
            }
            Some(Token::Open) => {
                self.at += 1;
                let value = self.or()?;
                if self.tokens.get(self.at) != Some(&Token::Close) {
                    return Err("нет закрывающей скобки".into());
                }
                self.at += 1;
                Ok(value)
            }
            Some(Token::Name(name)) => {
                self.at += 1;
                if let Some(value) = self.names.get(&name) {
                    return Ok(Value::Num(*value));
                }
                let args = self.arguments()?;
                let arg = |i: usize| -> Result<Ratio, String> {
                    args.get(i)
                        .copied()
                        .ok_or(format!("{name}: мало аргументов"))
                };
                let int = |i: usize| -> Result<i128, String> {
                    arg(i)?.as_integer().ok_or(format!("{name}: нужны целые"))
                };
                match name.as_str() {
                    "abs" if args.len() == 1 => {
                        let a = arg(0)?;
                        Ok(Value::Num(if a.num < 0 { a.neg() } else { a }))
                    }
                    "gcd" if args.len() == 2 => Ok(Value::Num(Ratio::int(gcd(int(0)?, int(1)?)))),
                    "divides" if args.len() == 2 => {
                        let (a, b) = (int(0)?, int(1)?);
                        Ok(Value::Bool(a != 0 && b % a == 0))
                    }
                    _ => Err(format!("неизвестное имя «{name}»")),
                }
            }
            _ => Err("неполное выражение".into()),
        }
    }

    fn arguments(&mut self) -> Result<Vec<Ratio>, String> {
        if self.tokens.get(self.at) != Some(&Token::Open) {
            return Ok(Vec::new());
        }
        self.at += 1;
        let mut args = vec![self.sum()?.num()?];
        while self.tokens.get(self.at) == Some(&Token::Comma) {
            self.at += 1;
            args.push(self.sum()?.num()?);
        }
        if self.tokens.get(self.at) != Some(&Token::Close) {
            return Err("нет закрывающей скобки".into());
        }
        self.at += 1;
        Ok(args)
    }
}

fn is_word(name: &str) -> bool {
    matches!(name, "and" | "or" | "not")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num(text: &str) -> String {
        eval(text, &HashMap::new())
            .unwrap()
            .num()
            .unwrap()
            .to_string()
    }

    #[test]
    fn exact_fractions_and_human_notation() {
        assert_eq!(num("1/3*6"), "2");
        assert_eq!(num("⅓".replace('⅓', "1/3").as_str()), "1/3");
        assert_eq!(num("2·3 − 10"), "−4");
        assert_eq!(num("2²+3·2−1"), "9");
        assert_eq!(num("2,4*1000"), "2400");
        assert_eq!(num("(−3)^2"), "9");
        assert_eq!(num("-3**2"), "−9");
        assert_eq!(num("7 % 3"), "1");
        assert_eq!(num("7 // 2"), "3");
        assert_eq!(num("-7 // 2"), "−4");
    }

    #[test]
    fn names_functions_and_conditions() {
        let names = HashMap::from([
            ("a".to_string(), Ratio::int(6)),
            ("b".into(), Ratio::int(4)),
        ]);
        let check = |t: &str| eval(t, &names).unwrap().truth().unwrap();
        assert!(check("gcd(a, b) == 2"));
        assert!(check("divides(2, a) and not divides(5, b)"));
        assert!(check("abs(b - a) == 2 or a < 0"));
        assert!(check("2a == 12"));
    }

    #[test]
    fn errors_are_reported_not_guessed() {
        let names = HashMap::new();
        assert!(eval("1/0", &names).is_err());
        assert!(eval("x + 1", &names).is_err());
        assert!(eval("2 **", &names).is_err());
        assert!(eval("2 ** (1/2)", &names).is_err());
        assert!(eval("99999999999 ** 9", &names).is_err());
    }
}
