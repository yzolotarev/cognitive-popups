//! Merging the selected fragments of one block (B78). Exact text only: a
//! fragment inside another disappears; a suffix equal to a prefix of at least
//! `MIN_OVERLAP` characters joins two fragments. Anything else stays apart —
//! the gap between them is never filled.

/// The shortest overlap that joins two fragments, in characters.
pub const MIN_OVERLAP: usize = 20;

/// The fragments merged, in first-seen order of what they became.
pub fn merge(fragments: &[String]) -> Vec<String> {
    let mut merged: Vec<String> = Vec::new();
    for fragment in fragments {
        let mut current = fragment.trim().to_string();
        if current.is_empty() {
            continue;
        }
        // Absorb every earlier piece that joins with this one; the result
        // takes the place of the first of them.
        let mut place: Option<usize> = None;
        let mut i = 0;
        while i < merged.len() {
            match join(&merged[i], &current) {
                Some(joined) => {
                    current = joined;
                    merged.remove(i);
                    place = Some(place.map_or(i, |p| p.min(i)));
                    i = 0;
                }
                None => i += 1,
            }
        }
        match place {
            Some(p) => merged.insert(p.min(merged.len()), current),
            None => merged.push(current),
        }
    }
    merged
}

/// `a` and `b` as one text, when one contains the other or they overlap by at
/// least `MIN_OVERLAP` characters (either way round).
fn join(a: &str, b: &str) -> Option<String> {
    if a.contains(b) {
        return Some(a.to_string());
    }
    if b.contains(a) {
        return Some(b.to_string());
    }
    overlap(a, b)
        .map(|k| format!("{a}{}", &b[k..]))
        .or_else(|| overlap(b, a).map(|k| format!("{b}{}", &a[k..])))
}

/// The byte length of the longest prefix of `b` that ends `a`, if at least
/// `MIN_OVERLAP` characters long.
fn overlap(a: &str, b: &str) -> Option<usize> {
    let ends: Vec<usize> = b
        .char_indices()
        .map(|(i, _)| i)
        .skip(1)
        .chain(std::iter::once(b.len()))
        .collect();
    ends.iter()
        .enumerate()
        .rev()
        .filter(|(chars, _)| chars + 1 >= MIN_OVERLAP)
        .map(|(_, end)| *end)
        .find(|end| a.ends_with(&b[..*end]))
}
