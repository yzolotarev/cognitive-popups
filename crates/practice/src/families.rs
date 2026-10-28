//! Task families checked by code (v1 `recall_tasks.py`, B31): every showing is
//! a new instance from a seed, and the answer is graded by the code, so the
//! review is `verified` (F-97: `independent_correct` / `verified_incorrect`).
//! No model is involved at all.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    /// Integer roots of a cubic polynomial.
    IntegerRoots,
    /// A direct or inverse proportion («работа по пропорции»).
    WorkProportion,
}

impl Family {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "integer_roots" => Some(Family::IntegerRoots),
            "work_proportion" => Some(Family::WorkProportion),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Family::IntegerRoots => "integer_roots",
            Family::WorkProportion => "work_proportion",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FamilyTask {
    pub family: Family,
    pub seed: u64,
    pub prompt: String,
    /// Hidden until the reader answers.
    pub answer: String,
}

pub fn generate(family: Family, seed: u64) -> FamilyTask {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let (prompt, answer) = match family {
        Family::IntegerRoots => {
            let mut roots: Vec<i64> = Vec::new();
            while roots.len() < 3 {
                let r = rng.range(-6, 6);
                if !roots.contains(&r) {
                    roots.push(r);
                }
            }
            roots.sort_unstable();
            let (r1, r2, r3) = (roots[0], roots[1], roots[2]);
            let a = -(r1 + r2 + r3);
            let b = r1 * r2 + r1 * r3 + r2 * r3;
            let c = -(r1 * r2 * r3);
            (
                format!("Найди все целые корни многочлена {}.", cubic(a, b, c)),
                roots
                    .iter()
                    .map(|r| sign(*r))
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        }
        Family::WorkProportion => {
            if rng.next().is_multiple_of(2) {
                // Inverse: more workers, fewer days.
                let d2 = rng.range(2, 12);
                let n2 = rng.range(2, 9);
                let n1 = loop {
                    let n = rng.range(2, 9);
                    if n != n2 {
                        break n;
                    }
                };
                let total = d2 * n2;
                if total % n1 != 0 {
                    return generate(family, seed.wrapping_add(1));
                }
                let d1 = total / n1;
                (
                    format!(
                        "{n1} рабочих выполняют работу за {d1} дн. За сколько дней ту же работу выполнят {n2} рабочих, если все работают одинаково?"
                    ),
                    d2.to_string(),
                )
            } else {
                // Direct: the same machine, a different time.
                let rate = rng.range(3, 15);
                let t1 = rng.range(2, 8);
                let t2 = loop {
                    let t = rng.range(2, 12);
                    if t != t1 {
                        break t;
                    }
                };
                (
                    format!(
                        "Станок за {t1} ч делает {} деталей. Сколько деталей он сделает за {t2} ч при той же скорости?",
                        rate * t1
                    ),
                    (rate * t2).to_string(),
                )
            }
        }
    };
    FamilyTask {
        family,
        seed,
        prompt,
        answer,
    }
}

/// Grades a free-form answer: the set of integers for roots (any order and
/// separators), the number for a proportion. Units and words are ignored.
pub fn check(task: &FamilyTask, answer: &str) -> bool {
    let given = integers(answer);
    let expected = integers(&task.answer);
    match task.family {
        Family::IntegerRoots => {
            let mut given = given;
            given.sort_unstable();
            given.dedup();
            given == expected
        }
        Family::WorkProportion => given.len() == 1 && given == expected,
    }
}

fn integers(text: &str) -> Vec<i64> {
    let text = text.replace(['−', '–'], "-");
    let mut out = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let negative = chars[i] == '-' && chars.get(i + 1).is_some_and(char::is_ascii_digit);
        let start = if negative { i + 1 } else { i };
        if chars.get(start).is_some_and(char::is_ascii_digit) {
            let mut end = start;
            while chars.get(end).is_some_and(char::is_ascii_digit) {
                end += 1;
            }
            let digits: String = chars[start..end].iter().collect();
            if let Ok(n) = digits.parse::<i64>() {
                out.push(if negative { -n } else { n });
            }
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

fn sign(n: i64) -> String {
    if n < 0 {
        format!("−{}", -n)
    } else {
        n.to_string()
    }
}

/// x³ + ax² + bx + c the way a person writes it.
fn cubic(a: i64, b: i64, c: i64) -> String {
    let mut out = String::from("x³");
    for (coef, power) in [(a, "x²"), (b, "x"), (c, "")] {
        if coef == 0 {
            continue;
        }
        out.push_str(if coef < 0 { " − " } else { " + " });
        let abs = coef.abs();
        if abs != 1 || power.is_empty() {
            out.push_str(&abs.to_string());
        }
        out.push_str(power);
    }
    out
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next() % (hi - lo + 1) as u64) as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roots_really_are_roots_of_the_printed_polynomial() {
        for seed in 0..200 {
            let task = generate(Family::IntegerRoots, seed);
            let roots = integers(&task.answer);
            assert_eq!(roots.len(), 3, "{task:?}");
            let poly = task
                .prompt
                .trim_start_matches("Найди все целые корни многочлена ");
            let poly = poly.trim_end_matches('.');
            for r in roots {
                let value =
                    crate::expr::eval(&poly.replace('x', &format!("({r})")), &Default::default())
                        .unwrap()
                        .num()
                        .unwrap();
                assert_eq!(value.to_string(), "0", "{poly} at {r}");
            }
        }
    }

    #[test]
    fn proportions_have_whole_answers_and_differ_by_seed() {
        let mut prompts = std::collections::HashSet::new();
        for seed in 0..100 {
            let task = generate(Family::WorkProportion, seed);
            assert_eq!(integers(&task.answer).len(), 1, "{task:?}");
            prompts.insert(task.prompt);
        }
        assert!(prompts.len() > 50);
    }

    #[test]
    fn answers_are_graded_by_the_code() {
        let task = FamilyTask {
            family: Family::IntegerRoots,
            seed: 0,
            prompt: String::new(),
            answer: "−2; 1; 3".into(),
        };
        assert!(check(&task, "x = 3, x = -2, x = 1"));
        assert!(check(&task, "1; −2; 3; 3"));
        assert!(!check(&task, "1; 2; 3"));
        assert!(!check(&task, "1; −2"));
        let task = FamilyTask {
            family: Family::WorkProportion,
            seed: 0,
            prompt: String::new(),
            answer: "12".into(),
        };
        assert!(check(&task, "12 дней"));
        assert!(!check(&task, "12 или 13"));
        assert_eq!(cubic(-2, -5, 6), "x³ − 2x² − 5x + 6");
        assert_eq!(cubic(0, -1, 0), "x³ − x");
    }
}
