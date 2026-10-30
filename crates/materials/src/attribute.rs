//! Which block each part of a selection belongs to (B78). A hidden mark sits
//! at the start of a sentence, so:
//! - a part that starts at a mark belongs to that mark's block;
//! - the part before the first mark (or a selection without marks) is found
//!   by exact text, after whitespace normalization, in the lesson's
//!   registered text, else in its reconstructed fragments; one match gives
//!   its block, none or several leave it unknown. Never a guess.

use serde::Serialize;

/// A piece of a selection and its block, if known.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Part {
    pub block: Option<String>,
    pub text: String,
}

/// The text compared: soft hyphens removed, whitespace runs collapsed to one
/// space, trimmed. (NFC is not applied: no normalization crate in the
/// workspace; texts from the same chat share their form.)
pub fn normalize(text: &str) -> String {
    text.chars()
        .filter(|c| *c != '\u{00AD}')
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The selection cut at its marks: the leading part without a block, then
/// one part per mark (consecutive parts of one block joined). Visible marker
/// lines (`⟦b2 · механизм⟧`) belong to no block's text and are dropped;
/// empty parts are left out.
pub fn split(text: &str, marks: &[(usize, String)]) -> Vec<Part> {
    let chars: Vec<char> = text.chars().collect();
    let mut cuts: Vec<(usize, Option<String>)> = vec![(0, None)];
    cuts.extend(
        marks
            .iter()
            .filter(|(at, _)| *at <= chars.len())
            .map(|(at, mark)| (*at, Some(mark.clone()))),
    );
    let mut parts: Vec<Part> = Vec::new();
    for (i, (start, block)) in cuts.iter().enumerate() {
        let end = cuts.get(i + 1).map_or(chars.len(), |(at, _)| *at);
        let piece: String = chars[*start..end.max(*start)].iter().collect();
        match parts.last_mut() {
            Some(last) if block.is_some() && last.block == *block => last.text.push_str(&piece),
            _ => parts.push(Part {
                block: block.clone(),
                text: piece,
            }),
        }
    }
    parts
        .into_iter()
        .map(|p| Part {
            text: without_markers(&p.text).trim().to_string(),
            ..p
        })
        .filter(|p| !p.text.is_empty())
        .collect()
}

/// The block of every occurrence of `needle` in `texts`, after
/// normalization: one entry means a unique match; none or several mean the
/// block is not known from this source.
pub fn locate<'a>(
    needle: &str,
    texts: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Vec<String> {
    let needle = normalize(needle);
    if needle.is_empty() {
        return Vec::new();
    }
    texts
        .into_iter()
        .flat_map(|(block, text)| {
            let n = normalize(text).matches(needle.as_str()).count();
            std::iter::repeat_n(block.to_string(), n)
        })
        .collect()
}

/// The text without its visible marker lines.
fn without_markers(text: &str) -> String {
    text.split_inclusive('\n')
        .filter(|line| crate::parse::marker(line).is_none())
        .collect()
}
