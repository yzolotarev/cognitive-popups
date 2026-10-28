//! Numeric probes (v1 live contract, kind `numeric`): the model writes one
//! template — integer parameters, derived values, constraints, a check and the
//! texts — and the code produces the cases. A template whose check fails for any
//! admissible case is rejected whole: the model never states an answer we show.

use std::collections::{BTreeMap, HashMap};

use serde_json::Value;

use crate::expr::{Ratio, eval};

/// Largest parameter range we enumerate; wider ranges are sampled.
const ENUMERATE_UP_TO: u64 = 20_000;
const SAMPLES: usize = 4_000;

#[derive(Debug, Clone, PartialEq)]
pub struct Template {
    params: BTreeMap<String, (i128, i128)>,
    derived: BTreeMap<String, String>,
    constraints: Vec<String>,
    check: String,
    prompt: String,
    answer: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Case {
    pub prompt: String,
    pub answer: String,
}

impl Template {
    pub fn from_json(value: &Value) -> Result<Self, String> {
        let mut params = BTreeMap::new();
        for (name, spec) in value["params"].as_object().ok_or("нет params")? {
            if spec["kind"].as_str() != Some("int") {
                return Err(format!("параметр {name}: только целые"));
            }
            let min = spec["min"]
                .as_i64()
                .ok_or(format!("параметр {name}: нет min"))?;
            let max = spec["max"]
                .as_i64()
                .ok_or(format!("параметр {name}: нет max"))?;
            if min > max {
                return Err(format!("параметр {name}: min больше max"));
            }
            params.insert(name.clone(), (min as i128, max as i128));
        }
        if params.is_empty() {
            return Err("нет параметров".into());
        }
        let check = value["check"].as_str().unwrap_or_default();
        let derived = match &value["derived"] {
            Value::Null => BTreeMap::new(),
            Value::Object(map) => map
                .iter()
                .map(|(k, v)| match v {
                    Value::String(s) => Ok((k.clone(), s.clone())),
                    Value::Number(n) => Ok((k.clone(), n.to_string())),
                    // Models often describe a derived value like a parameter and
                    // put its formula into the check: «nm == n - 1».
                    _ => formula_in_check(check, k)
                        .map(|f| (k.clone(), f))
                        .ok_or(format!(
                            "derived.{k} должен быть формулой (например \"n - 1\"), а не диапазоном"
                        )),
                })
                .collect::<Result<_, _>>()?,
            _ => return Err("derived должен быть объектом".into()),
        };
        let constraints = match &value["constraints"] {
            Value::Null => Vec::new(),
            Value::Array(items) => items
                .iter()
                .map(|v| v.as_str().map(str::to_string).ok_or("constraints — строки"))
                .collect::<Result<_, _>>()?,
            _ => return Err("constraints должен быть массивом".into()),
        };
        let text = |key: &str| {
            value[key]
                .as_str()
                .map(str::to_string)
                .ok_or(format!("нет {key}"))
        };
        Ok(Self {
            params,
            derived,
            constraints,
            check: text("check")?,
            prompt: text("prompt")?,
            answer: text("answer")?,
        })
    }

    /// Every admissible case (within limits) must pass the check; returns up to
    /// `want` distinct cases in a stable order derived from `seed`.
    pub fn cases(&self, want: usize, seed: u64) -> Result<Vec<Case>, String> {
        let mut rng = Rng(seed | 1);
        let total: u64 = self
            .params
            .values()
            .map(|(lo, hi)| (hi - lo + 1) as u64)
            .try_fold(1u64, |acc, n| acc.checked_mul(n))
            .unwrap_or(u64::MAX);
        let mut assignments: Vec<Vec<i128>> = if total <= ENUMERATE_UP_TO {
            let mut all = vec![Vec::new()];
            for (lo, hi) in self.params.values() {
                all = all
                    .into_iter()
                    .flat_map(|prefix| {
                        (*lo..=*hi).map(move |v| {
                            let mut next = prefix.clone();
                            next.push(v);
                            next
                        })
                    })
                    .collect();
            }
            all
        } else {
            (0..SAMPLES)
                .map(|_| {
                    self.params
                        .values()
                        .map(|(lo, hi)| lo + (rng.next() % ((hi - lo + 1) as u64)) as i128)
                        .collect()
                })
                .collect()
        };
        rng.shuffle(&mut assignments);

        let mut cases: Vec<Case> = Vec::new();
        let mut admissible = 0;
        for values in assignments {
            let Some(names) = self.resolve(&values)? else {
                continue;
            };
            admissible += 1;
            if !eval(&self.check, &names)?.truth()? {
                let shown: Vec<String> = self
                    .params
                    .keys()
                    .zip(&values)
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect();
                return Err(format!("check ложен при {}", shown.join(", ")));
            }
            let case = Case {
                prompt: render(&self.prompt, &names)?,
                answer: render(&self.answer, &names)?,
            };
            if case.answer.trim() == case.prompt.trim() {
                return Err("ответ совпадает с заданием — это тождество, не операция".into());
            }
            if cases.len() < want && !cases.iter().any(|c| c.prompt == case.prompt) {
                cases.push(case);
            }
        }
        if admissible == 0 {
            return Err("ни одно значение не проходит constraints".into());
        }
        Ok(cases)
    }

    /// Names for one assignment, or None when a constraint rules it out.
    fn resolve(&self, values: &[i128]) -> Result<Option<HashMap<String, Ratio>>, String> {
        let mut names: HashMap<String, Ratio> = self
            .params
            .keys()
            .cloned()
            .zip(values.iter().map(|v| Ratio::int(*v)))
            .collect();
        let mut pending: Vec<(&String, &String)> = self.derived.iter().collect();
        while !pending.is_empty() {
            let before = pending.len();
            let mut failed = None;
            pending.retain(|(name, expression)| match eval(expression, &names) {
                Ok(value) => match value.num() {
                    Ok(v) => {
                        names.insert((*name).clone(), v);
                        false
                    }
                    Err(e) => {
                        failed = Some(format!("derived.{name}: {e}"));
                        false
                    }
                },
                Err(e) if e.contains("деление на ноль") => {
                    failed = Some(String::new());
                    false
                }
                Err(_) => true,
            });
            match failed {
                Some(e) if e.is_empty() => return Ok(None),
                Some(e) => return Err(e),
                None => {}
            }
            if pending.len() == before {
                let (name, expression) = pending[0];
                return Err(format!(
                    "derived.{name} = «{expression}»: {}",
                    eval(expression, &names).unwrap_err()
                ));
            }
        }
        for constraint in &self.constraints {
            match eval(constraint, &names) {
                Ok(value) if value.truth()? => {}
                Ok(_) => return Ok(None),
                Err(e) if e.contains("деление на ноль") => return Ok(None),
                Err(e) => return Err(format!("constraint «{constraint}»: {e}")),
            }
        }
        Ok(Some(names))
    }
}

/// `name == formula` among the top-level `and` parts of the check.
fn formula_in_check(check: &str, name: &str) -> Option<String> {
    check.split(" and ").find_map(|part| {
        let (left, right) = part.trim().split_once("==")?;
        let (left, right) = (left.trim(), right.trim());
        if left == name && !right.contains(name) {
            Some(right.to_string())
        } else if right == name && !left.contains(name) {
            Some(left.to_string())
        } else {
            None
        }
    })
}

/// Substitutes `{name}` and `{expression}`; an unknown name is an error.
fn render(template: &str, names: &HashMap<String, Ratio>) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let close = rest[open..].find('}').ok_or("незакрытая фигурная скобка")? + open;
        let inner = &rest[open + 1..close];
        let value = eval(inner, names)
            .and_then(|v| v.num())
            .map_err(|e| format!("подстановка {{{inner}}}: {e}"))?;
        out.push_str(&value.to_string());
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    Ok(superscripts(&out))
}

/// «x^2» → «x²», «x^1» → «x»: cards read like the text, not like code.
fn superscripts(text: &str) -> String {
    const DIGITS: [char; 10] = ['⁰', '¹', '²', '³', '⁴', '⁵', '⁶', '⁷', '⁸', '⁹'];
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let after_letter = i > 0 && (chars[i - 1].is_alphabetic() || chars[i - 1] == ')');
        if chars[i] == '^' && after_letter && chars.get(i + 1).is_some_and(char::is_ascii_digit) {
            let start = i + 1;
            let mut end = start;
            while chars.get(end).is_some_and(char::is_ascii_digit) {
                end += 1;
            }
            let digits: String = chars[start..end].iter().collect();
            if digits != "1" {
                out.extend(digits.chars().map(|d| DIGITS[d as usize - '0' as usize]));
            }
            i = end;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// xorshift64*: deterministic, no dependency; quality is irrelevant here.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = (self.next() % (i as u64 + 1)) as usize;
            items.swap(i, j);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn template(check: &str) -> Template {
        Template::from_json(&json!({
            "params":{"a":{"kind":"int","min":1,"max":9},"b":{"kind":"int","min":1,"max":9}},
            "derived":{"r":"a+b","d":"r*2"},
            "constraints":["a != b"],
            "check": check,
            "prompt":"{a} + {b}","answer":"{r}"
        }))
        .unwrap()
    }

    #[test]
    fn the_code_computes_the_answers() {
        let cases = template("r == a + b and d == 2*(a+b)").cases(3, 7).unwrap();
        assert_eq!(cases.len(), 3);
        for case in &cases {
            let (a, b) = case.prompt.split_once(" + ").unwrap();
            let sum: i32 = a.parse::<i32>().unwrap() + b.parse::<i32>().unwrap();
            assert_eq!(case.answer, sum.to_string());
            assert_ne!(a, b, "constraint respected");
        }
        assert_eq!(
            cases,
            template("r == a + b").cases(3, 7).unwrap(),
            "stable for a seed"
        );
    }

    #[test]
    fn a_check_that_fails_on_any_case_rejects_the_template() {
        let error = template("r == a * b").cases(3, 7).unwrap_err();
        assert!(error.starts_with("check ложен при"), "{error}");
    }

    #[test]
    fn identity_and_unknown_names_are_rejected() {
        let identity = Template::from_json(&json!({
            "params":{"a":{"kind":"int","min":1,"max":5}},
            "check":"a == a","prompt":"{a}","answer":"{a}"}))
        .unwrap();
        assert!(identity.cases(3, 1).unwrap_err().contains("тождество"));
        let unknown = Template::from_json(&json!({
            "params":{"a":{"kind":"int","min":1,"max":5}},
            "check":"a > 0","prompt":"{a}+{z}","answer":"{a}"}))
        .unwrap();
        assert!(unknown.cases(3, 1).unwrap_err().contains("{z}"));
    }

    #[test]
    fn powers_are_written_as_a_person_writes_them() {
        assert_eq!(superscripts("(5x^2)′ = 10x^1"), "(5x²)′ = 10x");
        assert_eq!(superscripts("(x+1)^12, 2^3"), "(x+1)¹², 2^3");
    }

    #[test]
    fn a_derived_value_described_as_a_range_takes_its_formula_from_the_check() {
        let t = Template::from_json(&json!({
            "params":{"c":{"kind":"int","min":2,"max":5},"p":{"kind":"int","min":2,"max":4}},
            "derived":{"pm":{"kind":"int","min":1,"max":3},"res":{"kind":"int","min":4,"max":60}},
            "check":"res == c * p and pm == p - 1",
            "prompt":"({c}x^{p})′","answer":"{res}x^{pm}"}))
        .unwrap();
        let expected: Vec<Case> = (2..=5)
            .flat_map(|c| (2..=4).map(move |p| (c, p)))
            .map(|(c, p)| Case {
                prompt: superscripts(&format!("({c}x^{p})′")),
                answer: superscripts(&format!("{}x^{}", c * p, p - 1)),
            })
            .collect();
        let cases = t.cases(12, 2).unwrap();
        assert_eq!(cases.len(), 12);
        for case in cases {
            assert!(expected.contains(&case), "{case:?}");
        }
        let none = Template::from_json(&json!({
            "params":{"n":{"kind":"int","min":2,"max":9}},
            "derived":{"m":{"kind":"int","min":1,"max":8}},
            "check":"m > 0","prompt":"{n}","answer":"{m}"}));
        assert!(none.unwrap_err().contains("формулой"));
    }

    #[test]
    fn non_integer_parameters_are_refused() {
        assert!(
            Template::from_json(&json!({"params":{"a":{"kind":"float","min":0,"max":1}},
                "check":"a>0","prompt":"{a}","answer":"{a}"}))
            .is_err()
        );
    }

    #[test]
    fn division_by_zero_in_a_case_excludes_that_case() {
        let t = Template::from_json(&json!({
            "params":{"a":{"kind":"int","min":0,"max":4}},
            "derived":{"r":"12/a"},
            "check":"r*a == 12","prompt":"12 : {a}","answer":"{r}"}))
        .unwrap();
        let cases = t.cases(10, 3).unwrap();
        assert_eq!(cases.len(), 4);
    }
}
