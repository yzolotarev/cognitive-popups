//! Arithmetic filter for drill pairs (F-96, v1 `arith.check`).
//!
//! A pair whose input is plain arithmetic («2·3 + 1», «⅓·6», «2,4 · 1000») and
//! whose answer is a number is computed exactly; a wrong answer removes the pair
//! before anyone sees it. Anything else is not arithmetic and goes to the model
//! verifier, which can only remove pairs, never grade the reader.

use std::collections::HashMap;

use crate::expr::{Ratio, eval};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Correct,
    Wrong { expected: String },
    NotArithmetic,
}

pub fn check(prompt: &str, answer: &str) -> Verdict {
    let Some(input) = arithmetic(prompt.trim().trim_end_matches(['=', '?', ' '])) else {
        return Verdict::NotArithmetic;
    };
    let Some(given) = arithmetic(answer.trim()) else {
        return Verdict::NotArithmetic;
    };
    match (value(&input), value(&given)) {
        (Some(expected), Some(given)) if expected == given => Verdict::Correct,
        (Some(expected), Some(_)) => Verdict::Wrong {
            expected: expected.to_string(),
        },
        _ => Verdict::NotArithmetic,
    }
}

/// The text, with vulgar fractions spelled out, if it is only numbers and operators.
fn arithmetic(text: &str) -> Option<String> {
    let mut out = String::new();
    for c in text.chars() {
        match c {
            '½' => out.push_str("(1/2)"),
            '⅓' => out.push_str("(1/3)"),
            '⅔' => out.push_str("(2/3)"),
            '¼' => out.push_str("(1/4)"),
            '¾' => out.push_str("(3/4)"),
            '0'..='9'
            | ' '
            | '.'
            | ','
            | '+'
            | '-'
            | '−'
            | '–'
            | '*'
            | '·'
            | '×'
            | '/'
            | '÷'
            | ':'
            | '^'
            | '²'
            | '³'
            | '('
            | ')' => out.push(c),
            _ => return None,
        }
    }
    out.chars().any(|c| c.is_ascii_digit()).then_some(out)
}

fn value(text: &str) -> Option<Ratio> {
    eval(text, &HashMap::new()).ok()?.num().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrong_arithmetic_is_caught_exactly() {
        assert_eq!(check("⅓·6", "2"), Verdict::Correct);
        assert_eq!(check("2²+3·2−1 =", "9"), Verdict::Correct);
        assert_eq!(check("2,4 · 1000", "2400"), Verdict::Correct);
        assert_eq!(
            check("7·8", "54"),
            Verdict::Wrong {
                expected: "56".into()
            }
        );
        assert_eq!(
            check("1/2 + 1/3", "2/5"),
            Verdict::Wrong {
                expected: "5/6".into()
            }
        );
    }

    #[test]
    fn non_arithmetic_pairs_are_left_to_the_verifier() {
        assert_eq!(
            check("die Schwester", "meine Schwester"),
            Verdict::NotArithmetic
        );
        assert_eq!(check("x+3=8", "x=5"), Verdict::NotArithmetic);
        assert_eq!(check("B3", "$B$3"), Verdict::NotArithmetic);
        assert_eq!(check("2,4 км", "2400 м"), Verdict::NotArithmetic);
    }
}
