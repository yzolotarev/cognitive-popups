//! A study text with blocks (B75, B76): visible lines `⟦b2 · механизм⟧` and an
//! invisible mark before every sentence.

use cognitive_exoskeleton_domain::marks;
use serde::Serialize;

/// The roles the prompt `NEW_study_text` allows; anything else is «другое».
pub const ROLES: [&str; 6] = [
    "проблема",
    "определение",
    "механизм",
    "пример",
    "ошибка",
    "обобщение",
];
pub const OTHER_ROLE: &str = "другое";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Block {
    /// `b2`
    pub block: String,
    /// From 1, in the order of the text.
    pub position: i64,
    pub role: String,
    /// Clean text: no tag characters, no marker line, trimmed.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("В тексте нет меток блоков")]
    NoBlocks,
}

/// The first and the last line of our own prompt `NEW_study_text` (B74): a
/// copy of the chat often carries it above the answer (B93, owner's logs).
const PROMPT_OPENING: &str = "Напиши учебный текст для самостоятельной работы";
const PROMPT_CLOSING: &str = "Опирайся на стандартные учебники";

/// The text without our prompt above it: what follows the prompt's last
/// line; nothing when the prompt is all there is. Another text is unchanged.
pub fn strip_prompt(text: &str) -> &str {
    if !text.trim_start().starts_with(PROMPT_OPENING) {
        return text;
    }
    match text.find(PROMPT_CLOSING) {
        Some(at) => text[at..]
            .split_once('\n')
            .map_or("", |(_, rest)| rest)
            .trim_start_matches("```")
            .trim_start(),
        None => "",
    }
}

/// The text without tag characters and without our prompt, line ends
/// normalised and trimmed: what the material id is computed from.
pub fn clean(text: &str) -> String {
    let text = text.replace("\r\n", "\n");
    marks::read(strip_prompt(&text)).0.trim().to_string()
}

/// Blocks by the visible marker lines; without any, by the invisible marks
/// (role «другое»). Text before the first block (a title) belongs to none.
pub fn parse(text: &str) -> Result<Vec<Block>, ParseError> {
    let text = text.replace("\r\n", "\n");
    let text = strip_prompt(&text).to_string();
    let mut blocks: Vec<Block> = Vec::new();
    let mut current: Option<usize> = None;
    let mut found = false;
    for line in text.split_inclusive('\n') {
        let (visible, _) = marks::read(line);
        if let Some((id, role)) = marker(&visible) {
            found = true;
            current = Some(slot(&mut blocks, &id, &role));
        } else if let Some(i) = current {
            blocks[i].text.push_str(&visible);
        }
    }
    if !found {
        for span in marks::spans(&text) {
            match span.mark.as_deref().and_then(block_id) {
                Some(id) => {
                    let i = slot(&mut blocks, &id, OTHER_ROLE);
                    blocks[i].text.push_str(&span.text);
                }
                None => {
                    if let Some(last) = blocks.last_mut() {
                        last.text.push_str(&span.text);
                    }
                }
            }
        }
    }
    for block in &mut blocks {
        block.text = block.text.trim().to_string();
    }
    if blocks.is_empty() {
        return Err(ParseError::NoBlocks);
    }
    Ok(blocks)
}

/// The block's index; a repeated id continues the earlier block.
fn slot(blocks: &mut Vec<Block>, id: &str, role: &str) -> usize {
    if let Some(i) = blocks.iter().position(|b| b.block == id) {
        return i;
    }
    blocks.push(Block {
        block: id.to_string(),
        position: blocks.len() as i64 + 1,
        role: role.to_string(),
        text: String::new(),
    });
    blocks.len() - 1
}

/// `⟦b2 · механизм⟧` → (`b2`, `механизм`). Markdown decoration around the
/// brackets (`**`, `#`) is tolerated; any other text on the line is not.
pub(crate) fn marker(line: &str) -> Option<(String, String)> {
    let start = line.find('⟦')?;
    let end = line.rfind('⟧')?;
    if end < start {
        return None;
    }
    let decoration = |s: &str| s.chars().all(|c| c.is_whitespace() || "#*_>".contains(c));
    if !decoration(&line[..start]) || !decoration(&line[end + '⟧'.len_utf8()..]) {
        return None;
    }
    let inside = line[start + '⟦'.len_utf8()..end].trim();
    let digits_end = inside
        .char_indices()
        .skip(1)
        .find(|(_, c)| !c.is_ascii_digit())
        .map_or(inside.len(), |(i, _)| i);
    let id = block_id(&inside[..digits_end])?;
    let rest = inside[digits_end..]
        .trim_start()
        .trim_start_matches(['·', '-', ':', '—', '–', '.', '•'])
        .trim()
        .to_lowercase();
    let role = if ROLES.contains(&rest.as_str()) {
        rest
    } else {
        OTHER_ROLE.to_string()
    };
    Some((id, role))
}

/// `b2` / `B2` → `b2`; anything else is not a block id.
pub(crate) fn block_id(text: &str) -> Option<String> {
    let text = text.trim();
    let digits = text.strip_prefix(['b', 'B'])?;
    if !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let n: u32 = digits.parse().ok()?;
    Some(format!("b{n}"))
}

#[cfg(test)]
mod prompt_tests {
    use super::*;

    const ANSWER: &str = "⟦b1 · проблема⟧\nЗачем считать?\n⟦b2 · пример⟧\n3 из 8 → 3/8.";

    fn template() -> String {
        let file = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../docs/v2/port/prompts/NEW_study_text.md"),
        )
        .unwrap()
        // A Windows checkout ends lines with CRLF.
        .replace("\r\n", "\n");
        let start = file.find("```text\n").unwrap() + "```text\n".len();
        file[start..start + file[start..].find("\n```").unwrap()].to_string()
    }

    #[test]
    fn the_markers_are_the_prompt_s_own_first_and_last_lines() {
        let prompt = template();
        assert!(prompt.starts_with(PROMPT_OPENING), "first line changed");
        let last = prompt.lines().last().unwrap();
        assert!(last.contains(PROMPT_CLOSING), "last line changed: {last}");
    }

    #[test]
    fn our_prompt_above_the_answer_is_cut_and_alone_is_nothing() {
        let copy = format!("{}\n\n{ANSWER}", template());
        assert_eq!(strip_prompt(&copy), ANSWER);
        assert_eq!(
            parse(&copy).unwrap().len(),
            2,
            "the prompt's own ⟦b1⟧ is gone"
        );
        assert_eq!(clean(&copy), ANSWER);
        assert_eq!(strip_prompt(&template()), "");
        assert!(
            parse(&template()).is_err(),
            "the bare prompt is not a material"
        );
        assert_eq!(strip_prompt(ANSWER), ANSWER, "another text is unchanged");
    }
}
