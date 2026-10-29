//! «Мои 4 слова» (F-04, Q-06): the reader's words against the model's, by word
//! stems, decided by code. Matches are only highlighted; no score is shown.

use serde::Serialize;

use crate::Cue;

/// Russian endings, longest first; one is cut if at least 3 letters remain.
const ENDINGS: [&str; 46] = [
    "иями", "ями", "ами", "его", "ого", "ему", "ому", "ыми", "ими", "ией", "иях", "ах", "ях", "ий",
    "ый", "ой", "ая", "яя", "ое", "ее", "ие", "ые", "ую", "юю", "ов", "ев", "ей", "ам", "ям", "ом",
    "ем", "ию", "ия", "ья", "ье", "ьи", "ью", "а", "я", "о", "е", "ы", "и", "у", "ю", "ь",
];

/// A light stem: lower case, ё → е, one ending removed.
pub fn stem(word: &str) -> String {
    let word: String = word
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase()
        .replace('ё', "е");
    for ending in ENDINGS {
        if let Some(base) = word.strip_suffix(ending)
            && base.chars().count() >= 3
        {
            return base.to_string();
        }
    }
    word
}

/// Same word by stem: equal, or the shorter stem (4+ letters) begins the
/// longer — «ступени» and «ступенчатый» meet at «ступен».
fn same(a: &str, b: &str) -> bool {
    let (a, b) = (stem(a), stem(b));
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let (short, long) = if a.chars().count() <= b.chars().count() {
        (a, b)
    } else {
        (b, a)
    };
    short == long || (short.chars().count() >= 4 && long.starts_with(&short))
}

fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !c.is_alphanumeric() && c != '-')
        .filter(|w| !w.is_empty())
}

/// Whether a term (any of its words of 3+ letters) occurs in the text.
pub fn appears_in(term: &str, text: &str) -> bool {
    let lower = text.to_lowercase();
    if lower.contains(&term.trim().to_lowercase()) {
        return true;
    }
    let significant: Vec<&str> = words(term).filter(|w| w.chars().count() >= 3).collect();
    !significant.is_empty() && significant.iter().all(|w| words(text).any(|t| same(w, t)))
}

/// The reader's entries: separated by commas or new lines.
pub fn own_words(input: &str) -> Vec<String> {
    input
        .split([',', ';', '\n'])
        .map(str::trim)
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Comparison {
    /// For each of the reader's words: the model's cue it meets, if any.
    pub mine: Vec<Option<usize>>,
    /// For each cue: whether one of the reader's words meets it.
    pub cues: Vec<bool>,
}

impl Comparison {
    pub fn overlap(&self) -> usize {
        self.cues.iter().filter(|m| **m).count()
    }
}

pub fn compare(mine: &[String], cues: &[Cue]) -> Comparison {
    let meets = |entry: &str, cue: &Cue| {
        words(entry).any(|w| {
            words(&cue.simple)
                .chain(words(&cue.term))
                .any(|c| same(w, c))
        })
    };
    let mine: Vec<Option<usize>> = mine
        .iter()
        .map(|entry| cues.iter().position(|cue| meets(entry, cue)))
        .collect();
    let mut matched = vec![false; cues.len()];
    for index in mine.iter().flatten() {
        matched[*index] = true;
    }
    Comparison {
        mine,
        cues: matched,
    }
}
